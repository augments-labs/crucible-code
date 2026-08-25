#!/usr/bin/env bash
# Every gate that must pass before a commit. CI runs exactly this script, so a
# green run here means a green run there.
#
# The text-walking sections below — process memory, links, rules frontmatter,
# manifest comments — are guardrails rather than proofs. Each greps for the
# shapes a slip usually takes, and a slip written another way walks past it.
# Green means nothing it knows to look for was found, not that the prose is
# right; the reviewer still owns that judgement.
set -euo pipefail

# An unmatched glob expands to itself, which turns "there are no rules files"
# into a loop over one path that does not exist — a section that reports success
# because it checked nothing. With this, no match is an empty list, and every
# section below that walks a set says outright when it found none.
shopt -s nullglob

cd "$(dirname "$0")/.."

# A file longer than this is doing more than one thing. The compiler cannot see
# file boundaries, so this is the one structural rule a lint cannot carry.
#
# A ceiling, not a target. It is set where a file has plainly lost its subject
# rather than where a careful one lands, because the opposite failure is the
# one no number can measure: a directory of files too small to have a subject,
# each naming the next, where learning what any one of them does means opening
# all of them. Nothing rewards filling the budget and nothing rewards splitting
# to stay under it — what a file owes is one reason to change, and its length
# follows from that.
#
# It sat at 1000 and was moved because that was landing on the wrong side of the
# judgement: a module with its tests beside it reaches four figures while it
# still has one subject, and what the old number bought was a sibling `tests.rs`
# next to half the modules here, holding no subject of its own and named after
# the file it was cut out of.
readonly MAX_FILE_LINES=2000

# Every section runs, and the run reports all of them together.
#
# The three cargo sections used to run bare under `errexit`, so a formatting slip
# ended the script before the file-length cap, the process-memory scan, the link
# check, the agent-rules checks and both dependency gates had run at all. Nobody
# was told they had been skipped; the run simply stopped. That made the answer
# depend on how much was already fixed — fix clippy, re-run, meet a different set
# of checks — and it meant only a fully green run had ever executed all of them,
# which is the one run that had nothing to report.
#
# `section` closes the previous one before opening the next, so `failed` is a
# per-section flag every check can set the same way, and `failures` collects the
# names. A failing run prints hundreds of lines of cargo output, so the list at
# the foot is what makes the FAIL lines above it findable.
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

section "merge conflict markers"
# First, because it is the cheapest check here and because everything after it
# is measuring a tree that a marker has already made meaningless.
#
# A resolution that missed a line is the one defect the rest of this script
# waves through. In a `.rs` file the compiler catches it; in a changelog, a
# workflow, a JSON document or this script it is ordinary text, and one reached
# `main` through five green jobs. That is what this is here for.
#
# Every tracked file, rather than the shipped set the scan below reads: a marker
# is not a matter of who the file is addressed to. Untracked files are nobody's
# business and `target/` is never walked, both of which `git ls-files` decides
# for free.
#
# The patterns are counts rather than seven literal characters, because a check
# that greps for its own source fails on a clean tree. `={7}$` is the one that
# could match something innocent — a markdown heading underlined with exactly
# seven `=` looks the same from here. Underline it with any other number.
#
# Exit codes are read the way they are for the shipped-files scan below, with
# one more case: outside a repository `git grep` answers 128, which lands in the
# same arm as a scan that could not finish, and says so rather than passing.
scan=0
conflicted=$(git grep -InE '^(<{7}|\|{7}|={7}|>{7})( |$)') || scan=$?
case $scan in
    0)
        printf '%s\n' "$conflicted"
        printf '    FAIL the lines above are what a merge left behind\n'
        failed=1
        ;;
    1) ;;
    *)
        printf '%s\n' "$conflicted"
        printf '    FAIL the scan could not be completed; this check measured nothing\n'
        failed=1
        ;;
esac

section "rustfmt"
if ! cargo fmt --all --check; then
    printf '    FAIL cargo fmt --all rewrites the files above; never hand-format around it\n'
    failed=1
fi

section "installer"
for script in scripts/install.sh scripts/uninstall.sh scripts/install-tests.sh; do
    if ! bash -n "$script"; then
        printf '    FAIL %s is not valid Bash\n' "$script"
        failed=1
    fi
done
if ! scripts/install-tests.sh; then
    printf '    FAIL the installer did not preserve its checksum, ownership, or rollback contract\n'
    failed=1
fi

# `--locked` on both, because CI passes it and a release builds with it. Without
# it cargo rewrites `Cargo.lock` on the way past whenever the manifest asks for
# something the lock does not have, and the run goes green on a lockfile that
# only exists on this machine. The change then lands with the manifest edit and
# without the lock, and the first machine to build it with `--locked` is a
# release runner. A green run here has to mean a green run there, and that is
# only true if both are answering the same question.
section "clippy"
if ! cargo clippy --workspace --all-targets --locked -- -D warnings; then
    printf '    FAIL clippy warnings are errors; an #[allow] needs a comment saying what the lint got wrong\n'
    failed=1
fi

# Captured before the tests, because two of them are agreement gates and those
# two write. `the_checked_in_schema_is_what_this_generates` regenerates the
# schema from `shape.rs`; `the_checked_in_table_is_what_this_generates`
# regenerates the model table from the slice of the database recorded beside it.
# Each rewrites its file and then fails where the checked-in copy has gone
# stale. Every other section here only reads the tree. Those two do not, and a
# run that edited a tracked file has to say which file rather than leave it to
# be found in `git status`.
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

section "generated"
# Not a second comparison — the tests above are the comparison, and this is the
# report of what they did to the tree. The distinction matters because the
# rewrite means a stale file fails once and passes on the next run, so the only
# run that ever mentions it is the one somebody may not have read to the end.
for index in "${!generated[@]}"; do
    file=${generated[index]}
    if [[ "${before[index]}" != "$(cksum "$file" 2>/dev/null || true)" ]]; then
        printf '    FAIL %s was stale; the tests regenerated it — review the diff and commit it\n' "$file"
        failed=1
    fi
done

section "component fit sweep"
# The sweep in `fits.rs` asserts that no component draws past the window it was
# given. It can only assert it about components it names, so what this checks is
# not the rule but the *coverage* of it: a component written and never added
# there passes a green suite while nothing has ever laid it out at twenty
# columns. That is the failure the sweep exists to prevent, and the sweep cannot
# catch it about itself.
#
# A component is a type that lays rows out against a width it was handed, which
# is a signature rather than a naming convention. Coverage is asked as an import
# of the module, because that is the one thing a sweep cannot have without
# reaching the component behind it.
sweep=crates/crucible-tui/src/fits.rs
if [[ ! -f "$sweep" ]]; then
    printf '    FAIL %s is missing; nothing holds the components to one width rule\n' "$sweep"
    failed=1
else
    swept=0
    for file in crates/crucible-tui/src/*.rs; do
        module=$(basename "$file" .rs)
        grep -qE '^[[:space:]]*pub fn (rows|row|within)\(&self, columns: usize' "$file" || continue
        swept=$((swept + 1))
        if ! grep -q "^use crate::$module::" "$sweep"; then
            printf '    FAIL %s lays rows out against a width and %s never draws it\n' "$file" "$sweep"
            failed=1
        fi
    done
    if ((swept == 0)); then
        printf '    FAIL no component found to sweep; the signature this looks for has moved\n'
        failed=1
    fi
fi

section "the replay seam"
# `ToolOutput::replayed` is the one way files reach a tool result with no
# `Approved` beside them. It exists because a log cannot hold one: the verdict
# was reached about a call that is over, and reading back what this build
# recorded is not deciding it again.
#
# What keeps that narrow is that exactly one caller replays a log. A tool
# reaching this would put files on its own result without being permitted to,
# and a doc comment asking it not to is a request rather than a rule. So the
# rule is here, where it fails a run.
#
# `grep -c` counts rather than lists, because the one legitimate call is in the
# file this names and a second call in that same file is as wrong as a call
# anywhere else.
replay="crates/crucible-session/src/session/wire.rs"
elsewhere=$(grep -rln '\.replayed(' --include='*.rs' crates src | grep -Fxv "$replay" || true)
if [[ -n "$elsewhere" ]]; then
    while IFS= read -r file; do
        printf '    FAIL %s calls ToolOutput::replayed; only %s may\n' "$file" "$replay"
    done <<< "$elsewhere"
    failed=1
fi
here=$(grep -c '\.replayed(' "$replay" || true)
if ((here != 1)); then
    printf '    FAIL %s calls ToolOutput::replayed %d times; the replay is one call\n' "$replay" "$here"
    failed=1
fi

section "file length (<= ${MAX_FILE_LINES} lines)"
# `find` printing nothing would walk this loop zero times and pass. The error
# stream is not discarded either: a renamed source directory is exactly the way
# this check would go quiet, and quiet is the failure it cannot afford.
counted=0
while IFS= read -r file; do
    counted=$((counted + 1))
    lines=$(wc -l <"$file")
    if ((lines > MAX_FILE_LINES)); then
        printf '    FAIL %s: %d lines > %d\n' "$file" "$lines" "$MAX_FILE_LINES"
        failed=1
    fi
done < <(find crates src -type f -name '*.rs')
if ((counted == 0)); then
    printf '    FAIL no .rs files under crates/ or src/; this check measured nothing\n'
    failed=1
fi

section "no process memory in shipped files"
# `docs/` is published as a website and `crates/`, `src/` and `schema/` are the
# program, so all four are read by people who have never opened this repository.
# `README.md` is read by more of them than any of the four: crates.io renders it
# as the package page and `.github/workflows/release.yml` copies it into every
# archive a release publishes, so it leaves this repository literally rather than
# only in effect. The rest of the root markdown stays out for the same reason
# `CLAUDE.md` does — CONTRIBUTING, RELEASING and the changelog are addressed to
# someone who has the repository open, and naming its own machinery is what they
# are for.
#
# The manifests are read with them, because they leave too: crates.io renders
# what `[package]` declares, and `cargo package` puts both the manifest and its
# unedited original inside the `.crate` file, comments and all. A reason written
# above a dependency is addressed to a reviewer here, but it ships either way, so
# it is held to the same rule as a doc comment.
#
# A decision identifier, an assumption label or the name of a planning directory
# is this repository talking to itself: a stranger cannot resolve it and has no
# reason to want to. Those notes are allowed to exist — just not to travel — so
# the files that hold them are deliberately not scanned here. There are four of
# those directories now rather than one: `.claude/` and `.agents/` are the two
# harnesses' rules and skills, `.codex/` is a third harness's settings, and
# `.sdlc-skills/` is where the briefs, designs, decisions and audits live. All
# four are excluded from the operands and named in the pattern, which is the
# whole arrangement: they may hold this, and a shipped file may not cite them.
#
# The identifier shape is a prefix of one to six capitals. It was a single
# capital, which matched `R-1` and let `REQ-001`, `FR-12`, `NFR-3` and `AC-14`
# past — every spelling anybody actually writes. Widening it reaches real
# standards too, so the ones this tree can be expected to name are subtracted
# afterwards by token rather than by line, and `-o` is what makes that possible:
# a line holding `UTF-8` and `REQ-001` yields two matches and only the first is
# dropped. Each name on that list is a deliberate hole, so add one only when the
# tree actually needs it.
#
# The exit code is read rather than the command being used as the condition
# directly. `grep` answers 0 for a match, 1 for none and 2 for "I could not
# finish" — an unreadable file, a directory that is not there. Only the first of
# those is a violation and only the second is a pass, so a bare `if grep` treats
# a scan that never happened as a clean one, and prints whatever it did find on
# the way past. `|| scan=$?` is what carries that status out: `errexit` ends the
# script on a failed assignment, and "no match" is a failed assignment, so a bare
# `memory=$(grep ...)` would exit 1 with nothing printed on precisely the clean
# tree this is meant to wave through.
scan=0
memory=$(grep -rIonE '\b[A-Z]{1,6}-[0-9]{1,4}\b|sdlc-skills|\bADR\b|\.claude/|\.agents/|\.codex/' \
    --include='*.rs' --include='*.md' --include='*.json' --include='*.toml' \
    crates src docs schema README.md Cargo.toml) || scan=$?
case $scan in
    0)
        # Published standards that share the identifier's shape. Subtracted here
        # rather than written into the pattern above, because a negative match is
        # the one thing an extended regular expression cannot say.
        memory=$(printf '%s\n' "$memory" |
            grep -vE ':(UTF|SHA|ISO|IEC|RFC|IEEE|ECMA|ANSI|CVE|AES)-[0-9]+$') || memory=""
        if [[ -n "$memory" ]]; then
            printf '%s\n' "$memory"
            printf '    FAIL the lines above name something only this repository can resolve\n'
            failed=1
        fi
        ;;
    1) ;;
    *)
        printf '%s\n' "$memory"
        printf '    FAIL the scan could not be completed; this check measured nothing\n'
        failed=1
        ;;
esac

section "documentation links"
# Every topic under `docs/` is a directory whose path is a public URL, so a
# rename that misses one link publishes a dead page rather than failing a build.
# The markdown at the root is read with it: GitHub renders those pages too, and
# a changelog entry keeps pointing at the tree it was written against long after
# that tree has moved. `AGENTS.md` is a symlink, so its links are CLAUDE.md's
# and are already checked under that name.
#
# Only repository-relative links are checked: an external one can rot without
# this tree changing, and a gate that goes red for someone else's outage stops
# being read.
pages=()
while IFS= read -r -d '' page; do
    pages+=("$page")
done < <(
    find docs -name '*.md' -type f -print0
    find . -maxdepth 1 -name '*.md' -type f -print0
)
if ((${#pages[@]} == 0)); then
    printf '    FAIL no markdown under docs/ or at the root; this check measured nothing\n'
    failed=1
fi

while IFS= read -r found; do
    file=${found%%:*}
    target=${found#*:}

    # A destination ends at the first space unless it is wrapped in angle
    # brackets; whatever follows that space is the optional title. Keeping the
    # title turned `[read](../read.md "what read bounds")` — a legal link that
    # every renderer resolves — into a FAIL for a file that is right there, and
    # a gate that goes red for a correct page stops being read.
    if [[ "$target" == \<* ]]; then
        target=${target#<}
        target=${target%%>*}
    else
        target=${target%%[[:space:]]*}
    fi

    [[ -z "$target" || "$target" == \#* ]] && continue
    if [[ ! -e "$(dirname "$file")/${target%%#*}" ]]; then
        printf '    FAIL %s: link to %s leads nowhere\n' "$file" "$target"
        failed=1
    fi
done < <(
    # Both spellings, because a page written in either is a page that can rot.
    # `</dev/null` is what keeps the empty case a report rather than a hang: a
    # `grep` handed no file operands reads standard input, so a tree with no
    # markdown at all would leave this waiting instead of failing above.
    {
        # Inline: `[text](target)`.
        grep -IHoE '\]\([^)#][^)]*\)' -- "${pages[@]}" </dev/null |
            sed -E 's/\]\(([^)]*)\)$/\1/'

        # Reference: `[label]: target`, defined once at the foot of a page and
        # used by label everywhere above. Nothing on the line that uses it names
        # a path, so the definition is the only place a rename can be caught.
        grep -IHoE '^[[:space:]]{0,3}\[[^]]+\]:[[:space:]]*[^[:space:]]+' \
            -- "${pages[@]}" </dev/null |
            sed -E 's/:[[:space:]]*\[[^]]*\]:[[:space:]]*/:/'
    } | grep -v '://'
)

section "agent rules files"
# One set of rules, one file. CLAUDE.md is the original everywhere in this
# repo; AGENTS.md is a symlink beside it, so no tool reads a stale copy.
linked=0
while IFS= read -r link; do
    linked=$((linked + 1))
    if [[ ! -L "$link" ]]; then
        printf '    FAIL %s: must be a symlink to CLAUDE.md, not a file\n' "$link"
        failed=1
    elif [[ "$(readlink "$link")" != "CLAUDE.md" ]]; then
        printf '    FAIL %s: points at %s, expected CLAUDE.md\n' "$link" "$(readlink "$link")"
        failed=1
    fi
done < <(find . -path ./target -prune -o -name AGENTS.md -print)
if ((linked == 0)); then
    printf '    FAIL no AGENTS.md anywhere; Codex reads that name and nothing else\n'
    failed=1
fi

# The rules are the same idea one directory down: written once under .claude/,
# and reached under whichever name a harness looks for. A copy is the failure —
# two sets of rules that agree today and diverge on the first edit. The
# direction matters and is checked, because both directories look plausible and
# only one of them is where a file is edited; skills below point the same way.
if [[ ! -L .agents/rules ]]; then
    printf '    FAIL .agents/rules must be a symlink to ../.claude/rules, not a directory\n'
    failed=1
elif [[ "$(readlink .agents/rules)" != "../.claude/rules" ]]; then
    printf '    FAIL .agents/rules points at %s, expected ../.claude/rules\n' "$(readlink .agents/rules)"
    failed=1
fi

section "agent rules scope"
# Two kinds of rule live here and only one of them is checked for scope.
#
# A rule *with* `paths:` is read when a file it claims is opened, so a glob
# aimed at a crate that has since been renamed stops applying without anyone
# noticing — that is what the loop below catches.
#
# A rule *without* `paths:` is read every session instead, which is the harness
# behaviour rather than an oversight, and is the only way to state something
# that no file can trigger: what a commit message may say, what may be taken
# from reading another project. Those have no glob that could ever match,
# because the thing they govern is not a file. So an absent `paths:` passes
# here and is left to be justified by the rule itself.
rules=(.claude/rules/*.md)
if ((${#rules[@]} == 0)); then
    printf '    FAIL .claude/rules/ holds no rules file; every per-crate rule has stopped loading\n'
    failed=1
fi

# Every glob every rule claims, gathered as the loop below validates them. The
# per-crate check reads this rather than the files themselves: searching a whole
# rules file would let a crate named once in a sentence satisfy the requirement
# while no frontmatter aims anything at it.
claimed=""
unscoped=0

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

    # Unscoped: loaded every session, nothing to validate. Counted so the
    # summary can say how much of every session's context these cost.
    if [[ -z "$globs" ]]; then
        unscoped=$((unscoped + 1))
        continue
    fi

    while IFS= read -r glob; do
        claimed+="$glob"$'\n'

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

# One per package, and `src/` is one of them. It is not under `crates/`, so a
# loop over `crates/*/` alone reported complete coverage of a tree it was only
# looking at part of — and of the part it skipped, CLAUDE.md says "wiring only,
# the sole place concrete types meet". That is the one directory where a
# provider, a tool and a runner are allowed to know about each other, which
# makes the obligations binding there the most particular in the tree rather
# than the least.
#
# A package nothing claims is a corner where those obligations are written down
# nowhere. Asked of `claimed` rather than of the rules files: searching a whole
# file would let a package named once in a sentence satisfy the requirement
# while no frontmatter aims anything at it. The leading newline anchors the test
# to the start of a claim, so `crates/crucible-tools/src/**` cannot answer for
# `src/`.
for package in crates/*/ src/; do
    if [[ $'\n'"$claimed" != *$'\n'"$package"* ]]; then
        printf '    FAIL %s: no paths: frontmatter under .agents/rules/ claims it\n' "$package"
        failed=1
    fi
done

section "agent skills"
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

# Every manifest in the workspace, root first. The three checks below are all
# about what a manifest declares, and a manifest gate that reads one file is a
# statement about one seventh of this tree phrased as a statement about the
# tree.
manifests=(Cargo.toml crates/*/Cargo.toml)

section "workspace lints"
# Reported under the first of the three that reads it, so that "this check
# measured nothing" belongs to a section rather than to the gap between two.
if ((${#manifests[@]} < 2)); then
    printf '    FAIL no manifest under crates/; every manifest gate here would read the root alone\n'
    failed=1
fi

# `[workspace.lints]` in the root manifest is where the standard is written, but
# a lint table binds only the crates that ask for it — `[lints] workspace = true`,
# once per manifest. A crate without that line is not held to a weaker rule 1;
# it is not held to it at all. `unwrap_used`, `panic`, `indexing_slicing`,
# `print_stdout` and `unsafe_code` are every one of them allow-by-default, so
# `cargo clippy -- -D warnings` above stays green while `.unwrap()`, `panic!()`
# and `unsafe {}` are all legal in that crate. Nothing in the compiler can
# notice a table that was never applied to it, which is why it is checked here.
for manifest in "${manifests[@]}"; do
    # Both spellings TOML allows for the same table, because a manifest written
    # in either is opted in and reporting otherwise would send the reader
    # looking for a line that is already there.
    if ! awk '
        /^[[:space:]]*lints\.workspace[[:space:]]*=[[:space:]]*true/ { found = 1 }
        /^[[:space:]]*\[lints\]/ { table = 1; next }
        /^[[:space:]]*\[/        { table = 0; next }
        table && /^[[:space:]]*workspace[[:space:]]*=[[:space:]]*true/ { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$manifest"; then
        printf '    FAIL %s: no [lints] workspace = true, so every lint the workspace denies is back to its default here\n' "$manifest"
        failed=1
    fi
done

section "dependency pinning"
# Exact pins keep a release reproducible and make a version bump a reviewed
# change rather than a side effect of somebody else's publish.
#
# Cargo accepts several spellings and this has to read all of them: a grep for a
# literal `version =` key sees only the inline-table form and waves the bare one
# straight through, which is most of them. The key is not always on the crate's
# own line either — an inline table wraps once it carries a feature list, and
# the version can end up after the array — so any `version =` inside a
# dependency table is a pin site wherever it sits, and brace depth is tracked so
# that a continuation line is never read as a new dependency.
unpinned=$(awk '
    function opens(s,   n)  { n = gsub(/\{/, "&", s); return n }
    function closes(s,   n) { n = gsub(/\}/, "&", s); return n }
    function report(   text) {
        text = $0
        sub(/^[[:space:]]+/, "", text)
        print "        " FILENAME ": " (named == "" ? "" : named ": ") text
    }

    # `[dependencies]`, its dev and build variants, the same three behind a
    # `[target.<cfg>.]` prefix, and `[workspace.dependencies]` in the root. Each
    # has a second form that puts one crate in the header: there the header is
    # the dependency and every key beneath it is an option of that one crate
    # rather than a dependency of its own.
    /^[[:space:]]*\[/ {
        header = $0
        sub(/^[[:space:]]*\[+[[:space:]]*/, "", header)
        sub(/[[:space:]]*\]+.*$/, "", header)
        table = 0
        named = ""
        depth = 0
        if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)$/) {
            table = 1
        } else if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)\.[^.]+$/) {
            table = 1
            named = header
            sub(/^.*\./, "", named)
        }
        next
    }

    table == 0       { next }
    /^[[:space:]]*#/ { next }

    {
        if ($0 ~ /version[[:space:]]*=/) {
            if ($0 !~ /version[[:space:]]*=[[:space:]]*"=/) report()
        } else if (depth == 0 && named == "" &&
                   $0 ~ /^[a-zA-Z0-9_-]+[[:space:]]*=/ &&
                   $0 !~ /^[a-zA-Z0-9_-]+\./ && $0 !~ /\{/) {
            # The bare spelling, where the whole value is the requirement. A
            # dotted key and an inline table are both handled above: one names
            # its version on a `.version` line, the other inside the braces, and
            # a path dependency inside this workspace names none because it
            # needs none.
            if ($0 !~ /=[[:space:]]*"=/) report()
        }

        depth += opens($0) - closes($0)
        if (depth < 0) depth = 0
    }
' "${manifests[@]}")
if [[ -n "$unpinned" ]]; then
    printf '    FAIL not =-pinned:\n%s\n' "$unpinned"
    failed=1
fi

section "dependency justification"
# And a comment above it saying why it is there. A dependency is a permanent
# cost paid for what is usually a temporary convenience, so the reason has to
# outlive the pull request that added it — whoever later asks whether it can go
# is never the person who knew.
#
# One comment still covers a group, since the ripgrep crates are one decision
# rather than four. What it may no longer do is cover a crate it never mentions:
# the flag used to be raised by a comment and lowered only by a blank line, so a
# crate pasted straight under an existing one inherited its reason and passed —
# which made the sentence in `CLAUDE.md` true of the `=` pin and theatre for the
# comment. A comment is now spent on the first dependency beneath it, and any
# further member of the same run has to be named in it. That is the property
# worth having anyway: a reason that does not name the crate is not a reason
# about that crate, and a paste is exactly the case a comment cannot name.
#
# The table starts unjustified. Seeding it from whatever line preceded the
# header meant the comment block explaining the crate layering counted as a
# reason for the first crate beneath it, so a dependency added there needed no
# comment of its own.
unjustified=$(awk '
    function opens(s,   n)  { n = gsub(/\{/, "&", s); return n }
    function closes(s,   n) { n = gsub(/\}/, "&", s); return n }
    function report(what,   text) {
        text = what
        sub(/^[[:space:]]+/, "", text)
        print "        " FILENAME ": " text
    }

    # Whole-word, so that `grep-searcher` in the prose cannot answer for
    # `grep-regex`. Backticks, commas and full stops are all boundaries.
    function names(crate) {
        return block ~ ("(^|[^a-zA-Z0-9_-])" crate "([^a-zA-Z0-9_-]|$)")
    }

    # A comment run starts a new block once the previous one has met a
    # dependency, so a reason written between two crates belongs to the second.
    function collect() {
        if (spent) { block = ""; spent = 0 }
        block = block " " $0
        covers = 1
    }

    FNR == 1 { table = 0; covers = 0; block = ""; spent = 0; depth = 0; named = ""; last = "" }

    /^[[:space:]]*\[/ {
        header = $0
        sub(/^[[:space:]]*\[+[[:space:]]*/, "", header)
        sub(/[[:space:]]*\]+.*$/, "", header)
        above = covers
        table = 0
        named = ""
        depth = 0
        covers = 0
        block = ""
        spent = 0
        last = ""
        if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)$/) {
            table = 1
        } else if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)\.[^.]+$/) {
            table = 1
            named = header
            sub(/^.*\./, "", named)
            if (above == 0) report(named)
        }
        next
    }

    table == 0 {
        if ($0 ~ /^[[:space:]]*#/) collect()
        else { covers = 0; block = ""; spent = 0 }
        next
    }

    /^[[:space:]]*#/ { collect(); next }
    /^[[:space:]]*$/ { covers = 0; block = ""; spent = 0; next }

    {
        # Inside `[dependencies.foo]` the header is the dependency and the
        # comment above it is the reason; the keys beneath belong to that one
        # crate, so asking each of them for a justification names lines that
        # never could carry one. The same goes for the body of an inline table
        # that wrapped onto a second line — and that body must not spend the
        # comment either, or the first line after the closing brace inherits it.
        wrapped = (depth > 0)
        depth += opens($0) - closes($0)
        if (depth < 0) depth = 0
        if (wrapped || named != "") next

        # A crate that takes the workspace entry inherits the reason with the
        # pin. Both are written once, at the root, beside the version — asking
        # for a second copy in the member manifest would put the same sentence
        # in two files and make one of them wrong later.
        if ($0 ~ /workspace[[:space:]]*=[[:space:]]*true/) next

        # A dotted key declares a dependency too, and is read the same way.
        if ($0 !~ /^[a-zA-Z0-9_-]+(\.[a-zA-Z0-9_-]+)*[[:space:]]*=/) next

        crate = $0
        sub(/[[:space:]]*=.*$/, "", crate)
        sub(/\..*$/, "", crate)

        # `some-crate.features` under `some-crate.version` is the same
        # dependency saying one more thing about itself, not a second one
        # arriving without a reason. TOML forbids the same key twice, so equal
        # names here can only be the dotted spelling continuing.
        if (crate == last) next
        last = crate

        spent = 1

        # A path dependency on a member of this workspace is this project
        # depending on itself. There is no publisher whose release could move it
        # and nobody to ask whether it can go, which is what the reason is for —
        # the layering comment above `[workspace.dependencies]` is the whole
        # answer, and it is one answer rather than six. It still spends the
        # comment, so a third-party crate pasted under the run does not inherit
        # it. `crates/` and not any path: outside the workspace it is a third
        # party again, on a path with no version for the pin to check.
        if ($0 ~ /path[[:space:]]*=[[:space:]]*"crates\//) { covers = 0; next }

        if (covers) { covers = 0; next }

        if (names(crate)) next

        report($0)
    }
' "${manifests[@]}")
if [[ -n "$unjustified" ]]; then
    printf '    FAIL no comment saying why it is needed:\n%s\n' "$unjustified"
    printf '         a comment covers the first dependency under it; name the crate in that\n'
    printf '         comment to add it to the group, or give it a comment of its own\n'
    failed=1
fi

section "github actions pinning"
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

section "benchmark gate"
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

close_section

if ((any)); then
    echo
    echo "FAILED — see the lines marked FAIL above, under:"
    printf '    %s\n' "${failures[@]}"
    exit 1
fi

echo
echo "all gates passed"
