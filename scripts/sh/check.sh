#!/usr/bin/env bash
# Compatibility entrypoint for every deterministic local gate. Each child owns
# one concern and CI calls those children directly.
set -uo pipefail

cd "$(dirname "$0")/../.."

failed=()

if ! scripts/sh/rust-checks.sh; then
    failed+=("Rust")
fi

# The repository and Python gates build nothing, and neither reads what the
# other writes, so they run at once. The Python gate's output, its errors
# included, is held and printed on standard output after the repository gate's,
# so the two never interleave. A script's background job ignores Ctrl-C, so the
# subshell takes that back before it starts the gate; otherwise Ctrl-C would
# stop check.sh and leave the gate running. Bash before 4.3, macOS's /bin/bash
# among them, cannot take it back, so there the two run one after the other.
python_failed=0
if ((BASH_VERSINFO[0] * 100 + BASH_VERSINFO[1] >= 403)); then
    python_output=$(mktemp)
    trap 'rm -f "$python_output"' EXIT
    (
        trap - INT QUIT
        exec scripts/sh/python-checks.sh
    ) >"$python_output" 2>&1 &
    python_gate=$!

    if ! scripts/sh/repo-checks.sh; then
        failed+=("repository")
    fi

    wait "$python_gate" || python_failed=1
    cat "$python_output"
else
    if ! scripts/sh/repo-checks.sh; then
        failed+=("repository")
    fi

    scripts/sh/python-checks.sh || python_failed=1
fi
if ((python_failed)); then
    failed+=("Python")
fi

if ((${#failed[@]})); then
    echo
    printf 'FAILED gates:\n'
    printf '    %s\n' "${failed[@]}"
    exit 1
fi

echo
echo "all deterministic gates passed"
