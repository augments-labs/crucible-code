#!/usr/bin/env bash
# Kill-point probes for the shell installer's staging, receipt and activation.
#
# Usage: tests/fixtures/installer/crash-probes.sh INSTALLER
#
# Every probe starts from one installed release and runs INSTALLER to install
# the next with `kill-point` standing in for each command that changes the
# filesystem. An install is first run whole to number its calls, then once for
# every call, killed just before it and again just after it returns. After each
# kill, `<dir>/crucible` must still be one complete release, the one before or
# the one being installed, with a receipt that describes it, and the next
# install must finish the job. Two more probes kill an install that holds the
# lock and run two installs at once.
#
# Each probe prints `ok` or every kill point it failed at, and the script exits
# 1 when any failed. Archives, installs and whatever a killed install leaves
# behind stay in one temporary directory, removed on exit.
set -euo pipefail

(($# == 1)) || {
    echo 'usage: crash-probes.sh INSTALLER' >&2
    exit 2
}
fixtures=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
installer=$(cd -P -- "$(dirname -- "$1")" && pwd -P)/${1##*/}
readonly fixtures installer
# shellcheck source=receipt.sh
. "$fixtures/receipt.sh"

case $(uname -s) in
Linux) platform=linux ;;
Darwin) platform=macos ;;
FreeBSD) platform=freebsd ;;
*) echo 'crash probes require Linux, macOS, or FreeBSD'; exit 0 ;;
esac
case $(uname -m) in
x86_64 | amd64) architecture=x86_64 ;;
aarch64 | arm64) architecture=aarch64 ;;
*) echo 'crash probes require x86-64 or ARM64'; exit 0 ;;
esac
if [[ $platform == freebsd && $architecture != x86_64 ]]; then
    echo 'crash probes skip an unpublished platform'
    exit 0
fi

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
scratch=$(cd -P -- "$scratch" && pwd -P)
readonly scratch
# A killed install leaves its work directory behind, so it is made in here.
export TMPDIR=$scratch/tmp
mkdir -- "$TMPDIR"

readonly before=9.8.6 installing=9.8.7 later=9.8.8
readonly dest=$scratch/bin probe=$scratch/probe shims=$scratch/shims
mkdir -- "$shims"
for command in chmod cp install ln mkdir mktemp mv rm sync touch; do
    ln -s -- "$fixtures/kill-point" "$shims/$command"
done

checksum() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{ print $1 }'
    elif command -v shasum >/dev/null; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    else
        sha256 -q "$1"
    fi
}

# A release archive and its SHA256SUMS. Each executable names its release, so
# an install that mixes two releases shows it.
release() {
    local version=$1 at=$scratch/assets/$1 stem=crucible-$1-$platform-$architecture
    mkdir -p -- "$at/$stem"
    printf '#!/bin/sh\necho "crucible %s"\n' "$version" >"$at/$stem/crucible"
    printf '#!/bin/sh\n# crucible %s\nexit 125\n' "$version" >"$at/$stem/crucible-sandbox-broker"
    chmod 755 "$at/$stem/crucible" "$at/$stem/crucible-sandbox-broker"
    tar -czf "$at/$stem.tar.gz" -C "$at" "$stem"
    (cd "$at" && printf '%s  %s\n' "$(checksum "$stem.tar.gz")" "$stem.tar.gz") >"$at/SHA256SUMS"
}
for version in "$before" "$installing" "$later"; do
    release "$version"
done

# Sets `args` to the installer's arguments for one release.
arguments() {
    local stem=crucible-$1-$platform-$architecture
    args=(--version "$1" --dir "$dest"
        --archive "$scratch/assets/$1/$stem.tar.gz" --checksums "$scratch/assets/$1/SHA256SUMS")
}

# plain VERSION OUTPUT: an install with nothing standing in for anything.
plain() {
    arguments "$1"
    "$installer" "${args[@]}" >"$2" 2>&1
}

# probed VERSION AT ACTION OUTPUT: an install with `kill-point` first on PATH.
# The pid is written before the installer is started, by the process that then
# becomes it, so a kill-point always knows whom to kill. It runs in a subshell
# that waits for it, and whose own standard error is dropped, since that is
# where bash reports a job it saw killed.
probed() {
    arguments "$1"
    rm -rf -- "$probe"
    mkdir -- "$probe"
    (
        KILL_POINT_DIR=$probe KILL_POINT_PATH=$PATH KILL_POINT_AT=$2 KILL_POINT_ACTION=$3 \
            PATH=$shims:$PATH bash -c 'echo "$$" >"$KILL_POINT_DIR/pid"; exec "$@"' \
            bash "$installer" "${args[@]}" >"$4" 2>&1
        status=$?
        exit "$status"
    ) 2>/dev/null
}

fresh() {
    rm -rf -- "$dest"
    plain "$before" "$scratch/before.out" || {
        cat "$scratch/before.out" >&2
        echo 'crash probes: the release before could not be installed' >&2
        exit 1
    }
}

# The file a path names once every link on the way is followed.
resolve() {
    local path=$1 target hops=0
    while [[ -L $path ]] && ((hops++ < 40)); do
        target=$(readlink -- "$path")
        case $target in
        /*) path=$target ;;
        *) path=${path%/*}/$target ;;
        esac
    done
    printf '%s\n' "$path"
}

# Sets `active` to the release `<dir>/crucible` runs and `unit` to the
# directory that executable is in, or `problem` to why the two executables
# there are not one release.
active_pair() {
    local said broker_said
    active= unit=
    said=$("$dest/crucible" --version 2>/dev/null) || {
        problem='crucible does not run'
        return 1
    }
    active=${said#crucible }
    unit=$(resolve "$dest/crucible")
    unit=${unit%/*}
    broker_said=$(sed -n 2p "$unit/crucible-sandbox-broker" 2>/dev/null) || true
    [[ $broker_said == "# crucible $active" ]] || {
        problem="crucible $active runs beside the broker of ${broker_said:-no release}"
        return 1
    }
}

# Whether the receipt in `unit` describes the release `active_pair` found.
active_receipt() {
    [[ -f $unit/receipt ]] || {
        problem="no receipt describes the crucible $active that runs"
        return 1
    }
    LC_ALL=C crucible_receipt_read "$unit/receipt" 2>"$scratch/refusal" || {
        problem="the receipt of crucible $active is $(<"$scratch/refusal")"
        return 1
    }
    if [[ $receipt_version != "$active" ||
        $receipt_prefix != "$dest/.crucible-install" ||
        $receipt_crucible != "$(checksum "$unit/crucible")" ||
        $receipt_broker != "$(checksum "$unit/crucible-sandbox-broker")" ]]; then
        problem="the receipt of crucible $active describes another release"
        return 1
    fi
}

# Whether an interrupted install left a staged copy or link anywhere.
no_leftovers() {
    local left
    left=$(find "$dest" -name '.*incoming*' -o -name '.*previous*' -o -name '.current.*' | head -n 1)
    [[ -z $left ]] || {
        problem="${left#"$dest/"} is still there"
        return 1
    }
}

readonly lock=$dest/.crucible-install/lock
# The next install after a kill. A lock the killed install held must stop it,
# with a message that names the lock, and removing the lock lets it finish.
recover() {
    if plain "$installing" "$scratch/recover.out"; then
        return 0
    fi
    if [[ -L $lock ]] && grep -qF -- "$lock" "$scratch/recover.out"; then
        rm -f -- "$lock"
        plain "$installing" "$scratch/recover.out" && return 0
    fi
    problem="the next install failed: $(tr '\n' ' ' <"$scratch/recover.out")"
    return 1
}

failures=0
report() {
    local name=$1
    shift
    if (($# == 0)); then
        printf '    ok   %s\n' "$name"
        return
    fi
    failures=$((failures + 1))
    printf '    FAIL %s\n' "$name"
    printf '         %s\n' "$@"
}

echo '==> numbering the calls of a whole install'
fresh
probed "$installing" 0 none "$scratch/whole.out" || {
    cat "$scratch/whole.out" >&2
    echo 'crash probes: the install under kill-point did not finish' >&2
    exit 1
}
cp -- "$probe/calls" "$scratch/calls"
total=$(wc -l <"$scratch/calls")
total=$((total))
printf '    %s calls\n' "$total"

echo '==> every call, killed before it runs and as it returns'
intact=() described=() recovered=()
for ((at = 1; at <= total; at++)); do
    for action in kill-before kill-after; do
        fresh
        status=0
        probed "$installing" "$at" "$action" "$scratch/killed.out" || status=$?
        call=$(sed -n "${at}p" "$scratch/calls")
        point="${action#kill-} ${call#* }"
        point=${point:0:160}
        if ((status != 137)); then
            intact+=("$point: the install ran on to exit $status")
            continue
        fi
        if ! active_pair; then
            intact+=("$point: $problem")
        elif [[ $active != "$before" && $active != "$installing" ]]; then
            intact+=("$point: crucible $active runs")
        elif ! active_receipt; then
            described+=("$point: $problem")
        fi
        if ! recover || ! active_pair || [[ $active != "$installing" ]] ||
            ! active_receipt || ! no_leftovers; then
            recovered+=("$point: ${problem:-crucible $active runs}")
        fi
        problem=
    done
done
report 'interrupted staging leaves the previous activation intact' ${intact[@]+"${intact[@]}"}
report 'what runs after an interruption has a receipt that describes it' ${described[@]+"${described[@]}"}
report 'the next install finishes an interrupted one' ${recovered[@]+"${recovered[@]}"}

echo '==> an install killed while it holds the lock'
stale=()
fresh
taken=$(grep -n -m 1 -E '^[0-9]+ ln .*/\.crucible-install/lock$' "$scratch/calls" | cut -d: -f1) || true
if [[ -z $taken ]]; then
    stale+=('no install took the lock')
else
    status=0
    probed "$installing" "$taken" kill-after "$scratch/killed.out" || status=$?
    if ((status != 137)); then
        stale+=("the install ran on to exit $status")
    elif plain "$installing" "$scratch/stale.out"; then
        stale+=('the next install went ahead while the lock was held')
    elif ! grep -qF -- "$lock" "$scratch/stale.out"; then
        stale+=("the refusal does not name the lock: $(tr '\n' ' ' <"$scratch/stale.out")")
    elif ! active_pair || [[ $active != "$before" ]]; then
        stale+=("the refused install changed what runs: ${problem:-crucible $active runs}")
    else
        rm -f -- "$lock"
        if ! plain "$installing" "$scratch/stale.out" || ! active_pair ||
            [[ $active != "$installing" ]] || ! active_receipt; then
            stale+=("once the lock was removed the install did not finish: ${problem:-crucible $active runs}")
        fi
    fi
fi
report 'a lock left by a killed install refuses the next until it is removed' ${stale[@]+"${stale[@]}"}

echo '==> two installs at once'
serial=()
fresh
held=$(grep -n -m 1 -E "^[0-9]+ mktemp .*$dest/" "$scratch/calls" | cut -d: -f1) || true
if [[ -z $held ]]; then
    serial+=('no install made a file in the installation directory')
else
    probed "$installing" "$held" hold-after "$scratch/first.out" &
    first=$!
    for ((wait = 0; wait < 300; wait++)); do
        [[ ! -e $probe/held ]] || break
        sleep 0.1
    done
    if [[ ! -e $probe/held ]]; then
        serial+=('the first install never reached the installation directory')
    else
        plain "$later" "$scratch/second.out" &
        second=$!
        for ((wait = 0; wait < 20; wait++)); do
            kill -0 "$second" 2>/dev/null || break
            sleep 0.1
        done
        if ! kill -0 "$second" 2>/dev/null; then
            serial+=('the second install finished while the first was still installing')
        elif ! active_pair || [[ $active != "$before" ]]; then
            serial+=("the waiting install changed what runs: ${problem:-crucible $active runs}")
        fi
    fi
    : >"$probe/release"
    first_status=0 second_status=0
    wait "$first" || first_status=$?
    [[ -z ${second:-} ]] || wait "$second" || second_status=$?
    if ((${#serial[@]} == 0)); then
        if ((first_status != 0 || second_status != 0)); then
            serial+=("the installs ended $first_status and $second_status: $(tr '\n' ' ' <"$scratch/second.out")")
        elif ! active_pair || [[ $active != "$later" ]] || ! active_receipt; then
            serial+=("after both, ${problem:-crucible $active runs}")
        fi
    fi
fi
report 'concurrent installs serialize on the lock' ${serial[@]+"${serial[@]}"}

((failures == 0))
