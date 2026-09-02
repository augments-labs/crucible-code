#!/usr/bin/env bash
# Removes the installed executable and only state the user explicitly purges.
set -euo pipefail

destination=${CRUCIBLE_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}
dry_run=0
purge=0
confirmed=0

usage() {
    cat <<'USAGE'
Usage: scripts/uninstall.sh [--dir DIRECTORY] [--dry-run] [--purge --yes]

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
if [[ -e $alias_path || -L $alias_path ]]; then
    if [[ ! -L $alias_path || $(readlink "$alias_path") != crucible ]]; then
        printf 'uninstall: preserving unrelated %s\n' "$alias_path" >&2
    elif ((dry_run)); then
        printf 'Would remove %s\n' "$alias_path"
    else
        rm -f -- "$alias_path"
    fi
fi
if [[ -e $binary ]]; then
    if ((dry_run)); then
        printf 'Would remove %s\n' "$binary"
    else
        rm -f -- "$binary"
    fi
fi
if [[ -e $broker ]]; then
    if ((dry_run)); then
        printf 'Would remove %s\n' "$broker"
    else
        rm -f -- "$broker"
    fi
fi

if ((purge)); then
    if [[ -n $purge_target ]]; then
        if ((dry_run)); then
            printf 'Would permanently remove %s\n' "$purge_target"
        else
            rm -rf -- "$purge_target"
        fi
    fi
else
    printf 'Preserved configuration, credentials and sessions in %s\n' "$data_home"
fi

echo 'crucible is uninstalled.'
