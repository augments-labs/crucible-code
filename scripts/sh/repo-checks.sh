#!/usr/bin/env bash
# Cross-file repository policy. Language compilation and tests belong in their
# language gates; this file may inspect any language where repository structure
# spans files or ecosystems.
set -uo pipefail
shopt -s nullglob

cd "$(dirname "$0")/../.."

readonly MAX_RUST_FILE_LINES=2000

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

section "script layout"
# `scripts/sh` holds what a shell runs and `scripts/python` what python3 runs,
# so a reader looking for one language opens one directory. Nothing else in the
# tree names a script, and every reference in the repository spells the
# language directory, so a file left at the top drops out of both gates
# silently rather than failing.
for script in scripts/*.sh scripts/*.py; do
    if [[ -e $script ]]; then
        printf '    FAIL %s belongs under scripts/sh or scripts/python\n' "$script"
        failed=1
    fi
done
for stray in $(find scripts -mindepth 2 -name '*.sh' -not -path 'scripts/sh/*'); do
    printf '    FAIL %s is Bash outside scripts/sh\n' "$stray"
    failed=1
done
for stray in $(find scripts -mindepth 2 -name '*.py' -not -path 'scripts/python/*'); do
    printf '    FAIL %s is Python outside scripts/python\n' "$stray"
    failed=1
done
for language in sh python; do
    if [[ -z $(find "scripts/$language" -maxdepth 1 -type f -print -quit 2>/dev/null) ]]; then
        printf '    FAIL scripts/%s holds nothing; the split it names is gone\n' "$language"
        failed=1
    fi
done

section "generated artifacts"
# A run writes its budgets, campaign and canary reports under `generated/`, so
# a person can read what one produced without a measurement ever entering the
# history. The directory earns that only while it is ignored and empty of
# tracked files: one committed there is a generated file the repository now has
# to keep true, which is what `schema/` is for instead.
if ! grep -qx 'generated/' .gitignore; then
    printf '    FAIL .gitignore does not ignore generated/\n'
    failed=1
fi
while read -r tracked; do
    [[ -n $tracked ]] || continue
    printf '    FAIL %s is tracked under generated/\n' "$tracked"
    failed=1
done < <(git ls-files -- generated)

section "installer"
for script in scripts/sh/install.sh scripts/sh/uninstall.sh scripts/sh/install-tests.sh; do
    if ! bash -n "$script"; then
        printf '    FAIL %s is not valid Bash\n' "$script"
        failed=1
    fi
done
if ! scripts/sh/install-tests.sh; then
    printf '    FAIL the installer did not preserve its checksum, ownership, or rollback contract\n'
    failed=1
fi

section "tracked source secrets"
# Deliberately narrow signatures: each names a credential format whose prefix is
# part of the provider's contract. Generic entropy and words such as `password`
# produce findings nobody can distinguish from fixtures and teach maintainers to
# ignore this gate. Files come from Git, so build output and a developer's local
# untracked configuration are never read.
secret_scan=0
# The expression's alternatives are assembled across lines so the scanner also
# covers this file: policy source must not need an exclusion from its own rule.
secret_parts=(
    'AKIA[0-9A-Z]{16}'
    'ASIA[0-9A-Z]{16}'
    'gh[pousr]_[A-Za-z0-9]{20,}'
    'github_pat_[A-Za-z0-9_]{20,}'
    'glpat-[A-Za-z0-9_-]{20,}'
    'xox[baprs]-[A-Za-z0-9-]{10,}'
    'AIza[A-Za-z0-9_-]{35}'
    'sk_live_[A-Za-z0-9]{16,}'
    'sk-ant-[A-Za-z0-9_-]{20,}'
    'sk-(proj-)?[A-Za-z0-9_-]{20,}'
    'BEGIN ([A-Z0-9 ]+ )?PRIVATE KEY'
)
secret_pattern=$(IFS='|'; printf '%s' "${secret_parts[*]}")
secrets=$(git grep -InE "$secret_pattern" -- .) || secret_scan=$?
case $secret_scan in
    0)
        printf '%s\n' "$secrets"
        printf '    FAIL the tracked lines above contain a credential-shaped value\n'
        printf '         replace a fixture with a conspicuously fake value; never allowlist a real secret\n'
        failed=1
        ;;
    1) ;;
    *)
        printf '%s\n' "$secrets"
        printf '    FAIL the tracked-source secret scan could not be completed\n'
        failed=1
        ;;
esac

section "cross-crate wildcard re-exports"
# A sibling crate re-exported whole becomes this crate's public API by accident:
# every public item added there is published here without an edit at this seam.
# Explicit names keep that API reviewable. `pub use crate::...::*` stays a local
# module choice and is not what this cross-crate boundary guards.
wildcard_scan=0
wildcards=$(git grep -InE \
    '^[[:space:]]*pub[[:space:]]+use[[:space:]]+(::)?crucible_[a-z0-9_]+(::[A-Za-z0-9_]+)*::\*[[:space:]]*;' \
    -- '*.rs') || wildcard_scan=$?
case $wildcard_scan in
    0)
        printf '%s\n' "$wildcards"
        printf '    FAIL cross-crate wildcard re-exports make another crate public wholesale\n'
        printf '         re-export the intended names explicitly\n'
        failed=1
        ;;
    1) ;;
    *)
        printf '%s\n' "$wildcards"
        printf '    FAIL the cross-crate wildcard re-export scan could not be completed\n'
        failed=1
        ;;
esac

section "file length (<= ${MAX_RUST_FILE_LINES} lines)"
counted=0
while IFS= read -r file; do
    counted=$((counted + 1))
    lines=$(wc -l <"$file")
    if ((lines > MAX_RUST_FILE_LINES)); then
        printf '    FAIL %s: %d lines > %d\n' "$file" "$lines" "$MAX_RUST_FILE_LINES"
        failed=1
    fi
done < <(find crates src -type f -name '*.rs')
if ((counted == 0)); then
    printf '    FAIL no .rs files under crates/ or src/; this check measured nothing\n'
    failed=1
fi

section "no process memory in shipped files"
scan=0
memory=$(grep -rIonE '\b[A-Z]{1,6}-[0-9]{1,4}\b|sdlc-skills|\bADR\b|\.claude/|\.agents/|\.codex/' \
    --include='*.rs' --include='*.md' --include='*.json' --include='*.toml' \
    crates src docs schema README.md Cargo.toml) || scan=$?
case $scan in
    0)
        # Standards and the public GPT model family are not internal work-item
        # identities; their names remain meaningful outside this repository.
        memory=$(printf '%s\n' "$memory" |
            grep -vE ':(UTF|SHA|ISO|IEC|RFC|IEEE|ECMA|ANSI|CVE|AES|GPT)-[0-9]+$') || memory=""
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
    {
        grep -IHoE '\]\([^)#][^)]*\)' -- "${pages[@]}" </dev/null |
            sed -E 's/\]\(([^)]*)\)$/\1/'
        grep -IHoE '^[[:space:]]{0,3}\[[^]]+\]:[[:space:]]*[^[:space:]]+' \
            -- "${pages[@]}" </dev/null |
            sed -E 's/:[[:space:]]*\[[^]]*\]:[[:space:]]*/:/'
    } | grep -v '://'
)

section "agent guidance"
guides=0
while IFS= read -r guide; do
    guides=$((guides + 1))
    if [[ -L "$guide" || ! -f "$guide" ]]; then
        printf '    FAIL %s: must be a regular canonical file\n' "$guide"
        failed=1
        continue
    fi
    if (($(awk 'END { print NR }' "$guide") > 200)); then
        printf '    FAIL %s: exceeds the 200-line guidance limit\n' "$guide"
        failed=1
    fi
    link="$(dirname "$guide")/CLAUDE.md"
    if [[ ! -L "$link" ]]; then
        printf '    FAIL %s: must be a symlink to AGENTS.md\n' "$link"
        failed=1
    elif [[ "$(readlink "$link")" != "AGENTS.md" ]]; then
        printf '    FAIL %s: points at %s, expected AGENTS.md\n' "$link" "$(readlink "$link")"
        failed=1
    fi
done < <(find . -path ./target -prune -o -name AGENTS.md -print)
if ((guides == 0)); then
    printf '    FAIL no AGENTS.md anywhere; harnesses lose the canonical guide\n'
    failed=1
fi
while IFS= read -r link; do
    if [[ ! -L "$link" || "$(readlink "$link")" != "AGENTS.md" || ! -f "$link" ]]; then
        printf '    FAIL %s: must resolve through a symlink to its canonical AGENTS.md\n' "$link"
        failed=1
    fi
done < <(find . -path ./target -prune -o -name CLAUDE.md -print)

if [[ -L ".agents/skills" || ! -d ".agents/skills" ]]; then
    printf '    FAIL .agents/skills must be a real canonical directory\n'
    failed=1
fi

skills=(.agents/skills/*)
if ((${#skills[@]} == 0)); then
    printf '    FAIL .agents/skills/ holds no skill\n'
    failed=1
fi
for skill in "${skills[@]}"; do
    name=$(basename "$skill")
    if [[ -L "$skill" || ! -d "$skill" || -L "$skill/SKILL.md" || ! -f "$skill/SKILL.md" ]]; then
        printf '    FAIL %s: must be a canonical skill directory with a regular SKILL.md\n' "$skill"
        failed=1
    fi

    link=".claude/skills/$name"
    want="../../.agents/skills/$name"
    if [[ ! -L "$link" ]]; then
        printf '    FAIL %s: must be a symlink to %s\n' "$link" "$want"
        failed=1
    elif [[ "$(readlink "$link")" != "$want" ]]; then
        printf '    FAIL %s: points at %s, expected %s\n' "$link" "$(readlink "$link")" "$want"
        failed=1
    fi
done
for entry in .claude/skills/*; do
    [[ -e "$entry" || -L "$entry" ]] || continue
    want="../../.agents/skills/$(basename "$entry")"
    if [[ ! -L "$entry" || "$(readlink "$entry")" != "$want" || ! -f "$entry/SKILL.md" ]]; then
        printf '    FAIL %s: must resolve through a symlink to %s\n' "$entry" "$want"
        failed=1
    fi
done

section "component fit sweep"
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

section "accepted screens"
# The whole-screen suite proves a capture matches its accepted picture. It
# cannot prove the picture is still the one a reviewer accepted, because a
# regenerated picture agrees with whatever drew it. This reads the manifest
# instead, and is the check a redrawn screen has to be explained to.
if ! python3 scripts/python/screen-baseline.py; then
    printf '    FAIL a terminal screen changed without scripts/screen-baseline.json saying so\n'
    failed=1
fi

# Both spellings, and the tests with the source. A call written
# `RecordedToolOutput::replayed(output, ..)` is the same call as
# `output.replayed(..)`, and a pin that only knew the dot form would be a pin
# anyone could walk past without meaning to. Occurrences rather than lines,
# because two calls on one line are two calls.
doors() {
    grep -rEoh --include='*.rs' "$1" "${@:2}" | wc -l
}

section "the replay seam"
replay="crates/crucible-session/src/session/wire.rs"
opens='(\.|RecordedToolOutput::)replayed\('
elsewhere=$(grep -rlE --include='*.rs' "$opens" crates src tests | grep -Fxv "$replay" || true)
if [[ -n "$elsewhere" ]]; then
    while IFS= read -r file; do
        printf '    FAIL %s calls RecordedToolOutput::replayed; only %s may\n' "$file" "$replay"
    done <<<"$elsewhere"
    failed=1
fi
here=$(doors "$opens" "$replay")
if ((here != 1)); then
    printf '    FAIL %s calls RecordedToolOutput::replayed %d times; the replay is one call\n' "$replay" "$here"
    failed=1
fi

# The door the extraction opened. `RecordedToolOutput` lives in a crate that
# cannot name `Approved`, so the constructor that mints one with attachments
# cannot ask for the permission proof the live `with_attachments` requires. What
# stands in for the type is this: one caller, inside the conversion the live
# value walks out through, so an attachment still reaches a request only from a
# value the permission engine bound. Unlike the seam above there is no dot form
# to pin -- `recorded` takes no `self` -- and the bare name belongs to other
# types, so only qualified spellings are pinned: the type's own name, and
# `Self`, which is how a second door would be opened from inside the file that
# defines it, beside the builders already living there.
mints="crates/crucible-core/src/tool.rs"
attaches='(RecordedToolOutput|Self)::recorded\('
elsewhere=$(grep -rlE --include='*.rs' "$attaches" crates src tests | grep -Fxv "$mints" || true)
if [[ -n "$elsewhere" ]]; then
    while IFS= read -r file; do
        printf '    FAIL %s calls RecordedToolOutput::recorded; only %s may\n' "$file" "$mints"
    done <<<"$elsewhere"
    failed=1
fi
here=$(doors "$attaches" "$mints")
if ((here != 1)); then
    printf '    FAIL %s calls RecordedToolOutput::recorded %d times; the recording is one call\n' "$mints" "$here"
    failed=1
fi

# The other half of the same seam. What a pruning cleared is held beside the
# transcript so a resumed screen can say it again, and Pruned::showed is the one
# door out of that side-table. A second caller is how text the model was told to
# stop being sent finds its way back into a request, which is the whole thing the
# side-table is shaped to prevent. The one file that defines and tests the type
# is left alone; the rest of its crate is not, because a door is no narrower for
# being opened by a neighbour.
reader="src/cli/converse/replaying.rs"
owner="crates/crucible-session/src/session/replay.rs"
reads='(\.|Pruned::)showed\('
elsewhere=$(grep -rlE --include='*.rs' "$reads" crates src tests |
    grep -Fxv "$reader" |
    grep -Fxv "$owner" || true)
if [[ -n "$elsewhere" ]]; then
    while IFS= read -r file; do
        printf '    FAIL %s reads Pruned::showed; only %s may\n' "$file" "$reader"
    done <<<"$elsewhere"
    failed=1
fi
here=$(doors "$reads" "$reader")
if ((here != 1)); then
    printf '    FAIL %s reads Pruned::showed %d times; the substitution is one call\n' "$reader" "$here"
    failed=1
fi

section "the path that is described, not opened"
# `Workspace::intended` hands back a plain path instead of a proof, and it
# resolves through the nearest *existing* ancestor — the one shape that must
# never be opened by name. The permission engine needs exactly that, because it
# describes a call rather than making one. Splitting the workspace out of core
# turned the call `pub`, and Cargo cannot say "public to one caller", so the pin
# says it here. The owning crate defines and tests it, as the pins above leave
# their owners.
asker="crates/crucible-core/src/permission/sensitivity.rs"
owner="crates/crucible-workspace/src/resolve.rs"
tests="crates/crucible-workspace/src/tests.rs"
asks='(\.|Workspace::|Self::)intended\('
elsewhere=$(grep -rlE --include='*.rs' "$asks" crates src tests |
    grep -Fxv "$asker" |
    grep -Fxv "$owner" |
    grep -Fxv "$tests" || true)
if [[ -n "$elsewhere" ]]; then
    while IFS= read -r file; do
        printf '    FAIL %s calls Workspace::intended; only %s may\n' "$file" "$asker"
    done <<<"$elsewhere"
    failed=1
fi
here=$(doors "$asks" "$asker")
if ((here != 1)); then
    printf '    FAIL %s calls Workspace::intended %d times; the question is asked once\n' "$asker" "$here"
    failed=1
fi

section "the file opened by walking, not by name"
# `Opened::named` opens a path the way the operating system resolves it, which
# is right for one the person at the keyboard typed in full and wrong for one a
# model reached: the workspace settled containment at an earlier instant, and
# only the descriptor walk behind `Opened::reached` proves the tree still agrees
# at the open.
owner="crates/crucible-attachments/src/lib.rs"
typed=(
    "crates/crucible-runner/src/runner/attachments.rs"
    "src/cli/converse/attaching.rs"
)
by_name='Opened::named\('
elsewhere=$(grep -rlE --include='*.rs' "$by_name" crates src tests |
    grep -Fxv "$owner" |
    grep -Fxv "${typed[0]}" |
    grep -Fxv "${typed[1]}" || true)
if [[ -n "$elsewhere" ]]; then
    while IFS= read -r file; do
        printf '    FAIL %s opens an attachment by name; a reached path takes the walk\n' "$file"
    done <<<"$elsewhere"
    failed=1
fi
# The counter-assertion the negative check cannot make: `attaching.rs` is
# allowed to open by name, so nothing above would notice its workspace arm
# turning into a second one. Each of these reaches for the walk exactly once.
by_walk='Opened::reached\('
for reader in crates/crucible-tools/src/read.rs src/cli/converse/attaching.rs; do
    here=$(doors "$by_walk" "$reader")
    if ((here != 1)); then
        printf '    FAIL %s opens a workspace path through the walk %d times; it is opened once\n' "$reader" "$here"
        failed=1
    fi
done

member_manifests=(crates/*/Cargo.toml)
manifests=(Cargo.toml "${member_manifests[@]}")

section "crate layering"
crate_edges() {
    awk '
        FNR == 1 {
            crate = FILENAME
            sub(/^crates\//, "", crate)
            sub(/\/Cargo.toml$/, "", crate)
            if (FILENAME == "Cargo.toml") crate = "crucible-code"
            # Process substitution names files `/dev/fd/N`; `N` is the fixture
            # crate name, which keeps the expected output independent of Bash.
            if (FILENAME ~ /^\/dev\/fd\//) {
                sub(/^.*\//, "", crate)
                crate = "fixture-" crate
            }
            # Both ends of an edge are written the short way, so the allowed
            # list reads as the layering rather than package names.
            sub(/^crucible-/, "", crate)
            table = 0
        }
        /^[[:space:]]*\[/ {
            header = $0
            sub(/^[[:space:]]*\[+[[:space:]]*/, "", header)
            sub(/[[:space:]]*\]+.*$/, "", header)
            # Workspace dependencies agree versions; they do not take edges.
            table = (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)$/ &&
                     header !~ /^workspace\./)
            next
        }
        # Dotted keys and inline tables both end the name at dot, space or `=`.
        table && /^[[:space:]]*crucible-[a-z0-9-]+[[:space:].=]/ {
            dependency = $0
            sub(/^[[:space:]]*/, "", dependency)
            sub(/[[:space:].=].*$/, "", dependency)
            sub(/^crucible-/, "", dependency)
            print crate " " dependency
        }
    ' "$@"
}

# Pin both Cargo spellings the parser promises to understand. Accepting the
# workspace's current dotted keys alone would let an inline-table refactor
# silently empty part of the graph.
layer_fixture=$(crate_edges \
    <(printf '[dependencies]\ncrucible-core.workspace = true\n') \
    <(printf '[dev-dependencies]\ncrucible-session = { workspace = true, features = ["proof"] }\n'))
if [[ $(printf '%s\n' "$layer_fixture" | sed 's/^fixture-[0-9][0-9]* //') != $'core\nsession' ]]; then
    printf '    FAIL the crate-layer parser did not read dotted and inline dependency spellings\n'
    failed=1
fi

if ((${#manifests[@]} < 2)); then
    printf '    FAIL no manifest under crates/; the dependency graph measured nothing\n'
    failed=1
fi
edges=$(crate_edges "${manifests[@]}")
if [[ -z "$edges" ]]; then
    printf '    FAIL no internal dependency edges found; this check measured nothing\n'
    failed=1
fi

# `core` names the eight crates its old names now come from. Those edges are
# the compatibility facade and go away with the crate that holds them; every
# other crate still reaches the domain through one name.
#
# The exception is `attachments`, which three crates name past the facade. The
# two types a file's bytes are read through are withheld from the facade, so a
# caller that wants one takes the edge, and the edge shows up here.
allowed='code attachments
code auth
code config
code core
code extension
code mcp
code privacy
code provider
code runner
code session
code tools
code sandbox-broker
code sandbox-local
code tui
attachments types
attachments workspace
auth core
auth privacy
config core
core attachments
core credentials
core registry
core runtime
core sandbox
core storage
core types
core workspace
credentials types
extension core
mcp core
provider core
runner attachments
runner core
runner session
session core
session privacy
sandbox storage
sandbox types
sandbox workspace
sandbox-local privacy
sandbox-local sandbox
sandbox-local sandbox-broker
sandbox-local storage
sandbox-local types
sandbox-local workspace
storage types
tools attachments
tools core
tools sandbox-local'
while IFS= read -r edge; do
    [[ -z "$edge" ]] && continue
    if ! grep -Fxq "$edge" <<<"$allowed"; then
        printf '    FAIL dependency edge %s is outside the workspace layering\n' "$edge"
        failed=1
    fi
done <<<"$edges"
# Tighter than the allowed-edge list above for these crates, and for
# crucible-runtime tighter than the architecture's maximum: giving one of them
# a workspace dependency is a decision to take here rather than a line to add.
for crate in privacy registry runtime sandbox-broker tui types workspace; do
    if grep -qE "^$crate " <<<"$edges"; then
        printf '    FAIL crucible-%s must not depend on another workspace crate\n' "$crate"
        failed=1
    fi
done

# `tools sandbox-local` above is a test-support edge, and a test-support edge
# never justifies a shipped one. A tool names the sandbox service contract;
# naming one machine's answer to it in a table that ships is how that
# distinction would quietly disappear. Every such table counts, not only
# `[dependencies]`: a build script that pulls a backend in ships it too.
shipped_sandbox_local_edge() {
    awk '
        /^[[:space:]]*\[/ {
            header = $0
            sub(/^[[:space:]]*\[+[[:space:]]*/, "", header)
            sub(/[[:space:]]*\]+.*$/, "", header)
            table = (header ~ /(^|[.-])dependencies$/ \
                && header !~ /(^|\.)dev-dependencies$/ \
                && header !~ /^workspace\./)
            next
        }
        table && /^[[:space:]]*crucible-sandbox-local[[:space:].=]/ { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$1"
}

# A check that cannot say yes has not said no. Both answers are taken from it
# here before the manifest that matters is put to it.
if ! shipped_sandbox_local_edge \
    <(printf '[build-dependencies]\ncrucible-sandbox-local.workspace = true\n'); then
    printf '    FAIL the shipped-edge parser did not read a build-dependency\n'
    failed=1
fi
if shipped_sandbox_local_edge \
    <(printf '[dev-dependencies]\ncrucible-sandbox-local.workspace = true\n'); then
    printf '    FAIL the shipped-edge parser read a dev-dependency as a shipped one\n'
    failed=1
fi
if shipped_sandbox_local_edge crates/crucible-tools/Cargo.toml; then
    printf '    FAIL crucible-tools must reach crucible-sandbox-local only as a dev-dependency\n'
    failed=1
fi

section "workspace inheritance"
if ((${#member_manifests[@]} == 0)); then
    printf '    FAIL no member manifests found; workspace inheritance measured nothing\n'
    failed=1
fi

if ! awk '
    /^[[:space:]]*\[workspace\.package\]/ { table = 1; next }
    /^[[:space:]]*\[/                    { table = 0; next }
    table && /^[[:space:]]*publish[[:space:]]*=[[:space:]]*false/ { found = 1 }
    END { exit found ? 0 : 1 }
' Cargo.toml; then
    printf '    FAIL Cargo.toml: no [workspace.package] publish = false\n'
    failed=1
fi

for manifest in "${manifests[@]}"; do
    if ! awk '
        /^[[:space:]]*lints\.workspace[[:space:]]*=[[:space:]]*true/ { found = 1 }
        /^[[:space:]]*\[lints\]/ { table = 1; next }
        /^[[:space:]]*\[/        { table = 0; next }
        table && /^[[:space:]]*workspace[[:space:]]*=[[:space:]]*true/ { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$manifest"; then
        printf '    FAIL %s: no [lints] workspace = true\n' "$manifest"
        failed=1
    fi

done

for manifest in "${manifests[@]}"; do
    if ! awk '
        /^[[:space:]]*\[package\]/ { table = 1; next }
        /^[[:space:]]*\[/          { table = 0; next }
        table && /^[[:space:]]*publish\.workspace[[:space:]]*=[[:space:]]*true/ { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$manifest"; then
        printf '    FAIL %s: no package.publish.workspace = true\n' "$manifest"
        failed=1
    fi
done

unowned=$(awk '
    function opens(s,   n)  { n = gsub(/\{/, "&", s); return n }
    function closes(s,   n) { n = gsub(/\}/, "&", s); return n }
    FNR == 1 { table = 0; depth = 0 }
    /^[[:space:]]*\[/ {
        header = $0
        sub(/^[[:space:]]*\[+[[:space:]]*/, "", header)
        sub(/[[:space:]]*\]+.*$/, "", header)
        table = (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)$/)
        depth = 0
        next
    }
    table == 0 || /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    {
        if (depth == 0 &&
            $0 ~ /^[[:space:]]*[a-zA-Z0-9_-]+(\.[a-zA-Z0-9_-]+)*[[:space:]]*=/ &&
            $0 !~ /workspace[[:space:]]*=[[:space:]]*true/) {
            text = $0
            sub(/^[[:space:]]+/, "", text)
            print "        " FILENAME ": " text
        }
        depth += opens($0) - closes($0)
        if (depth < 0) depth = 0
    }
' "${member_manifests[@]}" </dev/null)
if [[ -n "$unowned" ]]; then
    printf '    FAIL member dependencies not inherited from [workspace.dependencies]:\n%s\n' "$unowned"
    failed=1
fi

section "dependency pinning"
unpinned=$(awk '
    function opens(s,   n)  { n = gsub(/\{/, "&", s); return n }
    function closes(s,   n) { n = gsub(/\}/, "&", s); return n }
    function report(   text) {
        text = $0
        sub(/^[[:space:]]+/, "", text)
        print "        " FILENAME ": " (named == "" ? "" : named ": ") text
    }
    /^[[:space:]]*\[/ {
        header = $0
        sub(/^[[:space:]]*\[+[[:space:]]*/, "", header)
        sub(/[[:space:]]*\]+.*$/, "", header)
        table = 0
        named = ""
        depth = 0
        if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)$/) table = 1
        else if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)\.[^.]+$/) {
            table = 1
            named = header
            sub(/^.*\./, "", named)
        }
        next
    }
    table == 0 || /^[[:space:]]*#/ { next }
    {
        if ($0 ~ /version[[:space:]]*=/) {
            if ($0 !~ /version[[:space:]]*=[[:space:]]*"=/) report()
        } else if (depth == 0 && named == "" &&
                   $0 ~ /^[a-zA-Z0-9_-]+[[:space:]]*=/ &&
                   $0 !~ /^[a-zA-Z0-9_-]+\./ && $0 !~ /\{/) {
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
unjustified=$(awk '
    function opens(s,   n)  { n = gsub(/\{/, "&", s); return n }
    function closes(s,   n) { n = gsub(/\}/, "&", s); return n }
    function report(what,   text) {
        text = what
        sub(/^[[:space:]]+/, "", text)
        print "        " FILENAME ": " text
    }
    function names(crate) {
        return block ~ ("(^|[^a-zA-Z0-9_-])" crate "([^a-zA-Z0-9_-]|$)")
    }
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
        if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)$/) table = 1
        else if (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)\.[^.]+$/) {
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
        wrapped = (depth > 0)
        depth += opens($0) - closes($0)
        if (depth < 0) depth = 0
        if (wrapped || named != "") next
        if ($0 ~ /workspace[[:space:]]*=[[:space:]]*true/) next
        if ($0 !~ /^[a-zA-Z0-9_-]+(\.[a-zA-Z0-9_-]+)*[[:space:]]*=/) next
        crate = $0
        sub(/[[:space:]]*=.*$/, "", crate)
        sub(/\..*$/, "", crate)
        if (crate == last) next
        last = crate
        spent = 1
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
workflow_count=0
if [[ -d .github/workflows ]]; then
    while IFS= read -r workflow; do
        workflow_count=$((workflow_count + 1))
    done < <(find .github/workflows -maxdepth 1 -type f \( -name '*.yml' -o -name '*.yaml' \))
    floating=$(grep -rn 'uses:' .github/workflows |
        grep -vE 'uses: *\./' |
        grep -vE 'uses: *[^@]+@[0-9a-f]{40} +# ' || true)
    if [[ -n "$floating" ]]; then
        printf '    FAIL not pinned to a commit sha:\n%s\n' "$floating"
        failed=1
    fi
fi
if ((workflow_count == 0)); then
    printf '    FAIL no workflow files found; this check measured nothing\n'
    failed=1
fi

close_section

if ((any)); then
    echo
    echo "FAILED — see the lines marked FAIL above, under:"
    printf '    %s\n' "${failures[@]}"
    exit 1
fi

echo
echo "all repository checks passed"
