#!/usr/bin/env bash
# Cross-file repository policy. Language compilation and tests belong in their
# language gates; this file may inspect any language where repository structure
# spans files or ecosystems.
set -uo pipefail
shopt -s nullglob

cd "$(dirname "$0")/.."

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
    printf '    FAIL no AGENTS.md anywhere; harnesses reading that name lose the guide\n'
    failed=1
fi

if [[ ! -L .agents/rules ]]; then
    printf '    FAIL .agents/rules must be a symlink to ../.claude/rules\n'
    failed=1
elif [[ "$(readlink .agents/rules)" != "../.claude/rules" ]]; then
    printf '    FAIL .agents/rules points at %s, expected ../.claude/rules\n' "$(readlink .agents/rules)"
    failed=1
fi

rules=(.claude/rules/*.md)
if ((${#rules[@]} == 0)); then
    printf '    FAIL .claude/rules/ holds no always-on rule\n'
    failed=1
fi

skills=(.claude/skills/*/)
if ((${#skills[@]} == 0)); then
    printf '    FAIL .claude/skills/ holds no skill\n'
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
for entry in .agents/skills/*; do
    [[ -e "$entry" || -L "$entry" ]] || continue
    if [[ ! -L "$entry" ]]; then
        printf '    FAIL %s: real file or directory, expected a symlink into .claude/skills/\n' "$entry"
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

# Both spellings, and the tests with the source. A call written
# `ToolOutput::replayed(output, ..)` is the same call as `output.replayed(..)`,
# and a pin that only knew the dot form would be a pin anyone could walk past
# without meaning to. Occurrences rather than lines, because two calls on one
# line are two calls.
doors() {
    grep -rEoh --include='*.rs' "$1" "${@:2}" | wc -l
}

section "the replay seam"
replay="crates/crucible-session/src/session/wire.rs"
opens='(\.|ToolOutput::)replayed\('
elsewhere=$(grep -rlE --include='*.rs' "$opens" crates src tests | grep -Fxv "$replay" || true)
if [[ -n "$elsewhere" ]]; then
    while IFS= read -r file; do
        printf '    FAIL %s calls ToolOutput::replayed; only %s may\n' "$file" "$replay"
    done <<<"$elsewhere"
    failed=1
fi
here=$(doors "$opens" "$replay")
if ((here != 1)); then
    printf '    FAIL %s calls ToolOutput::replayed %d times; the replay is one call\n' "$replay" "$here"
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

member_manifests=(crates/*/Cargo.toml)
manifests=(Cargo.toml "${member_manifests[@]}")

section "crate layering"
if ((${#manifests[@]} < 2)); then
    printf '    FAIL no manifest under crates/; the dependency graph measured nothing\n'
    failed=1
fi
edges=$(awk '
    FNR == 1 {
        crate = FILENAME
        sub(/^crates\//, "", crate)
        sub(/\/Cargo.toml$/, "", crate)
        if (FILENAME == "Cargo.toml") crate = "crucible-code"
        table = 0
    }
    /^[[:space:]]*\[/ {
        header = $0
        sub(/^[[:space:]]*\[+[[:space:]]*/, "", header)
        sub(/[[:space:]]*\]+.*$/, "", header)
        table = (header ~ /(^|\.)(dependencies|dev-dependencies|build-dependencies)$/)
        next
    }
    table && /^[[:space:]]*crucible-[a-z0-9-]+[[:space:].]*=/ {
        dependency = $0
        sub(/^[[:space:]]*/, "", dependency)
        sub(/[[:space:].]*=.*/, "", dependency)
        sub(/^crucible-/, "", dependency)
        print crate " " dependency
    }
' "${manifests[@]}")
if [[ -z "$edges" ]]; then
    printf '    FAIL no internal dependency edges found; this check measured nothing\n'
    failed=1
fi

allowed='crucible-code auth
crucible-code config
crucible-code core
crucible-code extension
crucible-code mcp
crucible-code privacy
crucible-code provider
crucible-code runner
crucible-code session
crucible-code tools
crucible-code sandbox-broker
crucible-code tui
auth core
auth privacy
config core
extension core
mcp core
provider core
runner core
runner session
session core
session privacy
tools core
tools sandbox-broker'
while IFS= read -r edge; do
    [[ -z "$edge" ]] && continue
    if ! grep -Fxq "$edge" <<<"$allowed"; then
        printf '    FAIL dependency edge %s is outside the workspace layering\n' "$edge"
        failed=1
    fi
done <<<"$edges"
for crate in core privacy sandbox-broker tui; do
    if grep -qE "^$crate " <<<"$edges"; then
        printf '    FAIL crucible-%s must not depend on another workspace crate\n' "$crate"
        failed=1
    fi
done

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
