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
/usr/bin/sw_vers
/usr/bin/uname -mrv
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /usr/bin/python3 -I "$probe_root/macos-foreign-signature.py" "$probe_root" "$(id -u)" "$(id -g)" "$probe_nonce" < /dev/null
