#!/bin/sh
# Cross-UID effect challenge against our own fixed root bystander only.
set -eu
test "$(uname -s)" = Darwin
test "$(uname -m)" = x86_64
test "$(id -u)" != 0
probe_root=$(mktemp -d /tmp/crucible-macos-foreign-signature.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
probe_nonce=$(/usr/bin/uuidgen | tr -d '-' | tr 'A-F' 'a-f')
cp .github/probes/macos-foreign-signature-target.c .github/probes/macos-foreign-signature-caller.c .github/probes/macos-foreign-signature.py "$probe_root/"
printf '#define PROBE_NONCE "%s"\n' "$probe_nonce" > "$probe_root/nonce.h"
/usr/bin/xcrun clang -Wall -Wextra -Werror -Wl,-no_adhoc_codesign "$probe_root/macos-foreign-signature-target.c" -o "$probe_root/target"
/usr/bin/xcrun clang -Wall -Wextra -Werror -lbsm "$probe_root/macos-foreign-signature-caller.c" -o "$probe_root/caller"
/usr/bin/nm -u "$probe_root/caller" > "$probe_root/caller-imports"
/usr/bin/python3 - "$probe_root/caller-imports" <<'CHECK'
from pathlib import Path
import sys
with Path(sys.argv[1]).open('rb') as stream:
    raw = stream.read(65537)
if len(raw) > 65536:
    raise RuntimeError('symbol output bound')
names = [line.split()[-1] for line in raw.decode('ascii').splitlines() if line.split()]
if names.count('_getgroups') != 1 or any('getgroups$DARWIN_EXTSN' in name for name in names):
    raise RuntimeError('ordinary getgroups binding unavailable')
print('CALLER-GROUP-API ordinary_getgroups=1 directory_extension=0')
CHECK
/usr/bin/sw_vers
/usr/bin/uname -mrv
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /usr/bin/python3 -I "$probe_root/macos-foreign-signature.py" "$probe_root" "$(id -u)" "$(id -g)" "$probe_nonce" < /dev/null
