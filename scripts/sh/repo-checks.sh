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

section "who may be sent a restricted result is decided once"
# A vendor's term keeping what it answered to its own models travels on the
# result, and `crucible_models::transfer` is where it is read. A caller that
# asked a provider what it restricts would be deciding the question again from a
# vendor's name, which is the branch the runner no longer has; tests may ask.
decider="crates/crucible-models/src/transfer.rs"
asks='(\.|::)restricts_results\('
# Comment lines are not callers: a documentation example may show the method.
askers=$(grep -rnE --include='*.rs' "$asks" crates src tests |
    grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' |
    cut -d: -f1 | sort -u |
    grep -vE '(^|/)tests(/|\.rs$)|_tests\.rs$' || true)
if [[ "$askers" != "$decider" ]]; then
    while IFS= read -r file; do
        [[ -z "$file" || "$file" == "$decider" ]] && continue
        printf '    FAIL %s asks a provider what it restricts; only %s decides that\n' "$file" "$decider"
    done <<<"$askers"
    if ! grep -Fxq "$decider" <<<"$askers"; then
        printf '    FAIL %s no longer asks what a provider restricts; this check measured nothing\n' "$decider"
    fi
    failed=1
fi

# Keeping a restricted result where it is, for a recipient that reaches no model,
# is safe only because that recipient sends nothing anywhere. A provider that said
# so and still reached a model would be handed what its vendor may not see, so the
# one shipped stand-in is the only provider that says it. Counted by definition
# rather than by file: the trait's default, the transfer tests' provider and the
# runner's fake each write it once beside the stand-in, and a second definition in
# any of those files is as much a new provider saying it as one anywhere else.
stand_in="crates/crucible-provider/src/unavailable.rs"
expected=$(printf '%s:1\n' \
    crates/crucible-models/src/provider.rs \
    crates/crucible-models/src/transfer.rs \
    crates/crucible-runner/src/fake.rs \
    "$stand_in" | sort)
written=$(grep -rcE --include='*.rs' 'fn reaches_a_model\(' crates src tests |
    grep -vE ':0$' |
    grep -vE '(^|/)tests(/|\.rs:)|_tests\.rs:' | sort || true)
if [[ "$written" != "$expected" ]]; then
    printf '    FAIL only %s may say a provider reaches no model; reaches_a_model is written, by file and count:\n' "$stand_in"
    sed 's/^/        /' <<<"$written"
    printf '    and is expected once in each of:\n'
    sed 's/^/        /' <<<"$expected"
    failed=1
fi

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
mints="crates/crucible-tools/src/tool.rs"
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
asker="crates/crucible-tools/src/permissions/sensitivity.rs"
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
for reader in crates/crucible-builtins/src/read.rs src/cli/converse/attaching.rs; do
    here=$(doors "$by_walk" "$reader")
    if ((here != 1)); then
        printf '    FAIL %s opens a workspace path through the walk %d times; it is opened once\n' "$reader" "$here"
        failed=1
    fi
done

member_manifests=(crates/*/Cargo.toml)
manifests=(Cargo.toml "${member_manifests[@]}")

section "crate layering"
# Cargo is asked which crates each crate takes. A reader of the manifests here
# sees only the spellings it was written for, and an edge spelled another way
# would pass unseen. Every manifest is handed over as well, and the reader refuses
# unless they are exactly the workspace's packages, because a crate Cargo does
# not count as a member is one whose edges it never describes. It also refuses a
# workspace that patches, replaces or overrides a dependency, or includes
# configuration that could, because Cargo names where such a dependency comes from
# only when resolving.
if ! python3 scripts/python/crate-edges.py --self-test; then
    printf '    FAIL the crate-edge reader failed its self-test\n'
    failed=1
fi
# Both ends of an edge are written the short way, so the allowed list reads as
# the layering rather than package names.
if ! edges=$(python3 scripts/python/crate-edges.py Cargo.toml "${manifests[@]}" | sed -E 's/(^| )crucible-/\1/g'); then
    printf '    FAIL the crate-edge reader gave no answer for Cargo.toml\n'
    failed=1
elif [[ -z "$edges" ]]; then
    printf '    FAIL no internal dependency edges found; this check measured nothing\n'
    failed=1
fi

# `core` names the twelve crates its old names now come from. Those edges are
# the compatibility facade and go away with the crate that holds them.
#
# Edges past the facade are listed here as they are taken. `attachments` is
# named directly because the two types a file's bytes are read through are
# withheld from the facade; the sandbox crates are named directly because a
# backend and the contract it answers are what this split gave their own names;
# `tools` and `builtins` name their owners directly because neither may reach
# back into core.
#
# `app` is where a run is composed, so it is the one crate that names concrete
# providers, tools, storage and sandboxes together. It names their owners
# directly and never the facade, and never the broker, which is the command
# line's to install. `code extension` and `code mcp` are test-only edges: the
# integration tests drive those two crates, and nothing that ships names them,
# which the next section holds. `code tools` is the probes': `bench-grep` and
# `bench-tools` lend the calls they time the tool worker, which the probe list
# below writes down.
#
# `client-api` is what a front end and the application say to each other, so it
# has one owner below it -- `types`, for the identity a session is resumed by --
# and two crates above it: `app`, which carries a request out, and the command
# line, which is one front end. The check after the list holds both ends.
#
# `runtime` is named by each crate whose contract hands back its future, each
# that implements one, and each that crosses into one through a bridge while its
# own callers are synchronous; the bridges are listed where they are defined.
# `extension`'s shipped source names it for none of these, only for `Unready`,
# so that a stop the transport dropped rather than wait on is told apart from
# one that failed; its tests also name `BoxFuture`, because a stand-in process
# implements `SandboxProcess`.
# `storage` hands back the same type spelled out, because it names no workspace
# crate but `types`, and so it names no runtime.
# `auth` names it for `BoxFuture`, the shape a renewal's request is handed to
# the owner of renewals in, and for the one waiting crossing it owns, which the
# thread a login runs on takes to wait for each of that login's requests.
#
# `http` is an HTTP client built for outgoing requests to share. It sends the
# headers a credential was applied to and hands its connector's work back as a
# runtime future, and it names nothing else in the workspace. `auth` sends
# every account login and renewal request through it.
allowed='code app
code attachments
code auth
code client-api
code config
code context
code core
code extension
code mcp
code privacy
code provider
code runner
code runtime
code session
code builtins
code sandbox-broker
code sandbox-local
code tools
code tui
app agents
app auth
app builtins
app client-api
app config
app context
app credentials
app extension
app http
app mcp
app models
app privacy
app provider
app registry
app runner
app runtime
app sandbox
app sandbox-local
app session
app tools
app types
app workspace
agents models
agents tools
agents types
attachments types
attachments workspace
client-api types
auth core
auth http
auth privacy
auth runtime
config core
config models
context models
context tools
context types
context workspace
core attachments
core context
core credentials
core models
core registry
core runtime
core sandbox
core storage
core tools
core transport
core types
core workspace
credentials types
models credentials
models runtime
models types
extension registry
extension runtime
extension sandbox
extension transport
extension types
http credentials
http runtime
mcp runtime
mcp sandbox
mcp tools
mcp transport
mcp types
provider core
provider credentials
provider http
provider models
provider runtime
provider types
runner agents
runner attachments
runner context
runner core
runner models
runner runtime
runner types
session core
session privacy
session runtime
session storage
session types
sandbox runtime
sandbox storage
sandbox types
sandbox workspace
sandbox-local privacy
sandbox-local runtime
sandbox-local sandbox
sandbox-local sandbox-broker
sandbox-local storage
sandbox-local types
sandbox-local workspace
storage types
tools registry
tools runtime
tools sandbox
tools storage
tools types
tools workspace
transport runtime
transport sandbox
transport types
builtins attachments
builtins runtime
builtins sandbox
builtins sandbox-local
builtins tools
builtins types
builtins workspace'
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

# The client contract is what may leave the process, so no engine type may
# become reachable from it, and nothing the engine is made of may come to depend
# on how a front end spells a request. The list above already says so; this
# says it as a rule, so that adding a line there is not all it takes.
while IFS= read -r edge; do
    [[ -z "$edge" ]] && continue
    case "$edge" in
        'client-api types' | 'app client-api' | 'code client-api') ;;
        'client-api '*)
            printf '    FAIL crucible-client-api depends on crucible-%s; it may name crucible-types alone\n' "${edge#client-api }"
            failed=1
            ;;
        *' client-api')
            printf '    FAIL crucible-%s depends on crucible-client-api; only the application and a front end may\n' "${edge% client-api}"
            failed=1
            ;;
    esac
done <<<"$edges"
if ! grep -Fxq 'client-api types' <<<"$edges" || ! grep -Fxq 'app client-api' <<<"$edges"; then
    printf '    FAIL the client contract is not between the application and crucible-types; this check measured nothing\n'
    failed=1
fi

# `builtins sandbox-local` above is a test-support edge, and a test-support edge
# never justifies a shipped one. A tool names the sandbox service contract;
# naming one machine's answer to it in a table that ships is how that
# distinction would quietly disappear. Every such table counts, not only
# `[dependencies]`: a build script that pulls a backend in ships it too. Cargo
# answers which tables those are, so this section runs `cargo`, and it asks
# manifests whose answer is known as well as the one that matters. Only 3 is a
# clean answer, because 1 is also what a crashed reader exits with.
if ! python3 scripts/python/shipped-edge.py --self-test; then
    printf '    FAIL the shipped-edge check failed its self-test\n'
    failed=1
fi
python3 scripts/python/shipped-edge.py crates/crucible-builtins/Cargo.toml crucible-sandbox-local
case $? in
    0)
        printf '    FAIL crucible-builtins must reach crucible-sandbox-local only as a dev-dependency\n'
        failed=1
        ;;
    3) ;;
    *)
        printf '    FAIL the shipped-edge check gave no answer for crates/crucible-builtins/Cargo.toml\n'
        failed=1
        ;;
esac

section "a test build's sandbox state stays out of a release"
# `per-checkout-state` names the Linux sandbox's state directory after the
# checkout a test build was compiled from, so that two checkouts testing at once
# keep apart. What ships uses the directory the security documentation names,
# and a build that turned the feature on would move every user's state. This
# holds the manifests to that: no table Cargo resolves for a build that ships
# may turn the feature on. A release command that asks for it itself, with
# `--features`, flags or configuration, is not something a manifest says, and
# is not read here.
#
# Resolver 2 and later resolve dev-dependencies only for tests, benches and
# examples; resolver 1 folds their features into every build. So the root
# manifest must name its resolver, and name 2 or later. The feature may then be
# turned on by a dev-dependency and nothing else: not by a normal or build
# dependency, under any target, not by the workspace table a member inherits
# from, and not by a feature of any package, the crate's own `default` included.
# Cargo is asked for the manifests as it reads them, with every inherited line
# folded in; the resolver, which it does not describe, is read from the root
# manifest it names. Every package that takes the crate turns the feature on
# for its own tests, because a narrow `cargo test -p` of it builds no other
# package's dev-dependencies. A narrow run of the crate's own integration tests
# cannot, since a crate that took itself would be an edge, and passes
# `--features per-checkout-state` instead.
sandbox_state=$(cargo metadata --no-deps --offline --format-version 1 --color never --manifest-path Cargo.toml 2>/dev/null |
    python3 -c '
import json, os, sys, tomllib

OWNER, FEATURE = "crucible-sandbox-local", "per-checkout-state"
described = json.load(sys.stdin)
packages = described["packages"]
with open(os.path.join(described["workspace_root"], "Cargo.toml"), "rb") as source:
    root = tomllib.load(source)
resolvers = [table["resolver"] for table in (root.get("workspace", {}), root.get("package", {})) if "resolver" in table]
if not resolvers:
    print(f"the root Cargo.toml names no resolver, so nothing says a release build leaves out {FEATURE}")
for resolver in resolvers:
    if not (isinstance(resolver, str) and resolver.isdigit() and int(resolver) >= 2):
        print(f"the workspace resolver is {resolver!r}; before 2, a release build takes the {FEATURE} a dev-dependency turns on")
owner = [package for package in packages if package["name"] == OWNER]
if len(owner) != 1 or FEATURE not in owner[0]["features"]:
    sys.exit(0)
tested = 0
for package in packages:
    name = package["name"]
    keys, testing = set(), False
    for dependency in package["dependencies"]:
        if dependency["name"] != OWNER:
            continue
        keys.add(dependency.get("rename") or dependency["name"])
        if FEATURE not in dependency["features"]:
            continue
        kind = dependency["kind"] or "normal"
        if kind == "dev":
            testing = True
        else:
            print(f"{name} turns on {FEATURE} in a {kind} dependency, which a release build resolves")
    for feature, enables in package["features"].items():
        if name == OWNER:
            named = feature != FEATURE and FEATURE in enables
        else:
            named = any(f"{key}{joint}{FEATURE}" in enables for key in keys for joint in ("/", "?/"))
        if named:
            print(f"{name} turns on {FEATURE} through its feature {feature}, which a release build can ask for")
    if keys and name != OWNER:
        if testing:
            tested += 1
        else:
            print(f"{name} takes {OWNER} and its tests do not turn on {FEATURE}; they would share the state of every checkout")
if tested:
    print("measured")
')
if ! grep -Fxq measured <<<"$sandbox_state"; then
    printf '    FAIL crucible-sandbox-local was not found declaring per-checkout-state and a package turning it on for its tests; this check measured nothing\n'
    failed=1
fi
while IFS= read -r line; do
    [[ -z "$line" || "$line" == measured ]] && continue
    printf '    FAIL %s\n' "$line"
    failed=1
done <<<"$sandbox_state"

section "bridge ledger"
# `Bridge` is the ledger of every synchronous caller that crosses into an
# asynchronous contract, and each entry says what bounds it, which crate owns
# it and what retires it. The code is held to it as it is written. Outside the
# ledger's package, a bridge is named only in the crate that owns it. Every
# bridge is named in its owner's shipped source, judged by path under `src/`,
# inline test modules included; in the ledger's own package no bridge's path
# counts, so a bridge that package owns always fails here. No shipped source
# but the ledger builds the waker or the context of a hand poll in a spelling
# the check knows, which is one way a crossing nobody wrote down can look:
# `Context` reached through an alias, a context an enclosing `poll` hands in,
# and a library call that polls or waits for its caller, such as tokio's
# `block_on`, build nothing it can see. None of the spellings the check knows
# hides a bridge from the search for its path: an alias of the enum by `use` or
# `type`, or a glob or braced import of its variants, and `Bridge as` is
# refused wherever it is written, a qualified trait path included. `Self::`
# inside an impl for `Bridge`, `<Bridge>::Name`, and a `Bridge` handed across
# crates as a value, are seen by none of these. Two more are refused, likewise
# only as they are written: a crossing whose own argument spells a borrow of a
# future, beginning with `&mut`, `Pin::new(&mut` or `Box::pin(&mut`, or ending
# with an `.as_mut()` after an argument that holds no `;`, `{` or `}`, not even
# in a literal, and parentheses at most two deep; and a bridge taken off a
# refusal and crossed again, where `.bridge()` comes directly before `.cross`.
# `Pin::as_mut(&mut …)`, a path-qualified `std::pin::Pin::new(…)`, a crossing
# written as a path call, `Bridge::cross(Bridge::Name, &mut …)`, and either one
# bound to a name first pass, and a literal holding an unpaired parenthesis can
# make the borrow check report a crossing that lends nothing or miss one that
# lends. Only whole `//` comment lines are left out: a trailing or block
# comment and a literal are read as code, so a spelling refused here is
# reported there too, and a bridge's path written there in its owner's shipped
# source counts as a crossing. Any answer but 0 fails, because 2 is a ledger
# the check could not read and 1 is also what a crashed reader exits with.
if ! python3 scripts/python/bridge-ledger.py --self-test; then
    printf '    FAIL the bridge-ledger check failed its self-test\n'
    failed=1
fi
if ! python3 scripts/python/bridge-ledger.py crates/crucible-runtime/src/bridge.rs "${manifests[@]}"; then
    printf '    FAIL the bridge ledger and the code that crosses it disagree\n'
    failed=1
fi

section "no spawned thread in the hosted crates"
# No shipped source in the transport, MCP, or extension crates starts its own
# thread: a `thread::spawn` in any of the three fails, which is also what keeps
# a detached pipe thread from coming back. Sync calls remain —
# `Frames::next_frame`/`Written::send` and `Hosted::greet`/`catalogue`/`call` —
# so this checks threads, not all blocking. Files that
# compile only under test are left out, by the same path rule the shipping
# source boundary uses: a `tests.rs`, anything under a `tests/` directory, and
# a `testing.rs` never ship.
shipped=()
while IFS= read -r file; do
    shipped+=("$file")
done < <(
    find crates/crucible-transport/src crates/crucible-mcp/src crates/crucible-extension/src \
        -name '*.rs' -type f \
        -not -name 'tests.rs' -not -name 'testing.rs' -not -path '*/tests/*' |
        LC_ALL=C sort
)
if ((${#shipped[@]} == 0)); then
    printf '    FAIL no shipped sources found in the hosted crates; this check measured nothing\n'
    failed=1
else
    spawned=$(grep -Hn 'thread::spawn' -- "${shipped[@]}" || true)
    if [[ -n "$spawned" ]]; then
        printf '%s\n' "$spawned"
        printf '    FAIL the lines above start a thread in shipped source of the hosted crates\n'
        failed=1
    fi
fi

section "shipping source boundary"
# The crate graph above is about packages, and the root package is three things
# at once: the command line that ships, the probes that measure it, and the
# integration tests. What the package may depend on is therefore wider than what
# the shipping command line may name. This section is about source text: the
# command line composes a run through `crucible_app` and draws it through
# `crucible_tui`, and the crates that implement a provider, a tool, storage --
# the credential store included -- or a sandbox are named in the application
# instead.
#
# Two of them are named nowhere in the command line and may not come back. The
# rest are still named in the files listed here, each for something that reads a
# key or draws a cell around a concrete value, and the list is a ratchet: a name
# in a file that is not listed fails, and so does a line whose file no longer
# names the crate, so the list can only get shorter.
#
# Files that compile only under test are left out, because a fixture has to
# build the thing it stands in for -- which is why a test module that names one
# of these crates is a `tests.rs` of its own and not a block inside the file it
# tests: inside, its names would be counted as the command line's. Comment lines
# are left out because a sentence about a crate is not a dependency on it.
names_in() {
    local file
    for file in "$@"; do
        grep -vE '^[[:space:]]*//' "$file" |
            grep -oE '\bcrucible_[a-z_]+\b' |
            LC_ALL=C sort -u |
            sed "s|^|$file |"
    done
}

shipping=()
while IFS= read -r file; do
    shipping+=("$file")
done < <(
    {
        printf '%s\n' src/main.rs src/cli.rs
        find src/cli -name '*.rs'
    } | grep -vE '(^|/)tests(\.rs|/)|/fake\.rs$|/sample\.rs$' | LC_ALL=C sort
)
if ((${#shipping[@]} < 3)); then
    printf '    FAIL no command-line sources found; this check measured nothing\n'
    failed=1
fi

named=$(names_in "${shipping[@]}")
for never in crucible_extension crucible_mcp; do
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        printf '    FAIL %s names %s; the command line reaches it through crucible_app\n' "${line% *}" "$never"
        failed=1
    done < <(grep -E " $never\$" <<<"$named")
done

residue='src/cli.rs crucible_auth
src/cli.rs crucible_builtins
src/cli.rs crucible_sandbox_broker
src/cli.rs crucible_session
src/cli/converse.rs crucible_auth
src/cli/converse.rs crucible_builtins
src/cli/converse.rs crucible_session
src/cli/converse/answering.rs crucible_builtins
src/cli/converse/asking.rs crucible_builtins
src/cli/converse/attaching.rs crucible_privacy
src/cli/converse/command/login.rs crucible_auth
src/cli/converse/command/resume.rs crucible_session
src/cli/converse/command/sandbox.rs crucible_sandbox_local
src/cli/converse/leaving.rs crucible_builtins
src/cli/converse/planning.rs crucible_builtins
src/cli/converse/recalling.rs crucible_session
src/cli/converse/replaying.rs crucible_session
src/cli/converse/resuming.rs crucible_session
src/cli/converse/typing.rs crucible_builtins
src/cli/draw.rs crucible_builtins
src/cli/draw/opening.rs crucible_session
src/cli/release.rs crucible_privacy
src/cli/release.rs crucible_provider
src/cli/standing.rs crucible_builtins'
concrete=$(grep -E ' crucible_(auth|builtins|privacy|provider|sandbox_broker|sandbox_local|session)$' <<<"$named")
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    if ! grep -Fxq "$line" <<<"$residue"; then
        printf '    FAIL %s names %s; the command line reaches it through crucible_app\n' "${line% *}" "${line#* }"
        failed=1
    fi
done <<<"$concrete"
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    if ! grep -Fxq "$line" <<<"$concrete"; then
        printf '    FAIL %s no longer names %s; take the line out of the residue so it stays out\n' "${line% *}" "${line#* }"
        failed=1
    fi
done <<<"$residue"

# The client contract says what crosses; it does not carry it anywhere. Nothing
# in the contract or in the module that carries a request out opens a socket or
# listens on one, so a front end off this machine is a decision about a
# transport that has not been taken, and cannot be taken by accident here.
contract=(crates/crucible-client-api/src crates/crucible-app/src/client.rs crates/crucible-app/src/client)
for owner in "${contract[@]}"; do
    if [[ ! -e "$owner" ]]; then
        printf '    FAIL %s is missing; the client contract check measured nothing\n' "$owner"
        failed=1
    fi
done
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    printf '    FAIL %s reaches for the network; the client contract names values, not a transport\n' "$line"
    failed=1
done < <(grep -rnE --include='*.rs' 'std::net|std::os::unix::net|TcpListener|TcpStream|UdpSocket|UnixListener|UnixStream' "${contract[@]}" 2>/dev/null | cut -d: -f1,2)

# That is a search of the source, and a crate that speaks HTTP needs none of
# those words written here to be used. So what the contract takes is written
# down whole, in every dependency table, and Cargo is asked for it rather than
# the manifest being read: one dependency can be spelled many ways.
contract_takes='crucible-types
serde_core
serde_json'
if ! contract_taken=$(cargo metadata --no-deps --offline --format-version 1 --color never --manifest-path Cargo.toml 2>/dev/null |
    python3 -c '
import json, sys
for package in json.load(sys.stdin)["packages"]:
    if package["name"] == "crucible-client-api":
        print("\n".join(sorted({dependency["name"] for dependency in package["dependencies"]})))
'); then
    printf '    FAIL Cargo did not describe the workspace; what the client contract takes was not measured\n'
    failed=1
elif ! grep -Fxq 'serde_json' <<<"$contract_taken"; then
    printf '    FAIL crucible-client-api was not found taking serde_json; this check measured nothing\n'
    failed=1
else
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        if ! grep -Fxq "$line" <<<"$contract_takes"; then
            printf '    FAIL crucible-client-api takes %s; the contract names values, and what carries them is taken by a front end\n' "$line"
            failed=1
        fi
    done <<<"$contract_taken"
    while IFS= read -r line; do
        if ! grep -Fxq "$line" <<<"$contract_taken"; then
            printf '    FAIL crucible-client-api no longer takes %s; take the line out so it stays out\n' "$line"
            failed=1
        fi
    done <<<"$contract_takes"
fi

# A probe measures one owner and imports it directly: routed through the
# application it would measure the composition instead, and a budget would move
# for a reason the probe cannot see. What each probe names is written down
# whole, so a new import is a decision taken here. `generate-models` imports
# `crucible_core` alone; `crucible_types` is in the text of the table it writes,
# which is compiled where the table is kept and not where it is generated.
# `bench-grep` and `bench-tools` name `crucible_tools` for the tool worker they
# lend the calls they time, so what they time includes handing the work to it,
# and `crucible_runtime` for the future their permission prompts answer with.
probes='src/bin/bench-grep.rs crucible_builtins
src/bin/bench-grep.rs crucible_core
src/bin/bench-grep.rs crucible_runtime
src/bin/bench-grep.rs crucible_tools
src/bin/bench-live-burst.rs crucible_tui
src/bin/bench-render-burst.rs crucible_tui
src/bin/bench-session-rss.rs crucible_attachments
src/bin/bench-session-rss.rs crucible_config
src/bin/bench-tools.rs crucible_builtins
src/bin/bench-tools.rs crucible_core
src/bin/bench-tools.rs crucible_runtime
src/bin/bench-tools.rs crucible_sandbox_local
src/bin/bench-tools.rs crucible_tools
src/bin/generate-models.rs crucible_core
src/bin/generate-models.rs crucible_types'
probe_sources=(src/bin/*.rs)
if ((${#probe_sources[@]} == 0)); then
    printf '    FAIL no probes found under src/bin; this check measured nothing\n'
    failed=1
fi
probing=$(names_in "${probe_sources[@]}")
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    if ! grep -Fxq "$line" <<<"$probes"; then
        printf '    FAIL %s names %s, which is not what this probe is recorded as measuring\n' "${line% *}" "${line#* }"
        failed=1
    fi
done <<<"$probing"
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    if ! grep -Fxq "$line" <<<"$probing"; then
        printf '    FAIL %s no longer names %s; take the line out of the probe list\n' "${line% *}" "${line#* }"
        failed=1
    fi
done <<<"$probes"

section "a request read from bytes is decided about first"
# Five commands change what the session may do or whom it acts as: a permission
# mode, the sandbox, and an account signed in or out. The application performs
# them for whoever hands them in, which is safe while every request is built on
# the host by the front end standing there. `Request::decode` is where one could
# come from somewhere else, and nothing that ships calls it. A file that starts
# to is written down here with what it does about each of the five, so that
# reading requests from outside is a decision taken command by command and not
# a line added. A row is `file command what-is-done-about-it`.
request_owner=crates/crucible-client-api/src
authority='cycle_mode
login
logout
sandbox
set_mode'
decided=''
kinds=$(sed -n '/pub const KINDS: \[/,/\];/p' "$request_owner/command.rs" | grep -oE '"[a-z_]+"' | tr -d '"' | sort)
# The five were picked out of the eighteen commands there were. One more is one
# nobody has asked this of.
if (($(grep -c . <<<"$kinds") != 18)); then
    printf '    FAIL the client contract no longer has the 18 commands the five were picked out of; decide whether the new one changes what a session may do, then move the 18 in this check\n'
    failed=1
fi
while IFS= read -r word; do
    if ! grep -Fxq "$word" <<<"$kinds"; then
        printf '    FAIL %s is not a command of the client contract; this check measured nothing\n' "$word"
        failed=1
    fi
done <<<"$authority"
if ! grep -qE 'pub fn decode\(bytes: &\[u8\]\) -> Result<Self, Refused>' "$request_owner/request.rs"; then
    printf '    FAIL %s/request.rs no longer reads a request with Request::decode; this check measured nothing\n' "$request_owner"
    failed=1
fi
reading=$(grep -rlE --include='*.rs' 'Request::decode' src crates tests 2>/dev/null | grep -v "^$request_owner/" | sort || true)
if ! grep -qE '(^|/)tests(/|\.rs$)|_tests\.rs$' <<<"$reading"; then
    printf '    FAIL no test was found reading a request with Request::decode; this check measured nothing\n'
    failed=1
fi
reading=$(grep -vE '(^|/)tests(/|\.rs$)|_tests\.rs$' <<<"$reading" || true)
while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    while IFS= read -r word; do
        # A row is looked up as the words it is, never as a pattern: a dot in a
        # file name would otherwise stand for any character.
        if ! cut -d' ' -f1,2 <<<"$decided" | grep -Fxq "$file $word"; then
            printf '    FAIL %s reads a request from bytes, and nothing is written here about what it does with %s\n' "$file" "$word"
            failed=1
        fi
    done <<<"$authority"
done <<<"$reading"
while IFS= read -r row; do
    [[ -z "$row" ]] && continue
    file=${row%% *}
    rest=${row#* }
    if ! grep -Fxq "$file" <<<"$reading"; then
        printf '    FAIL %s no longer reads a request from bytes; take its rows out so they stay out\n' "$file"
        failed=1
    elif ! grep -Fxq "${rest%% *}" <<<"$authority"; then
        printf '    FAIL %s is written down about %s, which is not one of the commands this asks about\n' "$file" "${rest%% *}"
        failed=1
    elif [[ "$rest" != *' '* || -z "${rest#* }" ]]; then
        printf '    FAIL %s is written down about %s with nothing said about what it does\n' "$file" "${rest%% *}"
        failed=1
    fi
done <<<"$decided"
# Reading bytes is one way a request arrives from somewhere else. The other is
# an adapter that reads a format of its own and builds the command: the variants
# are public, and no `decode` is named on that road. So the files that name one
# of the five are written down too. They are the terminal, which builds them
# from keys pressed on the host, and the application, which performs them. A
# file that joins them is one more place a session's mode, sandbox or account
# can be changed from, and it is added here by somebody who looked at where its
# commands come from.
naming='crates/crucible-app/src/client/performing.rs
crates/crucible-app/src/client/turning.rs
src/cli/converse.rs
src/cli/converse/command.rs
src/cli/converse/command/login.rs
src/cli/converse/command/logout.rs
src/cli/converse/command/sandbox.rs
src/cli/converse/typing.rs'
variants=''
while IFS= read -r word; do
    variant=$(awk -F_ '{ for (i = 1; i <= NF; i++) printf "%s%s", toupper(substr($i, 1, 1)), substr($i, 2) }' <<<"$word")
    # The variant is found by the word it is written as, so the two lists
    # cannot drift apart without this saying so.
    if ! grep -qE "Self::$variant( \{ \.\. \}|\(_\))? => \"$word\"" "$request_owner/command.rs"; then
        printf '    FAIL no variant %s is written as %s in %s/command.rs; this check measured nothing\n' "$variant" "$word" "$request_owner"
        failed=1
    fi
    variants+="${variants:+|}$variant"
done <<<"$authority"
named=$(grep -rlE --include='*.rs' "(^|[^A-Za-z0-9_])Command::($variants)([^A-Za-z0-9_]|\$)" src crates 2>/dev/null |
    grep -v "^$request_owner/" | grep -vE '(^|/)tests(/|\.rs$)|_tests\.rs$' |
    while IFS= read -r file; do
        # Other enums are called `Command` too; the contract's is the one a
        # file cannot reach without naming the crate.
        if grep -q 'crucible_client_api' "$file"; then printf '%s\n' "$file"; fi
    done | sort || true)
while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    if ! grep -Fxq "$file" <<<"$naming"; then
        printf '    FAIL %s names a command that changes what a session may do, and is not one of the files known to; find where its commands come from, decide about each of the five if that is outside the host, then add the file here\n' "$file"
        failed=1
    fi
done <<<"$named"
while IFS= read -r file; do
    if ! grep -Fxq "$file" <<<"$named"; then
        printf '    FAIL %s was not found naming one of the five; take it out, or the search measured nothing\n' "$file"
        failed=1
    fi
done <<<"$naming"

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
