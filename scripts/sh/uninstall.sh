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

Removes `crucible`, its sandbox broker and its owned `cru` alias. Where
install.sh made DIRECTORY/.crucible-install, each release there loses the
executables and the receipt its receipt describes, and anything else is kept
and named. Configuration, credentials and sessions are preserved unless both
--purge and --yes are supplied.
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

refuse() {
    printf 'uninstall: %s\n' "$1" >&2
    exit 1
}

# --- receipt reader: begin ---
# A copy of tests/fixtures/installer/receipt.sh, the reader crucible-update is
# held to; the installer tests fail when the two differ.
crucible_receipt_read() {
    receipt_file=$1
    receipt_installation=
    receipt_target=
    receipt_prefix=
    receipt_version=
    receipt_crucible=
    receipt_broker=
    receipt_line=0

    if ! receipt_size=$(wc -c <"$receipt_file"); then
        crucible_receipt_refuse 'the receipt could not be measured'
        return 1
    fi
    if [ "$((receipt_size))" -gt 8192 ]; then
        crucible_receipt_refuse 'the receipt is larger than 8192 bytes'
        return 1
    fi
    # A pipeline's status is its last command's, so a `tr` that fails adds a
    # byte of its own to what is counted rather than going unseen.
    if ! receipt_controls=$(
        { LC_ALL=C tr -d '\n\040-\176\200-\377' <"$receipt_file" || printf x; } | wc -c
    ); then
        crucible_receipt_refuse 'the receipt could not be checked for control characters'
        return 1
    fi
    if [ "$((receipt_controls))" -ne 0 ]; then
        crucible_receipt_refuse 'the receipt holds a control character or could not be read'
        return 1
    fi
    # The `x` keeps the newline from being stripped. A `tail` that fails
    # leaves it out, and the substitution's status then refuses.
    receipt_newline='
x'
    if [ "$((receipt_size))" -gt 0 ]; then
        if ! receipt_last=$(tail -c 1 <"$receipt_file" && printf x); then
            crucible_receipt_refuse 'the end of the receipt could not be read'
            return 1
        fi
        if [ "$receipt_last" != "$receipt_newline" ]; then
            crucible_receipt_refuse 'the last line of the receipt does not end'
            return 1
        fi
    fi

    while IFS= read -r receipt_text; do
        receipt_line=$((receipt_line + 1))
        case $receipt_line in
        1)
            case $receipt_text in
            'crucible-installer-receipt 1') ;;
            'crucible-installer-receipt '[1-9]*)
                case ${receipt_text#crucible-installer-receipt } in
                *[!0-9]*)
                    crucible_receipt_refuse 'this is not an installer receipt'
                    return 1
                    ;;
                esac
                crucible_receipt_refuse 'the receipt was written by a newer installer'
                return 1
                ;;
            *)
                crucible_receipt_refuse 'this is not an installer receipt'
                return 1
                ;;
            esac
            ;;
        2)
            crucible_receipt_value manager || return 1
            if [ "$receipt_value" != crucible-installer ]; then
                crucible_receipt_refuse 'manager is not crucible-installer'
                return 1
            fi
            ;;
        3)
            crucible_receipt_value installation || return 1
            crucible_receipt_hex 32 installation || return 1
            receipt_installation=$receipt_value
            ;;
        4)
            crucible_receipt_value target || return 1
            case $receipt_value in
            linux-x86_64 | linux-aarch64 | macos-x86_64 | macos-aarch64 | freebsd-x86_64) ;;
            *)
                crucible_receipt_refuse 'target names no platform the installer supports'
                return 1
                ;;
            esac
            receipt_target=$receipt_value
            ;;
        5)
            crucible_receipt_value layout || return 1
            if [ "$receipt_value" != versioned ]; then
                crucible_receipt_refuse 'layout is not versioned'
                return 1
            fi
            ;;
        6)
            crucible_receipt_value prefix || return 1
            case $receipt_value in
            /*) ;;
            *)
                crucible_receipt_refuse 'prefix is not an absolute path'
                return 1
                ;;
            esac
            case $receipt_value in
            */ | *//* | */./* | */. | */../* | */..)
                crucible_receipt_refuse 'prefix is not a canonical path'
                return 1
                ;;
            esac
            receipt_prefix=$receipt_value
            ;;
        7)
            crucible_receipt_value version || return 1
            receipt_rest=${receipt_value#*.}
            case $receipt_value in
            *.*) ;;
            *) receipt_rest= ;;
            esac
            case $receipt_rest in
            *.*) ;;
            *) receipt_rest= ;;
            esac
            if [ -z "$receipt_rest" ] ||
                ! crucible_receipt_number "${receipt_value%%.*}" ||
                ! crucible_receipt_number "${receipt_rest%%.*}" ||
                ! crucible_receipt_number "${receipt_rest#*.}"; then
                crucible_receipt_refuse 'version is not a release number'
                return 1
            fi
            receipt_version=$receipt_value
            ;;
        8)
            crucible_receipt_value sha256.crucible || return 1
            crucible_receipt_hex 64 sha256.crucible || return 1
            receipt_crucible=$receipt_value
            ;;
        9)
            crucible_receipt_value sha256.crucible-sandbox-broker || return 1
            crucible_receipt_hex 64 sha256.crucible-sandbox-broker || return 1
            receipt_broker=$receipt_value
            ;;
        *)
            crucible_receipt_refuse 'nothing may follow the last key'
            return 1
            ;;
        esac
    done <"$receipt_file"

    if [ "$receipt_line" -lt 8 ]; then
        crucible_receipt_refuse 'the receipt ends before its last key'
        return 1
    fi
}

# Takes the value of line `receipt_text` into `receipt_value` when the line
# names key `$1`.
crucible_receipt_value() {
    case $receipt_text in
    "$1="*) receipt_value=${receipt_text#"$1="} ;;
    *)
        crucible_receipt_refuse "line $receipt_line is not $1"
        return 1
        ;;
    esac
}

# Whether `receipt_value` is exactly `$1` lowercase hex digits.
crucible_receipt_hex() {
    case $receipt_value in
    '' | *[!0-9a-f]*)
        crucible_receipt_refuse "$2 is not lowercase hex"
        return 1
        ;;
    esac
    if [ "${#receipt_value}" -ne "$1" ]; then
        crucible_receipt_refuse "$2 is not $1 hex digits"
        return 1
    fi
}

# Whether `$1` is a decimal number written without a leading zero.
crucible_receipt_number() {
    case $1 in
    '' | 0?* | *[!0-9]*) return 1 ;;
    esac
}

crucible_receipt_refuse() {
    if [ "$receipt_line" -gt 0 ]; then
        printf 'refused: line %s: %s\n' "$receipt_line" "$1" >&2
    else
        printf 'refused: %s\n' "$1" >&2
    fi
}
# --- receipt reader: end ---

# The layout install.sh makes, under the installation directory:
#
#     crucible -> .crucible-install/current/crucible
#     cru -> crucible
#     .crucible-install/
#         current -> releases/<version>
#         lock -> <process id>@<host name>
#         releases/<version>/crucible
#         releases/<version>/crucible-sandbox-broker   (when the release has one)
#         releases/<version>/receipt
#
# What a release's receipt describes is what is removed there: each executable
# whose SHA-256 is the one the receipt records, and the receipt. Anything else,
# a file put in a release, a release with no receipt that names it, or a file
# beside the layout, is kept and named. A `crucible-sandbox-broker` file beside
# the links is the one a flat install left for the processes started from it,
# and is removed as a flat install's is. Every target is settled before the
# first is removed, and the removal holds the installer's lock, so no install
# runs while it does.
readonly link_target=.crucible-install/current/crucible
# The class is spelled out, as the installer spells it: a range such as [0-9]
# follows the locale.
digit=0123456789
hex=${digit}abcdef
release_part="(0|[${digit#0}][$digit]*)"
release_number="^$release_part\\.$release_part\\.$release_part\$"
me=$(id -u)

owner_of() { stat -c '%u' -- "$1" 2>/dev/null || stat -f '%u' -- "$1"; }
mode_of() { stat -c '%a' -- "$1" 2>/dev/null || stat -f '%Lp' -- "$1"; }
# Whether a mode as `mode_of` prints it lets group or others write; one that is
# not octal is taken to let them.
others_can_write() { [[ $1 =~ ^[0-7]+$ ]] || return 0; ((8#$1 & 8#022)); }

# A directory of the layout is read only when it is one, is not a link, and
# belongs to root or to this user with nobody else able to write it, as the
# installer holds it to.
trusted_directory() {
    local dir=$1 owner mode
    [[ -d $dir && ! -L $dir ]] || refuse "refusing to use $dir, which is not a directory"
    owner=$(owner_of "$dir")
    mode=$(mode_of "$dir")
    [[ $owner == 0 || $owner == "$me" ]] ||
        refuse "refusing to use $dir, which belongs to another user"
    if others_can_write "$mode"; then
        refuse "refusing to use $dir, which group or others can write; run chmod go-w $dir"
    fi
}

# The SHA-256 of a file in lowercase hex, read from standard input as the
# installer reads it, or a failure when none was printed.
sha256_of() {
    local sum
    case $hasher in
    sha256sum) sum=$(sha256sum <"$1" | awk '{ print $1 }') || return 1 ;;
    shasum) sum=$(shasum -a 256 <"$1" | awk '{ print $1 }') || return 1 ;;
    sha256) sum=$(sha256 -q <"$1") || return 1 ;;
    esac
    sum=$(printf '%s' "$sum" | tr '[:upper:]' '[:lower:]')
    [[ $sum =~ ^[$hex]{64}$ ]] || return 1
    printf '%s' "$sum"
}

# Whether `$1` is an executable the receipt just read records as `$2`.
described() {
    local sum
    [[ -n $2 && -f $1 && ! -L $1 ]] || return 1
    sum=$(sha256_of "$1") || return 1
    [[ $sum == "$2" ]]
}

binary=$destination/crucible
broker=$destination/crucible-sandbox-broker
alias_path=$destination/cru
prefix=$destination/.crucible-install
releases=$prefix/releases
current=$prefix/current
lock=$prefix/lock
if [[ -L $binary ]]; then
    [[ $(readlink -- "$binary") == "$link_target" ]] ||
        refuse "refusing to remove $binary, which is not this installer's link into $prefix"
elif [[ -e $binary ]]; then
    [[ -f $binary ]] || refuse "refusing to remove non-regular $binary"
fi
if [[ -e $broker || -L $broker ]]; then
    [[ -f $broker && ! -L $broker ]] || refuse "refusing to remove non-regular $broker"
fi

# What is removed, in order, each as `file PATH`, `tree PATH` or `dir PATH`,
# and what is kept, each as the line that names it.
plan=() kept=()
shopt -s dotglob nullglob
if [[ -e $alias_path || -L $alias_path ]]; then
    if [[ ! -L $alias_path || $(readlink "$alias_path") != crucible ]]; then
        kept+=("preserving unrelated $alias_path")
    else
        plan+=("file $alias_path")
    fi
fi
[[ ! -e $binary && ! -L $binary ]] || plan+=("file $binary")
[[ ! -e $broker ]] || plan+=("file $broker")
for entry in "$destination"/.crucible.link.*; do
    [[ ! -L $entry ]] || plan+=("file $entry")
done
if [[ -e $prefix || -L $prefix ]]; then
    trusted_directory "$prefix"
    [[ ! -e $lock && ! -L $lock ]] ||
        refuse "an install holds $lock, or one that stopped before it finished left it behind; once no install is running, remove it and run the uninstall again"
    if command -v sha256sum >/dev/null; then
        hasher=sha256sum
    elif command -v shasum >/dev/null; then
        hasher=shasum
    elif command -v sha256 >/dev/null; then
        hasher=sha256
    else
        refuse "sha256sum, shasum, or sha256 is required to tell what the installer put in $prefix"
    fi
    if [[ -e $current || -L $current ]]; then
        [[ -L $current ]] || refuse "refusing to use $current, which is not a link"
        target=$(readlink -- "$current")
        [[ $target == releases/* && ${target#releases/} =~ $release_number ]] ||
            refuse "refusing to use $current, which names no release"
        plan+=("file $current")
    fi
    prefix_kept=0
    for entry in "$prefix"/*; do
        case ${entry##*/} in
        current | releases) ;;
        .current.*)
            if [[ -L $entry ]]; then
                plan+=("file $entry")
            else
                kept+=("preserving $entry, which the installer did not make")
                prefix_kept=1
            fi
            ;;
        *)
            kept+=("preserving $entry, which the installer did not make")
            prefix_kept=1
            ;;
        esac
    done
    releases_kept=0
    if [[ -e $releases || -L $releases ]]; then
        trusted_directory "$releases"
        for release in "$releases"/*; do
            name=${release##*/}
            if [[ $name == .incoming.* && -d $release && ! -L $release ]]; then
                # A release an install that stopped was staging.
                plan+=("tree $release")
                continue
            fi
            if [[ ! $name =~ $release_number || -L $release || ! -d $release ]]; then
                kept+=("preserving $release, which is no release the installer made")
                releases_kept=1
                continue
            fi
            trusted_directory "$release"
            receipt=$release/receipt
            if [[ ! -f $receipt || -L $receipt ]] ||
                ! LC_ALL=C crucible_receipt_read "$receipt" 2>/dev/null ||
                [[ $receipt_prefix != "$prefix" || $receipt_version != "$name" ]]; then
                kept+=("preserving $release, which has no receipt that describes it")
                releases_kept=1
                continue
            fi
            release_kept=0
            for entry in "$release"/*; do
                case ${entry##*/} in
                crucible) described "$entry" "$receipt_crucible" && plan+=("file $entry") && continue ;;
                crucible-sandbox-broker)
                    described "$entry" "$receipt_broker" && plan+=("file $entry") && continue
                    ;;
                receipt) plan+=("file $entry"); continue ;;
                esac
                kept+=("preserving $entry, which its release's receipt does not describe")
                release_kept=1
            done
            if ((release_kept)); then
                releases_kept=1
            else
                plan+=("dir $release")
            fi
        done
        ((releases_kept)) || plan+=("dir $releases")
    fi
    ((prefix_kept || releases_kept)) || plan+=("dir $prefix")
fi
shopt -u dotglob nullglob

# The lock is taken as the installer takes it, a link naming this process, and
# released before the directory that holds it is removed.
locked=0
if ((!dry_run)) && [[ -d $prefix ]]; then
    ln -sn -- "$$@$(uname -n)" "$lock" 2>/dev/null ||
        refuse "$lock could not be taken; once no install is running, run the uninstall again"
    locked=1
    trap '((!locked)) || rm -f -- "$lock"' EXIT
fi

((!fancy)) || printf '\n%scrucible%s %s uninstall\n\n' "$bold" "$plain" "$dot"
for line in ${kept[@]+"${kept[@]}"}; do
    printf 'uninstall: %s\n' "$line" >&2
done
gone_alias=0 gone_binary=0 gone_broker=0
for step in ${plan[@]+"${plan[@]}"}; do
    kind=${step%% *}
    path=${step#* }
    if ((dry_run)); then
        printf 'Would remove %s\n' "$path"
        continue
    fi
    case $kind in
    file) rm -f -- "$path" ;;
    tree) rm -rf -- "$path" ;;
    dir)
        if [[ $path == "$prefix" ]]; then
            rm -f -- "$lock"
            locked=0
        fi
        rmdir -- "$path" 2>/dev/null ||
            printf 'uninstall: preserving %s, which is not empty\n' "$path" >&2
        ;;
    esac
    case $path in
    "$alias_path") gone_alias=1 ;;
    "$binary" | "$releases"/*/crucible) gone_binary=1 ;;
    "$broker" | "$releases"/*/crucible-sandbox-broker) gone_broker=1 ;;
    esac
done
if ((locked)); then
    rm -f -- "$lock"
    locked=0
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
