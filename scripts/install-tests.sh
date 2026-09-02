#!/usr/bin/env bash
# Offline behavioral tests for the release installer and uninstaller.
set -euo pipefail

cd "$(dirname "$0")/.."
readonly INSTALL=$PWD/scripts/install.sh
readonly UNINSTALL=$PWD/scripts/uninstall.sh
scratch=$(mktemp -d)
readonly scratch
trap 'rm -rf -- "$scratch"' EXIT

case $(uname -s) in
Linux) platform=linux ;;
Darwin) platform=macos ;;
FreeBSD) platform=freebsd ;;
*) echo 'installer tests require Linux, macOS, or FreeBSD'; exit 0 ;;
esac
case $(uname -m) in
x86_64|amd64) architecture=x86_64 ;;
aarch64|arm64) architecture=aarch64 ;;
*) echo 'installer tests require x86-64 or ARM64'; exit 0 ;;
esac
if [[ $platform == freebsd && $architecture != x86_64 ]]; then
    echo 'installer tests skip an unpublished platform'
    exit 0
fi

version=9.8.7
stem=crucible-$version-$platform-$architecture

checksum() {
    local file=$1
    if command -v sha256sum >/dev/null; then
        sha256sum "$file"
    elif command -v shasum >/dev/null; then
        shasum -a 256 "$file"
    else
        sha256 "$file"
    fi
}

# A release archive. Linux archives also carry the sandbox broker, a program
# that only ever runs as PID 1 inside a confined command and exits 125 when
# started any other way; the fixture broker records which release it came from.
release() {
    local at=$1 said=${2:-"crucible $version"} broker=${3:-broker}
    mkdir -p "$at/$stem"
    printf '#!/usr/bin/env sh\nprintf "%%s\\n" %q\n' "$said" >"$at/$stem/crucible"
    chmod +x "$at/$stem/crucible"
    if [[ $broker == broker ]]; then
        printf '#!/usr/bin/env sh\n# %s\nexit 125\n' "$said" >"$at/$stem/crucible-sandbox-broker"
        chmod +x "$at/$stem/crucible-sandbox-broker"
    fi
    printf 'readme\n' >"$at/$stem/README.md"
    printf 'licence\n' >"$at/$stem/LICENSE"
    printf '#!/usr/bin/env bash\n' >"$at/$stem/install.sh"
    printf '#!/usr/bin/env bash\n' >"$at/$stem/uninstall.sh"
    tar -czf "$at/$stem.tar.gz" -C "$at" "$stem"
    (cd "$at" && checksum "$stem.tar.gz") >"$at/SHA256SUMS"
}

install_from() {
    local asset=$1 destination=$2
    "$INSTALL" --version "$version" --dir "$destination" \
        --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS"
}

broker_exit() {
    local status=0
    "$1" >/dev/null 2>&1 || status=$?
    printf '%s\n' "$status"
}

echo '==> verified local install and idempotent update'
asset=$scratch/good
release "$asset"
destination=$scratch/bin
install_from "$asset" "$destination"
[[ $($destination/crucible --version) == "crucible $version" ]]
[[ -L $destination/cru && $(readlink "$destination/cru") == crucible ]]
[[ -f $destination/crucible-sandbox-broker && -x $destination/crucible-sandbox-broker ]]
[[ $(broker_exit "$destination/crucible-sandbox-broker") == 125 ]]
install_from "$asset" "$destination"

echo '==> an archive without a sandbox broker still installs the executable'
brokerless=$scratch/brokerless
release "$brokerless" "crucible $version" none
brokerless_bin=$scratch/brokerless-bin
install_from "$brokerless" "$brokerless_bin"
[[ $($brokerless_bin/crucible --version) == "crucible $version" ]]
[[ ! -e $brokerless_bin/crucible-sandbox-broker ]]

echo '==> dry run makes no destination'
dry=$scratch/dry
"$INSTALL" --dry-run --version "$version" --dir "$dry" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS" >/dev/null
[[ ! -e $dry ]]

echo '==> install refuses root spellings and root-pointing directories'
if "$INSTALL" --dry-run --version "$version" --dir /tmp/.. \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS" \
    2>/dev/null; then
    echo 'installer accepted a spelling of the filesystem root' >&2
    exit 1
fi
ln -s / "$scratch/root-link"
if "$INSTALL" --dry-run --version "$version" --dir "$scratch/root-link" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS" \
    2>/dev/null; then
    echo 'installer accepted a directory pointing at the filesystem root' >&2
    exit 1
fi

echo '==> checksum mismatch is refused'
bad_sum=$scratch/bad-sum
cp -R "$asset" "$bad_sum"
printf '%064d  %s.tar.gz\n' 0 "$stem" >"$bad_sum/SHA256SUMS"
if install_from "$bad_sum" "$scratch/checksum-bin" 2>/dev/null; then
    echo 'installer accepted a mismatched checksum' >&2
    exit 1
fi

echo '==> an archive directory cannot be a symbolic link'
symlink_dir=$scratch/symlink-dir
mkdir -p "$symlink_dir/payload"
ln -s payload "$symlink_dir/$stem"
tar -czf "$symlink_dir/$stem.tar.gz" -C "$symlink_dir" "$stem"
(cd "$symlink_dir" && checksum "$stem.tar.gz") >"$symlink_dir/SHA256SUMS"
if problem=$(install_from "$symlink_dir" "$scratch/symlink-bin" 2>&1); then
    echo 'installer accepted a symbolic-link archive directory' >&2
    exit 1
fi
[[ $problem == *'is not a directory'* ]] || {
    printf 'installer rejected the symbolic link for the wrong reason: %s\n' "$problem" >&2
    exit 1
}

echo '==> an archive binary cannot be a hard link'
hardlink=$scratch/hardlink
mkdir -p "$hardlink/$stem"
printf 'same inode\n' >"$hardlink/$stem/README.md"
ln "$hardlink/$stem/README.md" "$hardlink/$stem/crucible"
tar -czf "$hardlink/$stem.tar.gz" -C "$hardlink" \
    "$stem" "$stem/README.md" "$stem/crucible"
(cd "$hardlink" && checksum "$stem.tar.gz") >"$hardlink/SHA256SUMS"
if problem=$(install_from "$hardlink" "$scratch/hardlink-bin" 2>&1); then
    echo 'installer accepted a hard-link archive binary' >&2
    exit 1
fi
[[ $problem == *'is not a regular file'* ]] || {
    printf 'installer rejected the hard link for the wrong reason: %s\n' "$problem" >&2
    exit 1
}

echo '==> a failed replacement restores the installed binary'
bad_binary=$scratch/bad-binary
release "$bad_binary" 'crucible wrong'
if install_from "$bad_binary" "$destination" 2>/dev/null; then
    echo 'installer accepted a binary reporting the wrong version' >&2
    exit 1
fi
[[ $($destination/crucible --version) == "crucible $version" ]]
grep -q "crucible $version" "$destination/crucible-sandbox-broker" || {
    echo 'a failed replacement left the wrong sandbox broker installed' >&2
    exit 1
}

echo '==> an unrelated alias is never overwritten'
foreign=$scratch/foreign
mkdir -p "$foreign"
printf 'mine\n' >"$foreign/cru"
if install_from "$asset" "$foreign" 2>/dev/null; then
    echo 'installer overwrote an unrelated alias' >&2
    exit 1
fi
[[ $(cat "$foreign/cru") == mine && ! -e $foreign/crucible ]]

echo '==> a non-regular executable path is never replaced'
occupied=$scratch/occupied
mkdir -p "$occupied/crucible"
printf 'kept\n' >"$occupied/crucible/sentinel"
if install_from "$asset" "$occupied" 2>/dev/null; then
    echo 'installer replaced a non-regular executable path' >&2
    exit 1
fi
[[ $(cat "$occupied/crucible/sentinel") == kept ]]

echo '==> a non-regular sandbox broker path is never replaced'
occupied_broker=$scratch/occupied-broker
mkdir -p "$occupied_broker/crucible-sandbox-broker"
if install_from "$asset" "$occupied_broker" 2>/dev/null; then
    echo 'installer replaced a non-regular sandbox broker path' >&2
    exit 1
fi
[[ -d $occupied_broker/crucible-sandbox-broker && ! -e $occupied_broker/crucible ]]

echo '==> a group-writable installation directory is reported as untrusted'
loose=$scratch/loose
mkdir -p "$loose"
chmod g+w "$loose"
warned=$(install_from "$asset" "$loose" 2>&1 >/dev/null)
[[ $warned == *"$loose is writable by group or others"* && $warned == *'chmod go-w'* ]] || {
    printf 'installer did not warn about a group-writable directory: %s\n' "$warned" >&2
    exit 1
}
[[ -x $loose/crucible-sandbox-broker ]]

echo '==> uninstall preserves data by default'
data=$scratch/home/.crucible
mkdir -p "$data"
printf 'secret\n' >"$data/auth.json"
CRUCIBLE_CODE_HOME=$data "$UNINSTALL" --dir "$destination" >/dev/null
[[ ! -e $destination/crucible && ! -e $destination/cru && -e $data/auth.json ]]
[[ ! -e $destination/crucible-sandbox-broker ]]

echo '==> purge is explicit and confirmed'
if CRUCIBLE_CODE_HOME=$data "$UNINSTALL" --dir "$destination" --purge 2>/dev/null; then
    echo 'uninstaller purged data without confirmation' >&2
    exit 1
fi
CRUCIBLE_CODE_HOME=$data "$UNINSTALL" --dir "$destination" --purge --yes >/dev/null
[[ ! -e $data ]]

echo '==> purge refuses a path that resolves to the filesystem root'
install_from "$asset" "$destination"
if CRUCIBLE_CODE_HOME=/tmp/.. "$UNINSTALL" --dir "$destination" \
    --purge --yes 2>/dev/null; then
    echo 'uninstaller accepted a spelling of the filesystem root' >&2
    exit 1
fi
[[ -x $destination/crucible ]]

echo '==> purge refuses a symbolic-link data directory'
mkdir -p "$scratch/kept"
ln -s "$scratch/kept" "$scratch/data-link"
if CRUCIBLE_CODE_HOME=$scratch/data-link "$UNINSTALL" --dir "$destination" \
    --purge --yes 2>/dev/null; then
    echo 'uninstaller accepted a symbolic-link data directory' >&2
    exit 1
fi
[[ -d $scratch/kept ]]

echo '==> uninstall validates the executable before removing its alias'
guarded=$scratch/guarded
mkdir -p "$guarded/crucible"
ln -s crucible "$guarded/cru"
if "$UNINSTALL" --dir "$guarded" 2>/dev/null; then
    echo 'uninstaller accepted a non-regular executable path' >&2
    exit 1
fi
[[ -d $guarded/crucible && -L $guarded/cru ]]

echo 'installer tests passed'
