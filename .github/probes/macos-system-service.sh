#!/bin/sh
# Disposable native system launch topology; one synthetic root helper only.
set -eu
test "$(uname -s)" = Darwin
test "$(id -u)" != 0
probe_root=$(mktemp -d /tmp/crucible-macos-service.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
cp .github/probes/macos-system-service.c .github/probes/macos-system-service.py "$probe_root/"
/usr/bin/xcrun clang -Wall -Wextra -Werror -Wno-deprecated-declarations -framework Security -framework CoreFoundation -lbsm "$probe_root/macos-system-service.c" -o "$probe_root/identity"
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 "$probe_root/macos-system-service.c" "$probe_root/macos-system-service.py" "$probe_root/identity"
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin /usr/bin/python3 -I "$probe_root/macos-system-service.py" "$probe_root" < /dev/null
