#!/bin/sh
set -eu
umask 022
test "${GITHUB_ACTIONS:-}" = true
test "$(uname -s)" = Darwin
probe_tmp=$(mktemp -d "${TMPDIR:-/tmp}/crucible-filesystem-driver.XXXXXX")
/usr/bin/xcrun clang -Wall -Wextra -Werror .github/probes/macos-filesystem.c -o "$probe_tmp/worker"
sudo -n /usr/bin/env GITHUB_ACTIONS=true python3 .github/probes/macos-filesystem.py "$probe_tmp/worker"
