#!/usr/bin/env bash
# Kill-point probes for the shell installer's migration of a flat install.
#
# Usage: tests/fixtures/installer/migration-crash-probes.sh INSTALLER FLAT_INSTALLER
#
# Every probe starts from the flat install FLAT_INSTALLER makes, one of the
# installers of 0.43.0 to 0.45.3 kept beside this script, and runs INSTALLER
# over it with `kill-point` standing in for each command that changes the
# filesystem. The migration is first run whole to number its calls, then once
# for every call, killed just before it and again just after it returns. After
# each kill, `<dir>/crucible` must still run one complete release beside its
# own broker, the flat one or the one being installed, and the flat install's
# broker must still be the one it shipped with, since a crucible started from
# the flat install finds its broker there. The next install must then finish
# the migration.
#
# The probe prints `ok` or every kill point it failed at, and the script exits
# 1 when any failed. Archives, installs and whatever a killed install leaves
# behind stay in one temporary directory, removed on exit.
set -euo pipefail

(($# == 2)) || {
    echo 'usage: migration-crash-probes.sh INSTALLER FLAT_INSTALLER' >&2
    exit 2
}
fixtures=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
installer=$(cd -P -- "$(dirname -- "$1")" && pwd -P)/${1##*/}
flat_installer=$(cd -P -- "$(dirname -- "$2")" && pwd -P)/${2##*/}
readonly fixtures installer flat_installer
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

readonly before=0.45.3 installing=9.8.7
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
for version in "$before" "$installing"; do
    release "$version"
done

# Sets `args` to an installer's arguments for one release.
arguments() {
    local stem=crucible-$1-$platform-$architecture
    args=(--version "$1" --dir "$dest"
        --archive "$scratch/assets/$1/$stem.tar.gz" --checksums "$scratch/assets/$1/SHA256SUMS")
}

# plain VERSION OUTPUT: an install with nothing standing in for anything.
plain() {
    arguments "$1"
    "$installer" "${args[@]}" </dev/null >"$2" 2>&1
}

# probed VERSION AT ACTION OUTPUT: an install with `kill-point` first on PATH,
# started as crash-probes.sh starts one.
probed() {
    arguments "$1"
    rm -rf -- "$probe"
    mkdir -- "$probe"
    (
        KILL_POINT_DIR=$probe KILL_POINT_PATH=$PATH KILL_POINT_AT=$2 KILL_POINT_ACTION=$3 \
            PATH=$shims:$PATH bash -c 'echo "$$" >"$KILL_POINT_DIR/pid"; exec "$@"' \
            bash "$installer" "${args[@]}" </dev/null >"$4" 2>&1
        status=$?
        exit "$status"
    ) 2>/dev/null
}

# A flat install of the release before, made by the installer that shipped it.
fresh() {
    rm -rf -- "$dest"
    arguments "$before"
    "$flat_installer" "${args[@]}" </dev/null >"$scratch/before.out" 2>&1 || {
        cat "$scratch/before.out" >&2
        echo 'crash probes: the flat install could not be made' >&2
        exit 1
    }
    [[ -f $dest/crucible && ! -L $dest/crucible && -f $dest/crucible-sandbox-broker ]] || {
        echo 'crash probes: the flat installer made no flat install' >&2
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

# Whether the flat install's broker is still the one it shipped with.
flat_broker_kept() {
    local said
    said=$(sed -n 2p "$dest/crucible-sandbox-broker" 2>/dev/null) || true
    [[ -f $dest/crucible-sandbox-broker && ! -L $dest/crucible-sandbox-broker &&
        $said == "# crucible $before" ]] || {
        problem="the flat install's broker is ${said:-gone}"
        return 1
    }
}

# Whether the migration is finished: `crucible` is this installer's link, its
# release is the one installed, and the receipt there describes it.
migrated() {
    [[ -L $dest/crucible && $(readlink -- "$dest/crucible") == .crucible-install/current/crucible ]] || {
        problem='crucible is not the link into .crucible-install'
        return 1
    }
    active_pair || return 1
    [[ $active == "$installing" ]] || {
        problem="crucible $active runs"
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
    left=$(find "$dest" -name '.*incoming*' -o -name '.*previous*' -o -name '.current.*' \
        -o -name '.crucible.link.*' | head -n 1)
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

echo '==> numbering the calls of a whole migration'
fresh
probed "$installing" 0 none "$scratch/whole.out" || {
    cat "$scratch/whole.out" >&2
    echo 'crash probes: the migration under kill-point did not finish' >&2
    exit 1
}
problem=
if ! migrated || ! flat_broker_kept || ! no_leftovers; then
    printf 'crash probes: the whole migration did not finish: %s\n' "$problem" >&2
    exit 1
fi
cp -- "$probe/calls" "$scratch/calls"
total=$(wc -l <"$scratch/calls")
total=$((total))
printf '    %s calls\n' "$total"

echo '==> every call of a migration, killed before it runs and as it returns'
intact=() kept=() recovered=()
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
        problem=
        if ! active_pair; then
            intact+=("$point: $problem")
        elif [[ $active != "$before" && $active != "$installing" ]]; then
            intact+=("$point: crucible $active runs")
        fi
        problem=
        flat_broker_kept || kept+=("$point: $problem")
        problem=
        if ! recover || ! migrated || ! flat_broker_kept || ! no_leftovers; then
            recovered+=("$point: $problem")
        fi
    done
done
report 'an interrupted migration leaves one whole release in use' ${intact[@]+"${intact[@]}"}
report "an interrupted migration keeps the flat install's broker" ${kept[@]+"${kept[@]}"}
report 'the next install finishes an interrupted migration' ${recovered[@]+"${recovered[@]}"}

((failures == 0))
