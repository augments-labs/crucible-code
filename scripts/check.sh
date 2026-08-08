#!/usr/bin/env bash
# Every gate that must pass before a commit. CI runs exactly this script, so a
# green run here means a green run there.
set -euo pipefail

cd "$(dirname "$0")/.."

# A file longer than this is doing more than one thing. The compiler cannot see
# file boundaries, so this is the one structural rule a lint cannot carry.
readonly MAX_FILE_LINES=400

failed=0

echo "==> rustfmt"
cargo fmt --all --check

echo "==> clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> tests"
cargo test --workspace

echo "==> file length (<= ${MAX_FILE_LINES} lines)"
while IFS= read -r file; do
    lines=$(wc -l <"$file")
    if ((lines > MAX_FILE_LINES)); then
        printf '    FAIL %s: %d lines > %d\n' "$file" "$lines" "$MAX_FILE_LINES"
        failed=1
    fi
done < <(find crates src -type f -name '*.rs' 2>/dev/null)

echo "==> agent rules files"
# One set of rules, one file. CLAUDE.md is the original everywhere in this
# repo; AGENTS.md is a symlink beside it, so no tool reads a stale copy.
while IFS= read -r link; do
    if [[ ! -L "$link" ]]; then
        printf '    FAIL %s: must be a symlink to CLAUDE.md, not a file\n' "$link"
        failed=1
    elif [[ "$(readlink "$link")" != "CLAUDE.md" ]]; then
        printf '    FAIL %s: points at %s, expected CLAUDE.md\n' "$link" "$(readlink "$link")"
        failed=1
    fi
done < <(find . -path ./target -prune -o -name AGENTS.md -print)

echo "==> agent rules scope"
# A rule under .claude/rules/ is read only when a file it claims is opened, so
# one without `paths:` is dead text that nothing ever loads — and it fails by
# staying quiet, which is the same silent shape the symlink check above exists
# to catch. The globs are checked too: a rule aimed at a crate that has since
# been renamed stops applying without anyone noticing.
for rule in .claude/rules/*.md; do
    [[ -f "$rule" ]] || continue

    globs=$(awk '
        NR == 1 && $0 != "---" { exit }
        NR > 1 && $0 == "---"  { exit }
        /^paths:/              { paths = 1; next }
        paths && /^[[:space:]]*-[[:space:]]*/ {
            sub(/^[[:space:]]*-[[:space:]]*/, "")
            gsub(/["\x27]/, "")
            print
            next
        }
        paths && /^[^[:space:]]/ { paths = 0 }
    ' "$rule")

    if [[ -z "$globs" ]]; then
        printf '    FAIL %s: no paths: frontmatter, so nothing ever loads it\n' "$rule"
        failed=1
        continue
    fi

    while IFS= read -r glob; do
        # Everything before the first wildcard has to exist as written.
        base=${glob%%[*?]*}
        base=${base%/}
        if [[ -n "$base" && ! -e "$base" ]]; then
            printf '    FAIL %s: paths: %s matches nothing in the tree\n' "$rule" "$glob"
            failed=1
        fi
    done <<<"$globs"
done

echo "==> agent skills"
# A skill is written once under .claude/skills/ and reaches Codex through a
# symlink, exactly as CLAUDE.md reaches it through AGENTS.md. A copy drifts; a
# missing link means Codex users silently lose the skill.
for skill in .claude/skills/*/; do
    [[ -d "$skill" ]] || continue
    name=$(basename "$skill")

    if [[ ! -f "$skill/SKILL.md" ]]; then
        printf '    FAIL %s: no SKILL.md\n' "$skill"
        failed=1
    fi

    link=".agents/skills/$name"
    want="../../.claude/skills/$name"
    if [[ ! -L "$link" ]]; then
        printf '    FAIL %s: must be a symlink to %s\n' "$link" "$want"
        failed=1
    elif [[ "$(readlink "$link")" != "$want" ]]; then
        printf '    FAIL %s: points at %s, expected %s\n' "$link" "$(readlink "$link")" "$want"
        failed=1
    fi
done

# The other direction: anything under .agents/skills/ that is not a symlink is
# a second copy of a skill, which is the thing the symlink exists to prevent.
for entry in .agents/skills/*; do
    [[ -e "$entry" || -L "$entry" ]] || continue
    if [[ ! -L "$entry" ]]; then
        printf '    FAIL %s: real file or directory, expected a symlink into .claude/skills/\n' "$entry"
        failed=1
    fi
done

echo "==> dependency pinning"
# Exact pins keep a release reproducible and make a version bump a reviewed
# change rather than a side effect of somebody else's publish.
unpinned=$(awk '/^\[workspace\.dependencies\]/{f=1;next} /^\[/{f=0} f' Cargo.toml |
    grep -n 'version *= *"[^=]' || true)
if [[ -n "$unpinned" ]]; then
    printf '    FAIL not =-pinned:\n%s\n' "$unpinned"
    failed=1
fi

echo "==> dependency justification"
# And a comment above it saying why it is there. A dependency is a permanent
# cost paid for what is usually a temporary convenience, so the reason has to
# outlive the pull request that added it — whoever later asks whether it can go
# is never the person who knew. One comment covers the group beneath it, since
# the four ripgrep crates are one decision rather than four.
unjustified=$(awk '
    /^\[workspace\.dependencies\]/ { table = 1; justified = comment; next }
    table == 0 { comment = ($0 ~ /^[[:space:]]*#/); next }
    /^\[/                         { table = 0; next }
    /^[[:space:]]*#/              { justified = 1; next }
    /^[[:space:]]*$/              { justified = 0; next }
    justified == 0 && /^[a-zA-Z0-9_-]+[[:space:]]*=/ { print "        " $0 }
' Cargo.toml)
if [[ -n "$unjustified" ]]; then
    printf '    FAIL no comment saying why it is needed:\n%s\n' "$unjustified"
    failed=1
fi

echo "==> github actions pinning"
# Same rule as the crates, for the same reason. An action referenced by tag is
# a moving dependency, and a release workflow runs it with write access to the
# repository — so pin the commit and keep the version in a trailing comment.
if [[ -d .github/workflows ]]; then
    floating=$(grep -rn 'uses:' .github/workflows |
        grep -vE 'uses: *[^@]+@[0-9a-f]{40} +# ' || true)
    if [[ -n "$floating" ]]; then
        printf '    FAIL not pinned to a commit sha:\n%s\n' "$floating"
        failed=1
    fi
fi

echo "==> benchmark gate"
# CI is allowed to tolerate a failing budgets job only while there is nothing
# to measure — every probe is still unwritten, so `scripts/bench.sh` exits 1 by
# design and would block every pull request. The first probe makes that failure
# real, and the escape hatch has to leave with it, or a regression lands in an
# artifact nobody reads. This is what makes that automatic instead of
# remembered.
if compgen -G 'src/bin/bench-*.rs' >/dev/null; then
    # The key, anchored — the prose above it in ci.yml names the same word, and
    # what matters is the directive that is actually in effect.
    if grep -qE '^[[:space:]]*continue-on-error:' .github/workflows/ci.yml; then
        printf '    FAIL %s exists, so drop continue-on-error from the budgets job in .github/workflows/ci.yml\n' \
            "$(compgen -G 'src/bin/bench-*.rs' | head -1)"
        failed=1
    fi
fi

if ((failed)); then
    echo
    echo "FAILED — see the lines marked FAIL above."
    exit 1
fi

echo
echo "all gates passed"
