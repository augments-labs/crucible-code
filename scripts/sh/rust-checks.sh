#!/usr/bin/env bash
# Deterministic checks owned by the Rust ecosystem. Repository structure is
# checked separately by scripts/sh/repo-checks.sh.
#
#   scripts/sh/rust-checks.sh                     every section
#   scripts/sh/rust-checks.sh --skip tests        every section but the suite
#   scripts/sh/rust-checks.sh --only tests --partition hash:1/4
#                                                 one part of the suite
#
# CI spreads the gate over several machines: the suite in parts, the rest in a
# job that names only what it leaves out. A section added below is therefore
# run there without anyone listing it, and a name this script does not know is
# refused rather than quietly matching nothing.
set -uo pipefail

cd "$(dirname "$0")/../.."

# A snapshot suite that is allowed to write is not a check. Left to the
# environment, `INSTA_UPDATE=always` makes every capture agree with whatever
# just drew it, and the run goes green having asserted nothing. The gate
# decides this, not the shell it was started from.
export INSTA_UPDATE=no

# In the order they run.
readonly SECTIONS=(rustfmt clippy isolation tests examples required rustdoc generated)

usage() {
    printf 'usage: %s [--only SECTION,... | --skip SECTION,...] [--partition hash:K/N]\n' "$0" >&2
    printf 'sections: %s\n' "${SECTIONS[*]}" >&2
    exit 2
}

# Refuses a name that is not a section.
named() {
    local one known
    (($#)) || usage
    for one in "$@"; do
        for known in "${SECTIONS[@]}"; do
            [[ $one == "$known" ]] && continue 2
        done
        printf 'rust-checks: no section is called "%s"\n' "$one" >&2
        usage
    done
}

only=()
skip=()
partition=()
while (($#)); do
    case $1 in
    --only | --skip | --partition) (($# > 1)) || usage ;;&
    --only) IFS=, read -ra only <<<"$2" && named "${only[@]}" ;;
    --skip) IFS=, read -ra skip <<<"$2" && named "${skip[@]}" ;;
    --partition) partition=(--partition "$2") ;;
    *) usage ;;
    esac
    shift 2
done
((${#only[@]} == 0 || ${#skip[@]} == 0)) || usage
# A part of the suite says nothing about any other section, so it runs alone.
if ((${#partition[@]})) && [[ "${only[*]}" != tests ]]; then
    printf 'rust-checks: --partition runs part of the suite, so it needs --only tests\n' >&2
    exit 2
fi

wanted() {
    local one
    for one in "${skip[@]}"; do
        [[ $one == "$1" ]] && return 1
    done
    ((${#only[@]} == 0)) && return 0
    for one in "${only[@]}"; do
        [[ $one == "$1" ]] && return 0
    done
    return 1
}

failed=0
any=0
current=""
failures=()

section() {
    close_section
    current=$1
    failed=0
    echo "==> $1"
}

close_section() {
    if [[ -n "$current" ]] && ((failed)); then
        failures+=("$current")
        any=1
    fi
}

# One selection, read twice: the suite runs under it, and the required-case
# manifest is checked against what it built. Narrowing it to skip a package
# therefore loses the obligations that package owns instead of shrinking a
# total nobody reads.
readonly TEST_SELECTION=(--workspace --locked)

generated=(schema/crucible-code-schema.json)
before=()
for file in "${generated[@]}"; do
    before+=("$(cksum "$file" 2>/dev/null || true)")
done

check_rustfmt() {
    section "rustfmt"
    if ! cargo fmt --all --check; then
        printf '    FAIL cargo fmt --all rewrites the files above; never hand-format around it\n'
        failed=1
    fi
}

check_clippy() {
    section "clippy"
    if ! cargo clippy --workspace --all-targets --all-features --locked -- -D warnings; then
        printf '    FAIL clippy warnings are errors; an #[allow] needs a comment saying what the lint got wrong\n'
        failed=1
    fi
}

check_isolation() {
    section "package isolation"
    local package has_features has_defaults
    local packages=() featured=() no_default=()
    while IFS=$'\t' read -r package has_features has_defaults; do
        [[ -n "$package" ]] || continue
        packages+=("$package")
        [[ $has_features == yes ]] && featured+=("$package")
        [[ $has_defaults == yes ]] && no_default+=("$package")
    done < <(
        cargo metadata --no-deps --format-version 1 --locked |
            python3 -c '
import json, sys
metadata = json.load(sys.stdin)
for package in sorted(metadata["packages"], key=lambda one: one["name"]):
    features = package.get("features", {})
    has_features = "yes" if any(name != "default" for name in features) else "no"
    has_defaults = "yes" if features.get("default") else "no"
    print(package["name"], has_features, has_defaults, sep="\t")
'
    )
    if ((${#packages[@]} == 0)); then
        printf '    FAIL cargo metadata reported no workspace packages\n'
        failed=1
        return
    fi
    for package in "${packages[@]}"; do
        if ! cargo check --quiet --locked -p "$package"; then
            printf '    FAIL cargo check -p %s did not compile the package in isolation\n' "$package"
            failed=1
        fi
    done
    # Cargo metadata includes explicit and implicit optional-dependency
    # features. Only packages whose all-feature graph differs get a second
    # invocation; only a non-empty default list earns a no-default invocation.
    for package in "${featured[@]}"; do
        if ! cargo check --quiet --locked -p "$package" --all-features; then
            printf '    FAIL %s did not compile with all package features\n' "$package"
            failed=1
        fi
    done
    for package in "${no_default[@]}"; do
        if ! cargo check --quiet --locked -p "$package" --no-default-features; then
            printf '    FAIL %s did not compile without its default features\n' "$package"
            failed=1
        fi
    done
}

# cargo-nextest runs each test in a process of its own, as many at once as
# there are cores, and `.config/nextest.toml` names the tests that share state
# and so wait for each other. It runs no documentation example, so rustdoc's
# own harness runs those in the next section.
check_tests() {
    section "tests"
    if ! cargo nextest --version >/dev/null 2>&1; then
        printf '    FAIL cargo-nextest is not installed; CONTRIBUTING.md says how to install it\n'
        failed=1
    elif ! cargo nextest run "${TEST_SELECTION[@]}" "${partition[@]}" --no-fail-fast; then
        printf '    FAIL read the assertion, not the count\n'
        failed=1
    fi
}

check_examples() {
    section "documentation examples"
    if ! cargo test "${TEST_SELECTION[@]}" --doc; then
        printf '    FAIL a documentation example failed\n'
        failed=1
    fi
}

check_required() {
    section "required cases"
    artifacts=$(mktemp)
    examples=$(mktemp)
    silenced=$(mktemp)
    trap 'rm -f "$artifacts" "$examples" "$silenced"' EXIT
    # Documentation examples are built by no binary, so the same selection is
    # asked a second time for the inventory rustdoc's own harness keeps.
    if ! cargo test "${TEST_SELECTION[@]}" --no-run --message-format=json >"$artifacts"; then
        printf '    FAIL the test selection did not build; the required cases were not checked\n'
        failed=1
    elif ! cargo test "${TEST_SELECTION[@]}" --doc -- --list >"$examples" ||
        ! cargo test "${TEST_SELECTION[@]}" --doc -- --list --ignored >"$silenced"; then
        printf '    FAIL the documentation examples did not list; the required cases were not checked\n'
        failed=1
    elif ! python3 scripts/python/required-cases.py "$artifacts" "$examples" "$silenced" "${TEST_SELECTION[@]}"; then
        printf '    FAIL a named obligation in scripts/required-cases.json is missing, silenced or changed\n'
        failed=1
    fi
}

check_rustdoc() {
    section "rustdoc"
    if ! RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked; then
        printf '    FAIL public documentation did not compile cleanly; fix the first rustdoc diagnostic\n'
        failed=1
    fi
    # Private items as well, which the run above leaves out of every library:
    # their documentation is what a contributor reads before changing the code
    # under it. This run has a tree of its own because the two modes invalidate
    # each other in a shared one, so every warm check would document every
    # library twice.
    if ! RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --document-private-items --target-dir generated/rustdoc-private; then
        printf '    FAIL private documentation did not compile cleanly; fix the first rustdoc diagnostic\n'
        failed=1
    fi
}

# What the tests regenerate is committed, so a run that changed it found it
# stale. Run without the suite, this compares the files with themselves.
check_generated() {
    section "generated files"
    local index file
    for index in "${!generated[@]}"; do
        file=${generated[index]}
        if [[ "${before[index]}" != "$(cksum "$file" 2>/dev/null || true)" ]]; then
            printf '    FAIL %s was stale; the tests regenerated it — review the diff and commit it\n' "$file"
            failed=1
        fi
    done
}

for name in "${SECTIONS[@]}"; do
    if wanted "$name"; then
        "check_$name"
    fi
done

close_section

if ((any)); then
    echo
    echo "FAILED — see the lines marked FAIL above, under:"
    printf '    %s\n' "${failures[@]}"
    exit 1
fi

echo
echo "all Rust checks passed"
