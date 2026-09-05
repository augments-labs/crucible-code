"""Disposable exact-image APFS reference and readonly-export control."""
import hashlib
import json
import os
import pathlib
import plistlib
import select
import signal
import subprocess
import sys
import tempfile

assert os.geteuid() == 0
base = pathlib.Path(tempfile.mkdtemp(prefix='crucible-apfs-publication-')).resolve(strict=True)
os.chmod(base, 0o700)
image = base / 'fixture.dmg'
rw = base / 'rw'
ro = base / 'ro'
rw.mkdir(); ro.mkdir()
worker = pathlib.Path(sys.argv[1]).resolve(strict=True)
print('fixture=' + str(base), flush=True)


def command(argv, allowed=(0,), timeout=30):
    print('command=' + json.dumps([str(x) for x in argv]), flush=True)
    with tempfile.TemporaryFile() as output:
        proc = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT,
                                cwd=base, start_new_session=True)
        try:
            result = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            # This is one owned synchronous native command. Never touch its image
            # again after uncertain completion, even if termination now succeeds.
            proc.kill()
            try:
                proc.wait(timeout=3)
                print('timed_out_operation_reaped=1', flush=True)
            except subprocess.TimeoutExpired:
                print('timed_out_operation_reaped=0', flush=True)
            raise RuntimeError('operation timed out; all fixture state quarantined')
        size = output.tell()
        output.seek(0)
        data = output.read(16385)
        assert size <= 16384 and len(data) <= 16384, 'operation output exceeded bound'
    print(data.decode(errors='replace'), end='', flush=True)
    assert result in allowed, (argv[0], result)
    return result, data


def volume(path):
    _, data = command(['/usr/sbin/diskutil', 'info', '-plist', str(path)])
    value = plistlib.loads(data)
    assert value.get('FilesystemType') == 'apfs', value
    identity = value.get('VolumeUUID')
    device = value.get('DeviceNode')
    assert isinstance(identity, str) and identity
    assert isinstance(device, str) and device.startswith('/dev/disk')
    assert value.get('MountPoint') == str(path)
    return identity, device


def attach(path, readonly=False):
    argv = ['/usr/bin/hdiutil', 'attach', str(image), '-nobrowse', '-owners', 'on', '-mountpoint', str(path), '-plist']
    if readonly:
        argv.append('-readonly')
    _, data = command(argv)
    entities = plistlib.loads(data)['system-entities']
    mounted = [e for e in entities if e.get('mount-point') == str(path)]
    assert len(mounted) == 1, entities
    identity, device = volume(path)
    assert mounted[0].get('dev-entry') == device, (mounted, device)
    # The image is a single fixture. The attach receipt owns its whole device.
    disks = [e['dev-entry'] for e in entities if e.get('content-hint') == 'GUID_partition_scheme']
    assert len(disks) == 1 and disks[0].startswith('/dev/disk'), entities
    _, flags = command([worker, 'flags', path])
    assert ('readonly=' + str(int(readonly))).encode() in flags
    return identity, disks[0]


def digest():
    with image.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


command(['/usr/bin/sw_vers'])
command(['/usr/bin/uname', '-a'])
command(['/usr/bin/hdiutil', 'create', '-size', '128m', '-fs', 'APFS', '-volname', 'CrucibleFixture', str(image)])
expected_uuid = None
for mode in ['read', 'write', 'mmap', 'cwd', 'aio']:
    uuid, disk = attach(rw)
    if expected_uuid is None:
        expected_uuid = uuid
    assert uuid == expected_uuid
    payload = rw / 'payload'
    payload.write_bytes(b'Z' * 4096)
    argument = rw if mode == 'cwd' else payload
    # Worker only owns this fixture reference, never forks, and has a 30s alarm.
    proc = subprocess.Popen([worker, mode, argument], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, cwd=base)
    try:
        assert select.select([proc.stdout], [], [], 5)[0], 'holder did not report ready'
        assert proc.stdout.readline(64) == b'READY\n', 'holder did not establish reference'
        result, _ = command([worker, 'unmount', rw], allowed=(0, 16))
        assert result == 16, (mode, 'nonforced unmount accepted a live reference')
        assert proc.poll() is None, 'holder exited before busy check'
    finally:
        if proc.poll() is None:
            proc.kill()
        try:
            result = proc.wait(timeout=3)
            print('holder=' + mode + ' reaped=1 status=' + str(result), flush=True)
        except subprocess.TimeoutExpired:
            print('holder=' + mode + ' reaped=0', flush=True)
            raise RuntimeError('holder unresolved; fixture quarantined')
        proc.stdout.close()
    data = payload.read_bytes()
    expected = {'write':b'W', 'mmap':b'M', 'aio':b'A'}.get(mode,b'Z') + b'Z' * 4095
    assert data == expected, 'write control did not reach expected bytes'
    command([worker, 'unmount', rw])
    command(['/usr/bin/hdiutil', 'detach', disk])
    before = digest()
    exported_uuid, export_disk = attach(ro, readonly=True)
    assert exported_uuid == expected_uuid, 'exported another volume'
    assert (ro / 'payload').read_bytes() == expected
    command([worker, 'readonly', ro / 'payload'])
    command([worker, 'unmount', ro])
    command(['/usr/bin/hdiutil', 'detach', export_disk])
    assert digest() == before, 'readonly interval mutated backing image'
    print('case=' + mode + ' busy=1 holder_reaped=1 unmounted=1 same_uuid=1 readonly=1 stable_image=1', flush=True)
print('all_five_cases_pass=1', flush=True)
# Retain scratch image and diagnostics in the disposable VM; no generic cleanup.
