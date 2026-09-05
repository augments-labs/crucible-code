"""Finite synthetic native filesystem tests; experiment branch only."""
import json
import os
import pathlib
import plistlib
import shutil
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


def command(argv, allowed=(0,), timeout=30):
    print('COMMAND ' + json.dumps([str(x) for x in argv]), flush=True)
    with tempfile.TemporaryFile() as output:
        proc = subprocess.Popen(argv, cwd=base, stdin=subprocess.DEVNULL,
                                stdout=output, stderr=subprocess.STDOUT,
                                close_fds=True, start_new_session=True,
                                env={'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LANG': 'C'})
        try:
            result = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            try:
                proc.wait(timeout=3)
                print('TIMEOUT direct_process_reaped=1 fixture_quarantined=1', flush=True)
            except subprocess.TimeoutExpired:
                print('TIMEOUT direct_process_reaped=0 fixture_quarantined=1', flush=True)
            raise RuntimeError('No further fixture operations after uncertainty')
        size = output.tell()
        output.seek(0)
        data = output.read(32769)
        assert size <= 32768 and len(data) <= 32768, 'output bound'
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
operations = ['allowed-read', 'allowed-write', 'source-read', 'source-write', 'readonly-write',
              'protected-write', 'protected-unlink', 'protected-rename', 'ancestor-rename',
              'protected-link', 'protected-symlink', 'source-symlink', 'unreadable-read',
              'unreadable-alias', 'unreadable-case', 'unreadable-create', 'unreadable-rename', 'device', 'setuid']
failures = []
sequence = 0
for mode in ('control', 'confined'):
    for operation in operations:
        command([worker, 'empty'])
        root, outside = make_case(sequence)
        policy = base / ('profile-' + str(sequence) + '.sb')
        policy.write_text(profile_for(root))
        os.chmod(policy, 0o644)
        code, data = command([worker, 'launch', mode, policy, operation, root, outside], allowed=(0, 1, 77))
        # Every guest fixture is single-process, including exec of the setuid worker.
        # The parent wait reaps it. A new case requires an error-checked empty UID.
        command([worker, 'empty'])
        if code:
            failures.append({'mode': mode, 'operation': operation, 'status': code})
        print('RESULT ' + json.dumps({'mode': mode, 'operation': operation, 'status': code}), flush=True)
        sequence += 1
command([worker, 'unmount', mount])
command(['/usr/bin/hdiutil', 'detach', disks[0]])
print('COMPLETE cases=' + str(sequence) + ' failures=' + json.dumps(failures) + ' uid_empty=1 nonforced_detach=1', flush=True)
assert not failures, failures
# Retain exact owned fixture until disposable VM destruction; never generic cleanup.
