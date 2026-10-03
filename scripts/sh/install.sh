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

Downloads and verifies a crucible release, then installs `crucible` and the
`cru` alias. Linux and macOS archives also carry `crucible-sandbox-broker`, the
native confinement helper; it is installed beside `crucible`. A local
archive still requires its matching SHA256SUMS file.
USAGE
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

stop_spinner() {
    [[ -n $spinner ]] || return 0
    kill "$spinner" 2>/dev/null || true
    wait "$spinner" 2>/dev/null || true
    spinner=
}

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
# The classes are spelled out: a range such as [0-9] or [A-Za-z] follows the
# locale, and under a UTF-8 one takes non-ASCII digits and letters too.
digit=0123456789
alnum=${digit}ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz
[[ $version =~ ^[$digit]+\.[$digit]+\.[$digit]+([.-][$alnum.-]+)?$ ]] || {
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
    actual=$(sha256sum "$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null; then
    actual=$(shasum -a 256 "$archive" | awk '{ print $1 }')
elif command -v sha256 >/dev/null; then
    actual=$(sha256 -q "$archive")
else
    fail 1 'sha256sum, shasum, or sha256 is required'
fi
actual=$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')
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

step_begin install

if [[ -d $destination ]]; then
    destination=$(cd -- "$destination" && pwd -P)
    [[ $destination != / ]] || fail 2 'the installation directory resolves to root'
fi
alias_path=$destination/cru
if [[ -e $alias_path || -L $alias_path ]]; then
    [[ -L $alias_path && $(readlink "$alias_path") == crucible ]] ||
        fail 1 "refusing to replace unrelated $alias_path"
fi
broker_path=$destination/crucible-sandbox-broker
if [[ -n $broker && (-e $broker_path || -L $broker_path) ]]; then
    [[ -f $broker_path && ! -L $broker_path ]] ||
        fail 1 "refusing to replace non-regular $broker_path"
fi
if ((dry_run)); then
    step_done "$(shown "$destination") (dry run)" "$(shown "$destination") (dry run)"
    printf 'Would install crucible %s in %s and create %s -> crucible\n' \
        "$version" "$destination" "$alias_path"
    [[ -z $broker ]] || printf 'Would install %s beside it\n' "$broker_path"
    exit 0
fi

# Crucible trusts the broker only below directories that belong to root or to
# the user running it and that neither group nor others can write, so a loose
# directory is named here with its remedy rather than discovered when the first
# confined command refuses to start.
warn_where_broker_is_untrusted() {
    local dir=$destination owner mode me
    me=$(id -u)
    while :; do
        owner=$(stat -c '%u' -- "$dir" 2>/dev/null || stat -f '%u' -- "$dir")
        mode=$(stat -c '%a' -- "$dir" 2>/dev/null || stat -f '%Lp' -- "$dir")
        if [[ $owner != 0 && $owner != "$me" ]]; then
            warn "$dir belongs to another user, so confined commands will not trust $broker_path"
        elif ((8#${mode: -3} & 8#022)); then
            warn "$dir is writable by group or others, so confined commands will not trust $broker_path; run chmod go-w $dir"
        fi
        [[ $dir != / ]] || break
        dir=$(dirname -- "$dir")
    done
}

mkdir -p -- "$destination"
destination=$(cd -- "$destination" && pwd -P)
[[ $destination != / ]] || fail 2 'the installation directory resolves to root'
incoming=$(mktemp "$destination/.crucible.incoming.XXXXXX")
broker_incoming=
[[ -z $broker ]] ||
    broker_incoming=$(mktemp "$destination/.crucible-sandbox-broker.incoming.XXXXXX")
previous=
broker_previous=
landed=0
broker_landed=0
# Either everything lands or nothing changes: a failure after the broker has
# landed puts the previous broker back along with the previous executable.
cleanup_install() {
    rm -f -- "$incoming"
    [[ -z $broker_incoming ]] || rm -f -- "$broker_incoming"
    if ((broker_landed)); then
        if [[ -n $broker_previous && -e $broker_previous ]]; then
            mv -f -- "$broker_previous" "$broker_path"
        else
            rm -f -- "$broker_path"
        fi
    fi
    if ((landed)); then
        if [[ -n $previous && -e $previous ]]; then
            mv -f -- "$previous" "$destination/crucible"
        else
            rm -f -- "$destination/crucible"
        fi
    fi
}
on_exit='end_step $?; cleanup_install; rm -rf -- "$work"'
trap "$on_exit" EXIT
install -m 755 "$binary" "$incoming"
[[ -z $broker ]] || install -m 755 "$broker" "$broker_incoming"
if [[ -e $destination/crucible || -L $destination/crucible ]]; then
    [[ -f $destination/crucible && ! -L $destination/crucible ]] ||
        fail 1 "refusing to replace non-regular $destination/crucible"
    candidate=$(mktemp "$destination/.crucible.previous.XXXXXX")
    if ! cp -p -- "$destination/crucible" "$candidate"; then
        rm -f -- "$candidate"
        exit 1
    fi
    previous=$candidate
fi
# The broker lands first so the executable never runs beside a stale broker.
if [[ -n $broker ]]; then
    if [[ -e $broker_path ]]; then
        candidate=$(mktemp "$destination/.crucible-sandbox-broker.previous.XXXXXX")
        if ! cp -p -- "$broker_path" "$candidate"; then
            rm -f -- "$candidate"
            exit 1
        fi
        broker_previous=$candidate
    fi
    mv -f -- "$broker_incoming" "$broker_path"
    broker_landed=1
fi
mv -f -- "$incoming" "$destination/crucible"
landed=1
if ! said=$("$destination/crucible" --version) || [[ $said != "crucible $version" ]]; then
    printf -v problem 'installed binary reported %q, expected %q' \
        "${said:-nothing}" "crucible $version"
    rm -f -- "$destination/crucible"
    fail 1 "$problem"
fi
ln -sfn crucible "$alias_path"
[[ -n $previous ]] && rm -f -- "$previous"
[[ -n $broker_previous ]] && rm -f -- "$broker_previous"
previous=
broker_previous=
landed=0
broker_landed=0
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
