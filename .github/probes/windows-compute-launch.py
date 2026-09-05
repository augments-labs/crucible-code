"""Disposable native Windows compute prerequisite; never a backend or user workload."""
import json
import re
import shutil
import subprocess
import threading
import uuid

IMAGE = 'mcr.microsoft.com/windows/nanoserver@sha256:4249ba8974b8996812967d35c46c4e66afd771f929f96917ef6f7592a55edb12'
CONFIG = 'sha256:9ad1338502d7e1ce244fc1e8e6d2c307f6201dfa9bf9e0428b99b78cb1633075'
MARKER = 'CRUCIBLE-NATIVE-WINDOWS-PROBE'
LABEL = 'crucible.disposable-native-probe'

class UnknownCompletion(RuntimeError):
    """Client completion cannot establish the daemon operation's final state."""

def require(condition, message):
    if not condition:
        raise RuntimeError(message)

executable = shutil.which('docker.exe')
require(executable is not None, 'preinstalled Docker missing')
require(shutil.disk_usage('C:\\').free >= 8 * 1024**3, 'disk prerequisite unavailable')

def query(args, timeout=20):
    process = subprocess.Popen([executable, '-H', 'npipe:////./pipe/docker_engine', *args],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = [bytearray(), bytearray()]
    overflow = threading.Event()
    errors = threading.Event()
    def reader(index, stream):
        try:
            while True:
                block = stream.read(4096)
                if not block:
                    break
                if len(output[index]) + len(block) > 65536:
                    overflow.set()
                    process.kill()
                    break
                output[index].extend(block)
        except Exception:
            errors.set()
        finally:
            stream.close()
    threads = [threading.Thread(target=reader, args=(i, stream), daemon=True)
               for i, stream in enumerate((process.stdout, process.stderr))]
    try:
        try:
            for thread in threads:
                thread.start()
            process.wait(timeout=timeout)
        except Exception as error:
            raise UnknownCompletion('local query did not complete; daemon completion unknown') from error
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
        for thread in threads:
            try:
                thread.join(timeout=3)
            except Exception:
                cleanup_failed = True
        if cleanup_failed:
            raise UnknownCompletion('local client cleanup failed; daemon completion unknown')
    if overflow.is_set() or errors.is_set() or any(t.is_alive() for t in threads):
        raise UnknownCompletion('query stream failure, bound or cleanup unknown')
    return process.returncode, output[0].decode('utf-8').strip(), output[1].decode('utf-8').strip()

def success(args, timeout=20):
    status, stdout, stderr = query(args, timeout)
    require(status == 0, 'local command failed: ' + stderr[:512])
    return stdout

def inspect(name):
    # Exact-name lookup avoids treating unrelated daemon errors as absence.
    ids = success(['container', 'ls', '--all', '--filter', 'name=^/' + name + '$', '--format', '{{.ID}}']).splitlines()
    require(len(ids) <= 1, 'container lookup ambiguous')
    if not ids:
        return None
    require(re.fullmatch('[0-9a-f]{12,64}', ids[0]) is not None, 'invalid container identity')
    data = json.loads(success(['container', 'inspect', name]))
    require(isinstance(data, list) and len(data) == 1, 'container inspect shape')
    item = data[0]
    require(item['Name'] == '/' + name and item['Id'].startswith(ids[0]), 'container identity mismatch')
    require(re.fullmatch('[0-9a-f]{64}', item['Id']) is not None, 'invalid full identity')
    return item

require(success(['info', '--format', '{{.OSType}}']) == 'windows', 'not a Windows engine')
print('PINNED-IMAGE-PULL-BEGIN', flush=True)
success(['pull', '--quiet', IMAGE], 180)
image = json.loads(success(['image', 'inspect', IMAGE]))
require(len(image) == 1 and image[0]['Id'] == CONFIG and image[0]['Os'] == 'windows'
        and image[0]['Architecture'] == 'amd64', 'pinned Windows image mismatch')
print('PINNED-IMAGE-VERIFIED config=' + CONFIG, flush=True)
for isolation in ('hyperv', 'process'):
    nonce = uuid.uuid4().hex
    name = 'crucible-native-' + nonce
    attempted = False
    uncertain = False
    print('COMPUTE-CELL-BEGIN isolation=' + isolation, flush=True)
    require(inspect(name) is None, 'unexpected preexisting container')
    try:
        attempted = True
        identifier = success(['container', 'create', '--name', name, '--label', LABEL + '=' + nonce,
                              '--isolation', isolation, '--network', 'none', '--user', 'ContainerUser',
                              '--memory', '1g', '--cpu-count', '1', '--entrypoint', 'cmd.exe',
                              IMAGE, '/d', '/s', '/c', 'echo ' + MARKER], 60)
        item = inspect(name)
        require(item is not None and item['Id'] == identifier, 'create identity unknown')
        host = item['HostConfig']
        require(item['Image'] == CONFIG and item['Config']['Labels'].get(LABEL) == nonce,
                'container ownership mismatch')
        require(item['Config']['User'] == 'ContainerUser' and host['Isolation'] == isolation
                and host['NetworkMode'] == 'none' and not item['Mounts']
                and not host.get('Binds') and not host.get('PortBindings'), 'boundary configuration mismatch')
        require(host['Memory'] == 1024**3 and host['CpuCount'] == 1, 'resource configuration mismatch')
        output = success(['container', 'start', '--attach', identifier], 60)
        item = inspect(name)
        require(item is not None and not item['State']['Running'] and item['State']['ExitCode'] == 0,
                'guest exit not confirmed')
        require(output == MARKER, 'guest marker mismatch')
        print('COMPUTE-CELL-RESULT isolation=' + isolation + ' native_marker=true full_sandbox_tested=false', flush=True)
    except Exception as error:
        uncertain = isinstance(error, UnknownCompletion)
        print('COMPUTE-CELL-RESULT isolation=' + isolation + ' unavailable=' + str(error)[:768], flush=True)
    finally:
        if attempted:
            item = inspect(name)
            if item is not None:
                require(item['Image'] == CONFIG and item['Config']['Labels'].get(LABEL) == nonce,
                        'QUARANTINE cleanup ownership unknown')
                success(['container', 'rm', '--force', item['Id']])
            require(inspect(name) is None, 'QUARANTINE container remains')
            print('COMPUTE-CELL-CLEANUP isolation=' + isolation + ' absent_at_query=true completion_unknown=' + str(uncertain), flush=True)
    require(not uncertain, 'QUARANTINE daemon completion unknown; no further cell')
print('NATIVE-COMPUTE-DIAGNOSTIC-COMPLETE full_sandbox_tested=false')
