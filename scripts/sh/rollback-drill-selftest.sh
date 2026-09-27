#!/usr/bin/env bash
# Self-test for the headless prior-binary rollback drill.
#
# Two gates, no network, no credentials, no provider call:
#
#   1. the drill passes on clean fixtures;
#   2. the drill fails (exit 1, falsification held) when a fixture is
#      corrupted in a way the previous binary must refuse.
#
# The previous binary is built once here, from the local v0.42.0 tag in a
# scratch worktree, and handed to both runs — so this also exercises the
# drill's --prior-binary path. The drill's own default path (building the tag
# itself) is covered by running the drill directly. Everything the drill or
# the binaries touch lives under scratch directories; real session or
# credential files are never read.
set -euo pipefail

cd "$(dirname "$0")/../.."

readonly DRILL=$PWD/scripts/sh/rollback-drill.sh
readonly PRIOR_TAG=v0.42.0

failed=0
say() {
    printf '    %s\n' "$1"
}

for tool in bash cargo git mktemp rm; do
    command -v "$tool" >/dev/null || {
        printf 'FAIL %s is not installed\n' "$tool" >&2
        exit 2
    }
done

if ! bash -n "$DRILL"; then
    printf 'FAIL %s is not valid Bash\n' "$DRILL" >&2
    exit 2
fi
say "the drill parses"

scratch=$(mktemp -d)
readonly scratch
cleanup() {
    if [[ -d $scratch/prior-src ]]; then
        git worktree remove --force "$scratch/prior-src" >/dev/null 2>&1 || true
    fi
    rm -rf -- "$scratch"
}
trap cleanup EXIT

# One previous-binary build for both runs. The tag is local; nothing is
# fetched, and the scratch worktree is removed on the way out.
git rev-parse --verify "$PRIOR_TAG^{commit}" >/dev/null || {
    printf 'FAIL the local tag %s is missing; the drill fetches nothing\n' "$PRIOR_TAG" >&2
    exit 2
}
git worktree add --detach "$scratch/prior-src" "$PRIOR_TAG" >/dev/null
(cd "$scratch/prior-src" && cargo build --locked --bin crucible) >/dev/null 2>&1
prior=$scratch/prior-src/target/debug/crucible
[[ -x $prior ]] || prior=$scratch/prior-src/target/debug/crucible.exe
[[ -x $prior ]] || {
    printf 'FAIL the prior build left no binary\n' >&2
    exit 2
}
say "the previous binary builds from the local tag"

# Gate 1: clean fixtures pass.
if "$DRILL" --prior-binary "$prior" >"$scratch/clean.log" 2>&1; then
    if grep -Fq 'all rollback drill gates passed' "$scratch/clean.log"; then
        say "clean fixtures pass"
    else
        printf 'FAIL the clean run exited 0 without its verdict line\n' >&2
        failed=1
    fi
else
    printf 'FAIL the drill failed on clean fixtures; see %s\n' "$scratch/clean.log" >&2
    failed=1
fi

# Gate 2: a corrupted fixture fails the drill, at the refusal, and nowhere else.
status=0
"$DRILL" --prior-binary "$prior" --corrupt pending >"$scratch/corrupt.log" 2>&1 || status=$?
if ((status == 1)) &&
    grep -Fq 'FALSIFICATION HELD' "$scratch/corrupt.log" &&
    grep -Fq 'could not read the session log' "$scratch/corrupt.log"; then
    say "a corrupted fixture fails the drill at the prior binary's refusal"
else
    printf 'FAIL the corrupted run exited %d; see %s\n' "$status" "$scratch/corrupt.log" >&2
    failed=1
fi
if grep -Fq 'all rollback drill gates passed' "$scratch/corrupt.log"; then
    printf 'FAIL the corrupted run printed the clean verdict\n' >&2
    failed=1
fi

if ((failed)); then
    echo "rollback drill self-test failed"
    exit 1
fi

echo "rollback drill self-test passed"
