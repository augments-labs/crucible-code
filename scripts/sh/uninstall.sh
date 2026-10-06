#!/usr/bin/env bash
# Removes the installed executable and only state the user explicitly purges.
set -euo pipefail

destination=${CRUCIBLE_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}
dry_run=0
purge=0
confirmed=0

usage() {
    cat <<'USAGE'
Usage: scripts/sh/uninstall.sh [--dir DIRECTORY] [--dry-run] [--purge --yes]

Removes `crucible`, its sandbox broker and its owned `cru` alias.
Configuration, credentials and sessions are preserved unless both --purge and
--yes are supplied.
USAGE
}

while (($#)); do
    case "$1" in
    --dir) destination=${2:?--dir needs a value}; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    --purge) purge=1; shift ;;
    --yes) confirmed=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) printf 'uninstall: unknown argument %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ -n $destination ]] || {
    echo 'uninstall: the installation directory is unsafe' >&2
    exit 2
}
case "/$destination/" in
*/../*)
    echo 'uninstall: the installation directory is unsafe' >&2
    exit 2
    ;;
esac
if ((purge && !confirmed)); then
    echo 'uninstall: --purge permanently deletes data and requires --yes' >&2
    exit 2
fi

# Every destructive target is resolved before any of them is touched. An
# unsafe purge request must not remove the executable and only then report that
# it refused the data directory.
if [[ -d $destination ]]; then
    destination=$(cd -- "$destination" && pwd -P)
    [[ $destination != / ]] || {
        echo 'uninstall: the installation directory resolves to root' >&2
        exit 2
    }
fi
data_home=${CRUCIBLE_CODE_HOME:-${HOME:?HOME is not set}/.crucible}
purge_target=
if ((purge)); then
    [[ -n $data_home && $data_home == /* && ! -L $data_home ]] || {
        printf 'uninstall: refusing unsafe data directory %q\n' "$data_home" >&2
        exit 2
    }
    if [[ -e $data_home ]]; then
        [[ -d $data_home ]] || {
            printf 'uninstall: refusing non-directory data path %q\n' "$data_home" >&2
            exit 2
        }
        purge_target=$(cd -- "$data_home" && pwd -P)
        user_home=$(cd -- "${HOME:?HOME is not set}" && pwd -P)
        [[ $purge_target != / && $purge_target != "$user_home" ]] || {
            printf 'uninstall: refusing unsafe data directory %q\n' "$purge_target" >&2
            exit 2
        }
    fi
fi

# A removal in a terminal is shown as the installer shows its steps: a mark per
# step, in colour. A dry run, a pipe, `NO_COLOR` or `TERM=dumb` keeps the plain
# lines it has always printed.
fancy=0
if ((!dry_run)) && [[ -t 1 && -t 2 && -z ${NO_COLOR:-} && ${TERM:-} != dumb ]]; then
    fancy=1
fi
case ${LC_ALL:-${LC_CTYPE:-${LANG:-}}} in
*[Uu][Tt][Ff]-8*|*[Uu][Tt][Ff]8*) done_mark='✓' dot='·' ;;
*) done_mark=ok dot=- ;;
esac
readonly bold=$'\033[1m' dim=$'\033[2m' green=$'\033[32m' plain=$'\033[0m'

shown() {
    local path=$1
    if [[ -n ${HOME:-} && $HOME != / && $path == "$HOME"/* ]]; then
        printf '~/%s' "${path#"$HOME"/}"
    else
        printf '%s' "$path"
    fi
}

columns=80
if ((fancy)); then
    # Errors are silenced first: a session with no controlling terminal
    # cannot open /dev/tty, and the shell itself would say so.
    columns=$(stty size 2>/dev/null </dev/tty | awk '{ print $2 }') || true
    [[ $columns =~ ^[0-9]+$ ]] && ((columns >= 20)) || columns=80
fi

# The detail of a step starts here, after the indent, the mark and the label.
detail_column=$((2 + ${#done_mark} + 1 + 20))
# Decided once, before the first row, from every detail the run draws: when one
# has no room beside its label, every detail goes under its step, so the list
# reads as one column, as the installer's does.
narrow=0

# A step's mark and label, then its detail beside the label, or under it.
row() {
    printf '  %s%s%s %s' "$green" "$done_mark" "$plain" "$1"
    if ((!narrow)); then
        printf '%*s%s%s%s\n' $((20 - ${#1})) '' "$dim" "$2" "$plain"
    else
        printf '\n    %s%s%s\n' "$dim" "$2" "$plain"
    fi
}

binary=$destination/crucible
broker=$destination/crucible-sandbox-broker
alias_path=$destination/cru
if [[ -e $binary || -L $binary ]]; then
    [[ -f $binary && ! -L $binary ]] || {
        printf 'uninstall: refusing to remove non-regular %s\n' "$binary" >&2
        exit 1
    }
fi
if [[ -e $broker || -L $broker ]]; then
    [[ -f $broker && ! -L $broker ]] || {
        printf 'uninstall: refusing to remove non-regular %s\n' "$broker" >&2
        exit 1
    }
fi
((!fancy)) || printf '\n%scrucible%s %s uninstall\n\n' "$bold" "$plain" "$dot"
gone_alias=0 gone_binary=0 gone_broker=0
if [[ -e $alias_path || -L $alias_path ]]; then
    if [[ ! -L $alias_path || $(readlink "$alias_path") != crucible ]]; then
        printf 'uninstall: preserving unrelated %s\n' "$alias_path" >&2
    elif ((dry_run)); then
        printf 'Would remove %s\n' "$alias_path"
    else
        rm -f -- "$alias_path"
        gone_alias=1
    fi
fi
if [[ -e $binary ]]; then
    if ((dry_run)); then
        printf 'Would remove %s\n' "$binary"
    else
        rm -f -- "$binary"
        gone_binary=1
    fi
fi
if [[ -e $broker ]]; then
    if ((dry_run)); then
        printf 'Would remove %s\n' "$broker"
    else
        rm -f -- "$broker"
        gone_broker=1
    fi
fi
if ((fancy)); then
    names=()
    ((!gone_binary)) || names+=(crucible)
    ((!gone_broker)) || names+=(crucible-sandbox-broker)
    ((!gone_alias)) || names+=(cru)
    case ${#names[@]} in
    0) what="nothing in $(shown "$destination")" ;;
    1) what=${names[0]} ;;
    2) what="${names[0]} and ${names[1]}" ;;
    *) what="${names[0]}, ${names[1]} and ${names[2]}" ;;
    esac
    # The second row names the data directory: purged when it existed, or kept.
    data_detail=
    if ((!purge)); then
        data_detail=$(shown "$data_home")
    elif [[ -n $purge_target ]]; then
        data_detail=$(shown "$purge_target")
    fi
    for detail in "$what" "$data_detail"; do
        ((detail_column + ${#detail} <= columns)) || narrow=1
    done
    row remove "$what"
fi

if ((purge)); then
    if [[ -n $purge_target ]]; then
        if ((dry_run)); then
            printf 'Would permanently remove %s\n' "$purge_target"
        else
            rm -rf -- "$purge_target"
            ((!fancy)) || row purge "$(shown "$purge_target")"
        fi
    fi
elif ((fancy)); then
    row keep "$(shown "$data_home")"
else
    printf 'Preserved configuration, credentials and sessions in %s\n' "$data_home"
fi

((!fancy)) || printf '\n'
echo 'crucible is uninstalled.'
