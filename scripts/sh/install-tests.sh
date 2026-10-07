#!/usr/bin/env bash
# Offline behavioral tests for the release installer and uninstaller.
set -euo pipefail

cd "$(dirname "$0")/../.."
readonly INSTALL=$PWD/scripts/sh/install.sh
readonly UNINSTALL=$PWD/scripts/sh/uninstall.sh
readonly RECEIPT=$PWD/tests/fixtures/installer/receipt.sh
# shellcheck source=../../tests/fixtures/installer/receipt.sh
. "$RECEIPT"
scratch=$(mktemp -d)
# A case still running beside the others writes in here, so it is waited for
# before the directory goes, whichever way the script ends.
beside_pids=()
trap 'for p in ${beside_pids[@]+"${beside_pids[@]}"}; do wait "$p" 2>/dev/null || :; done
rm -rf -- "$scratch"' EXIT
# The installer prints the directory as it resolves it, and the temporary
# directory can sit behind a symbolic link (/var is /private/var on macOS), so
# every expected directory is spelled the same way.
scratch=$(cd -P -- "$scratch" && pwd -P)
readonly scratch

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

# The SHA-256 of a file alone, as a receipt records it.
sum_of() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{ print $1 }'
    elif command -v shasum >/dev/null; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    else
        sha256 -q "$1"
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

# The release `<dir>/.crucible-install/current` names, and the unit it is.
active_release() {
    local link
    link=$(readlink "$1/.crucible-install/current") || return 1
    printf '%s\n' "${link#releases/}"
}
active_unit() {
    printf '%s\n' "$1/.crucible-install/releases/$(active_release "$1")"
}

# Whether `<dir>` holds this installer's layout with `release` active: the two
# links, the active link, and a unit whose receipt describes it and accounts
# for every file in it.
assert_layout() {
    local dir=$1 release=$2 with_broker=${3:-broker} unit expected
    unit=$dir/.crucible-install/releases/$release
    [[ -L $dir/crucible && $(readlink "$dir/crucible") == .crucible-install/current/crucible ]]
    [[ -L $dir/cru && $(readlink "$dir/cru") == crucible ]]
    [[ $(active_release "$dir") == "$release" ]]
    [[ -d $unit && ! -L $unit && -f $unit/crucible && ! -L $unit/crucible ]]
    LC_ALL=C crucible_receipt_read "$unit/receipt"
    [[ $receipt_prefix == "$dir/.crucible-install" && $receipt_version == "$release" ]]
    [[ $receipt_target == "$platform-$architecture" ]]
    [[ $receipt_crucible == "$(sum_of "$unit/crucible")" ]]
    expected=$'crucible\nreceipt'
    if [[ $with_broker == broker ]]; then
        expected=$'crucible\ncrucible-sandbox-broker\nreceipt'
        [[ $receipt_broker == "$(sum_of "$unit/crucible-sandbox-broker")" ]]
    else
        [[ -z $receipt_broker ]]
    fi
    [[ $(cd "$unit" && LC_ALL=C ls -A) == "$expected" ]]
}

# Whether an install left anything of its own behind: a staged release, a link
# on its way to replacing `current`, or the lock.
assert_no_leftovers() {
    local prefix=$1/.crucible-install left
    for left in "$prefix"/releases/.incoming.* "$prefix"/.current.* "$prefix/lock"; do
        if [[ -e $left || -L $left ]]; then
            printf 'an install left %s behind\n' "$left" >&2
            return 1
        fi
    done
}

# A release at another version, and the install of it: `release` and
# `install_from` read the version from the shell they run in.
release_version() {
    local version=$1 stem=crucible-$1-$platform-$architecture
    shift
    release "$@"
}
install_version() {
    local version=$1 stem=crucible-$1-$platform-$architecture
    shift
    install_from "$@"
}

# The permission bits of a path, in octal.
mode_of() {
    stat -c '%a' -- "$1" 2>/dev/null || stat -f '%Lp' -- "$1"
}

# A 0.45 install, made by hand: the executable and its broker as regular files
# in the directory, and `cru` beside them. The uninstaller removes this layout
# as well as the one with links into `.crucible-install`.
flat_install() {
    local dir=$1
    mkdir -p "$dir"
    cp -- "$asset/$stem/crucible" "$dir/crucible"
    cp -- "$asset/$stem/crucible-sandbox-broker" "$dir/crucible-sandbox-broker"
    chmod 755 "$dir/crucible" "$dir/crucible-sandbox-broker"
    ln -sfn crucible "$dir/cru"
}

# The installers of 0.43.0 and 0.45.3 as they shipped, which made the flat
# layout every release from 0.43.0 to 0.45.3 shares.
readonly FLAT_INSTALLERS=$PWD/tests/fixtures/installer

# A release of that layout whose executable can be kept running. Given `--hold
# READY GO` it writes a line to READY, waits for one on GO, and then finds its
# broker as crucible does, beside the path it was started as, and prints the
# release that broker names and the status it exits with.
legacy_release() {
    local at=$1 old=$2 stem=crucible-$2-$platform-$architecture
    release_version "$old" "$at" "crucible $old"
    cat >"$at/$stem/crucible" <<HOLD
#!/usr/bin/env sh
if [ "\$1" = --hold ]; then
    printf 'ready\n' >"\$2"
    read -r _ <"\$3"
    broker=\${0%/*}/crucible-sandbox-broker
    sed -n 2p "\$broker"
    "\$broker"
    printf '%s\n' "\$?"
    exit 0
fi
printf '%s\n' 'crucible $old'
HOLD
    chmod +x "$at/$stem/crucible"
    tar -czf "$at/$stem.tar.gz" -C "$at" "$stem"
    (cd "$at" && checksum "$stem.tar.gz") >"$at/SHA256SUMS"
}

# What a user keeps in their home: configuration, credentials, a session and
# its write-ahead log, and a cache. Nothing in either script may change it.
user_data() {
    local data=$1/.crucible
    mkdir -p "$data/sessions" "$data/cache"
    printf 'model = "fixture"\n' >"$data/config.toml"
    printf '{"fixture": "no credential"}\n' >"$data/auth.json"
    printf '{"line": 1}\n' >"$data/sessions/fixture.jsonl"
    printf 'journal\n' >"$data/sessions/transaction.wal"
    printf 'cached\n' >"$data/cache/fixture"
}

mtime_of() {
    stat -c '%Y' -- "$1" 2>/dev/null || stat -f '%m' -- "$1"
}

# Every path under a directory with its mode, modification time and, for a
# file, the SHA-256 of what it holds, so that a change to any of them shows.
snapshot() {
    local path
    (cd "$1" && find . | LC_ALL=C sort) | while IFS= read -r path; do
        printf '%s %s %s' "$path" "$(mode_of "$1/$path")" "$(mtime_of "$1/$path")"
        [[ ! -f $1/$path ]] || printf ' %s' "$(sum_of "$1/$path")"
        printf '\n'
    done
}

# Runs a command as a user whose home is `$1`, with no data directory of its own
# named in the environment.
as_user() {
    local home=$1
    shift
    env -u CRUCIBLE_CODE_HOME HOME="$home" "$@"
}

# Runs the installer expecting a refusal that says `reason`.
refused() {
    local label=$1 reason=$2 problem
    shift 2
    if problem=$("$@" 2>&1); then
        printf 'installer accepted %s\n' "$label" >&2
        return 1
    fi
    [[ $problem == *"$reason"* ]] || {
        printf 'installer refused %s for the wrong reason: %s\n' "$label" "$problem" >&2
        return 1
    }
}

echo '==> verified local install and idempotent update'
asset=$scratch/good
release "$asset"
destination=$scratch/bin
install_from "$asset" "$destination"
[[ $($destination/crucible --version) == "crucible $version" ]]
[[ $($destination/cru --version) == "crucible $version" ]]
assert_layout "$destination" "$version"
active_broker=$destination/.crucible-install/current/crucible-sandbox-broker
[[ -x $active_broker ]]
[[ $(broker_exit "$active_broker") == 125 ]]
first_installation=$receipt_installation
install_from "$asset" "$destination"
assert_layout "$destination" "$version"
[[ $receipt_installation == "$first_installation" ]]
[[ $(cd "$destination/.crucible-install" && LC_ALL=C ls -A) == $'current\nreleases' ]]
[[ $(cd "$destination/.crucible-install/releases" && LC_ALL=C ls -A) == "$version" ]]

# Three cases spend most of their time waiting rather than working: one sits
# out the minute the installer waits on a held lock, and the crash probes run an
# install once for every point it could be killed at. Each keeps to a scratch
# directory of its own, so they run beside the cases that follow, from here on,
# and `join_beside` prints each one's output whole at the end, in the order they
# were started; a case that failed fails the run there.
beside_names=()
beside() {
    beside_names+=("$1")
    # The case is no place for this script's own exit trap, whatever a shell
    # passes to a background process.
    (trap - EXIT; "$1") >"$scratch/beside-$1.out" 2>&1 &
    beside_pids+=("$!")
}
join_beside() {
    local i status failed=0
    for ((i = 0; i < ${#beside_names[@]}; i++)); do
        status=0
        wait "${beside_pids[i]}" || status=$?
        cat "$scratch/beside-${beside_names[i]}.out"
        ((status == 0)) || {
            printf 'the case run as %s failed with status %s\n' "${beside_names[i]}" "$status" >&2
            failed=1
        }
    done
    beside_pids=()
    ((failed == 0)) || exit 1
}

lock_of_no_install() {
    local reused slow_tools bystander status started waited
    echo '==> a lock held by a process that is no install ends with a way out'
    # A pid that a stopped install left in the lock may now belong to any
    # process, so the wait ends, as long as any wait does, and the message says
    # what to do. Each look at the lock is slowed here as a loaded runner slows
    # it, and the wait still ends after the minute it promises rather than after
    # a count of looks.
    reused=$scratch/reused
    install_from "$asset" "$reused" >/dev/null
    slow_tools=$scratch/slow-tools
    mkdir -p "$slow_tools"
    cat >"$slow_tools/readlink" <<SLOW
#!/usr/bin/env bash
sleep 0.2
exec $(command -v readlink) "\$@"
SLOW
    chmod +x "$slow_tools/readlink"
    # The bystander outlives the longest wait, so it is never found gone instead.
    sleep 600 &
    bystander=$!
    status=0
    ln -s "$bystander@$(uname -n)" "$reused/.crucible-install/lock"
    started=$SECONDS
    PATH="$slow_tools:$PATH" refused 'a lock named for a process that is no install' \
        "still holds $reused/.crucible-install/lock after a minute" install_from "$asset" "$reused" ||
        status=$?
    waited=$((SECONDS - started))
    kill "$bystander" 2>/dev/null || :
    wait "$bystander" 2>/dev/null || :
    ((status == 0)) || exit 1
    ((waited <= 75)) || {
        printf 'installer waited %ss on a held lock, not a minute\n' "$waited" >&2
        exit 1
    }
    [[ -L $reused/.crucible-install/lock ]]
    rm -f -- "$reused/.crucible-install/lock"
    install_from "$asset" "$reused" >/dev/null
    assert_layout "$reused" "$version"
}

crash_probes() {
    echo '==> an interrupted install never leaves a broken release active'
    tests/fixtures/installer/crash-probes.sh "$INSTALL"
}

migration_crash_probes() {
    echo '==> an interrupted migration of a flat install never leaves a broken pair in use'
    tests/fixtures/installer/migration-crash-probes.sh "$INSTALL" "$FLAT_INSTALLERS/install-0.45.3.sh"
}

beside lock_of_no_install
beside crash_probes
beside migration_crash_probes

echo '==> an archive without a sandbox broker still installs the executable'
brokerless=$scratch/brokerless
release "$brokerless" "crucible $version" none
brokerless_bin=$scratch/brokerless-bin
install_from "$brokerless" "$brokerless_bin"
[[ $($brokerless_bin/crucible --version) == "crucible $version" ]]
assert_layout "$brokerless_bin" "$version" none
[[ ! -e $brokerless_bin/crucible-sandbox-broker ]]

echo '==> dry run makes no destination and names the release directory'
dry=$scratch/dry
said=$("$INSTALL" --dry-run --version "$version" --dir "$dry" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
[[ ! -e $dry ]]
[[ $said == *"$dry/.crucible-install/releases/$version"* ]] || {
    printf 'dry run did not name the release directory: %s\n' "$said" >&2
    exit 1
}
before_dry=$(cd "$destination" && find . | LC_ALL=C sort)
"$INSTALL" --dry-run --version "$version" --dir "$destination" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS" >/dev/null
[[ $(cd "$destination" && find . | LC_ALL=C sort) == "$before_dry" ]]

echo '==> a version with a suffix or a leading zero is refused'
for odd in 9.8.7-rc.1 09.8.7 9.8 9.8.7.1; do
    refused "the version $odd" 'invalid version' "$INSTALL" --dry-run --version "$odd" \
        --dir "$scratch/odd-version-bin" \
        --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS"
done

echo '==> a directory with a control character in its name is refused'
refused 'a tab in the directory' 'the installation directory is unsafe' \
    install_from "$asset" "$scratch/tab$(printf '\t')bin"
[[ ! -e "$scratch/tab$(printf '\t')bin" ]]

echo '==> a directory with spaces in its name installs'
spaced="$scratch/with spaces/bin"
install_from "$asset" "$spaced" >/dev/null
assert_layout "$spaced" "$version"
[[ $("$spaced/cru" --version) == "crucible $version" ]]

echo '==> a release is installed readable by everyone under a loose umask'
umasked=$scratch/umasked
(umask 002 && install_from "$asset" "$umasked" >/dev/null)
umasked_unit=$umasked/.crucible-install/releases/$version
for path in "$umasked/.crucible-install" "$umasked/.crucible-install/releases" "$umasked_unit" \
    "$umasked_unit/crucible" "$umasked_unit/crucible-sandbox-broker"; do
    [[ $(mode_of "$path") == 755 ]] || {
        printf '%s has mode %s, not 755\n' "$path" "$(mode_of "$path")" >&2
        exit 1
    }
done
[[ $(mode_of "$umasked_unit/receipt") == 644 ]]

# Whether a script carries the receipt reader the tests hold crucible to.
carries_reader() {
    local carrier=$1 embedded=$scratch/embedded-reader.sh readers
    readers=(crucible_receipt_read crucible_receipt_value crucible_receipt_hex
        crucible_receipt_number crucible_receipt_refuse)
    sed -n '/^# --- receipt reader: begin ---$/,/^# --- receipt reader: end ---$/p' \
        "$carrier" >"$embedded"
    [[ -s $embedded ]] || { printf '%s carries no receipt reader\n' "${carrier##*/}" >&2; exit 1; }
    [[ $(bash -c '. "$1"; shift; declare -f "$@"' _ "$embedded" "${readers[@]}") == \
        "$(bash -c '. "$1"; shift; declare -f "$@"' _ "$RECEIPT" "${readers[@]}")" ]] || {
        printf 'the receipt reader in %s differs from tests/fixtures/installer/receipt.sh\n' \
            "${carrier##*/}" >&2
        exit 1
    }
}

echo '==> the installer carries the receipt reader the tests hold crucible to'
carries_reader "$INSTALL"

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

echo '==> a version is three ASCII numbers under a UTF-8 locale too'
# Bracket ranges follow the locale: under en_US.UTF-8, [0-9] takes an
# Arabic-Indic digit and other characters besides.
utf8=$(locale -a 2>/dev/null | grep -ixE 'en_US\.utf-?8' | head -n 1) || true
if [[ -n $utf8 ]]; then
    for odd in "9.8.7-$(printf '\303\251')" "$(printf '\331\241').8.7" \
        "9.8.7-$(printf '\302\262')" "9.8.7-rc$(printf '\304\261')"; do
        if problem=$(LC_ALL=$utf8 "$INSTALL" --dry-run --version "$odd" \
            --dir "$scratch/odd-version-bin" \
            --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS" 2>&1); then
            printf 'installer accepted the version %s under %s\n' "$odd" "$utf8" >&2
            exit 1
        fi
        [[ $problem == *'invalid version'* ]] || {
            printf 'installer accepted the version %s under %s: %s\n' "$odd" "$utf8" "$problem" >&2
            exit 1
        }
    done
else
    echo '    skipped: this host has no en_US UTF-8 locale'
fi

echo '==> checksum mismatch is refused'
bad_sum=$scratch/bad-sum
cp -R "$asset" "$bad_sum"
printf '%064d  %s.tar.gz\n' 0 "$stem" >"$bad_sum/SHA256SUMS"
if install_from "$bad_sum" "$scratch/checksum-bin" 2>/dev/null; then
    echo 'installer accepted a mismatched checksum' >&2
    exit 1
fi

echo '==> a local archive is unpacked from the bytes that were hashed'
# The fake hasher reports the honest sum, then swaps the archive on disk for
# another release, as a writer racing the installer would.
swap_tools=$scratch/swap-tools
swapped=$scratch/swapped
mkdir -p "$swap_tools"
release "$swapped" "crucible $version" broker
printf '# swapped in after hashing\n' >>"$swapped/$stem/crucible-sandbox-broker"
tar -czf "$swapped/$stem.tar.gz" -C "$swapped" "$stem"
racing=$scratch/racing
cp -R "$asset" "$racing"
real_sum=$(command -v sha256sum || command -v shasum || command -v sha256)
cat >"$swap_tools/sha256sum" <<SUM
#!/usr/bin/env bash
set -euo pipefail
case \$(basename "$real_sum") in
sha256sum) "$real_sum" "\$1" ;;
shasum) "$real_sum" -a 256 "\$1" ;;
*) printf '%s  %s\n' "\$("$real_sum" -q "\$1")" "\$1" ;;
esac
cp -- "$swapped/$stem.tar.gz" "$racing/$stem.tar.gz"
SUM
chmod +x "$swap_tools/sha256sum"
PATH="$swap_tools:$PATH" install_from "$racing" "$scratch/racing-bin" >/dev/null 2>&1 || true
if grep -q 'swapped in after hashing' \
    "$scratch/racing-bin/.crucible-install/current/crucible-sandbox-broker" 2>/dev/null; then
    echo 'installer unpacked an archive other than the one it hashed' >&2
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

echo '==> a failed update keeps the active release'
bad_binary=$scratch/bad-binary
release_version 9.8.8 "$bad_binary" 'crucible wrong'
refused 'a binary reporting the wrong version' 'installed binary reported' \
    install_version 9.8.8 "$bad_binary" "$destination"
[[ $($destination/crucible --version) == "crucible $version" ]]
assert_layout "$destination" "$version"
grep -q "crucible $version" "$destination/.crucible-install/current/crucible-sandbox-broker" || {
    echo 'a failed update left the wrong sandbox broker active' >&2
    exit 1
}
[[ ! -e $destination/.crucible-install/releases/9.8.8 ]]
assert_no_leftovers "$destination"

echo '==> an update keeps the release before it and the same installation'
later=$scratch/later
release_version 9.8.9 "$later" 'crucible 9.8.9'
install_version 9.8.9 "$later" "$destination" >/dev/null
assert_layout "$destination" 9.8.9
[[ $receipt_installation == "$first_installation" ]]
[[ $($destination/cru --version) == 'crucible 9.8.9' ]]
[[ -f $destination/.crucible-install/releases/$version/receipt ]]
assert_no_leftovers "$destination"
install_from "$asset" "$destination" >/dev/null
assert_layout "$destination" "$version"
[[ $receipt_installation == "$first_installation" ]]

echo '==> an unrelated alias is never overwritten'
foreign=$scratch/foreign
mkdir -p "$foreign"
printf 'mine\n' >"$foreign/cru"
if install_from "$asset" "$foreign" 2>/dev/null; then
    echo 'installer overwrote an unrelated alias' >&2
    exit 1
fi
[[ $(cat "$foreign/cru") == mine && ! -e $foreign/crucible && ! -e $foreign/.crucible-install ]]

echo '==> a non-regular executable path is never replaced'
occupied=$scratch/occupied
mkdir -p "$occupied/crucible"
printf 'kept\n' >"$occupied/crucible/sentinel"
if install_from "$asset" "$occupied" 2>/dev/null; then
    echo 'installer replaced a non-regular executable path' >&2
    exit 1
fi
[[ $(cat "$occupied/crucible/sentinel") == kept ]]

# A crucible started from a flat install runs on while the install over it
# renames the link over `crucible`, and looks for its broker in the directory of
# the path it was started as: on Linux the kernel still names the replaced
# file's path, and on macOS crucible reads the path it was started as. The
# broker it finds there must still be its own.
for old in 0.43.0 0.45.3; do
    echo "==> a flat $old install migrates while a process started from it keeps its broker"
    legacy=$scratch/legacy-$old
    legacy_release "$legacy" "$old"
    old_stem=crucible-$old-$platform-$architecture
    flat=$scratch/flat-$old
    home=$scratch/home-$old
    user_data "$home"
    kept_data=$(snapshot "$home")
    as_user "$home" "$FLAT_INSTALLERS/install-$old.sh" --version "$old" --dir "$flat" \
        --archive "$legacy/$old_stem.tar.gz" --checksums "$legacy/SHA256SUMS" \
        </dev/null >/dev/null 2>&1
    [[ -f $flat/crucible && ! -L $flat/crucible && -L $flat/cru && ! -e $flat/.crucible-install ]]
    [[ $(sed -n 2p "$flat/crucible-sandbox-broker") == "# crucible $old" ]]
    mkfifo "$scratch/ready-$old" "$scratch/go-$old"
    as_user "$home" "$flat/crucible" --hold "$scratch/ready-$old" "$scratch/go-$old" \
        >"$scratch/held-$old" &
    held=$!
    read -r _ <"$scratch/ready-$old"
    migration=0
    as_user "$home" "$INSTALL" --version "$version" --dir "$flat" \
        --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS" \
        </dev/null >"$scratch/migrated-$old" 2>&1 || migration=$?
    printf 'go\n' >"$scratch/go-$old"
    wait "$held"
    if ((migration != 0)); then
        printf 'the install over a flat %s install failed: %s\n' \
            "$old" "$(tr '\n' ' ' <"$scratch/migrated-$old")" >&2
        exit 1
    fi
    [[ $(cat "$scratch/held-$old") == "# crucible $old"$'\n'125 ]] || {
        printf 'a crucible %s started before the install found the broker %s\n' \
            "$old" "$(tr '\n' ' ' <"$scratch/held-$old")" >&2
        exit 1
    }
    assert_layout "$flat" "$version"
    assert_no_leftovers "$flat"
    [[ $("$flat/crucible" --version) == "crucible $version" ]]
    [[ $("$flat/cru" --version) == "crucible $version" ]]
    [[ $(sed -n 2p "$flat/crucible-sandbox-broker") == "# crucible $old" ]]
    [[ -z $(cd "$flat" && find . -name '.crucible.link.*') ]]
    grep -qF "Kept $flat/crucible-sandbox-broker for the crucible processes started before this install" \
        "$scratch/migrated-$old" || {
        printf 'the install did not say it kept the flat broker: %s\n' \
            "$(tr '\n' ' ' <"$scratch/migrated-$old")" >&2
        exit 1
    }
    [[ $(snapshot "$home") == "$kept_data" ]] || {
        echo 'the migration changed what the user keeps in their home' >&2
        exit 1
    }
    install_from "$asset" "$flat" >/dev/null
    assert_layout "$flat" "$version"
done

echo '==> a dry run over a flat install says what it would replace and changes nothing'
dry_flat=$scratch/dry-flat
flat_install "$dry_flat"
dry_before=$(snapshot "$dry_flat")
said=$("$INSTALL" --dry-run --version "$version" --dir "$dry_flat" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
[[ $said == *"Would replace $dry_flat/crucible with that link"* ]]
[[ $(snapshot "$dry_flat") == "$dry_before" ]]

echo '==> what is not a flat install is not migrated'
# A flat install is `crucible` as a file with the `cru` link beside it, and its
# broker as a file when it has one. Anything else is refused and left as it is.
odd=$scratch/lone-crucible
mkdir -p "$odd"
cp -- "$asset/$stem/crucible" "$odd/crucible"
refused 'a crucible with no cru beside it' "which is not this installer's link" \
    install_from "$asset" "$odd"
[[ -f $odd/crucible && ! -L $odd/crucible && ! -e $odd/.crucible-install ]]
odd=$scratch/flat-broker-directory
flat_install "$odd"
rm -f -- "$odd/crucible-sandbox-broker"
mkdir -- "$odd/crucible-sandbox-broker"
refused 'a flat install whose broker is a directory' 'which is not a file' \
    install_from "$asset" "$odd"
[[ -f $odd/crucible && ! -L $odd/crucible && -d $odd/crucible-sandbox-broker ]]
[[ ! -e $odd/.crucible-install ]]
odd=$scratch/flat-broker-link
flat_install "$odd"
rm -f -- "$odd/crucible-sandbox-broker"
ln -s -- "$asset/$stem/crucible-sandbox-broker" "$odd/crucible-sandbox-broker"
refused 'a flat install whose broker is a link' 'which is not a file' \
    install_from "$asset" "$odd"
[[ -f $odd/crucible && ! -L $odd/crucible && -L $odd/crucible-sandbox-broker ]]
[[ ! -e $odd/.crucible-install ]]

echo '==> what stands where the layout goes is never replaced'
# Each case puts one thing where the layout expects another, and the install
# must refuse it and leave it as it was.
odd=$scratch/odd-prefix
mkdir -p "$odd"
printf 'kept\n' >"$odd/.crucible-install"
refused 'a file as the layout directory' 'is not a directory' install_from "$asset" "$odd"
[[ $(cat "$odd/.crucible-install") == kept && ! -e $odd/crucible ]]

odd=$scratch/odd-unit-file
mkdir -p "$odd/.crucible-install/releases"
chmod 755 "$odd/.crucible-install" "$odd/.crucible-install/releases"
printf 'kept\n' >"$odd/.crucible-install/releases/$version"
refused 'a file as the release directory' 'which is not a release directory' install_from "$asset" "$odd"
[[ $(cat "$odd/.crucible-install/releases/$version") == kept && ! -e $odd/crucible ]]
assert_no_leftovers "$odd"

odd=$scratch/odd-unit-link
mkdir -p "$odd/.crucible-install/releases" "$scratch/elsewhere"
chmod 755 "$odd/.crucible-install" "$odd/.crucible-install/releases"
ln -s "$scratch/elsewhere" "$odd/.crucible-install/releases/$version"
refused 'a link as the release directory' 'which is not a release directory' install_from "$asset" "$odd"
[[ -L $odd/.crucible-install/releases/$version && -z $(ls -A "$scratch/elsewhere") ]]

odd=$scratch/odd-current
mkdir -p "$odd/.crucible-install/current"
chmod 755 "$odd/.crucible-install"
refused 'a directory as the active link' 'is not a link' install_from "$asset" "$odd"
[[ -d $odd/.crucible-install/current && ! -e $odd/crucible ]]

echo '==> a release directory holding another build is refused and kept'
rebuilt=$scratch/rebuilt
release "$rebuilt"
printf '# another build\n' >>"$rebuilt/$stem/crucible"
tar -czf "$rebuilt/$stem.tar.gz" -C "$rebuilt" "$stem"
(cd "$rebuilt" && checksum "$stem.tar.gz") >"$rebuilt/SHA256SUMS"
kept_sum=$(sum_of "$destination/.crucible-install/releases/$version/crucible")
refused 'another build of the same release' 'holds another build of crucible' \
    install_from "$rebuilt" "$destination"
[[ $(sum_of "$destination/.crucible-install/releases/$version/crucible") == "$kept_sum" ]]
assert_layout "$destination" "$version"
assert_no_leftovers "$destination"

echo '==> a lock left by an install that stopped is refused by name'
stale=$scratch/stale
install_from "$asset" "$stale" >/dev/null
sh -c 'exit 0' &
dead=$!
wait "$dead"
ln -s "$dead@$(uname -n)" "$stale/.crucible-install/lock"
refused 'a stale lock' "left $stale/.crucible-install/lock behind" install_from "$asset" "$stale"
[[ -L $stale/.crucible-install/lock ]]
rm -f -- "$stale/.crucible-install/lock"
install_from "$asset" "$stale" >/dev/null
assert_layout "$stale" "$version"

echo '==> a lock held by a running install is waited on where ps cannot see it'
held=$scratch/held
install_from "$asset" "$held" >/dev/null
sleep 60 &
holder=$!
ln -s "$holder@$(uname -n)" "$held/.crucible-install/lock"
# A ps that finds nothing, as on a system without one.
blind=$scratch/blind-ps
mkdir -p "$blind"
printf '#!/bin/sh\nexit 1\n' >"$blind/ps"
chmod +x "$blind/ps"
(sleep 2 && rm -f -- "$held/.crucible-install/lock") &
releaser=$!
status=0
problem=$(PATH=$blind:$PATH install_from "$asset" "$held" 2>&1 >/dev/null) || status=$?
wait "$releaser"
kill "$holder" 2>/dev/null || :
wait "$holder" 2>/dev/null || :
((status == 0)) || {
    printf 'installer took a live lock for a stale one: %s\n' "$problem" >&2
    exit 1
}
assert_layout "$held" "$version"

echo '==> an archive unpacked below a name with a backslash is still verified'
slashed=$scratch/'back\slash'
mkdir -p "$slashed"
status=0
problem=$(TMPDIR=$slashed install_from "$asset" "$scratch/slashed" 2>&1 >/dev/null) || status=$?
((status == 0)) || {
    printf 'installer could not hash below a backslash: %s\n' "$problem" >&2
    exit 1
}
assert_layout "$scratch/slashed" "$version"
[[ -z $(ls -A "$slashed") ]]

echo '==> a layout directory nobody can write to is refused at once'
# Root writes to it regardless, so the case means something only to a user.
if (($(id -u) != 0)); then
    shut=$scratch/shut
    install_from "$asset" "$shut" >/dev/null
    chmod 555 "$shut/.crucible-install"
    started=$SECONDS
    status=0
    refused 'a read-only layout directory' 'is not writable' install_from "$asset" "$shut" ||
        status=$?
    # Put back first, or the scratch directory could not be removed.
    chmod 755 "$shut/.crucible-install"
    ((status == 0)) || exit 1
    ((SECONDS - started < 10)) || {
        echo 'installer waited on a lock it could never take' >&2
        exit 1
    }
else
    echo '    skipped: root can write to a read-only directory'
fi

echo '==> a group-writable installation directory is reported as untrusted'
loose=$scratch/loose
mkdir -p "$loose"
chmod g+w "$loose"
warned=$(install_from "$asset" "$loose" 2>&1 >/dev/null)
[[ $warned == *"$loose is writable by group or others"* && $warned == *'chmod go-w'* ]] || {
    printf 'installer did not warn about a group-writable directory: %s\n' "$warned" >&2
    exit 1
}
[[ -x $loose/.crucible-install/current/crucible-sandbox-broker ]]

echo '==> a directory owned by another user is reported as untrusted'
# Mapping this user to another id inside a user namespace makes every directory
# root really owns, `/` included, show up as somebody else's.
if unshare -U --map-user=1001 --map-group=1001 true 2>/dev/null; then
    foreign_owner=$scratch/foreign-owner
    warned=$(unshare -U --map-user=1001 --map-group=1001 \
        "$INSTALL" --version "$version" --dir "$foreign_owner" \
        --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS" 2>&1 >/dev/null)
    [[ $warned == *'install: / belongs to another user'* ]] || {
        printf 'installer did not warn about a directory owned by another user: %s\n' "$warned" >&2
        exit 1
    }
    [[ -x $foreign_owner/.crucible-install/current/crucible-sandbox-broker ]]
else
    echo '    skipped: this host cannot map another user id into a user namespace'
fi

# Runs a command with a terminal on standard input and output, `cols` wide, and
# prints what the terminal was sent followed by `status=<exit status>`. The
# status is printed from inside, since not every `script` returns it. A case
# that means a locale sets LC_ALL, which outranks any LC_CTYPE or LANG the
# caller exported.
#
# A case takes seconds. One still running after `terminal_bound` seconds is
# reported as `HANG in_terminal`, with the process tree under `script` (each
# process's state and what it waits in) and what the terminal was sent so far.
# Then its processes are killed, and so is this script, since a caller may hold
# the output inside a second command substitution that would hide a failed
# status. The watchdog is this shell polling, so it leaves nothing behind.
readonly terminal_bound=120
in_terminal() {
    local cols=$1 command sent pid status=0 deadline
    shift
    printf -v command '%q ' "$@"
    command="stty cols $cols rows 24; $command; echo status=\$?"
    sent=$(mktemp "$scratch/terminal.XXXXXX")
    if script --version 2>/dev/null | grep -q util-linux; then
        script -qec "$command" /dev/null </dev/null >"$sent" &
    else
        script -q /dev/null sh -c "$command" </dev/null >"$sent" &
    fi
    pid=$!
    deadline=$((SECONDS + terminal_bound))
    while kill -0 "$pid" 2>/dev/null && ((SECONDS < deadline)); do
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        terminal_hung "$pid" "$sent" "$command"
    fi
    wait "$pid" || status=$?
    cat "$sent"
    rm -f -- "$sent"
    return "$status"
}

# Reports and ends the case `in_terminal` started as `pid`: see above.
terminal_hung() {
    local pid=$1 sent=$2 command=$3 own_group tree groups p
    local where="line ${BASH_LINENO[1]}"
    [[ ${FUNCNAME[2]:-main} == main ]] ||
        where+=" in ${FUNCNAME[2]}, called from line ${BASH_LINENO[2]}"
    # `script` shares this script's process group, which is never signalled.
    own_group=$(ps -o pgid= -p "$$" | tr -d ' ')
    # `script`, everything under it, and every process in their groups but
    # this script's: an orphan left in the terminal's group is still the case's.
    tree=$(ps -A -o pid= -o ppid= -o pgid= | awk -v root="$pid" -v own="$own_group" '
        { parent[$1] = $2; group[$1] = $3 }
        END {
            keep[root] = 1
            do {
                grew = 0
                for (p in parent)
                    if (!(p in keep) && (parent[p] in keep)) { keep[p] = 1; grew = 1 }
            } while (grew)
            for (p in keep) if ((p in group) && group[p] != own) groups[group[p]] = 1
            for (p in group) if (group[p] in groups) keep[p] = 1
            for (p in keep) if (p in group) print p, group[p]
        }')
    groups=$(printf '%s\n' "$tree" | awk -v own="$own_group" '$2 != own { print $2 }' | sort -u)
    {
        printf 'HANG in_terminal at %s: still running after %s seconds:\n    %s\n' \
            "$where" "$terminal_bound" "$command"
        echo 'processes:'
        ps -o pid,ppid,pgid,stat,wchan,etime,command \
            -p "$(printf '%s\n' "$tree" | awk '{ print $1 }' | paste -s -d, -)"
        echo 'the terminal was sent, so far:'
        cat -v "$sent"
        echo
    } >&2
    for p in $groups; do
        kill -KILL -- "-$p" 2>/dev/null || true
    done
    for p in $(printf '%s\n' "$tree" | awk '{ print $1 }'); do
        kill -KILL "$p" 2>/dev/null || true
    done
    wait "$pid" 2>/dev/null || true
    rm -f -- "$sent"
    kill -TERM "$$"
    exit 1
}

# Every check below names what it found, so a failure reads as the screen.
expect() {
    local what=$1 output=$2 pattern=$3
    [[ $output == *"$pattern"* ]] || {
        printf '%s: expected %q in:\n%s\n' "$what" "$pattern" "$output" >&2
        exit 1
    }
}
refuse() {
    local what=$1 output=$2 pattern=$3
    [[ $output != *"$pattern"* ]] || {
        printf '%s: did not expect %q in:\n%s\n' "$what" "$pattern" "$output" >&2
        exit 1
    }
}
readonly ESC=$'\033'

# What a reader sees: the output without its colour and line-clearing codes.
visible() {
    printf '%s' "$1" | sed "s/$ESC\\[[0-9;?]*[A-Za-z]//g"
}

echo '==> piped output names each step and carries no escape sequence'
piped_bin=$scratch/piped-bin
piped=$(install_from "$asset" "$piped_bin" 2>&1)
refuse 'piped install' "$piped" "$ESC"
expect 'piped install' "$piped" "install: crucible $version for $platform-$architecture"
expect 'piped install' "$piped" "install: detect platform: $platform-$architecture"
expect 'piped install' "$piped" 'install: verify checksum: ok'
expect 'piped install' "$piped" 'install: unpack: ok'
expect 'piped install' "$piped" "install: install: $piped_bin"
expect 'piped install' "$piped" "Installed crucible, crucible-sandbox-broker and cru in $piped_bin"
expect 'piped install' "$piped" "Add $piped_bin to PATH to run crucible."

echo '==> NO_COLOR and TERM=dumb print the plain steps in a terminal'
for plain_env in NO_COLOR=1 TERM=dumb; do
    plain=$(in_terminal 80 env TERM=xterm "$plain_env" "$INSTALL" --version "$version" \
        --dir "$scratch/plain-bin-${plain_env%%=*}" \
        --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
    refuse "$plain_env in a terminal" "$plain" "$ESC"
    expect "$plain_env in a terminal" "$plain" 'install: verify checksum: ok'
    expect "$plain_env in a terminal" "$plain" 'status=0'
done

echo '==> a terminal sees each step, and the line that puts the directory on PATH'
# The default directory is under a home of the test's own, so the sentences
# that name it fit 80 columns however long the temporary directory's name is.
for glyphs in en_US.UTF-8 C; do
    shown_home=$scratch/home-$glyphs
    shown=$(in_terminal 80 env -u LC_ALL -u LC_CTYPE LANG=$glyphs TERM=xterm \
        HOME="$shown_home" "$INSTALL" --version "$version" \
        --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
    expect "a $glyphs terminal" "$shown" "crucible $version"
    expect "a $glyphs terminal" "$shown" "$ESC["
    for step in 'detect platform' 'verify checksum' 'unpack' 'install'; do
        if [[ $glyphs == C ]]; then
            expect "a $glyphs terminal" "$(visible "$shown")" "ok $step"
        else
            expect "a $glyphs terminal" "$(visible "$shown")" "✓ $step"
        fi
    done
    refuse "a $glyphs terminal" "$shown" 'install: '
    expect "a $glyphs terminal" "$shown" '~/.local/bin is not on your PATH. Add it with:'
    expect "a $glyphs terminal" "$shown" 'export PATH="$HOME/.local/bin:$PATH"'
    expect "a $glyphs terminal" "$shown" 'Then run: crucible'
    expect "a $glyphs terminal" "$shown" 'status=0'
    [[ -x $shown_home/.local/bin/crucible ]]
done
# A directory outside the home is exported as it was resolved, on one line.
shown_bin=$scratch/shown-bin
shown=$(in_terminal 80 env TERM=xterm "$INSTALL" --version "$version" \
    --dir "$shown_bin" --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
expect 'a directory outside the home' "$shown" "export PATH=\"$shown_bin:\$PATH\""
expect 'a directory outside the home' "$shown" 'status=0'
on_path_home=$scratch/home-on-path
shown=$(in_terminal 80 env TERM=xterm HOME="$on_path_home" \
    PATH="$on_path_home/.local/bin:$PATH" "$INSTALL" --version "$version" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
expect 'a directory on PATH' "$shown" '~/.local/bin is on your PATH.'
refuse 'a directory on PATH' "$shown" 'export PATH='

echo '==> a spinner that cannot act on its stop signal does not hold its step'
# bash runs a trap only once the foreground command returns, so a spinner whose
# frame pause does not return cannot act on the signal that ends it, and a step
# that waited for it would wait as long as the pause. This `sleep` holds the
# spinner's frame pause, and only that one, for longer than the whole step list
# takes; every other wait goes to the real sleep. The pause is held away from
# the terminal, so `script` is not kept open by what the spinner leaves behind.
held_for=120
real_sleep=$(command -v sleep)
held_tools=$scratch/held-tools
mkdir -p "$held_tools"
cat >"$held_tools/sleep" <<SLEEP
#!/usr/bin/env bash
if [[ \${1:-} == 0.1 ]]; then
    : >"$scratch/frame-held"
    exec "$real_sleep" $held_for </dev/null >/dev/null 2>&1
fi
exec "$real_sleep" "\$@"
SLEEP
chmod +x "$held_tools/sleep"
held_bin=$scratch/held-bin
started=$SECONDS
held=$(in_terminal 80 env TERM=xterm LC_ALL=C PATH="$held_tools:$PATH" "$INSTALL" \
    --version "$version" --dir "$held_bin" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
took=$((SECONDS - started))
[[ -e $scratch/frame-held ]] || {
    echo 'no spinner reached its frame pause, so no stop signal was held off' >&2
    exit 1
}
((took < held_for)) || {
    printf 'a held spinner kept the install waiting %s seconds\n' "$took" >&2
    exit 1
}
for step in 'detect platform' 'verify checksum' 'unpack' 'install'; do
    expect 'a held spinner' "$(visible "$held")" "ok $step"
done
refuse 'a held spinner' "$held" 'install.sh: line'
expect 'a held spinner' "$held" 'status=0'
[[ -x $held_bin/crucible ]]

echo '==> a spinner whose frame cannot be written keeps the error off the terminal'
# bash reports a builtin's failed write on the standard error of the shell that
# made it, and the spinner's is the user's terminal: on CI a stop signal that
# interrupted a frame's write put `printf: write error` above the step's row.
# An interrupted write cannot be had on demand, but a refused one can. Here the
# spinner's standard output is open for reading only, so its first frame's
# write fails. A whole install would fail its own first row the same way, so
# the installer's spinner functions are run alone, from `step_begin` to
# `stop_spinner`, in a shell that writes nothing else to either output.
cat >"$scratch/unwritable-spinner.sh" <<'SPINNER'
set -euo pipefail
frames=('|' '/' '-' '\') mark_width=2 fancy=1 spinner= download_to= download_headers= on_exit=:
eval "$(sed -n -e '/^progress() {$/,/^}/p' -e '/^spin() {$/,/^}/p' \
    -e '/^stop_spinner() {$/,/^}/p' -e '/^step_begin() {$/,/^}/p' "$1")"
step_begin unpack
status=0
wait "$spinner" || status=$?
stop_spinner
printf '%s\n' "$status" >"$2"
SPINNER
unwritable_err=$scratch/unwritable-spinner.err
bash "$scratch/unwritable-spinner.sh" "$INSTALL" "$scratch/unwritable-spinner.status" \
    </dev/null 1</dev/null 2>"$unwritable_err"
# The spinner ends on the failed write, which is how its drawing is known to
# have been reached.
[[ $(cat "$scratch/unwritable-spinner.status") == 1 ]] || {
    printf 'an unwritable spinner ended with status %s, not its failed write\n' \
        "$(cat "$scratch/unwritable-spinner.status")" >&2
    exit 1
}
[[ ! -s $unwritable_err ]] || {
    printf 'an unwritable spinner reached the terminal:\n%s\n' "$(cat "$unwritable_err")" >&2
    exit 1
}

echo '==> a checksum mismatch installs nothing and names the step'
mismatch_bin=$scratch/mismatch-bin
status=0
mismatch_out=$(install_from "$bad_sum" "$mismatch_bin" 2>"$scratch/mismatch.err") || status=$?
[[ $status == 1 ]] || {
    printf 'a checksum mismatch exited %s, expected 1\n' "$status" >&2
    exit 1
}
expect 'a piped mismatch' "$mismatch_out" 'install: verify checksum: failed'
[[ $(cat "$scratch/mismatch.err") == 'install: archive checksum does not match SHA256SUMS' ]] || {
    printf 'a piped mismatch changed its error: %s\n' "$(cat "$scratch/mismatch.err")" >&2
    exit 1
}
mismatch=$(in_terminal 80 env TERM=xterm LC_ALL=C "$INSTALL" --version "$version" \
    --dir "$mismatch_bin" \
    --archive "$bad_sum/$stem.tar.gz" --checksums "$bad_sum/SHA256SUMS")
expect 'a mismatch in a terminal' "$(visible "$mismatch")" 'x verify checksum'
expect 'a mismatch in a terminal' "$mismatch" 'archive checksum does not match SHA256SUMS'
expect 'a mismatch in a terminal' "$mismatch" 'Nothing was installed.'
expect 'a mismatch in a terminal' "$mismatch" 'status=1'
[[ ! -e $mismatch_bin ]] || {
    echo 'a checksum mismatch created the installation directory' >&2
    exit 1
}

echo '==> at 40 columns every line fits, with details under their step'
narrow=$(in_terminal 40 env TERM=xterm LC_ALL=C "$INSTALL" --version "$version" \
    --dir "$scratch/a-directory-whose-name-is-too-long-for-the-row" \
    --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
expect 'a narrow terminal' "$narrow" 'status=0'
# Every detail sits under its step, even one that would fit beside it, so the
# list reads as one column.
for step in "detect platform"$'\r\n'"    $platform-$architecture" \
    "verify checksum"$'\r\n'"    SHA256SUMS"; do
    expect 'a narrow terminal' "$(visible "$narrow")" $'\r'"  ok $step"
done
while IFS= read -r row; do
    row=${row%$'\r'}
    row=${row##*$'\r'}
    row=$(visible "$row")
    # The command that puts the directory on PATH is copied whole, so the
    # terminal wraps it rather than the installer breaking it.
    [[ $row != '  export PATH='* ]] || continue
    ((${#row} <= 40)) || {
        printf 'a row is wider than 40 columns (%s): %s\n' "${#row}" "$row" >&2
        exit 1
    }
done <<<"$narrow"

echo '==> a terminal with no controlling terminal behind it prints no shell error'
# A new session has no controlling terminal, so /dev/tty cannot be opened even
# though both outputs are still a terminal.
if command -v setsid >/dev/null; then
    ttyless_bin=$scratch/ttyless-bin
    ttyless=$(in_terminal 80 setsid -w env TERM=xterm LC_ALL=C "$INSTALL" \
        --version "$version" --dir "$ttyless_bin" \
        --archive "$asset/$stem.tar.gz" --checksums "$asset/SHA256SUMS")
    refuse 'install with no controlling terminal' "$ttyless" '/dev/tty'
    expect 'install with no controlling terminal' "$(visible "$ttyless")" 'ok install'
    expect 'install with no controlling terminal' "$ttyless" 'status=0'
    ttyless_flat=$scratch/ttyless-flat
    flat_install "$ttyless_flat"
    ttyless=$(in_terminal 80 setsid -w env TERM=xterm LC_ALL=C \
        CRUCIBLE_CODE_HOME="$scratch/ttyless-home" "$UNINSTALL" --dir "$ttyless_flat")
    refuse 'uninstall with no controlling terminal' "$ttyless" '/dev/tty'
    expect 'uninstall with no controlling terminal' "$(visible "$ttyless")" 'ok remove'
    expect 'uninstall with no controlling terminal' "$ttyless" 'status=0'
else
    echo '    skipped: this host has no setsid to start a session without a terminal'
fi

echo '==> uninstall marks its steps in a terminal and stays plain when piped'
look_bin=$scratch/look-bin
flat_install "$look_bin"
removed=$(in_terminal 80 env TERM=xterm LC_ALL=C CRUCIBLE_CODE_HOME="$scratch/look-home" \
    "$UNINSTALL" --dir "$look_bin")
expect 'uninstall in a terminal' "$removed" "$ESC["
expect 'uninstall in a terminal' "$(visible "$removed")" 'ok remove'
expect 'uninstall in a terminal' "$removed" 'crucible is uninstalled.'
expect 'uninstall in a terminal' "$removed" 'status=0'
[[ ! -e $look_bin/crucible ]]
flat_install "$look_bin"
removed=$(CRUCIBLE_CODE_HOME=$scratch/look-home "$UNINSTALL" --dir "$look_bin" 2>&1)
refuse 'piped uninstall' "$removed" "$ESC"
expect 'piped uninstall' "$removed" 'crucible is uninstalled.'

echo '==> uninstall puts every detail under its step when one does not fit beside it'
# At 50 columns the kept data directory, `~/h`, fits beside its step and the
# list of what was removed does not; the list still reads as one column.
flat_install "$look_bin"
removed=$(in_terminal 50 env TERM=xterm LC_ALL=C HOME="$scratch" CRUCIBLE_CODE_HOME="$scratch/h" \
    "$UNINSTALL" --dir "$look_bin")
expect 'a narrow uninstall' "$removed" 'status=0'
for step in "remove"$'\r\n'"    crucible, crucible-sandbox-broker and cru" \
    "keep"$'\r\n'"    ~/h"; do
    expect 'a narrow uninstall' "$(visible "$removed")" "  ok $step"
done
# At 80 columns both fit, and both stay beside their step.
flat_install "$look_bin"
removed=$(in_terminal 80 env TERM=xterm LC_ALL=C HOME="$scratch" CRUCIBLE_CODE_HOME="$scratch/h" \
    "$UNINSTALL" --dir "$look_bin")
expect 'a wide uninstall' "$(visible "$removed")" "  ok keep                ~/h"
expect 'a wide uninstall' "$(visible "$removed")" "  ok remove              crucible, crucible-sandbox-broker and cru"

echo '==> uninstall preserves data by default'
destination=$scratch/flat-bin
flat_install "$destination"
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
flat_install "$destination"
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

echo '==> uninstall removes a migrated install whole and leaves the user data'
gone=$scratch/flat-0.43.0
home=$scratch/home-0.43.0
kept_data=$(snapshot "$home")
removed=$(in_terminal 80 env -u CRUCIBLE_CODE_HOME TERM=xterm LC_ALL=C HOME="$home" \
    "$UNINSTALL" --dir "$gone")
expect 'a managed uninstall' "$removed" 'status=0'
expect 'a managed uninstall' "$(visible "$removed")" \
    "  ok remove              crucible, crucible-sandbox-broker and cru"
[[ -d $gone && -z $(ls -A "$gone") ]] || {
    printf 'uninstall left %s in the installation directory\n' "$(ls -A "$gone" | tr '\n' ' ')" >&2
    exit 1
}
[[ $(snapshot "$home") == "$kept_data" ]] || {
    echo 'the uninstall changed what the user keeps in their home' >&2
    exit 1
}

echo '==> uninstall removes what the receipts own and keeps everything else'
managed=$scratch/managed
home=$scratch/home-managed
user_data "$home"
kept_data=$(snapshot "$home")
install_from "$asset" "$managed" >/dev/null
install_version 9.8.9 "$later" "$managed" >/dev/null
prefix=$managed/.crucible-install
printf 'mine\n' >"$prefix/releases/9.8.9/notes"
printf 'mine\n' >"$prefix/mine"
printf 'mine\n' >"$managed/other-tool"
removed=$(as_user "$home" "$UNINSTALL" --dir "$managed" 2>&1)
expect 'an uninstall beside unowned files' "$removed" "preserving $prefix/releases/9.8.9/notes"
[[ ! -e $managed/crucible && ! -L $managed/crucible && ! -e $managed/cru && ! -L $managed/cru ]]
[[ ! -e $managed/crucible-sandbox-broker && ! -e $prefix/current && ! -L $prefix/current ]]
[[ ! -e $prefix/releases/$version && ! -e $prefix/lock && ! -L $prefix/lock ]]
[[ $(cd "$prefix/releases/9.8.9" && LC_ALL=C ls -A) == notes ]]
[[ $(cat "$prefix/releases/9.8.9/notes") == mine && $(cat "$prefix/mine") == mine ]]
[[ $(cat "$managed/other-tool") == mine ]]
[[ $(snapshot "$home") == "$kept_data" ]] || {
    echo 'the uninstall changed what the user keeps in their home' >&2
    exit 1
}

echo '==> uninstall keeps an executable its receipt does not describe'
edited=$scratch/edited
install_from "$asset" "$edited" >/dev/null
edited_unit=$edited/.crucible-install/releases/$version
printf '# changed\n' >>"$edited_unit/crucible"
removed=$(as_user "$home" "$UNINSTALL" --dir "$edited" 2>&1)
expect 'an uninstall beside a changed executable' "$removed" "preserving $edited_unit/crucible"
[[ -f $edited_unit/crucible && ! -e $edited_unit/crucible-sandbox-broker ]]
[[ ! -e $edited_unit/receipt && ! -L $edited/crucible ]]

echo '==> a dry run of a managed uninstall names what it would remove and removes nothing'
dry_managed=$scratch/dry-managed
install_from "$asset" "$dry_managed" >/dev/null
dry_before=$(snapshot "$dry_managed")
said=$(as_user "$home" "$UNINSTALL" --dry-run --dir "$dry_managed")
for path in crucible cru .crucible-install/current .crucible-install/releases/$version/crucible \
    .crucible-install/releases/$version/receipt .crucible-install; do
    expect 'a dry-run managed uninstall' $'\n'"$said"$'\n' $'\n'"Would remove $dry_managed/$path"$'\n'
done
[[ $(snapshot "$dry_managed") == "$dry_before" ]]

echo '==> uninstall refuses a layout it cannot trust and removes nothing'
odd=$scratch/foreign-link
mkdir -p "$odd"
ln -s "$asset/$stem/crucible" "$odd/crucible"
ln -s crucible "$odd/cru"
refused 'a crucible that links elsewhere' "which is not this installer's link" \
    as_user "$home" "$UNINSTALL" --dir "$odd"
[[ -L $odd/crucible && -L $odd/cru ]]
odd=$scratch/locked
install_from "$asset" "$odd" >/dev/null
ln -s "1@elsewhere" "$odd/.crucible-install/lock"
refused 'a layout whose lock is held' "$odd/.crucible-install/lock" \
    as_user "$home" "$UNINSTALL" --dir "$odd"
assert_layout "$odd" "$version"
rm -f -- "$odd/.crucible-install/lock"
chmod g+w "$odd/.crucible-install/releases"
refused 'a release directory others can write' 'which group or others can write' \
    as_user "$home" "$UNINSTALL" --dir "$odd"
chmod g-w "$odd/.crucible-install/releases"
assert_layout "$odd" "$version"

echo '==> the uninstaller carries the receipt reader the tests hold crucible to'
carries_reader "$UNINSTALL"

echo '==> hermetic platform and download discovery matrix'
discovery_tools=$scratch/discovery-tools
mkdir -p "$discovery_tools"
cat >"$discovery_tools/uname" <<'UNAME'
#!/usr/bin/env bash
set -euo pipefail
case ${1:-} in
-s) printf '%s\n' "${INSTALL_TEST_SYSTEM:?}" ;;
-m) printf '%s\n' "${INSTALL_TEST_MACHINE:?}" ;;
*) printf 'fake uname: unsupported argument %s\n' "${1:-}" >&2; exit 2 ;;
esac
UNAME
cat >"$discovery_tools/sysctl" <<'SYSCTL'
#!/usr/bin/env bash
set -euo pipefail
[[ $* == '-in sysctl.proc_translated' ]] || {
    printf 'fake sysctl: unsupported arguments %s\n' "$*" >&2
    exit 2
}
printf '%s\n' "${INSTALL_TEST_TRANSLATED:-0}"
SYSCTL
# The fake refuses a call that would read the user's ~/.curlrc: curl reads it
# unless `-q` is the first argument, and it can redirect where curl connects.
cat >"$discovery_tools/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail
[[ ${1:-} == -q ]] || {
    echo 'fake curl: called without -q first, so ~/.curlrc would be read' >&2
    exit 2
}
shift
head=0
output=
headers=
url=
while (($#)); do
    case $1 in
    --head) head=1; shift ;;
    --output) output=${2:?fake curl: --output needs a value}; shift 2 ;;
    --dump-header) headers=${2:?fake curl: --dump-header needs a value}; shift 2 ;;
    --write-out|--proto) shift 2 ;;
    *) url=$1; shift ;;
    esac
done
[[ -n $url ]] || { echo 'fake curl: no URL' >&2; exit 2; }
printf '%s\n' "$url" >>"${INSTALL_TEST_CURL_LOG:?}"
if ((head)); then
    printf 'https://github.com/augments-labs/crucible-code/releases/tag/v%s' \
        "${INSTALL_TEST_VERSION:?}"
    exit 0
fi
[[ -n $output ]] || { echo 'fake curl: no output path' >&2; exit 2; }
if [[ -n ${INSTALL_TEST_CURL_FAIL:-} ]]; then
    echo 'curl: (22) The requested URL returned error: 404' >&2
    exit 22
fi
asset=${INSTALL_TEST_RELEASE:?}/${url##*/}
size=$(wc -c <"$asset" | tr -d ' ')
[[ -z $headers ]] ||
    printf 'HTTP/2 200\r\ncontent-length: %s\r\n\r\n' "${INSTALL_TEST_CURL_LENGTH:-$size}" >"$headers"
if [[ -n ${INSTALL_TEST_CURL_SLOW:-} ]]; then
    # Half the archive, long enough for a terminal to draw the bar at half.
    head -c $((size / 2)) "$asset" >"$output"
    sleep 1
fi
cp "$asset" "$output"
CURL
chmod +x "$discovery_tools/uname" "$discovery_tools/sysctl" "$discovery_tools/curl"

# One minimal, valid release for the platform a discovery case is pretending to
# be. The install remains a dry run: extraction and verification are real, while
# the fixture executable never has to match the kernel running this test.
discovery_release() {
    local at=$1 release_platform=$2 release_architecture=$3
    local release_stem=crucible-$version-$release_platform-$release_architecture
    mkdir -p "$at/$release_stem"
    printf '#!/usr/bin/env sh\nprintf "crucible %s\\n"\n' "$version" \
        >"$at/$release_stem/crucible"
    chmod +x "$at/$release_stem/crucible"
    tar -czf "$at/$release_stem.tar.gz" -C "$at" "$release_stem"
    (cd "$at" && checksum "$release_stem.tar.gz") >"$at/SHA256SUMS"
}

assert_discovery() {
    local label=$1 release_system=$2 release_machine=$3
    local release_platform=$4 release_architecture=$5 translated=${6:-0}
    local releases=$scratch/discovery-$label log=$scratch/discovery-$label.urls
    local release_stem=crucible-$version-$release_platform-$release_architecture
    discovery_release "$releases" "$release_platform" "$release_architecture"
    : >"$log"

    INSTALL_TEST_SYSTEM=$release_system \
        INSTALL_TEST_MACHINE=$release_machine \
        INSTALL_TEST_TRANSLATED=$translated \
        INSTALL_TEST_VERSION=$version \
        INSTALL_TEST_RELEASE=$releases \
        INSTALL_TEST_CURL_LOG=$log \
        PATH="$discovery_tools:$PATH" \
        "$INSTALL" --dry-run --version "v$version" \
        --dir "$scratch/discovery-bin-$label" >/dev/null

    local base=https://github.com/augments-labs/crucible-code/releases/download/v$version
    local expected
    expected=$(printf '%s\n%s\n' \
        "$base/$release_stem.tar.gz" "$base/SHA256SUMS")
    if [[ $(cat "$log") != "$expected" ]]; then
        printf 'installer requested the wrong URLs for %s:\n%s\n' "$label" "$(cat "$log")" >&2
        exit 1
    fi
}

assert_discovery linux-x86-64 Linux x86_64 linux x86_64
assert_discovery linux-arm64 Linux aarch64 linux aarch64
assert_discovery macos-x86-64 Darwin x86_64 macos x86_64
assert_discovery macos-arm64 Darwin arm64 macos aarch64
assert_discovery macos-rosetta Darwin x86_64 macos aarch64 1
assert_discovery freebsd-x86-64 FreeBSD amd64 freebsd x86_64

# With no version argument, the redirect target is the only source of the
# version used by both asset requests.
latest=$scratch/discovery-latest
latest_log=$scratch/discovery-latest.urls
discovery_release "$latest" linux x86_64
: >"$latest_log"
INSTALL_TEST_SYSTEM=Linux \
    INSTALL_TEST_MACHINE=x86_64 \
    INSTALL_TEST_TRANSLATED=0 \
    INSTALL_TEST_VERSION=$version \
    INSTALL_TEST_RELEASE=$latest \
    INSTALL_TEST_CURL_LOG=$latest_log \
    PATH="$discovery_tools:$PATH" \
    "$INSTALL" --dry-run --dir "$scratch/discovery-bin-latest" >/dev/null
latest_base=https://github.com/augments-labs/crucible-code/releases
latest_expected=$(printf '%s\n%s\n%s\n' \
    "$latest_base/latest" \
    "$latest_base/download/v$version/crucible-$version-linux-x86_64.tar.gz" \
    "$latest_base/download/v$version/SHA256SUMS")
if [[ $(cat "$latest_log") != "$latest_expected" ]]; then
    printf 'installer requested the wrong URLs after latest-version discovery:\n%s\n' \
        "$(cat "$latest_log")" >&2
    exit 1
fi

assert_discovery_refused() {
    local label=$1 release_system=$2 release_machine=$3 wanted=$4
    local log=$scratch/discovery-refused-$label.urls problem
    : >"$log"
    if problem=$(INSTALL_TEST_SYSTEM=$release_system \
        INSTALL_TEST_MACHINE=$release_machine \
        INSTALL_TEST_TRANSLATED=0 \
        INSTALL_TEST_VERSION=$version \
        INSTALL_TEST_RELEASE=$scratch \
        INSTALL_TEST_CURL_LOG=$log \
        PATH="$discovery_tools:$PATH" \
        "$INSTALL" --dry-run --version "$version" \
        --dir "$scratch/discovery-refused-bin-$label" 2>&1); then
        printf 'installer accepted unsupported host %s/%s\n' \
            "$release_system" "$release_machine" >&2
        exit 1
    fi
    [[ $problem == *"$wanted"* ]] || {
        printf 'installer rejected %s/%s for the wrong reason: %s\n' \
            "$release_system" "$release_machine" "$problem" >&2
        exit 1
    }
    [[ ! -s $log ]] || {
        printf 'installer downloaded before rejecting %s/%s:\n%s\n' \
            "$release_system" "$release_machine" "$(cat "$log")" >&2
        exit 1
    }
}

assert_discovery_refused operating-system Plan9 x86_64 'unsupported operating system Plan9'
assert_discovery_refused architecture Linux riscv64 'unsupported architecture riscv64'
assert_discovery_refused freebsd-arm64 FreeBSD arm64 \
    'FreeBSD releases are available only for x86-64'

echo '==> a terminal download shows its bar, its size, and why it failed'
download_release=$scratch/download-release
discovery_release "$download_release" linux x86_64
in_download() {
    in_terminal 80 env TERM=xterm LC_ALL=C INSTALL_TEST_SYSTEM=Linux INSTALL_TEST_MACHINE=x86_64 \
        INSTALL_TEST_VERSION="$version" INSTALL_TEST_RELEASE="$download_release" \
        INSTALL_TEST_CURL_LOG="$scratch/download.urls" PATH="$discovery_tools:$PATH" "$@" \
        "$INSTALL" --dry-run --version "$version" --dir "$scratch/download-bin"
}
downloaded=$(visible "$(in_download INSTALL_TEST_CURL_SLOW=1)")
# The fake sends half the archive and pauses, so some frame shows a bar part
# filled and part empty.
expect 'a terminal download' "$downloaded" '#.'
expect 'a terminal download' "$downloaded" '.  0.0 / 0.0 MB'
expect 'a terminal download' "$downloaded" \
    "ok download            crucible-$version-linux-x86_64.tar.gz - 0.0 MB"
expect 'a terminal download' "$downloaded" 'status=0'
# A size that is not a count of bytes draws no bar, and is never evaluated:
# bash would read `inf` as a variable, and a subscript in its value would run.
for length in 1e400 -nan 12abc; do
    hostile=$(in_download INSTALL_TEST_CURL_SLOW=1 INSTALL_TEST_CURL_LENGTH="$length" \
        a=1 inf="a[\$(touch $scratch/length-ran)]" nan="a[\$(touch $scratch/length-ran)]")
    [[ ! -e $scratch/length-ran ]] || {
        echo "a Content-Length of $length ran a command" >&2
        exit 1
    }
    refuse "a Content-Length of $length" "$hostile" 'install.sh: line'
    # The bar's sizes read `got / total MB`.
    [[ ! $(visible "$hostile") =~ [0-9]\ /\ [0-9] ]] || {
        printf 'a Content-Length of %s drew a bar:\n%s\n' "$length" "$hostile" >&2
        exit 1
    }
    expect "a Content-Length of $length" "$hostile" 'status=0'
done
refused=$(visible "$(in_download INSTALL_TEST_CURL_FAIL=1)")
# `x` is right-aligned under `ok`, so the label starts in the same column.
expect 'a failed terminal download' "$refused" \
    $'\r   x download            curl: (22) The requested URL returned error: 404'
expect 'a failed terminal download' "$refused" 'Nothing was installed.'
expect 'a failed terminal download' "$refused" 'status=22'
status=0
refused=$(INSTALL_TEST_SYSTEM=Linux INSTALL_TEST_MACHINE=x86_64 INSTALL_TEST_VERSION=$version \
    INSTALL_TEST_RELEASE=$download_release INSTALL_TEST_CURL_LOG=$scratch/download.urls \
    INSTALL_TEST_CURL_FAIL=1 PATH="$discovery_tools:$PATH" \
    "$INSTALL" --dry-run --version "$version" --dir "$scratch/download-bin" 2>/dev/null) ||
    status=$?
((status == 22)) || { echo "a failed piped download exited $status, not curl's 22" >&2; exit 1; }
expect 'a failed piped download' "$refused" 'install: download: failed'

join_beside

echo 'installer tests passed'
