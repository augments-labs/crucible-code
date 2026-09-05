#!/bin/sh
set -eu
umask 077
probe_tmp=$(mktemp -d "${TMPDIR:-/tmp}/crucible-apfs-probe.XXXXXX")
/usr/bin/man -P cat hdiutil > "$probe_tmp/hdiutil-man.txt"
/usr/bin/shasum -a 256 "$probe_tmp/hdiutil-man.txt"
/usr/bin/xcrun clang -Wall -Wextra -Werror .github/probes/macos-apfs-publication.c -o "$probe_tmp/probe"
sudo -n python3 .github/probes/macos-apfs-publication.py "$probe_tmp/probe"
