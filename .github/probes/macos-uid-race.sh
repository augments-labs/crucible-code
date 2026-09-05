#!/bin/sh
set -eu
umask 022
probe_root=$(mktemp -d /tmp/crucible-uid-race.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
chmod 0755 "$probe_root"
/usr/bin/xcrun clang -std=c11 -Wall -Wextra -Werror -pthread .github/probes/macos-uid-race.c -o "$probe_root/probe"
/usr/bin/sw_vers
/usr/bin/uname -a
printf 'fixture=%s\n' "$probe_root"
sudo -n "$probe_root/probe" "$probe_root"
