#!/usr/bin/env bash
set -euo pipefail
[[ $(cat greeting.txt) == 'hello crucible' ]]
[[ $(find . -maxdepth 1 -type f -printf '%f\n' | sort | tr '\n' ' ') == 'greeting.txt verify.sh ' ]]
