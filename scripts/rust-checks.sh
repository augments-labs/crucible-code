#!/usr/bin/env bash
# Deterministic checks owned by the Rust ecosystem. Repository structure is
# checked separately by scripts/repo-checks.sh.
set -uo pipefail

cd "$(dirname "$0")/.."

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

section "rustfmt"
if ! cargo fmt --all --check; then
    printf '    FAIL cargo fmt --all rewrites the files above; never hand-format around it\n'
    failed=1
fi

section "clippy"
if ! cargo clippy --workspace --all-targets --all-features --locked -- -D warnings; then
    printf '    FAIL clippy warnings are errors; an #[allow] needs a comment saying what the lint got wrong\n'
    failed=1
fi

generated=(schema/crucible-code-schema.json)
before=()
for file in "${generated[@]}"; do
    before+=("$(cksum "$file" 2>/dev/null || true)")
done

section "tests"
if ! cargo test --workspace --locked; then
    printf '    FAIL read the assertion, not the count\n'
    failed=1
fi

section "rustdoc"
if ! RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked; then
    printf '    FAIL public documentation did not compile cleanly; fix the first rustdoc diagnostic\n'
    failed=1
fi

section "generated files"
for index in "${!generated[@]}"; do
    file=${generated[index]}
    if [[ "${before[index]}" != "$(cksum "$file" 2>/dev/null || true)" ]]; then
        printf '    FAIL %s was stale; the tests regenerated it — review the diff and commit it\n' "$file"
        failed=1
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
