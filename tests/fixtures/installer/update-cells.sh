#!/usr/bin/env bash
# The cells that stage `crucible update` on a platform install.sh installs.
#
# A release source on loopback, release-source.py, names NEWER as the newest
# release and serves NEWER's archive and SHA256SUMS where the command asks a
# release for them. Every crucible below is pointed at it through
# CRUCIBLE_CODE_UPDATE_SOURCE, and three cells run against it:
#
#   refused  OLDER's executable, copied into a directory with no receipt, is
#            refused by `crucible update`. The refusal names install.sh, no
#            file in the directory changes, and the source hears nothing.
#   applied  install.sh installs OLDER, and `crucible update` puts NEWER in its
#            place. The source hears the three requests an update makes, both
#            names then say NEWER, the receipt names NEWER, and one sandboxed
#            command runs.
#   over     PRIOR's own install.sh makes a managed install of the published
#            PRIOR, and the staged install.sh installs OLDER over it. Both names
#            then say OLDER, the receipt names OLDER, and one sandboxed command
#            runs.
#
# Each cell has a directory of its own under SCRATCH, with a home and a
# workspace beside the install rather than inside it, since the sandbox
# trusts its broker only below directories no group can write. The sandboxed
# command is asked for by loopback-model.py, so nothing reaches a vendor. A
# BACKEND of `none` is a platform with no sandbox, and there the command is
# left out. Every version comes from the flags, so no release is named here.
set -euo pipefail

usage() {
    echo 'usage: update-cells.sh --stem STEM --backend BACKEND|none --installer FILE' >&2
    echo '         --older VERSION --older-archive FILE --older-sums FILE' >&2
    echo '         --newer VERSION --newer-dir DIR --prior VERSION --prior-dir DIR --scratch DIR' >&2
    exit 2
}

while (($#)); do
    [[ $# -ge 2 ]] || usage
    case $1 in
        --stem) stem=$2 ;;
        --backend) backend=$2 ;;
        --installer) installer=$2 ;;
        --older) older=$2 ;;
        --older-archive) older_archive=$2 ;;
        --older-sums) older_sums=$2 ;;
        --newer) newer=$2 ;;
        --newer-dir) newer_dir=$2 ;;
        --prior) prior=$2 ;;
        --prior-dir) prior_dir=$2 ;;
        --scratch) scratch=$2 ;;
        *) usage ;;
    esac
    shift 2
done
for given in stem backend installer older older_archive older_sums newer newer_dir prior prior_dir scratch; do
    [[ -n ${!given:-} ]] || usage
done

here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$scratch"
scratch=$(cd "$scratch" && pwd)
started=()
trap 'for pid in ${started[@]+"${started[@]}"}; do kill "$pid" 2>/dev/null || true; done' EXIT

fail() {
    echo "::error::$*"
    exit 1
}

digest() {
    if command -v sha256sum >/dev/null; then
        sha256sum -- "$1"
    elif command -v shasum >/dev/null; then
        shasum -a 256 -- "$1"
    else
        sha256 -r -- "$1"
    fi
}

# Every path under DIR, with each file's digest and each link's target, so
# two listings are equal only when nothing in DIR changed.
listing() {
    (
        cd "$1"
        find . -print | LC_ALL=C sort | while IFS= read -r path; do
            if [[ -L $path ]]; then
                printf '%s -> %s\n' "$path" "$(readlink -- "$path")"
            elif [[ -f $path ]]; then
                digest "$path"
            else
                printf '%s\n' "$path"
            fi
        done
    )
}

# Starts the loopback server SCRIPT with ARGS, and waits for the port it
# writes to PORT_FILE.
serve() {
    local port_file=$1
    shift
    python3 -I "$@" &
    started+=("$!")
    for _ in $(seq 30); do
        [[ -s $port_file ]] && return 0
        kill -0 "$!" 2>/dev/null || fail "the loopback server $1 exited before it listened"
        sleep 1
    done
    fail "the loopback server $1 never started"
}

# The version the active release's receipt in BIN names.
receipt_version() {
    sed -n 's/^version=//p' "$1/.crucible-install/current/receipt"
}

# Both names in BIN say they are VERSION.
says() {
    local bin=$1 version=$2 name said
    for name in crucible cru; do
        said=$("$bin/$name" --version)
        [[ $said == "crucible $version" ]] || fail "$name --version said '$said', expected 'crucible $version'"
    done
}

# One command, run through the sandbox by the crucible in BIN, from a
# workspace and home under ROOT.
sandboxed() {
    local bin=$1 root=$2 pid watchdog status=0
    [[ $backend != none ]] || return 0
    mkdir -p "$root/model" "$root/session-home/.crucible" "$root/ws"
    serve "$root/model/port" "$here/loopback-model.py" "$root/model/port" "$root/model/result"
    printf '{"provider":"anthropic","providers":{"anthropic":{"baseUrl":"http://127.0.0.1:%s/v1/messages","model":"claude-staging"}},"sandbox":{"enabled":true},"permissions":{"allow":["bash(echo staged-sandbox-ok)"]},"updates":{"check":"never"}}\n' \
        "$(<"$root/model/port")" >"$root/session-home/.crucible/config.json"
    printf 'run it\n' >"$root/model/prompt"
    (
        cd "$root/ws"
        HOME=$root/session-home CRUCIBLE_CODE_HOME=$root/session-home/.crucible ANTHROPIC_API_KEY=loopback-only \
            exec "$bin/crucible" <"$root/model/prompt" >"$root/model/session" 2>&1
    ) &
    pid=$!
    # Bounded: a session that never ends is the failure, and waiting on it
    # longer than five minutes would only hide that.
    (sleep 300 && kill "$pid" 2>/dev/null) &
    watchdog=$!
    wait "$pid" || status=$?
    kill "$watchdog" 2>/dev/null || true
    if [[ $status != 0 || ! -s $root/model/result ]]; then
        cat "$root/model/session"
        fail "crucible exited $status, and no result of its sandboxed command came back"
    fi
    python3 -I - "$root/model/result" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
print(json.dumps(result))
if result.get("content") != "staged-sandbox-ok" or result.get("is_error"):
    print("::error::the sandboxed command did not run")
    sys.exit(1)
PY
}

# A fresh directory for one cell, with a home whose configuration checks for
# no release by itself.
cell() {
    local root
    root=$(mktemp -d "$scratch/$1.XXXXXX")
    mkdir -p "$root/home/.crucible"
    printf '%s\n' '{"sandbox":{"enabled":true},"updates":{"check":"never"}}' >"$root/home/.crucible/config.json"
    printf '%s\n' "$root"
}

# What `crucible update ARGS` does with the crucible in BIN, from ROOT's home,
# pointed at the release source: its status in `status`, and what it wrote in
# ROOT/out and ROOT/err.
update() {
    local root=$1 bin=$2
    shift 2
    status=0
    (
        cd "$root"
        HOME=$root/home CRUCIBLE_CODE_HOME=$root/home/.crucible CRUCIBLE_CODE_UPDATE_SOURCE=$release_source \
            exec "$bin/crucible" update "$@" </dev/null >"$root/out" 2>"$root/err"
    ) || status=$?
    printf 'crucible update %s exited %s\n  stdout: %s\n  stderr: %s\n' "$*" "$status" "$(<"$root/out")" "$(<"$root/err")"
}

[[ -f $newer_dir/crucible-$newer-$stem.tar.gz && -f $newer_dir/SHA256SUMS ]] ||
    fail "$newer_dir does not hold $newer's archive for $stem and its SHA256SUMS"
mkdir -p "$scratch/source"
serve "$scratch/source/port" "$here/release-source.py" "$scratch/source/port" "$scratch/source/heard" "$newer" "$newer_dir"
release_source=http://127.0.0.1:$(<"$scratch/source/port")
heard=$scratch/source/heard

echo "::group::refused: $older copied where no install.sh put it"
root=$(cell refused)
mkdir -p "$root/copied" "$root/unpacked"
tar -xzf "$older_archive" -C "$root/unpacked"
cp "$root/unpacked/crucible-$older-$stem/crucible" "$root/copied/crucible"
before=$(listing "$root/copied")
update "$root" "$root/copied"
[[ $status == 1 ]] || fail "crucible update exited $status in a directory with no receipt, expected 1"
grep -q 'install\.sh' "$root/err" || fail "the refusal does not name install.sh"
[[ $(listing "$root/copied") == "$before" ]] || fail "the refused update changed a file"
[[ ! -s $heard ]] || fail "the release source heard $(<"$heard") before an update that was refused"
echo "::endgroup::"

echo "::group::applied: $older installed by install.sh, updated to $newer"
root=$(cell applied)
bin=$root/bin
bash "$installer" --dir "$bin" --version "$older" --archive "$older_archive" --checksums "$older_sums"
says "$bin" "$older"
update "$root" "$bin"
[[ $status == 0 ]] || fail "crucible update exited $status, expected 0"
[[ $(<"$root/out") == "crucible $newer is installed and active in place of $older" ]] ||
    fail "crucible update said '$(<"$root/out")'"
expected=$(printf 'GET %s HTTP/1.1\n' /releases/latest "/releases/download/v$newer/SHA256SUMS" \
    "/releases/download/v$newer/crucible-$newer-$stem.tar.gz")
[[ $(<"$heard") == "$expected" ]] || fail "the release source heard '$(<"$heard")', expected '$expected'"
says "$bin" "$newer"
[[ $(receipt_version "$bin") == "$newer" ]] || fail "the active receipt names $(receipt_version "$bin"), not $newer"
sandboxed "$bin" "$root"
echo "::endgroup::"

echo "::group::over: $older installed over a managed install of the published $prior"
root=$(cell over)
bin=$root/bin
bash "$prior_dir/install.sh" --dir "$bin" --version "$prior" \
    --archive "$prior_dir/crucible-$prior-$stem.tar.gz" --checksums "$prior_dir/SHA256SUMS"
[[ -L $bin/crucible && $(receipt_version "$bin") == "$prior" ]] ||
    fail "$prior's install.sh left no managed install of $prior"
says "$bin" "$prior"
bash "$installer" --dir "$bin" --version "$older" --archive "$older_archive" --checksums "$older_sums"
says "$bin" "$older"
[[ $(receipt_version "$bin") == "$older" ]] || fail "the active receipt names $(receipt_version "$bin"), not $older"
sandboxed "$bin" "$root"
echo "::endgroup::"

echo "update cells passed for $stem: refused, applied $older to $newer, and $older over $prior"
