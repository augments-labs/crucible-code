#!/usr/bin/env bash
# Installs one verified release without requiring root or editing shell profiles.
set -euo pipefail

readonly REPO=augments-labs/crucible-code
readonly RELEASES="https://github.com/$REPO/releases"

version=
destination=${CRUCIBLE_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}
archive=
checksums=
dry_run=0

usage() {
    cat <<'USAGE'
Usage: scripts/sh/install.sh [--version VERSION] [--dir DIRECTORY] [--dry-run]
                          [--archive FILE --checksums FILE]

Downloads and verifies a crucible release, installs it in a directory of its
own under DIRECTORY/.crucible-install, makes it the active release, and links
`crucible` and the `cru` alias in DIRECTORY to it. Linux and macOS archives
also carry `crucible-sandbox-broker`, the native confinement helper; it is
installed beside `crucible` in the release's directory. A local archive still
requires its matching SHA256SUMS file.
USAGE
}

# Whether a name holds a byte that is not printable: the receipt that records
# the installation directory refuses one, and so does what reads it back. A name
# the check could not read through is taken to hold one.
has_control() {
    local count
    count=$(printf '%s' "$1" | LC_ALL=C tr -d '\040-\176\200-\377' | wc -c) || return 0
    ((count != 0))
}

while (($#)); do
    case "$1" in
    --version) version=${2:?--version needs a value}; shift 2 ;;
    --dir) destination=${2:?--dir needs a value}; shift 2 ;;
    --archive) archive=${2:?--archive needs a value}; shift 2 ;;
    --checksums) checksums=${2:?--checksums needs a value}; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) printf 'install: unknown argument %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ -n $destination ]] || {
    echo 'install: the installation directory is unsafe' >&2
    exit 2
}
case "/$destination/" in
*/../*)
    echo 'install: the installation directory is unsafe' >&2
    exit 2
    ;;
esac
if has_control "$destination"; then
    echo 'install: the installation directory is unsafe' >&2
    exit 2
fi
if [[ -n $archive || -n $checksums ]]; then
    [[ -n $archive && -n $checksums && -n $version ]] || {
        echo 'install: --archive requires --checksums and --version' >&2
        exit 2
    }
fi

work=$(mktemp -d)
readonly work

# What is printed, and nothing else, depends on where it goes. A terminal on
# both outputs gets the step list: a mark per step, colour, a spinner while one
# runs and a bar while the archive downloads. A pipe, a log, `NO_COLOR` or
# `TERM=dumb` gets one plain `install:` line per step, and the error lines
# install.sh has always printed on standard error.
fancy=0
if [[ -t 1 && -t 2 && -z ${NO_COLOR:-} && ${TERM:-} != dumb ]]; then
    fancy=1
fi
case ${LC_ALL:-${LC_CTYPE:-${LANG:-}}} in
*[Uu][Tt][Ff]-8*|*[Uu][Tt][Ff]8*)
    frames=('✳' '✻' '✺' '✱') done_mark='✓' fail_mark='✗' dot='·' fill='█' rest='░'
    ;;
*) frames=('|' '/' '-' '\') done_mark=ok fail_mark=x dot=- fill='#' rest=. ;;
esac
mark_width=${#done_mark}
readonly bold=$'\033[1m' dim=$'\033[2m' red=$'\033[31m' green=$'\033[32m' plain=$'\033[0m'
columns=80
if ((fancy)); then
    # Errors are silenced first: a session with no controlling terminal
    # cannot open /dev/tty, and the shell itself would say so.
    columns=$(stty size 2>/dev/null </dev/tty | awk '{ print $2 }') || true
    [[ $columns =~ ^[0-9]+$ ]] && ((columns >= 20)) || columns=80
fi
# The detail of a step starts here, after the indent, the mark and the label.
detail_column=$((2 + mark_width + 1 + 20))
# Set by the banner, which decides once where every detail goes.
narrow=0
step=
spinner=
download_to=
download_headers=

# `~` for the home directory, as the reader would write it.
shown() {
    local path=$1
    if [[ -n ${HOME:-} && $HOME != / && $path == "$HOME"/* ]]; then
        printf '~/%s' "${path#"$HOME"/}"
    else
        printf '%s' "$path"
    fi
}

megabytes() {
    awk -v bytes="$1" 'BEGIN { printf "%.1f MB", bytes / 1000000 }'
}

# Text wrapped to the terminal, each row indented by the first argument.
wrapped() {
    local indent=$1 text=$2
    printf '%s\n' "$text" | fold -s -w $((columns - ${#indent})) |
        sed -e 's/ *$//' -e "s/^/$indent/"
}

# One step's row: the mark, the label, then the detail, the detail and the
# mark each in their colour. The mark is padded before it is coloured, since
# the padding would count the colour's codes. In a narrow terminal, and for a
# detail that does not fit beside the label, the detail goes under it, each
# ` · `-separated part on its own rows.
row() {
    local mark=$1 label=$2 detail=$3 tint=$4 mark_tint=$5 part
    printf '\r\033[K  %s%*s%s %s' "$mark_tint" "$mark_width" "$mark" "$plain" "$label"
    if [[ -z $detail ]]; then
        printf '\n'
    elif ((!narrow && detail_column + ${#detail} <= columns)); then
        printf '%*s%s%s%s\n' $((20 - ${#label})) '' "$tint" "$detail" "$plain"
    else
        printf '\n'
        while IFS= read -r part; do
            printf '%s%s%s\n' "$tint" "$(wrapped '    ' "$part")" "$plain"
        done < <(printf '%s\n' "$detail" | awk -v sep=" $dot " '{ gsub(sep, "\n"); print }')
    fi
}

# The download so far against the size its response announced, or nothing
# while that size is unknown. The size comes off the network, so only digits
# are taken: bash arithmetic would read any other word as a variable to expand.
progress() {
    [[ -n $download_to && -s $download_headers ]] || return 0
    local total got room bar=
    total=$(tr -d '\r' <"$download_headers" | awk '
        tolower($1) == "content-length:" { n = ($2 ~ /^[0-9]+$/) ? $2 : "" }
        END { print n }')
    [[ $total =~ ^[0-9]{1,15}$ ]] && ((10#$total > 0)) || return 0
    total=$((10#$total))
    got=0
    [[ ! -f $download_to ]] || got=$(wc -c <"$download_to" | tr -d ' ')
    ((got <= total)) || got=$total
    local sizes
    sizes=$(awk -v got="$got" -v total="$total" \
        'BEGIN { printf "%.1f / %.1f MB", got / 1000000, total / 1000000 }')
    room=$((columns - detail_column - 2 - ${#sizes}))
    ((room <= 28)) || room=28
    if ((room >= 4)); then
        local filled=$((got * room / total)) i
        for ((i = 0; i < room; i++)); do
            if ((i < filled)); then bar+=$fill; else bar+="$dim$rest$plain"; fi
        done
        printf '%s  %s%s%s' "$bar" "$dim" "$sizes" "$plain"
    elif ((detail_column + ${#sizes} <= columns)); then
        printf '%s%s%s' "$dim" "$sizes" "$plain"
    fi
}

# Redraws the running step until it is stopped or this script is gone.
spin() {
    trap 'exit 0' TERM
    local i=0 label_room
    while kill -0 "$$" 2>/dev/null; do
        printf '\r\033[K  %*s %s' "$mark_width" "${frames[i % ${#frames[@]}]}" "$step"
        label_room=$((20 - ${#step}))
        ((label_room > 0)) || label_room=1
        printf '%*s%s' "$label_room" '' "$(progress)"
        i=$((i + 1))
        sleep 0.1
    done
}

# Ends the spinner. It exits on TERM within its frame, so the wait is short;
# but bash runs a trap only once the frame's `sleep` returns, and on macOS a
# spinner was seen looping two minutes after its TERM while the step waited for
# it. So a spinner still there after a second is killed. The pause here is not
# the frame's `sleep 0.1`, which lets a test hold one and not the other.
#
# A spinner the TERM reaches before its trap is set dies of the signal, and a
# bash that reports a job ended by a signal (3.2 does, for TERM as well) writes
# the report on its own standard error at whichever command of this function
# it is at when it notices: on macOS that was the pause, and the report reached
# the terminal above the step's row. The redirection on the function's body is
# in force for all of it, so the report goes nowhere whichever command that is.
stop_spinner() {
    [[ -n $spinner ]] || return 0
    kill "$spinner" || true
    local tries=0
    while kill -0 "$spinner" && ((tries < 20)); do
        sleep 0.05
        tries=$((tries + 1))
    done
    kill -KILL "$spinner" || true
    wait "$spinner" || true
    spinner=
} 2>/dev/null

# A terminal is narrow when the download's row, the widest a run draws, could
# not hold its detail beside its label.
banner() {
    local widest="crucible-$version-$1.tar.gz $dot 999.9 MB"
    ((detail_column + ${#widest} <= columns)) || narrow=1
    if ((fancy)); then
        printf '\n%scrucible %s%s%s\n\n' "$bold" "$version" "$plain" "${1:+ $dot $1}"
    else
        printf 'install: crucible %s%s\n' "$version" "${1:+ for $1}"
    fi
}

step_begin() {
    step=$1
    if ((fancy)); then
        # Started without the exit trap, as a precaution: bash 5.3 does not
        # pass it to a background process, but a shell that did would clean up
        # and report under this script's name if the spinner ran it.
        trap - EXIT
        spin &
        spinner=$!
        trap "$on_exit" EXIT
    fi
}

# Ends the running step: the first detail is the terminal's, the second the
# plain line's (`ok` when empty).
step_done() {
    stop_spinner
    if ((fancy)); then
        row "$done_mark" "$step" "$1" "$dim" "$green"
    else
        printf 'install: %s: %s\n' "$step" "${2:-ok}"
    fi
    step=
}

# Stops with `status`, naming the step that was running and why. Before the
# first step, and on standard error in the plain form, the reason is worded as
# install.sh has always worded it.
fail() {
    local status=$1 reason=$2
    stop_spinner
    if [[ -n $step ]]; then
        if ((fancy)); then
            row "$fail_mark" "$step" "$reason" "$red" "$red"
            printf '\nNothing was installed.\n'
            step=
            exit "$status"
        fi
        printf 'install: %s: failed\n' "$step"
        step=
    fi
    [[ -z $reason ]] || printf 'install: %s\n' "$reason" >&2
    exit "$status"
}

# A warning that does not stop the install, after the step list in a terminal.
warn() {
    if ((fancy)); then
        wrapped '' "$1" >&2
    else
        printf 'install: %s\n' "$1" >&2
    fi
}

# Whatever ends the script while a step runs still says which one it was.
end_step() {
    local status=$1
    stop_spinner
    [[ -n $step ]] || return 0
    if ((fancy)); then
        row "$fail_mark" "$step" "stopped with status $status" "$red" "$red"
        printf '\nNothing was installed.\n'
    else
        printf 'install: %s: failed\n' "$step"
    fi
    step=
}
on_exit='end_step $?; rm -rf -- "$work"'
trap "$on_exit" EXIT

# `-q` comes first in every curl call, so a ~/.curlrc cannot change where a
# release is fetched from or how; only an argument in that place turns it off.
download() {
    local url=$1 output=$2
    if ((fancy)); then
        # The response headers, flushed as they arrive, are where the bar
        # learns the size; curl's own complaint becomes the step's reason.
        curl -q --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
            --dump-header "$output.headers" --output "$output" "$url" \
            2>"$work/curl-error"
    else
        curl -q --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
            --output "$output" "$url"
    fi
}

download_or_fail() {
    local status=0
    download "$1" "$2" || status=$?
    ((status == 0)) && return 0
    local reason=
    [[ ! -s $work/curl-error ]] || reason=$(head -n 1 "$work/curl-error")
    ((fancy)) && [[ -z $reason ]] && reason="curl stopped with status $status"
    fail "$status" "$reason"
}

if [[ -z $archive ]]; then
    command -v curl >/dev/null || {
        echo 'install: curl is required to download a release' >&2
        exit 1
    }
    if [[ -z $version ]]; then
        latest=$(curl -q --proto '=https' --tlsv1.2 --fail --location --silent \
            --show-error --head --output /dev/null --write-out '%{url_effective}' \
            "$RELEASES/latest")
        version=${latest##*/}
    fi
fi
version=${version#v}
# The class is spelled out: a range such as [0-9] follows the locale, and under
# a UTF-8 one takes non-ASCII digits too. A release is three numbers with no
# leading zero and no suffix, the only form its receipt can record.
digit=0123456789
hex=${digit}abcdef
release_part="(0|[${digit#0}][$digit]*)"
release_number="^$release_part\\.$release_part\\.$release_part\$"
[[ $version =~ $release_number ]] || {
    printf 'install: invalid version %q\n' "$version" >&2
    exit 2
}

system=$(uname -s)
machine=$(uname -m)
platform=
architecture=
unsupported=
case "$system" in
Linux) platform=linux ;;
Darwin)
    platform=macos
    if [[ $machine == x86_64 ]] && command -v sysctl >/dev/null &&
        [[ $(sysctl -in sysctl.proc_translated 2>/dev/null || true) == 1 ]]; then
        machine=arm64
    fi
    ;;
FreeBSD) platform=freebsd ;;
*) unsupported="unsupported operating system $system" ;;
esac
if [[ -z $unsupported ]]; then
    case "$machine" in
    x86_64|amd64) architecture=x86_64 ;;
    aarch64|arm64) architecture=aarch64 ;;
    *) unsupported="unsupported architecture $machine" ;;
    esac
fi
if [[ -z $unsupported && $platform == freebsd && $architecture != x86_64 ]]; then
    unsupported='FreeBSD releases are available only for x86-64'
fi
if [[ -n $unsupported ]]; then
    banner ''
    step_begin 'detect platform'
    fail 1 "$unsupported"
fi
banner "$platform-$architecture"
step_begin 'detect platform'
step_done "$platform-$architecture" "$platform-$architecture"

stem=crucible-$version-$platform-$architecture
name=$stem.tar.gz
if [[ -z $archive ]]; then
    archive=$work/$name
    checksums=$work/SHA256SUMS
    # Set before the spinner starts, since it reads its own copy of them.
    download_to=$archive
    download_headers=$archive.headers
    step_begin download
    download_or_fail "$RELEASES/download/v$version/$name" "$archive"
    download_or_fail "$RELEASES/download/v$version/SHA256SUMS" "$checksums"
    size=$(megabytes "$(wc -c <"$archive" | tr -d ' ')")
    download_to=
    step_done "$name $dot $size" "$name ($size)"
else
    archive=$(cd "$(dirname "$archive")" && pwd -P)/$(basename "$archive")
    checksums=$(cd "$(dirname "$checksums")" && pwd -P)/$(basename "$checksums")
fi

step_begin 'verify checksum'
[[ -f $archive && -f $checksums ]] ||
    fail 1 'archive or checksum file does not exist'
# A local archive is copied into the private work directory first, and that
# copy is what is hashed and unpacked: the original could change between the
# two reads. A downloaded archive is already there.
if [[ $archive != "$work"/* ]]; then
    mkdir -- "$work/local"
    cp -- "$archive" "$work/local/$(basename "$archive")" ||
        fail 1 'the archive could not be copied for verification'
    archive=$work/local/$(basename "$archive")
fi

expected=$(awk -v name="$(basename "$archive")" '
    ($2 == name || $2 == "*" name) && $1 ~ /^[0-9A-Fa-f]+$/ { print tolower($1) }
' "$checksums")
[[ $(printf '%s\n' "$expected" | awk 'NF { n++ } END { print n + 0 }') == 1 &&
    ${#expected} == 64 ]] ||
    fail 1 'SHA256SUMS must contain exactly one valid line for the archive'
if command -v sha256sum >/dev/null; then
    hasher=sha256sum
elif command -v shasum >/dev/null; then
    hasher=shasum
elif command -v sha256 >/dev/null; then
    hasher=sha256
else
    fail 1 'sha256sum, shasum, or sha256 is required'
fi
# The SHA-256 of a file in lowercase hex, or a failure when none was printed.
# The file is read from standard input, since a tool handed a name with a
# backslash in it escapes the sum it prints.
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
actual=$(sha256_of "$archive") || actual=
[[ $actual == "$expected" ]] || fail 1 'archive checksum does not match SHA256SUMS'
step_done "$(basename "$checksums")" ok

step_begin unpack

members=$work/members
details=$work/member-details
tar -tzf "$archive" >"$members"
tar -tvzf "$archive" >"$details"
exec 3<"$details"
while IFS= read -r member; do
    IFS= read -r detail <&3 || fail 1 'archive listings disagreed'
    kind=${detail:0:1}
    case "$member" in
    "$stem"|"$stem/")
        [[ $kind == d ]] || {
            printf -v problem 'archive directory %q is not a directory' "$member"
            fail 1 "$problem"
        }
        ;;
    "$stem/crucible"|"$stem/crucible-sandbox-broker"|"$stem/README.md"|\
        "$stem/LICENSE"|"$stem/install.sh"|"$stem/uninstall.sh")
        [[ $kind == - ]] || {
            printf -v problem 'archive file %q is not a regular file' "$member"
            fail 1 "$problem"
        }
        ;;
    *) printf -v problem 'unexpected archive member %q' "$member"; fail 1 "$problem" ;;
    esac
done <"$members"
if IFS= read -r _ <&3; then
    fail 1 'archive listings disagreed'
fi
exec 3<&-
[[ $(grep -c "^$stem/crucible$" "$members") == 1 ]] ||
    fail 1 'archive does not contain exactly one crucible binary'
broker_members=$(grep -c "^$stem/crucible-sandbox-broker$" "$members") || true
((broker_members <= 1)) || fail 1 'archive contains more than one sandbox broker'

tar -xzf "$archive" -C "$work"
binary=$work/$stem/crucible
[[ -f $binary && ! -L $binary ]] ||
    fail 1 'crucible in the archive is not a regular file'
broker=
if ((broker_members)); then
    broker=$work/$stem/crucible-sandbox-broker
    [[ -f $broker && ! -L $broker ]] ||
        fail 1 'the sandbox broker in the archive is not a regular file'
fi
step_done '' ok

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

step_begin install

# The layout, under the directory the installer was given:
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
# A release is staged in a hidden directory beside the others, its receipt
# written last, and is renamed into place whole; then `current` is replaced by
# one rename of a new link over it. A crash at any point leaves the release
# that was active still active, or the new one active and complete. No release
# is ever removed. One install at a time holds the lock, and the next one waits
# for it, or refuses when the install that took it is gone.
readonly link_target=.crucible-install/current/crucible
me=$(id -u)

layout_paths() {
    prefix=$destination/.crucible-install
    releases=$prefix/releases
    current=$prefix/current
    lock=$prefix/lock
    unit=$releases/$version
    link_path=$destination/crucible
    alias_path=$destination/cru
    broker_path=$unit/crucible-sandbox-broker
}

owner_of() { stat -c '%u' -- "$1" 2>/dev/null || stat -f '%u' -- "$1"; }
mode_of() { stat -c '%a' -- "$1" 2>/dev/null || stat -f '%Lp' -- "$1"; }
# Whether a mode as `mode_of` prints it lets group or others write. The mode is
# printed without leading zeros, so it is read whole, and one that is not octal
# is taken to let them.
others_can_write() { [[ $1 =~ ^[0-7]+$ ]] || return 0; ((8#$1 & 8#022)); }

# A directory of the layout is used only when it is one, is not a link, and
# belongs to root or to this user with nobody else able to write it, which is
# what crucible holds the layout to when it reads it.
trusted_directory() {
    local dir=$1 owner mode
    [[ -d $dir && ! -L $dir ]] || fail 1 "refusing to use $dir, which is not a directory"
    owner=$(owner_of "$dir")
    mode=$(mode_of "$dir")
    [[ $owner == 0 || $owner == "$me" ]] ||
        fail 1 "refusing to use $dir, which belongs to another user"
    if others_can_write "$mode"; then
        fail 1 "refusing to use $dir, which group or others can write; run chmod go-w $dir"
    fi
}

# Reads a receipt, failing with the reader's reason.
read_receipt() {
    local file=$1 reason
    [[ -f $file && ! -L $file ]] || fail 1 "refusing to use $file, which is not a receipt"
    LC_ALL=C crucible_receipt_read "$file" 2>"$work/refusal" && return 0
    reason=$(head -n 1 "$work/refusal")
    fail 1 "refusing to use $file: ${reason#refused: }"
}

# What is there already, read and changed in nothing: the links in the
# directory must be this installer's, and the active release and its receipt
# whole. It runs before a dry run reports, and again once the lock is held.
inspect() {
    local target
    installation=
    if [[ -e $alias_path || -L $alias_path ]]; then
        [[ -L $alias_path && $(readlink -- "$alias_path") == crucible ]] ||
            fail 1 "refusing to replace unrelated $alias_path"
    fi
    if [[ -e $link_path || -L $link_path ]]; then
        [[ -L $link_path && $(readlink -- "$link_path") == "$link_target" ]] ||
            fail 1 "refusing to replace $link_path, which is not this installer's link into $prefix"
    fi
    [[ -e $prefix || -L $prefix ]] || return 0
    trusted_directory "$prefix"
    [[ ! -e $releases && ! -L $releases ]] || trusted_directory "$releases"
    [[ -e $current || -L $current ]] || return 0
    [[ -L $current ]] || fail 1 "refusing to use $current, which is not a link"
    target=$(readlink -- "$current")
    [[ $target == releases/* && ${target#releases/} =~ $release_number ]] ||
        fail 1 "refusing to use $current, which names no release"
    trusted_directory "$prefix/$target"
    read_receipt "$prefix/$target/receipt"
    [[ $receipt_prefix == "$prefix" && $receipt_version == "${target#releases/}" ]] ||
        fail 1 "refusing to use $prefix/$target/receipt, which describes another release"
    installation=$receipt_installation
}

lock_owner=
locked=0
# The lock is a link whose text names the install that holds it, so taking it
# is one call that fails while it exists, on every platform this runs on.
take_lock() {
    local tries=0 owner pid host
    lock_owner="$$@$(uname -n)"
    [[ ! -d $lock || -L $lock ]] || fail 1 "refusing to use $lock, which is a directory"
    until ln -sn -- "$lock_owner" "$lock" 2>/dev/null; do
        if owner=$(readlink -- "$lock" 2>/dev/null); then
            pid=${owner%%@*}
            host=${owner#*@}
            # Stale only when neither signal 0 nor ps finds the process, since
            # signal 0 cannot reach another user's and ps may be missing.
            if [[ $host == "${lock_owner#*@}" && $pid =~ ^[$digit]+$ ]] &&
                ! kill -0 "$pid" 2>/dev/null && ! ps -p "$pid" >/dev/null 2>&1; then
                fail 1 "an install that stopped before it finished left $lock behind; once no install is running, remove it and run the install again"
            fi
        elif [[ -e $lock || -L $lock ]]; then
            fail 1 "refusing to use $lock, which is not a link"
        elif [[ ! -w $prefix ]]; then
            fail 1 "$lock could not be created, since $prefix is not writable"
        fi
        tries=$((tries + 1))
        ((tries < 300)) ||
            fail 1 "another install, ${owner:-unknown}, still holds $lock after a minute"
        sleep 0.2
    done
    locked=1
}

# Writes what was written so far to disk where the platform's sync can say
# which files: GNU sync flushes each one it is given, and the BSDs' and
# macOS's take no names and schedule every write.
flush() {
    sync -- "$@" 2>/dev/null || sync
}

staging=
next_current=
cleanup_install() {
    [[ -z $staging ]] || rm -rf -- "$staging"
    [[ -z $next_current ]] || rm -f -- "$next_current"
    if ((locked)); then
        rm -f -- "$lock"
        locked=0
    fi
}

# What an install that stopped left behind, removed only under the lock.
remove_leftovers() {
    local left
    for left in "$releases"/.incoming.* "$prefix"/.current.*; do
        [[ ! -e $left && ! -L $left ]] || rm -rf -- "$left"
    done
}

# The receipt of a release, from the executables already staged in `$1`.
write_receipt() {
    local at=$1 crucible_sum broker_sum=
    crucible_sum=$(sha256_of "$at/crucible") || fail 1 "the staged crucible could not be hashed"
    if [[ -n $broker ]]; then
        broker_sum=$(sha256_of "$at/crucible-sandbox-broker") ||
            fail 1 "the staged sandbox broker could not be hashed"
    fi
    {
        printf 'crucible-installer-receipt 1\n'
        printf 'manager=crucible-installer\n'
        printf 'installation=%s\n' "$installation"
        printf 'target=%s\n' "$platform-$architecture"
        printf 'layout=versioned\n'
        printf 'prefix=%s\n' "$prefix"
        printf 'version=%s\n' "$version"
        printf 'sha256.crucible=%s\n' "$crucible_sum"
        [[ -z $broker_sum ]] || printf 'sha256.crucible-sandbox-broker=%s\n' "$broker_sum"
    } >"$at/receipt"
    chmod 644 "$at/receipt"
    # The same reader as crucible's must take what was written.
    read_receipt "$at/receipt"
}

stage_unit() {
    local said
    if [[ -z $installation ]]; then
        installation=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
        [[ $installation =~ ^[$hex]{32}$ ]] ||
            fail 1 'no installation identifier could be read from /dev/urandom'
    fi
    staging=$(mktemp -d "$releases/.incoming.XXXXXX")
    chmod 755 "$staging"
    install -m 755 "$binary" "$staging/crucible"
    [[ -z $broker ]] || install -m 755 "$broker" "$staging/crucible-sandbox-broker"
    if ! said=$("$staging/crucible" --version) || [[ $said != "crucible $version" ]]; then
        printf -v problem 'installed binary reported %q, expected %q' \
            "${said:-nothing}" "crucible $version"
        fail 1 "$problem"
    fi
    write_receipt "$staging"
    if [[ -n $broker ]]; then
        flush "$staging/crucible" "$staging/crucible-sandbox-broker" "$staging/receipt" "$staging"
    else
        flush "$staging/crucible" "$staging/receipt" "$staging"
    fi
    mv -- "$staging" "$unit"
    staging=
    flush "$releases"
}

# A release already in place is used again only when it is this one: the same
# installation, platform and build, and nothing in it the receipt does not
# account for. Any other is refused and left as it is.
reuse_unit() {
    local differs="refusing to replace $unit, which holds another build of crucible $version"
    local entries expected=$'crucible\nreceipt' sum
    trusted_directory "$unit"
    read_receipt "$unit/receipt"
    [[ $receipt_prefix == "$prefix" && $receipt_version == "$version" &&
        $receipt_target == "$platform-$architecture" ]] || fail 1 "$differs"
    [[ -z $installation || $receipt_installation == "$installation" ]] || fail 1 "$differs"
    installation=$receipt_installation
    [[ -f $unit/crucible && ! -L $unit/crucible ]] || fail 1 "$differs"
    sum=$(sha256_of "$binary") && [[ $sum == "$receipt_crucible" ]] || fail 1 "$differs"
    sum=$(sha256_of "$unit/crucible") && [[ $sum == "$receipt_crucible" ]] || fail 1 "$differs"
    if [[ -n $broker ]]; then
        expected=$'crucible\ncrucible-sandbox-broker\nreceipt'
        [[ -f $broker_path && ! -L $broker_path ]] || fail 1 "$differs"
        sum=$(sha256_of "$broker") && [[ $sum == "$receipt_broker" ]] || fail 1 "$differs"
        sum=$(sha256_of "$broker_path") && [[ $sum == "$receipt_broker" ]] || fail 1 "$differs"
    else
        [[ -z $receipt_broker ]] || fail 1 "$differs"
    fi
    entries=$(cd -- "$unit" && LC_ALL=C ls -A)
    [[ $entries == "$expected" ]] || fail 1 "$differs"
}

# `current` is replaced by renaming a new link over it. GNU and BusyBox mv say
# so with -T, the BSDs and macOS with -h; without either, mv would move the new
# link into the release `current` names. -f keeps mv from stopping to ask.
activate() {
    next_current=$prefix/.current.$$
    ln -sn -- "releases/$version" "$next_current"
    if ((rename_flag_t)); then
        mv -fT -- "$next_current" "$current"
    else
        mv -fh -- "$next_current" "$current"
    fi
    next_current=
    flush "$prefix"
}

if [[ -d $destination ]]; then
    destination=$(cd -- "$destination" && pwd -P)
    [[ $destination != / ]] || fail 2 'the installation directory resolves to root'
    ! has_control "$destination" || fail 2 'the installation directory is unsafe'
fi
layout_paths
inspect
if ((dry_run)); then
    step_done "$(shown "$destination") (dry run)" "$(shown "$destination") (dry run)"
    printf 'Would install crucible %s in %s and create %s -> %s and %s -> crucible\n' \
        "$version" "$unit" "$link_path" "$link_target" "$alias_path"
    [[ -z $broker ]] || printf 'Would install %s beside it\n' "$broker_path"
    exit 0
fi

# Crucible trusts the broker only below directories that belong to root or to
# the user running it and that neither group nor others can write, so a loose
# directory is named here with its remedy rather than discovered when the first
# confined command refuses to start.
warn_where_broker_is_untrusted() {
    local dir=$unit owner mode
    while :; do
        owner=$(owner_of "$dir")
        mode=$(mode_of "$dir")
        if [[ $owner != 0 && $owner != "$me" ]]; then
            warn "$dir belongs to another user, so confined commands will not trust $broker_path"
        elif others_can_write "$mode"; then
            warn "$dir is writable by group or others, so confined commands will not trust $broker_path; run chmod go-w $dir"
        fi
        [[ $dir != / ]] || break
        dir=$(dirname -- "$dir")
    done
}

on_exit='end_step $?; cleanup_install; rm -rf -- "$work"'
trap "$on_exit" EXIT
: >"$work/rename-test"
rename_flag_t=0
! mv -T -- "$work/rename-test" "$work/renamed" 2>/dev/null || rename_flag_t=1

mkdir -p -- "$destination"
destination=$(cd -- "$destination" && pwd -P)
[[ $destination != / ]] || fail 2 'the installation directory resolves to root'
! has_control "$destination" || fail 2 'the installation directory is unsafe'
layout_paths
# Two first installs can both find no prefix; the one that loses the race to
# create it uses the one the other made.
mkdir -m 755 -- "$prefix" 2>/dev/null || [[ -d $prefix && ! -L $prefix ]] ||
    fail 1 "$prefix could not be created"
trusted_directory "$prefix"
take_lock
inspect
remove_leftovers
[[ -e $releases || -L $releases ]] || mkdir -m 755 -- "$releases"
trusted_directory "$releases"
if [[ -e $unit || -L $unit ]]; then
    [[ -d $unit && ! -L $unit ]] ||
        fail 1 "refusing to replace $unit, which is not a release directory"
    reuse_unit
else
    stage_unit
fi
activate
[[ -L $link_path ]] || ln -sn -- "$link_target" "$link_path"
[[ -L $alias_path ]] || ln -sn -- crucible "$alias_path"
flush "$destination"
cleanup_install
on_exit='end_step $?; rm -rf -- "$work"'
trap "$on_exit" EXIT
# The plain lines spell the directory as the step list does: `~` is what the
# reader types back into this shell.
where=$(shown "$destination")
step_done "$where" "$where"

if [[ -n $broker ]]; then
    installed="Installed crucible, crucible-sandbox-broker and cru in $where"
else
    installed="Installed crucible and cru in $where"
fi
on_path=0
case ":$PATH:" in
*":$destination:"*) on_path=1 ;;
esac
if ((!fancy)); then
    printf '%s\n' "$installed"
    [[ -z $broker ]] || warn_where_broker_is_untrusted
    ((on_path)) || printf 'Add %s to PATH to run crucible.\n' "$where"
    exit 0
fi

printf '\n'
wrapped '' "$installed"
[[ -z $broker ]] || warn_where_broker_is_untrusted
printf '\n'
if ((on_path)); then
    printf '%s%s\n' "$dim" "$(wrapped '' "$where is on your PATH.")$plain"
else
    printf '%s%s\n\n' "$dim" "$(wrapped '' "$where is not on your PATH. Add it with:")$plain"
    # Copied whole into a shell, so never wrapped; a path the double quotes
    # would not keep literal is quoted for the shell instead.
    if [[ $destination == *[\"\$\`\\]* ]]; then
        printf '  export PATH=%q:"$PATH"\n' "$destination"
    elif [[ $where == "~/"* ]]; then
        printf '  export PATH="$HOME/%s:$PATH"\n' "${where#"~/"}"
    else
        printf '  export PATH="%s:$PATH"\n' "$destination"
    fi
fi
printf '\n%sThen run: crucible%s\n' "$dim" "$plain"
