"""Finite synthetic native filesystem tests; experiment branch only."""
import json
import os
import pathlib
import plistlib
import shutil
import select
import time
import stat
import subprocess
import sys
import tempfile

assert os.geteuid() == 0 and sys.platform == 'darwin'
assert os.environ.get('GITHUB_ACTIONS') == 'true'
base = pathlib.Path(tempfile.mkdtemp(prefix='crucible-fs-probe-')).resolve(strict=True)
os.chmod(base, 0o755)
worker = base / 'worker'
shutil.copyfile(pathlib.Path(sys.argv[1]).resolve(strict=True), worker)
os.chmod(worker, 0o755)
image = base / 'fixture.dmg'
mount = base / 'volume'
mount.mkdir()
print('FIXTURE ' + str(base), flush=True)
owned_pids = []


def command(argv, allowed=(0,), timeout=30):
    print('COMMAND ' + json.dumps([str(x) for x in argv]), flush=True)
    proc = subprocess.Popen(argv, cwd=base, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            close_fds=True, start_new_session=True,
                            env={'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LANG': 'C'})
    if len(argv) > 1 and str(argv[0]) == str(worker) and argv[1] in ('launch', 'launch-cwd'):
        owned_pids.append(proc.pid)
    data = bytearray()
    deadline = time.monotonic() + timeout
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError('operation deadline')
            if select.select([proc.stdout], [], [], min(remaining, 0.2))[0]:
                chunk = os.read(proc.stdout.fileno(), min(4096, 32769 - len(data)))
                if not chunk:
                    break
                data.extend(chunk)
                if len(data) > 32768:
                    raise RuntimeError('output exceeded bound')
        result = proc.wait(timeout=max(0.01, deadline - time.monotonic()))
    except BaseException:
        if proc.poll() is None:
            proc.kill()
        try:
            proc.wait(timeout=3)
            print('UNCERTAIN direct_process_reaped=1 fixture_quarantined=1', flush=True)
        except subprocess.TimeoutExpired:
            print('UNCERTAIN direct_process_reaped=0 fixture_quarantined=1', flush=True)
        raise
    finally:
        proc.stdout.close()
    data = bytes(data)
    print(data.decode(errors='replace'), end='', flush=True)
    assert result in allowed, (str(argv[0]), result)
    return result, data


def owned_file(path, data=b'fixture'):
    path.write_bytes(data)
    os.chown(path, 60000, 60000)
    os.chmod(path, 0o666)


def make_case(sequence):
    root = mount / ('case-' + str(sequence))
    for suffix in ('', 'ro', 'rw', 'rw/nested', 'rw/nested/.git'):
        item = root / suffix
        item.mkdir(exist_ok=True)
        os.chown(item, 60000, 60000)
        os.chmod(item, 0o755)
    for suffix in ('ro/payload', 'rw/nested/.git/config', 'rw/secret', 'rw/plain'):
        owned_file(root / suffix)
    outside = base / ('host-source-' + str(sequence))
    owned_file(outside)
    os.symlink(root / 'rw/nested/.git/config', root / 'rw/protected-alias')
    os.symlink(root / 'rw/secret', root / 'rw/secret-alias')
    os.symlink(outside, root / 'rw/source-alias')
    shutil.copyfile(worker, root / 'rw/setuid-worker')
    os.chown(root / 'rw/setuid-worker', 0, 0)
    os.chmod(root / 'rw/setuid-worker', 0o4755)
    os.mknod(root / 'rw/null-device', stat.S_IFCHR | 0o666, os.stat('/dev/null').st_rdev)
    os.chmod(root / 'rw/null-device', 0o666)
    return root, outside


def profile_for(root):
    # Only generated absolute fixture paths, with Scheme string escaping.
    q = lambda path: json.dumps(str(path))
    ancestors = {path for item in (root, worker, pathlib.Path('/usr/lib'), pathlib.Path('/System/Library'), pathlib.Path('/dev/null')) for path in item.parents}
    metadata = ' '.join('(literal ' + q(path) + ')' for path in sorted(ancestors))
    return '\n'.join([
        '(version 1)', '(allow default)', '(deny network*)',
        '(deny mach-lookup)', '(deny mach-register)',
        '(deny file-read*)', '(deny file-write*)',
        '(allow file-read-metadata ' + metadata + ')',
        '(allow file-read* (subpath "/System/Library") (subpath "/usr/lib") (literal "/dev/null") (literal ' + q(worker) + ') (subpath ' + q(root) + '))',
        '(allow file-write* (subpath ' + q(root / 'rw') + '))',
        '(deny file-write* (subpath ' + q(root / 'rw/nested/.git') + '))',
        '(deny file-write-unlink (literal ' + q(root / 'rw/nested') + '))',
        '(deny file-read* file-write* (literal ' + q(root / 'rw/secret') + ') (literal ' + q(root / 'rw/absent-secret') + '))',
    ]) + '\n'


command(['/usr/bin/sw_vers'])
command(['/usr/bin/uname', '-a'])
command([worker, 'empty'])
command(['/usr/bin/hdiutil', 'create', '-size', '128m', '-fs', 'APFS', '-volname', 'CrucibleFSFixture', image])
_, data = command(['/usr/bin/hdiutil', 'attach', image, '-nobrowse', '-owners', 'on', '-mountpoint', mount, '-plist'])
entities = plistlib.loads(data)['system-entities']
mounted = [entry for entry in entities if entry.get('mount-point') == str(mount)]
assert len(mounted) == 1
_, data = command(['/usr/sbin/diskutil', 'info', '-plist', mount])
info = plistlib.loads(data)
assert info.get('FilesystemType') == 'apfs' and info.get('MountPoint') == str(mount)
assert info.get('DeviceNode') == mounted[0].get('dev-entry')
disks = [entry['dev-entry'] for entry in entities if entry.get('content-hint') == 'GUID_partition_scheme']
assert len(disks) == 1 and disks[0].startswith('/dev/disk')
command(['/sbin/mount', '-u', '-o', 'nosuid,nodev', mount])
command([worker, 'flags', mount])
results = []
for sequence, variant in enumerate(['baseline', 'literal-root-read', 'literal-root-data', 'ancestor-directory-data', 'read-all-control']):
    command([worker, 'empty'])
    root, outside = make_case(sequence)
    policy = base / ('profile-' + str(sequence) + '.sb')
    text = profile_for(root)
    if variant == 'literal-root-read':
        text += '(allow file-read* (literal "/"))\n'
    if variant == 'literal-root-data':
        text += '(allow file-read-data (literal "/"))\n'
    if variant == 'ancestor-directory-data':
        ancestors = {path for item in (root, worker, pathlib.Path('/usr/lib'), pathlib.Path('/System/Library'), pathlib.Path('/dev/null')) for path in item.parents}
        text += '(allow file-read-data ' + ' '.join('(literal ' + json.dumps(str(path)) + ')' for path in sorted(ancestors)) + ')\n'
    if variant == 'read-all-control':
        text += '(allow file-read*)\n'
    policy.write_text(text)
    os.chmod(policy, 0o644)
    code, data = command([worker, 'launch', 'confined', policy, 'allowed-read', root, outside], allowed=range(-128, 256))
    command([worker, 'empty'])
    observation = {'variant': variant, 'status': code, 'entered': b'GUEST entered' in data}
    results.append(observation)
    print('DIAGNOSTIC ' + json.dumps(observation), flush=True)
command([worker, 'unmount', mount])
command(['/usr/bin/hdiutil', 'detach', disks[0]])
print('DIAGNOSTICS-COMPLETE ' + json.dumps(results) + ' uid_empty=1 nonforced_detach=1', flush=True)
assert len(owned_pids) == 5 and all(isinstance(pid, int) and pid > 1 for pid in owned_pids)
predicate = ' OR '.join('eventMessage CONTAINS "worker(' + str(pid) + ')"' for pid in owned_pids)
command(['/usr/bin/log', 'show', '--last', '2m', '--info', '--debug', '--style', 'compact', '--predicate', predicate], allowed=(0, 1))
# Diagnostics do not define a full filesystem pass; broad grants never become product policy.
