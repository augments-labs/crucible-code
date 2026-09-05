"""Query only the local Windows Docker engine; bounded streams, no mutations."""
import re
import shutil
import subprocess
import threading

def require(condition, message):
    if not condition:
        raise RuntimeError(message)

executable = shutil.which('docker.exe')
require(executable is not None, 'preinstalled Docker unavailable')
endpoint = 'npipe:////./pipe/docker_engine'

def query(arguments):
    process = subprocess.Popen([executable, '-H', endpoint, *arguments],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = [None, None]
    def read(index, stream):
        try:
            output[index] = stream.read(8192)
        finally:
            stream.close()
    threads = [threading.Thread(target=read, args=(i, stream), daemon=True)
               for i, stream in enumerate((process.stdout, process.stderr))]
    for thread in threads:
        thread.start()
    try:
        process.wait(timeout=15)
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=3)
        for thread in threads:
            thread.join(timeout=3)
    require(all(not thread.is_alive() for thread in threads), 'query stream cleanup unknown')
    require(all(data is not None and len(data) < 8192 for data in output), 'query incomplete or bounded')
    print(f'LOCAL-QUERY status={process.returncode} stdout_bytes={len(output[0])} stderr_bytes={len(output[1])}')
    require(process.returncode == 0, 'local Docker query failed')
    return output[0].decode('utf-8').strip()

info = query(['info', '--format', '{{.OSType}}|{{.Driver}}|{{.ServerVersion}}'])
require(re.fullmatch(r'windows\|[A-Za-z0-9_-]{1,64}\|[A-Za-z0-9_.+-]{1,64}', info) is not None, 'native Windows engine not confirmed')
print('WINDOWS-ENGINE ' + info)
images = query(['image', 'ls', '--filter', 'reference=mcr.microsoft.com/windows/*',
                '--format', '{{.Repository}}:{{.Tag}}']).splitlines()
require(len(images) <= 16, 'cached image inventory bound')
known = sorted(set(image for image in images if re.fullmatch(
    r'mcr\.microsoft\.com/windows/(servercore|nanoserver):ltsc(2022|2025)', image)))
for image in known:
    print('CACHED-OFFICIAL-IMAGE ' + image)
print(f'IMAGE-INVENTORY matched={len(known)} other_rows={len(images)-len(known)}')
