#!/usr/bin/env bash
# Every gate that must pass before a commit. CI runs exactly this script, so a
# green run here means a green run there.
set -euo pipefail

# An unmatched glob expands to itself, which turns "there are no rules files"
# into a loop over one path that does not exist — a section that reports success
# because it checked nothing. With this, no match is an empty list, and each
# section below says outright how many files it expected to find.
shopt -s nullglob

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
rules=(.claude/rules/*.md)
if ((${#rules[@]} == 0)); then
    printf '    FAIL .claude/rules/ holds no rules file; every per-crate rule has stopped loading\n'
    failed=1
fi

# One per crate. A crate nothing claims is a corner of the tree where the
# obligations that bind there are written down nowhere.
for crate in crates/*/; do
    name=$(basename "$crate")
    if ! grep -qs -- "crates/$name/" "${rules[@]}"; then
        printf '    FAIL crates/%s: no rules file under .claude/rules/ claims it\n' "$name"
        failed=1
    fi
done

for rule in "${rules[@]}"; do
    globs=$(awk '
        NR == 1 && $0 != "---" { exit }
        NR > 1 && $0 == "---"  { exit }

        # Both spellings YAML allows, because reporting "no paths:" for a rule
        # that has one would send the reader looking for the wrong thing.
        /^paths:/ {
            if ($0 ~ /\[/) {
                line = $0
                sub(/^paths:[[:space:]]*\[/, "", line)
                sub(/\].*$/, "", line)
                n = split(line, items, ",")
                for (i = 1; i <= n; i++) {
                    gsub(/^[[:space:]]+|[[:space:]]+$|["\x27]/, "", items[i])
                    if (items[i] != "") print items[i]
                }
                next
            }
            paths = 1
            next
        }

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
        # Only the literal prefix before the first wildcard can be checked
        # without walking the tree, so the message says that rather than
        # claiming the whole glob was tried and matched nothing.
        base=${glob%%[*?]*}
        base=${base%/}
        if [[ -z "$base" ]]; then
            printf '    FAIL %s: paths: %s begins with a wildcard, so it scopes the rule to nothing in particular\n' "$rule" "$glob"
            failed=1
        elif [[ ! -e "$base" ]]; then
            printf '    FAIL %s: paths: %s — %s does not exist, so nothing loads it\n' "$rule" "$glob" "$base"
            failed=1
        fi
    done <<<"$globs"
done

echo "==> agent skills"
# A skill is written once under .claude/skills/ and reaches Codex through a
# symlink, exactly as CLAUDE.md reaches it through AGENTS.md. A copy drifts; a
# missing link means Codex users silently lose the skill.
skills=(.claude/skills/*/)
if ((${#skills[@]} == 0)); then
    printf '    FAIL .claude/skills/ holds no skill; the procedures it carries reach neither harness\n'
    failed=1
fi

for skill in "${skills[@]}"; do
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
# Cargo accepts three spellings and this has to read all of them: a grep for a
# literal `version =` key sees only the inline-table form and waves the bare one
# straight through, which is most of them.
unpinned=$(awk '
    /^\[workspace\.dependencies\]/ { table = 1; named = ""; next }
    /^\[workspace\.dependencies\./ {
        table = 1
        named = $0
        gsub(/^\[workspace\.dependencies\.|\]/, "", named)
        next
    }
    /^\[/      { table = 0; named = ""; next }
    table == 0 { next }

    # `[workspace.dependencies.foo]` puts the crate in the header, so a bare
    # `version =` line beneath one is that crate pinning itself.
    named != "" && /^[[:space:]]*version[[:space:]]*=/ {
        if ($0 !~ /=[[:space:]]*"=/) print "        " named ": " $0
        next
    }

    /^[a-zA-Z0-9_-]+[[:space:]]*=/ {
        if ($0 ~ /\{/) {
            # A path dependency inside this workspace carries no version and
            # needs none; anything naming one has to pin it.
            if ($0 ~ /version[[:space:]]*=/ && $0 !~ /version[[:space:]]*=[[:space:]]*"=/) {
                print "        " $0
            }
        } else if ($0 !~ /=[[:space:]]*"=/) {
            print "        " $0
        }
    }
' Cargo.toml)
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
# The table starts unjustified. Seeding it from whatever line preceded the
# header meant the comment block explaining the crate layering counted as a
# reason for the first crate beneath it, so a dependency added there needed no
# comment of its own.
unjustified=$(awk '
    /^\[workspace\.dependencies\]/ { table = 1; justified = 0; next }
    /^\[workspace\.dependencies\./ {
        table = 1
        named = $0
        gsub(/^\[workspace\.dependencies\.|\]/, "", named)
        if (justified == 0) print "        " named
        justified = 0
        next
    }
    /^\[/      { table = 0; next }
    table == 0 { justified = ($0 ~ /^[[:space:]]*#/); next }

    /^[[:space:]]*#/ { justified = 1; next }
    /^[[:space:]]*$/ { justified = 0; next }
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
