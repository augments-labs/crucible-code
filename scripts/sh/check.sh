#!/usr/bin/env bash
# Compatibility entrypoint for every deterministic local gate. Each child owns
# one concern and CI calls those children directly.
set -uo pipefail

cd "$(dirname "$0")/../.."

failed=()

if ! scripts/sh/rust-checks.sh; then
    failed+=("Rust")
fi

if ! scripts/sh/repo-checks.sh; then
    failed+=("repository")
fi

if ! scripts/sh/python-checks.sh; then
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
