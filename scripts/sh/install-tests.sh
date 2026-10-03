#!/usr/bin/env bash
# Offline behavioral tests for the release installer and uninstaller.
set -euo pipefail

cd "$(dirname "$0")/../.."
readonly INSTALL=$PWD/scripts/sh/install.sh
readonly UNINSTALL=$PWD/scripts/sh/uninstall.sh
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
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

echo '==> a version is ASCII digits and letters under a UTF-8 locale too'
# Bracket ranges follow the locale: under en_US.UTF-8, [0-9] takes an
# Arabic-Indic digit and [A-Za-z] takes accented and other letters.
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
    [[ -x $foreign_owner/crucible-sandbox-broker ]]
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
    ttyless=$(in_terminal 80 setsid -w env TERM=xterm LC_ALL=C \
        CRUCIBLE_CODE_HOME="$scratch/ttyless-home" "$UNINSTALL" --dir "$ttyless_bin")
    refuse 'uninstall with no controlling terminal' "$ttyless" '/dev/tty'
    expect 'uninstall with no controlling terminal' "$(visible "$ttyless")" 'ok remove'
    expect 'uninstall with no controlling terminal' "$ttyless" 'status=0'
else
    echo '    skipped: this host has no setsid to start a session without a terminal'
fi

echo '==> uninstall marks its steps in a terminal and stays plain when piped'
look_bin=$scratch/look-bin
install_from "$asset" "$look_bin" >/dev/null
removed=$(in_terminal 80 env TERM=xterm LC_ALL=C CRUCIBLE_CODE_HOME="$scratch/look-home" \
    "$UNINSTALL" --dir "$look_bin")
expect 'uninstall in a terminal' "$removed" "$ESC["
expect 'uninstall in a terminal' "$(visible "$removed")" 'ok remove'
expect 'uninstall in a terminal' "$removed" 'crucible is uninstalled.'
expect 'uninstall in a terminal' "$removed" 'status=0'
[[ ! -e $look_bin/crucible ]]
install_from "$asset" "$look_bin" >/dev/null
removed=$(CRUCIBLE_CODE_HOME=$scratch/look-home "$UNINSTALL" --dir "$look_bin" 2>&1)
refuse 'piped uninstall' "$removed" "$ESC"
expect 'piped uninstall' "$removed" 'crucible is uninstalled.'

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
cat >"$discovery_tools/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail
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

echo 'installer tests passed'
