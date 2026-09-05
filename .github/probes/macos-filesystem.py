"""Finite synthetic native filesystem tests; experiment branch only."""
import json
import hashlib
import os
import pathlib
import plistlib
import re
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


def command(argv, allowed=(0,), timeout=30, private=False):
    print('COMMAND ' + json.dumps([str(x) for x in argv]), flush=True)
    proc = subprocess.Popen(argv, cwd=base, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            close_fds=True, start_new_session=True,
                            env={'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LANG': 'C'})
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
    if not private:
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
    owned_file(pathlib.Path(str(outside) + '-sibling'))
    os.symlink(root / 'rw/nested/.git/config', root / 'rw/protected-alias')
    os.symlink(root / 'rw/secret', root / 'rw/secret-alias')
    os.symlink(outside, root / 'rw/source-alias')
    shutil.copyfile(worker, root / 'rw/native-worker')
    os.chown(root / 'rw/native-worker', 60000, 60000)
    os.chmod(root / 'rw/native-worker', 0o755)
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
        '(version 1)', '(deny default)', '(deny network*)',
        '(allow process-exec)', '(allow process-info* (target self))',
        '(deny mach-lookup)', '(deny mach-register)',
        '(deny file-read*)', '(deny file-write*)',
        '(allow file-read-data (literal "/"))',
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
backing = image.lstat()
assert stat.S_ISREG(backing.st_mode) and backing.st_uid == 0 and backing.st_nlink == 1
_, data = command(['/usr/bin/hdiutil', 'attach', image, '-nobrowse', '-owners', 'on', '-mountpoint', mount, '-plist'])
entities = plistlib.loads(data)['system-entities']
mounted = [entry for entry in entities if entry.get('mount-point') == str(mount)]
assert len(mounted) == 1
_, data = command(['/usr/sbin/diskutil', 'info', '-plist', mount])
info = plistlib.loads(data)
assert info.get('FilesystemType') == 'apfs' and info.get('MountPoint') == str(mount)
assert info.get('DeviceNode') == mounted[0].get('dev-entry')
disks = [entry['dev-entry'] for entry in entities if entry.get('content-hint') == 'GUID_partition_scheme']
assert len(disks) == 1 and re.fullmatch(r'/dev/disk[0-9]+', disks[0])
device = os.stat(disks[0], follow_symlinks=False)
assert stat.S_ISBLK(device.st_mode)
command(['/sbin/mount', '-u', '-o', 'nosuid,nodev', mount])
command([worker, 'flags', mount])
def protected_state(root):
    protected = root / 'rw/nested/.git'
    names = sorted(path.name for path in protected.parent.iterdir())
    assert names == ['.git'], names
    result = {'names': names}
    for label, path in [('directory', protected), ('file', protected / 'config')]:
        info = path.lstat()
        result[label] = {'inode': info.st_ino, 'mode': info.st_mode, 'uid': info.st_uid,
                         'gid': info.st_gid, 'nlink': info.st_nlink, 'flags': info.st_flags}
    result['sha256'] = hashlib.sha256((protected / 'config').read_bytes()).hexdigest()
    return result


operations = ['allowed-read', 'allowed-write', 'source-read', 'source-write', 'sibling-read', 'sibling-write', 'readonly-write',
              'protected-read', 'protected-case-write', 'protected-case-rename', 'protected-clear-rename', 'protected-chmod-rename', 'protected-fchmod-write', 'protected-write', 'protected-replace', 'protected-unlink', 'protected-rename', 'ancestor-rename',
              'protected-link', 'protected-symlink', 'source-symlink', 'unreadable-read',
              'unreadable-alias', 'unreadable-case', 'unreadable-create', 'unreadable-rename', 'unreadable-create-case', 'unreadable-rename-case', 'unreadable-mkdir-case', 'device', 'setuid', 'ordinary-exec']
failures = []
sequence = 0
for mode in ('control', 'confined'):
    for operation in operations:
        command([worker, 'empty'])
        root, outside = make_case(sequence)
        if mode == 'confined':
            protected = root / 'rw/nested/.git'
            for item, bits in [(protected / 'config', 0o444), (protected, 0o555)]:
                os.chown(item, 0, 0)
                os.chmod(item, bits)
            before = protected_state(root)
        policy = base / ('profile-' + str(sequence) + '.sb')
        policy.write_text(profile_for(root))
        os.chmod(policy, 0o644)
        code, data = command([worker, 'launch', mode, policy, operation, root, outside], allowed=range(-128, 256))
        # Every guest fixture is single-process, including exec of the setuid worker.
        # The parent wait reaps it. A new case requires an error-checked empty UID.
        command([worker, 'empty'])
        if mode == 'confined':
            after = protected_state(root)
            assert after == before, (operation, before, after)
            print('PROTECTED-UNCHANGED ' + operation, flush=True)
        if code:
            failures.append({'mode': mode, 'operation': operation, 'status': code})
        print('RESULT ' + json.dumps({'mode': mode, 'operation': operation, 'status': code}), flush=True)
        sequence += 1
command([worker, 'unmount', mount])
def attached():
    _, data = command(['/usr/bin/hdiutil', 'info', '-plist'], private=True)
    matches = [entry for entry in plistlib.loads(data)['images']
               if pathlib.Path(entry['image-path']).resolve() == image]
    assert len(matches) <= 1, 'ambiguous image identity; quarantine'
    return matches


detached = False
for attempt in range(1, 4):
    current = image.lstat()
    assert stat.S_ISREG(current.st_mode)
    assert (current.st_dev, current.st_ino) == (backing.st_dev, backing.st_ino)
    matches = attached()
    assert len(matches) == 1, 'attachment missing before attempt; quarantine'
    entries = matches[0]['system-entities']
    assert sum(entry.get('dev-entry') == disks[0] for entry in entries) == 1
    assert not any(entry.get('mount-point') for entry in entries)
    current_device = os.stat(disks[0], follow_symlinks=False)
    assert stat.S_ISBLK(current_device.st_mode) and current_device.st_rdev == device.st_rdev
    code, _ = command(['/usr/bin/hdiutil', 'detach', disks[0]], allowed=(0, 16))
    print('DETACH ' + json.dumps({'attempt': attempt, 'status': code, 'identity_reconciled': True}), flush=True)
    if code == 0:
        assert not attached(), 'successful detach still has attachment; quarantine'
        detached = True
        break
    time.sleep(0.2)
assert detached, 'bounded detach exhausted; image quarantined'
print('COMPLETE cases=' + str(sequence) + ' failures=' + json.dumps(failures) + ' uid_empty=1 nonforced_detach=1', flush=True)
assert not failures, failures
# Retain exact owned fixture until disposable VM destruction; never generic cleanup.
