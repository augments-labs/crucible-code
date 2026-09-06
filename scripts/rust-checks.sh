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

section "package isolation"
packages=()
featured=()
no_default=()
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
else
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
