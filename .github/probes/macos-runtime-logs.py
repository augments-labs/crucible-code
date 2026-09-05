"""Bounded read-only native denial diagnostics; never a compatibility oracle."""
import os
import selectors
import signal
import subprocess
import sys
import time

predicate = ('((processID == 0 AND senderImagePath CONTAINS "/Sandbox") OR '
             '(subsystem == "com.apple.sandbox.reporting")) AND '
             '(eventMessage CONTAINS "Sandbox: ld(" OR '
             'eventMessage CONTAINS "Sandbox: cargo(" OR '
             'eventMessage CONTAINS "Sandbox: rustc(")')
command = ['/usr/bin/log', 'show', '--last', '2m', '--style', 'compact',
           '--info', '--debug', '--predicate', predicate]
process = subprocess.Popen(command, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, start_new_session=True)
assert process.stdout is not None
selector = selectors.DefaultSelector()
selector.register(process.stdout, selectors.EVENT_READ)
output = bytearray()
deadline = time.monotonic() + 15
complete = False
try:
    while time.monotonic() < deadline and len(output) <= 262144:
        if not selector.select(min(0.2, max(0, deadline - time.monotonic()))):
            continue
        chunk = os.read(process.stdout.fileno(), min(8192, 262145 - len(output)))
        if not chunk:
            complete = True
            break
        output.extend(chunk)
finally:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGKILL)
    status = process.wait(timeout=3)
    selector.close()
    process.stdout.close()
print(f'NATIVE-DENIAL-LOG status={status} complete={int(complete)} bytes={len(output)} scope=disposable-host-tool-names-correlation-only', flush=True)
sys.stdout.buffer.write(output[:262144])
print('\nNATIVE-DENIAL-LOG-END', flush=True)
