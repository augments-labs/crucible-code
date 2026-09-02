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
Usage: scripts/install.sh [--version VERSION] [--dir DIRECTORY] [--dry-run]
                          [--archive FILE --checksums FILE]

Downloads and verifies a crucible release, then installs `crucible` and the
`cru` alias. Linux archives also carry `crucible-sandbox-broker`, the program
confined commands run as PID 1; it is installed beside `crucible`. A local
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
trap 'rm -rf -- "$work"' EXIT

download() {
    local url=$1 output=$2
    curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
        --output "$output" "$url"
}

if [[ -z $archive ]]; then
    command -v curl >/dev/null || {
        echo 'install: curl is required to download a release' >&2
        exit 1
    }
    if [[ -z $version ]]; then
        latest=$(curl --proto '=https' --tlsv1.2 --fail --location --silent \
            --show-error --head --output /dev/null --write-out '%{url_effective}' \
            "$RELEASES/latest")
        version=${latest##*/}
    fi
fi
version=${version#v}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || {
    printf 'install: invalid version %q\n' "$version" >&2
    exit 2
}

system=$(uname -s)
machine=$(uname -m)
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
*) printf 'install: unsupported operating system %s\n' "$system" >&2; exit 1 ;;
esac
case "$machine" in
x86_64|amd64) architecture=x86_64 ;;
aarch64|arm64) architecture=aarch64 ;;
*) printf 'install: unsupported architecture %s\n' "$machine" >&2; exit 1 ;;
esac
if [[ $platform == freebsd && $architecture != x86_64 ]]; then
    echo 'install: FreeBSD releases are available only for x86-64' >&2
    exit 1
fi

stem=crucible-$version-$platform-$architecture
name=$stem.tar.gz
if [[ -z $archive ]]; then
    archive=$work/$name
    checksums=$work/SHA256SUMS
    download "$RELEASES/download/v$version/$name" "$archive"
    download "$RELEASES/download/v$version/SHA256SUMS" "$checksums"
else
    archive=$(cd "$(dirname "$archive")" && pwd -P)/$(basename "$archive")
    checksums=$(cd "$(dirname "$checksums")" && pwd -P)/$(basename "$checksums")
fi
[[ -f $archive && -f $checksums ]] || {
    echo 'install: archive or checksum file does not exist' >&2
    exit 1
}

expected=$(awk -v name="$(basename "$archive")" '
    ($2 == name || $2 == "*" name) && $1 ~ /^[0-9A-Fa-f]+$/ { print tolower($1) }
' "$checksums")
[[ $(printf '%s\n' "$expected" | awk 'NF { n++ } END { print n + 0 }') == 1 &&
    ${#expected} == 64 ]] || {
    echo 'install: SHA256SUMS must contain exactly one valid line for the archive' >&2
    exit 1
}
if command -v sha256sum >/dev/null; then
    actual=$(sha256sum "$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null; then
    actual=$(shasum -a 256 "$archive" | awk '{ print $1 }')
elif command -v sha256 >/dev/null; then
    actual=$(sha256 -q "$archive")
else
    echo 'install: sha256sum, shasum, or sha256 is required' >&2
    exit 1
fi
actual=$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')
[[ $actual == "$expected" ]] || {
    echo 'install: archive checksum does not match SHA256SUMS' >&2
    exit 1
}

members=$work/members
details=$work/member-details
tar -tzf "$archive" >"$members"
tar -tvzf "$archive" >"$details"
exec 3<"$details"
while IFS= read -r member; do
    IFS= read -r detail <&3 || {
        echo 'install: archive listings disagreed' >&2
        exit 1
    }
    kind=${detail:0:1}
    case "$member" in
    "$stem"|"$stem/")
        [[ $kind == d ]] || {
            printf 'install: archive directory %q is not a directory\n' "$member" >&2
            exit 1
        }
        ;;
    "$stem/crucible"|"$stem/crucible-sandbox-broker"|"$stem/README.md"|\
        "$stem/LICENSE"|"$stem/install.sh"|"$stem/uninstall.sh")
        [[ $kind == - ]] || {
            printf 'install: archive file %q is not a regular file\n' "$member" >&2
            exit 1
        }
        ;;
    *) printf 'install: unexpected archive member %q\n' "$member" >&2; exit 1 ;;
    esac
done <"$members"
if IFS= read -r _ <&3; then
    echo 'install: archive listings disagreed' >&2
    exit 1
fi
exec 3<&-
[[ $(grep -c "^$stem/crucible$" "$members") == 1 ]] || {
    echo 'install: archive does not contain exactly one crucible binary' >&2
    exit 1
}
broker_members=$(grep -c "^$stem/crucible-sandbox-broker$" "$members") || true
((broker_members <= 1)) || {
    echo 'install: archive contains more than one sandbox broker' >&2
    exit 1
}

tar -xzf "$archive" -C "$work"
binary=$work/$stem/crucible
[[ -f $binary && ! -L $binary ]] || {
    echo 'install: crucible in the archive is not a regular file' >&2
    exit 1
}
broker=
if ((broker_members)); then
    broker=$work/$stem/crucible-sandbox-broker
    [[ -f $broker && ! -L $broker ]] || {
        echo 'install: the sandbox broker in the archive is not a regular file' >&2
        exit 1
    }
fi

if [[ -d $destination ]]; then
    destination=$(cd -- "$destination" && pwd -P)
    [[ $destination != / ]] || {
        echo 'install: the installation directory resolves to root' >&2
        exit 2
    }
fi
alias_path=$destination/cru
if [[ -e $alias_path || -L $alias_path ]]; then
    [[ -L $alias_path && $(readlink "$alias_path") == crucible ]] || {
        printf 'install: refusing to replace unrelated %s\n' "$alias_path" >&2
        exit 1
    }
fi
broker_path=$destination/crucible-sandbox-broker
if [[ -n $broker && (-e $broker_path || -L $broker_path) ]]; then
    [[ -f $broker_path && ! -L $broker_path ]] || {
        printf 'install: refusing to replace non-regular %s\n' "$broker_path" >&2
        exit 1
    }
fi
if ((dry_run)); then
    printf 'Would install crucible %s in %s and create %s -> crucible\n' \
        "$version" "$destination" "$alias_path"
    [[ -z $broker ]] || printf 'Would install %s beside it\n' "$broker_path"
    exit 0
fi

# Crucible trusts the broker only below directories that neither group nor
# others can write, so a loose directory is named here with its remedy rather
# than discovered when the first confined command refuses to start.
warn_where_broker_is_untrusted() {
    local dir=$destination mode
    while :; do
        mode=$(stat -c '%a' -- "$dir" 2>/dev/null || stat -f '%Lp' -- "$dir")
        if ((8#${mode: -3} & 8#022)); then
            printf 'install: %s is writable by group or others, so confined commands will not trust %s; run chmod go-w %s\n' \
                "$dir" "$broker_path" "$dir" >&2
        fi
        [[ $dir != / ]] || break
        dir=$(dirname -- "$dir")
    done
}

mkdir -p -- "$destination"
destination=$(cd -- "$destination" && pwd -P)
[[ $destination != / ]] || {
    echo 'install: the installation directory resolves to root' >&2
    exit 2
}
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
trap 'cleanup_install; rm -rf -- "$work"' EXIT
install -m 755 "$binary" "$incoming"
[[ -z $broker ]] || install -m 755 "$broker" "$broker_incoming"
if [[ -e $destination/crucible || -L $destination/crucible ]]; then
    [[ -f $destination/crucible && ! -L $destination/crucible ]] || {
        printf 'install: refusing to replace non-regular %s\n' \
            "$destination/crucible" >&2
        exit 1
    }
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
    printf 'install: installed binary reported %q, expected %q\n' \
        "${said:-nothing}" "crucible $version" >&2
    rm -f -- "$destination/crucible"
    exit 1
fi
ln -sfn crucible "$alias_path"
[[ -n $previous ]] && rm -f -- "$previous"
[[ -n $broker_previous ]] && rm -f -- "$broker_previous"
previous=
broker_previous=
landed=0
broker_landed=0
trap 'rm -rf -- "$work"' EXIT

if [[ -n $broker ]]; then
    printf 'Installed %s, %s and %s\n' "$destination/crucible" "$broker_path" "$alias_path"
    warn_where_broker_is_untrusted
else
    printf 'Installed %s and %s\n' "$destination/crucible" "$alias_path"
fi
case ":$PATH:" in
*":$destination:"*) ;;
*) printf 'Add %s to PATH to run crucible.\n' "$destination" ;;
esac
