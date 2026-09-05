"""One owned synthetic launchd binding; never a production installer."""
import os
import pathlib
import plistlib
import re
import selectors
import stat
import subprocess
import sys
import time
import uuid

class UnknownCompletion(RuntimeError):
    pass

def require(condition, message):
    if not condition:
        raise RuntimeError(message)

require(os.getuid() == 0, 'root controller required')
root = pathlib.Path(sys.argv[1])
require(str(root).startswith('/private/tmp/crucible-macos-service.') and root.resolve(strict=True) == root
        and stat.S_ISDIR(root.lstat().st_mode), 'unexpected synthetic fixture')
os.chown(root, 0, 0)
os.chmod(root, 0o755)
for name in ('identity', 'macos-system-service.c', 'macos-system-service.py'):
    path = root / name
    require(stat.S_ISREG(path.lstat().st_mode) and path.lstat().st_nlink == 1, 'fixture file type')
    os.chown(path, 0, 0)
    os.chmod(path, 0o755 if name == 'identity' else 0o644)
label = 'com.crucible.disposable-task-access.' + uuid.uuid4().hex
service = 'system/' + label
plist = root / 'service.plist'
log = root / 'stdout'
error_log = root / 'stderr'
overall_deadline = time.monotonic() + 60

def query(*args):
    if time.monotonic() >= overall_deadline:
        raise UnknownCompletion('overall control deadline; no further operations')
    process = subprocess.Popen(['/bin/launchctl', *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = bytearray()
    deadline = min(overall_deadline, time.monotonic() + 20)
    try:
        with selectors.DefaultSelector() as selector:
            for stream in (process.stdout, process.stderr):
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() >= deadline:
                    raise UnknownCompletion('launchctl deadline; server completion unknown')
                for key, _ in selector.select(0.1):
                    block = os.read(key.fileobj.fileno(), 1024)
                    if not block:
                        selector.unregister(key.fileobj)
                    else:
                        if len(output) + len(block) > 16384:
                            raise UnknownCompletion('launchctl output bound')
                        output.extend(block)
            status = process.wait(timeout=max(0.01, deadline - time.monotonic()))
        return status, output.decode('utf-8', errors='strict')
    except Exception as error:
        raise UnknownCompletion('control operation completion unknown') from error
    finally:
        cleanup_failed = False
        try:
            if process.poll() is None:
                process.kill()
        except Exception:
            cleanup_failed = True
        try:
            process.wait(timeout=3)
        except Exception:
            cleanup_failed = True
        for stream in (process.stdout, process.stderr):
            try:
                stream.close()
            except Exception:
                cleanup_failed = True
        if cleanup_failed:
            raise UnknownCompletion('control client cleanup unknown')

def absent():
    status, text = query('print', service)
    return status == 113 and ('Could not find service "' + label + '" in domain for system') in text

def owned():
    status, text = query('print', service)
    require(status == 0, 'owned service query failed')
    require(re.search(r'^\s*path = ' + re.escape(str(plist)) + r'\s*$', text, re.M)
            and re.search(r'^\s*program = ' + re.escape(str(root / 'identity')) + r'\s*$', text, re.M),
            'QUARANTINE loaded service ownership mismatch')
    arguments = re.findall(r'(?ms)^[ \t]*arguments = \{\n(.*?)^[ \t]*\}[ \t]*$', text)
    require(len(arguments) == 1 and [line.strip() for line in arguments[0].splitlines() if line.strip()]
            == [str(root / 'identity')], 'QUARANTINE loaded service arguments mismatch')
    return text

require(absent(), 'preflight did not prove nonce service absent')
config = {'Label': label, 'ProgramArguments': [str(root / 'identity')], 'UserName': 'root',
          'RunAtLoad': True, 'KeepAlive': False, 'StandardOutPath': str(log), 'StandardErrorPath': str(error_log),
          'EnvironmentVariables': {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin'}}
with plist.open('xb') as stream:
    plistlib.dump(config, stream)
os.chmod(plist, 0o644)
started = False
uncertain = False
try:
    status, _ = query('bootstrap', 'system', str(plist))
    require(status == 0, 'bootstrap refused; no cleanup of an unconfirmed binding')
    started = True
    deadline = time.monotonic() + 30
    finished = False
    while time.monotonic() < deadline:
        value = owned()
        exit_match = re.search(r'^\s*last exit code = (\d+)\s*$', value, re.M)
        stopped = re.search(r'^\s*state = (not running|exited)\s*$', value, re.M)
        if exit_match and stopped:
            exit_code = int(exit_match.group(1))
            finished = True
            break
        time.sleep(0.1)
    require(finished, 'diagnostic daemon completion unavailable')
    with log.open('rb') as stream:
        data = stream.read(8193)
    require(len(data) <= 8192, 'daemon log bound')
    text = data.decode('utf-8')
    print(text, end='', flush=True)
    require(exit_code == 0, 'diagnostic daemon failed, exit=' + str(exit_code))
    require('SYSTEM-SERVICE-IDENTITY-END status=0 full_backend=0' in text, 'daemon identity result missing')
except UnknownCompletion:
    uncertain = True
    raise
finally:
    if started and not uncertain:
        owned()
        status, _ = query('bootout', 'system', str(plist))
        require(status == 0 and absent(), 'QUARANTINE binding cleanup unconfirmed')
        print('SYSTEM-SERVICE-CLEANUP binding_absent=1 files_retained_for_VM_disposal=1', flush=True)
    elif uncertain:
        print('QUARANTINE service_control_completion_unknown=1 no_reuse=1', flush=True)
