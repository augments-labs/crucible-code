#!/bin/sh
set -eu
umask 022
test "${GITHUB_ACTIONS:-}" = true
test "$(uname -s)" = Darwin
sudo -n /usr/bin/env GITHUB_ACTIONS=true python3 .github/probes/macos-detach.py
