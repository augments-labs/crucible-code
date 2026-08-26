#!/usr/bin/env bash
# The release gate that cannot run before there is a release: take the artifact
# people will actually download, and run it somewhere that holds nothing else.
#
#     scripts/smoke.sh                     the tag matching the current version
#     scripts/smoke.sh v0.0.1              a published tag
#     scripts/smoke.sh ./crucible.tar.gz   a tarball already on disk
#     scripts/smoke.sh --no-provider v0.0.1  skip the live provider gate
#     scripts/smoke.sh --checksum HEX FILE   verify a local tarball first
#
# The deterministic gates check the source and `scripts/bench.sh` checks speed.
# Both build from this tree, with this machine's toolchain, in this
# working directory — so neither can see the ways a *shipped* binary fails: a
# library only the build machine has, a certificate store the container has not
# got, a first run that needs a home directory nobody created.
#
# The sandbox is the point. It holds the artifact, the dynamic loader and the
# libraries the binary itself names — and nothing else. No shell, no package
# manager, no toolchain, no source tree, no certificate bundle, and a home
# directory that did not exist a moment ago. What this proves is therefore not
# "the libraries it needs are present", which binding them guarantees, but the
# harder half: that nothing *else* on the build machine was holding it up.
set -euo pipefail

command -v dirname >/dev/null || {
    echo 'smoke: dirname is not installed' >&2
    exit 1
}
cd "$(dirname "$0")/.."

readonly REPO=augments-labs/crucible-code

# Which of the published artifacts this gate is about. `bwrap`, `ldd` and the
# glibc floor below are all Linux, so this is the one it can run against — the
# other six are proved by the release build and by CI, not from here.
readonly PLATFORM=linux-x86_64
readonly MAX_GLIBC=2.34

no_provider=0
checksum=
target=

while (($#)); do
    case "$1" in
    --no-provider) no_provider=1 ;;
    --checksum)
        shift
        if (($# == 0)); then
            echo 'smoke: --checksum needs a SHA-256 digest' >&2
            exit 2
        fi
        checksum=$1
        ;;
    --offline)
        echo 'smoke: --offline was ambiguous; use --no-provider with a local file for a network-free run' >&2
        exit 2
        ;;
    -*)
        echo "smoke: unknown option $1" >&2
        exit 2
        ;;
    *)
        if [[ -n $target ]]; then
            echo 'smoke: expected one tag or tarball' >&2
            exit 2
        fi
        target=$1
        ;;
    esac
    shift
done
readonly no_provider checksum

failed=0
echo "==> tools"
for tool in awk basename bwrap grep head ldd mkdir mktemp objdump realpath rm sed sha256sum sort tail tar; do
    command -v "$tool" >/dev/null || {
        printf '    FAIL %s is not installed\n' "$tool"
        failed=1
    }
done
((failed == 0)) || exit 1

# The version is declared in one place, so the default target is derivable
# rather than something to keep in step by hand.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
readonly version
[[ -n $target ]] || target=v$version

work=$(mktemp -d)
readonly work
trap 'rm -rf "$work"' EXIT

echo "==> artifact"
if [[ -f $target ]]; then
    tarball=$(realpath "$target")
    published=0

    # A file names no version, so what this tree holds is the only expectation
    # there is — and it is the right one, since a local tarball is one this tree
    # just packaged.
    expected=$version
else
    if [[ -n $checksum ]]; then
        echo 'smoke: --checksum is for a local tarball; a published tag uses its SHA256SUMS' >&2
        exit 2
    fi
    # An artifact is named for the version it holds, not for the tag it was cut
    # from — the `v` belongs to git.
    name=crucible-${target#v}-$PLATFORM.tar.gz

    # And so is what the binary inside it should say. Reading that off Cargo.toml
    # instead would fail every release but the one this tree is on, which is the
    # ordinary case: the gate is run again from a main that has moved on, and an
    # artifact nobody has touched is reported broken.
    expected=${target#v}
    command -v gh >/dev/null || {
        echo "    FAIL gh is not installed, and $target is not a file"
        exit 1
    }
    # Downloaded rather than built: a local build is the one artifact this gate
    # is not allowed to trust, because it is the thing every other gate already
    # used.
    gh release download "$target" --repo "$REPO" --dir "$work" \
        --pattern "$name" --pattern SHA256SUMS --clobber
    tarball=$work/$name
    published=1
fi
readonly tarball published expected
printf '    %s\n' "$tarball"

echo "==> checksum"
if ((published)); then
    # Select the one downloaded artifact exactly. `--ignore-missing` would also
    # ignore a misspelled name, which is the checksum failure this gate owes.
    mapfile -t matching < <(
        awk -v file="$(basename "$tarball")" '$2 == file { print $1 }' \
            "$(dirname "$tarball")/SHA256SUMS"
    )
    if ((${#matching[@]} != 1)); then
        printf '    FAIL SHA256SUMS has %d lines for %s, expected exactly one\n' \
            "${#matching[@]}" "$(basename "$tarball")"
        exit 1
    fi
    expected_checksum=${matching[0]}
elif [[ -n $checksum ]]; then
    expected_checksum=$checksum
else
    expected_checksum=
    echo '    SKIP local file has no independent checksum; pass --checksum HEX to verify one'
fi

if [[ -n $expected_checksum ]]; then
    if [[ ! $expected_checksum =~ ^[[:xdigit:]]{64}$ ]]; then
        printf '    FAIL expected checksum is not a SHA-256 digest: %q\n' "$expected_checksum"
        exit 1
    fi
    read -r actual_checksum _ < <(sha256sum "$tarball")
    if [[ ${actual_checksum,,} != "${expected_checksum,,}" ]]; then
        printf '    FAIL checksum was %s, expected %s\n' "$actual_checksum" "$expected_checksum"
        exit 1
    fi
    printf '    %s  %s\n' "$actual_checksum" "$(basename "$tarball")"
fi

echo "==> unpack"
root=$work/root
mkdir -p "$root"
tar -xzf "$tarball" -C "$root" --strip-components=1
binary=$root/crucible
readonly root binary
[[ -x $binary ]] || {
    echo "    FAIL the tarball holds no executable named crucible"
    exit 1
}

echo "==> library surface"
# Every absolute path the loader resolves, which is exactly what the sandbox
# will carry. A binary that grows a dependency on something the target machine
# may not have shows up here as a new line, in the diff, before a user finds it.
if ! linked=$(ldd "$binary"); then
    echo '    FAIL ldd could not read the artifact'
    exit 1
fi
mapfile -t libraries < <(printf '%s\n' "$linked" | grep -o '/[^ ]*' | sort -u)
printf '    %s\n' "${libraries[@]}"

# The oldest glibc that can load this binary. Weak symbols are excluded on
# purpose: the loader tolerates their absence, so a weak reference to a very new
# symbol is not a floor and reporting it as one would retire distributions that
# run this fine.
if ! symbols=$(objdump -T "$binary"); then
    echo '    FAIL objdump could not read the artifact'
    exit 1
fi
floor=$(printf '%s\n' "$symbols" | grep UND | grep -v ' w ' |
    grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1) || true
printf '    requires %s or newer\n' "${floor:-no versioned glibc symbols}"
if [[ -n $floor ]]; then
    floor_version=${floor#GLIBC_}
    newest=$(printf '%s\n%s\n' "$MAX_GLIBC" "$floor_version" | sort -uV | tail -1)
    if [[ $newest != "$MAX_GLIBC" ]]; then
        printf '    FAIL promised glibc floor is %s, artifact requires %s\n' \
            "$MAX_GLIBC" "$floor_version"
        failed=1
    fi
fi
((failed == 0)) || exit 1

# bubblewrap is useless without the namespaces it asks the kernel for, and the
# failure otherwise reads as a permission error after the artifact checks. Ask
# immediately before the first sandbox. The host root is read-only so this
# measures the kernel's answer rather than whether `true` can be reached.
if ! bwrap --unshare-all --ro-bind / / /bin/true 2>/dev/null; then
    echo '    FAIL bwrap cannot unshare — unprivileged user namespaces look disabled'
    failed=1
fi
((failed == 0)) || exit 1

binds=()
for library in "${libraries[@]}"; do
    [[ -e $library ]] && binds+=(--ro-bind "$library" "$library")
done

# --clearenv is what makes this a fresh user rather than this one: no API key,
# no XDG_DATA_HOME, no locale, nothing inherited that could be quietly load
# bearing.
sandbox() {
    bwrap --unshare-all --die-with-parent \
        "${binds[@]}" \
        --ro-bind "$root" /opt/crucible \
        --proc /proc --dev /dev --tmpfs /tmp \
        --tmpfs /home/user --chdir /tmp \
        --clearenv --setenv HOME /home/user --setenv PATH /opt/crucible \
        "$@"
}

echo "==> it runs at all"
said=$(sandbox /opt/crucible/crucible --version)
if [[ $said == "crucible $expected" ]]; then
    printf '    %s\n' "$said"
else
    printf '    FAIL --version said %q, expected %q\n' "$said" "crucible $expected"
    failed=1
fi

sandbox /opt/crucible/crucible --help >/dev/null || {
    echo '    FAIL --help did not run'
    failed=1
}

echo "==> a machine with no key"
# The first thing a new user meets if they miss a step in the README. A key is
# no longer needed to start — /login and /model are a key away — so a run with
# nothing to answer is allowed to end quietly, and what it owes instead is the
# sentence saying which key that is.
if said=$(sandbox /opt/crucible/crucible </dev/null 2>&1); then
    if [[ $said == */login* && $said == */model* ]]; then
        echo '    says what to do about it'
    else
        printf '    FAIL a run with no key never said what to do: %q\n' "$said"
        failed=1
    fi
else
    printf '    FAIL a run with nothing to answer exited %d\n' "$?"
    failed=1
fi

# Down a pipe there is nobody to press that key, so a prompt arriving there is
# unanswerable and has to be a failure: a run that answers nothing and exits 0
# is one people report as "it does nothing", and a script reads it as success.
if answer=$(echo 'what is 2+2' | sandbox /opt/crucible/crucible 2>&1); then
    printf '    FAIL a prompt it could not answer exited 0: %q\n' "$answer"
    failed=1
else
    echo '    a prompt it cannot answer ends as a failure'
fi

echo "==> a run nobody is watching"
# Read back from the run above, whose output was a pipe rather than a terminal.
# A redirected run must write no escape sequence at all: those bytes are not
# formatting once something other than a terminal has them, they are corruption
# in a log, a diff or whatever read the output. This is the failure that looks
# fine on screen and is only ever seen by the next program along, which is why
# the gate is here rather than in a rendering test -- the source can be right
# and the shipped binary still write one on a path no test takes.
if [[ $said == *$'\e'* ]]; then
    printf '    FAIL a redirected run wrote an escape sequence: %q\n' "$said"
    failed=1
else
    echo '    wrote nothing a terminal would have had to interpret'
fi

echo "==> a session, end to end"
# A key alone does not make a turn. There is no model built in and this home
# directory was made a moment ago, so nothing here names one — and a run with
# none stops at "no model selected" without opening a connection, which would
# leave this gate passing on a binary that could not reach anything. The flag is
# the one rung that needs no file.
#
# What either run below exits with is not the claim — what came back is — and
# under `set -e` a non-zero one would take the script down where it stands,
# before the line that says what that was.
model=claude-sonnet-5
if ((no_provider)); then
    echo '    SKIP --no-provider'
elif [[ -n ${CRUCIBLE_SMOKE_KEY:-} ]]; then
    # A real turn against a real model, which is the only thing that exercises
    # streaming and the transcript. Its own variable rather than
    # ANTHROPIC_API_KEY, so this can never pick up the key from a shell that
    # happened to have one exported and call the gate met by accident.
    said=$(printf 'reply with the single word: forged\n' |
        sandbox --share-net --ro-bind /etc/resolv.conf /etc/resolv.conf \
            --setenv ANTHROPIC_API_KEY "$CRUCIBLE_SMOKE_KEY" \
            /opt/crucible/crucible --model "$model" 2>&1) || true
    if [[ ${said,,} == *forged* ]]; then
        echo '    a model answered, and the answer reached the terminal'
    else
        printf '    FAIL no answer came back: %q\n' "$said"
        failed=1
    fi
else
    # No key, so the turn cannot complete — but everything up to the model
    # answering still can, and that is the half a clean machine breaks. A
    # deliberately invalid key reaching a 401 means DNS resolved, TLS verified
    # against roots compiled into the binary rather than a certificate bundle
    # this sandbox does not have, and the provider's own words came back and
    # were drawn.
    said=$(printf 'hello\n' |
        sandbox --share-net --ro-bind /etc/resolv.conf /etc/resolv.conf \
            --setenv ANTHROPIC_API_KEY "not-a-key" \
            /opt/crucible/crucible --model "$model" 2>&1) || true
    if [[ $said == *401* ]]; then
        echo '    reached the provider and drew its refusal — TLS needs no system certificates'
        echo '    SKIP no completed turn: set CRUCIBLE_SMOKE_KEY to spend a few tokens on one'
    else
        printf '    FAIL never reached the provider: %q\n' "$said"
        failed=1
    fi
fi

((failed == 0)) || {
    echo
    echo "smoke gates failed"
    exit 1
}

echo
echo "all smoke gates passed"
