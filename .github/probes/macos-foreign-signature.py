"""One controlled root bystander and one permanently nonroot restricted caller."""
import hashlib
import os
import pathlib
import re
import selectors
import sqlite3
import stat
import struct
import subprocess
import sys
import time

def require(ok, message):
    if not ok:
        raise RuntimeError(message)

root = pathlib.Path(sys.argv[1])
uid, gid = int(sys.argv[2]), int(sys.argv[3])
nonce = sys.argv[4]
require(os.getuid() == 0 and 0 < uid < 2**31 and 0 <= gid < 2**31, 'controller identity')
require(re.fullmatch('[0-9a-f]{32}', nonce), 'nonce')
require(str(root).startswith('/private/tmp/crucible-macos-foreign-signature.')
        and root.resolve(strict=True) == root and stat.S_ISDIR(root.lstat().st_mode), 'fixture root')
os.chown(root, 0, 0)
os.chmod(root, 0o755)
for name in ('target', 'caller', 'nonce.h', 'macos-foreign-signature-target.c', 'macos-foreign-signature-caller.c', 'macos-foreign-signature.py'):
    path = root / name
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_size <= 1024*1024, 'fixture file')
    os.chown(path, 0, 0)
    os.chmod(path, 0o755 if name in ('target','caller') else 0o644)
binary = root / 'target'
raw = binary.read_bytes()
fingerprint = hashlib.sha256(raw).digest()
require(len(raw) >= 32, 'Mach-O header')
magic, cpu, subtype, filetype, count, size, flags, reserved = struct.unpack_from('<8I', raw)
require(magic == 0xfeedfacf and cpu == 0x1000007 and filetype == 2 and count <= 128 and size <= len(raw)-32, 'thin Intel executable')
position, identifiers = 32, []
for _ in range(count):
    require(position + 8 <= 32 + size, 'load command header')
    command, length = struct.unpack_from('<2I', raw, position)
    require(length >= 8 and length % 4 == 0 and length <= 32+size-position, 'load command length')
    require(command != 0x1d, 'embedded signature is outside this calibration')
    if command == 0x1b:
        require(length == 24, 'UUID command length')
        identifiers.append(b'UUID' + raw[position+8:position+24])
    position += length
require(position == 32+size and len(identifiers) == 1, 'unique UUID')
identification = identifiers[0]
caller_bytes = (root / 'caller').read_bytes()
require(hashlib.sha256(caller_bytes).digest() != fingerprint and identification[4:] not in caller_bytes,
        'caller must have a distinct code identity from bystander')
identifier = 'com.crucible.disposable-foreign-signature.' + nonce
expiry = time.monotonic() + 45

def remaining(cap):
    value = min(cap, expiry-time.monotonic())
    require(value > 0, 'overall deadline exceeded')
    return value

def database_count():
    path = pathlib.Path('/private/var/db/DetachedSignatures')
    if not path.exists():
        require(not path.is_symlink(), 'database symlink')
        return (0, 0, 0)
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_nlink == 1, 'database identity')
    with sqlite3.connect('file:/private/var/db/DetachedSignatures?mode=ro', uri=True, timeout=1) as db:
        db.set_progress_handler(lambda: int(time.monotonic() >= min(expiry, query_deadline)), 1000)
        query_deadline = time.monotonic()+1
        exists = db.execute("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='code'").fetchone()[0]
        if not exists:
            return (0, 0, 0)
        count = db.execute('SELECT count(*) FROM code WHERE substr(identification,1,20)=? OR identifier=?', (identification, identifier)).fetchone()[0]
        owned = db.execute('SELECT count(*), coalesce(max(length(code.signature)),0) FROM code JOIN global ON code.global=global.id WHERE code.identification=? AND code.identifier=? AND global.sign_location=?', (identification, identifier, str(binary))).fetchone()
        return (count, owned[0], owned[1])

def cleanup(process, label):
    failed = False
    needs_termination = True
    try:
        needs_termination = process.poll() is None
    except Exception:
        failed = True
    if needs_termination:
        try:
            process.kill()
        except Exception:
            failed = True
    try:
        process.wait(timeout=3)
    except Exception:
        failed = True
    for stream in (process.stdin, process.stdout, process.stderr):
        if stream is not None:
            try:
                stream.close()
            except Exception:
                failed = True
    if failed:
        raise RuntimeError('QUARANTINE ' + label + ' cleanup or completion unknown; no retry/reuse')

def run_sign():
    command = ['/usr/bin/codesign', '--detached-database', '--sign', '-', '--identifier', identifier, '--timestamp=none', str(binary)]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env={'PATH':'/usr/bin:/bin'})
    output = bytearray()
    try:
        deadline = time.monotonic()+remaining(10)
        with selectors.DefaultSelector() as selected:
            os.set_blocking(process.stdout.fileno(), False)
            selected.register(process.stdout, selectors.EVENT_READ)
            while selected.get_map():
                require(time.monotonic() < deadline, 'QUARANTINE signing completion unknown; no retry')
                for key, _ in selected.select(0.1):
                    block = os.read(key.fileobj.fileno(), 1024)
                    if not block:
                        selected.unregister(key.fileobj)
                    else:
                        require(len(output)+len(block) <= 16384, 'signing output bound')
                        output.extend(block)
            status = process.wait(timeout=max(0.01, deadline-time.monotonic()))
        print('SIGN status=' + str(status) + ' output=' + repr(output.decode('utf-8')), flush=True)
        require(status == 0, 'signing failed; preserve unknown database effects, no retry')
    finally:
        cleanup(process, 'codesign')

def run_caller():
    command = [str(root / 'caller'), str(target.pid)]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env={'PATH':'/usr/bin:/bin'}, stdin=subprocess.DEVNULL, preexec_fn=drop)
    output = bytearray()
    try:
        deadline = time.monotonic()+remaining(10)
        with selectors.DefaultSelector() as selected:
            os.set_blocking(process.stdout.fileno(), False)
            selected.register(process.stdout, selectors.EVENT_READ)
            while selected.get_map():
                require(time.monotonic() < deadline, 'QUARANTINE caller completion unknown; no retry')
                for key, _ in selected.select(0.1):
                    block = os.read(key.fileobj.fileno(), 1024)
                    if not block:
                        selected.unregister(key.fileobj)
                    else:
                        require(len(output)+len(block) <= 8192, 'caller output bound')
                        output.extend(block)
            status = process.wait(timeout=max(0.01, deadline-time.monotonic()))
        print('CALLER-RESULT status=' + str(status) + ' output=' + repr(output.decode('utf-8')), flush=True)
        require(status == 0, 'caller failed; preserve possible target effects, no retry')
    finally:
        cleanup(process, 'caller')

require(database_count() == (0, 0, 0), 'prospective UUID/identifier already present; no signing')
print('DATABASE preflight_owned_rows=0', flush=True)

def drop():
    os.setgroups([gid])
    os.setgid(gid)
    os.setuid(uid)
    if os.getuid() != uid or os.geteuid() != uid:
        os._exit(77)

target = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, env={'PATH':'/usr/bin:/bin'})
pending = bytearray()
output_bytes = 0

def line():
    global output_bytes
    with selectors.DefaultSelector() as selected:
        os.set_blocking(target.stdout.fileno(), False)
        selected.register(target.stdout, selectors.EVENT_READ)
        while b'\n' not in pending:
            remaining(1)
            for key, _ in selected.select(0.1):
                block = os.read(key.fileobj.fileno(), 1024)
                require(block, 'target exited before result')
                output_bytes += len(block)
                require(output_bytes <= 8192, 'target output bound')
                pending.extend(block)
    value, _, tail = pending.partition(b'\n')
    pending[:] = tail
    text = value.decode('ascii')
    print(text, flush=True)
    return text

def state(stage):
    value = line()
    while value.startswith('RPC '):
        value = line()
    match = re.fullmatch(r'STATE stage='+stage+r' flags=(\d+) hash_status=(-?\d+) hash_errno=(\d+) hash=([0-9a-f]{40})', value)
    require(match, 'unexpected target state')
    return tuple(match.groups())

def send(value):
    target.stdin.write(value)
    target.stdin.flush()

try:
    require(line() == 'READY nonce='+nonce+' uid=0 pid='+str(target.pid), 'direct owned target identity')
    before = state('before')
    require(before[1] == '-1' and int(before[2]) > 0, 'baseline already has kernel CDHASH')
    send(b'b')
    baseline = state('baseline')
    require(baseline == before, 'baseline RPC changed target before detached signature exists')
    run_sign()
    require(hashlib.sha256(binary.read_bytes()).digest() == fingerprint, 'signing changed executable bytes')
    rows = database_count()
    require(rows[0:2] == (1, 1) and 32 <= rows[2] <= 1024*1024, 'exact new detached signature record unavailable')
    print('DATABASE owned_code_rows=1 owned_global_reference=1 unchanged_executable=1', flush=True)
    send(b'q')
    available = state('available')
    require(available == before, 'INCONCLUSIVE signature attached before calibrated RPC')
    run_caller()
    send(b'f')
    foreign = state('foreign')
    require(hashlib.sha256(binary.read_bytes()).digest() == fingerprint, 'foreign call changed executable bytes')
    if foreign != before:
        print('FOREIGN signature_change_observed=1 accepted_v3_invariance_failed=1 full_backend=0', flush=True)
        raise RuntimeError('TCB CONTRACT VIOLATION: controlled foreign target signature state changed')
    send(b'a')
    after = state('after')
    status = target.wait(timeout=remaining(3))
    require(status == 0, 'target failed')
    require(after[1] == '0' and after[3] != '0'*40, 'INCONCLUSIVE RPC did not attach observable CDHASH')
    print('FOREIGN signature_unchanged=1 exact_self_positive_control=1 full_backend=0', flush=True)
finally:
    cleanup(target, 'target')
    print('FOREIGN-CLEANUP controlled_bystander_reaped=1 fixture_and_own_database_record_retained_for_VM_disposal=1', flush=True)
