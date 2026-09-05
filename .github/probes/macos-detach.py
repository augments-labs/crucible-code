"""Disposable exact-image detach investigation; never production code."""
import ctypes
import hashlib
import json
import os
import pathlib
import plistlib
import re
import select
import stat
import subprocess
import sys
import tempfile
import time

assert sys.platform == 'darwin' and os.geteuid() == 0
assert os.environ.get('GITHUB_ACTIONS') == 'true'
base = pathlib.Path(tempfile.mkdtemp(prefix='crucible-detach-probe-')).resolve()
os.chmod(base, 0o700)
libc = ctypes.CDLL(None, use_errno=True)
libc.unmount.argtypes = [ctypes.c_char_p, ctypes.c_int]
libc.unmount.restype = ctypes.c_int
held = None


def close_owned():
    global held
    if held is not None:
        fd, held = held, None
        os.close(fd)
        print('OWNED_DEVICE_REFERENCE_CLOSED', flush=True)


def command(argv, timeout=30, private=False):
    print('COMMAND ' + json.dumps([str(x) for x in argv]), flush=True)
    proc = subprocess.Popen(argv, cwd=base, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            close_fds=True, start_new_session=True,
                            env={'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LANG': 'C'})
    data = bytearray()
    deadline = time.monotonic() + timeout
    try:
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise RuntimeError('operation deadline; image quarantined')
            if select.select([proc.stdout], [], [], min(left, 0.1))[0]:
                chunk = os.read(proc.stdout.fileno(), min(4096, 32769-len(data)))
                if not chunk:
                    break
                data.extend(chunk)
                if len(data) > 32768:
                    raise RuntimeError('output bound; image quarantined')
        status = proc.wait(timeout=max(0.01, deadline-time.monotonic()))
    except BaseException:
        # Release our controlled reference before the bounded direct-child reap.
        close_owned()
        if proc.poll() is None:
            proc.kill()
        try:
            proc.wait(timeout=3)
            print('UNCERTAIN direct_child_reaped=1 image_quarantined=1', flush=True)
        except subprocess.TimeoutExpired:
            print('UNCERTAIN direct_child_reaped=0 image_quarantined=1', flush=True)
        raise
    finally:
        proc.stdout.close()
    if not private:
        print(bytes(data).decode(errors='replace'), end='', flush=True)
    print('STATUS ' + str(status), flush=True)
    return status, bytes(data)


def success(argv, private=False):
    code, data = command(argv, private=private)
    assert code == 0, (code, 'image quarantined')
    return data


def attachments(image):
    data = success(['/usr/bin/hdiutil', 'info', '-plist'], private=True)
    found = [item for item in plistlib.loads(data)['images']
             if pathlib.Path(item['image-path']).resolve() == image]
    assert len(found) <= 1, 'ambiguous image receipt'
    return found


try:
    success(['/usr/bin/sw_vers'])
    success(['/usr/bin/uname', '-a'])
    for case in ['no-held-device', 'held-readonly-device']:
        image = base / (case + '.dmg')
        mount = base / (case + '-volume')
        mount.mkdir()
        success(['/usr/bin/hdiutil', 'create', '-size', '128m', '-fs', 'APFS',
                 '-volname', 'CrucibleDetachFixture', image])
        os.chmod(image, 0o600)
        backing = os.stat(image, follow_symlinks=False)
        assert stat.S_ISREG(backing.st_mode) and backing.st_uid == 0 and backing.st_nlink == 1
        data = success(['/usr/bin/hdiutil', 'attach', image, '-nobrowse', '-owners',
                        'on', '-mountpoint', mount, '-plist'])
        receipt = plistlib.loads(data)['system-entities']
        disks = [item['dev-entry'] for item in receipt
                 if item.get('content-hint') == 'GUID_partition_scheme'
                 and re.fullmatch(r'/dev/disk[0-9]+', item.get('dev-entry', ''))]
        assert len(disks) == 1
        assert sum(item.get('mount-point') == str(mount) for item in receipt) == 1
        disk = disks[0]
        device = os.stat(disk, follow_symlinks=False)
        assert stat.S_ISBLK(device.st_mode)
        payload = mount / 'payload'
        payload.write_bytes(b'x' * 4096)
        assert hashlib.sha256(payload.read_bytes()).hexdigest() == hashlib.sha256(b'x' * 4096).hexdigest()
        result = libc.unmount(os.fsencode(mount), 0)
        print('UNMOUNT ' + json.dumps({'case': case, 'result': result, 'errno': ctypes.get_errno()}), flush=True)
        assert result == 0, 'nonforced unmount failed; image quarantined'
        detached = False
        results = []
        for attempt in range(1, 4):
            current_backing = os.stat(image, follow_symlinks=False)
            assert (current_backing.st_dev, current_backing.st_ino) == (backing.st_dev, backing.st_ino)
            current = attachments(image)
            assert len(current) == 1, 'attachment missing before detach attempt'
            entries = current[0]['system-entities']
            assert any(item.get('dev-entry') == disk for item in entries), 'device receipt changed'
            assert all(not item.get('mount-point') for item in entries), 'image still mounted'
            current_device = os.stat(disk, follow_symlinks=False)
            assert stat.S_ISBLK(current_device.st_mode) and current_device.st_rdev == device.st_rdev
            print('OWNED_RECEIPT ' + json.dumps({'case': case, 'attempt': attempt,
                  'image': str(image), 'backing_device': backing.st_dev,
                  'backing_inode': backing.st_ino, 'disk': disk, 'device_number': device.st_rdev}), flush=True)
            start = time.monotonic()
            use_hold = case == 'held-readonly-device' and attempt == 1
            try:
                if use_hold:
                    held = os.open(disk, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
                    opened = os.fstat(held)
                    assert stat.S_ISBLK(opened.st_mode) and opened.st_rdev == device.st_rdev
                    print('OWNED_DEVICE_REFERENCE_OPEN readonly=1 nonblocking=1', flush=True)
                code, _ = command(['/usr/bin/hdiutil', 'detach', disk], timeout=4 if use_hold else 30)
            finally:
                close_owned()
            duration = time.monotonic() - start
            if use_hold:
                assert duration <= 5, 'controlled reference exceeded time box'
            results.append({'attempt': attempt, 'status': code, 'held': use_hold, 'seconds': duration})
            if code == 0:
                assert not attachments(image), 'successful detach still has attachment'
                detached = True
                break
            assert code == 16, 'unexpected detach outcome; image quarantined'
            time.sleep(0.2)
        print('RESULT ' + json.dumps({'case': case, 'attempts': results, 'detached': detached}), flush=True)
        assert detached, 'bounded nonforced detach failed; image quarantined'
    print('COMPLETE controlled_observations_only=1 original_v10_cause_unresolved=1', flush=True)
except BaseException:
    close_owned()
    print('QUARANTINED ' + str(base), flush=True)
    raise
