#!/usr/bin/env bash
# The release gate that cannot run before there is a release: take the artifact
# people will actually download, and run it somewhere that holds nothing else.
#
#     scripts/smoke.sh                     the tag matching the current version
#     scripts/smoke.sh v0.0.1              a published tag
#     scripts/smoke.sh ./crucible.tar.gz   a tarball already on disk
#     scripts/smoke.sh --offline v0.0.1    skip the gate that needs the network
#
# `scripts/check.sh` proves the source is correct and `scripts/bench.sh` proves
# it is fast. Both build from this tree, with this machine's toolchain, in this
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

cd "$(dirname "$0")/.."

readonly REPO=augments-labs/crucible-code

# Which of the published artifacts this gate is about. `bwrap`, `ldd` and the
# glibc floor below are all Linux, so this is the one it can run against — the
# other six are proved by the release build and by CI, not from here.
readonly PLATFORM=linux-x86_64

offline=0
target=

for argument in "$@"; do
    case "$argument" in
    --offline) offline=1 ;;
    -*)
        echo "smoke: unknown option $argument" >&2
        exit 2
        ;;
    *) target=$argument ;;
    esac
done

# The version is declared in one place, so the default target is derivable
# rather than something to keep in step by hand.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
readonly version
[[ -n $target ]] || target=v$version

failed=0
work=$(mktemp -d)
readonly work
trap 'rm -rf "$work"' EXIT

echo "==> tools"
for tool in bwrap tar sha256sum; do
    command -v "$tool" >/dev/null || {
        printf '    FAIL %s is not installed\n' "$tool"
        failed=1
    }
done
# bubblewrap is useless without the namespaces it asks the kernel for, and the
# failure reads as a permission error several steps later. Ask now. The host
# root is bound read-only for this one probe so that the thing being tested is
# the kernel's answer and not whether `true` happens to be reachable.
if command -v bwrap >/dev/null &&
    ! bwrap --unshare-all --ro-bind / / /bin/true 2>/dev/null; then
    echo '    FAIL bwrap cannot unshare — unprivileged user namespaces look disabled'
    failed=1
fi
((failed == 0)) || exit 1

echo "==> artifact"
if [[ -f $target ]]; then
    tarball=$(realpath "$target")
    echo "    a local file, so its checksum is not the published one"
    published=0
else
    # An artifact is named for the version it holds, not for the tag it was cut
    # from — the `v` belongs to git.
    name=crucible-${target#v}-$PLATFORM.tar.gz
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
readonly tarball published
printf '    %s\n' "$tarball"

echo "==> checksum"
if ((published)); then
    # One file for the whole release rather than one beside each artifact, so
    # every line in it names something this gate did not download.
    (cd "$(dirname "$tarball")" && sha256sum --ignore-missing -c SHA256SUMS)
else
    echo "    SKIP no published checksum for a local file"
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
mapfile -t libraries < <(ldd "$binary" | grep -o '/[^ ]*' | sort -u)
printf '    %s\n' "${libraries[@]}"

# The oldest glibc that can load this binary. Weak symbols are excluded on
# purpose: the loader tolerates their absence, so a weak reference to a very new
# symbol is not a floor and reporting it as one would retire distributions that
# run this fine.
floor=$(objdump -T "$binary" | grep UND | grep -v ' w ' |
    grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
printf '    requires %s or newer\n' "${floor:-no versioned glibc symbols}"

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
if [[ $said == "crucible $version" ]]; then
    printf '    %s\n' "$said"
else
    printf '    FAIL --version said %q, expected %q\n' "$said" "crucible $version"
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
if ((offline)); then
    echo '    SKIP --offline'
elif [[ -n ${CRUCIBLE_SMOKE_KEY:-} ]]; then
    # A real turn against a real model, which is the only thing that exercises
    # streaming and the transcript. Its own variable rather than
    # ANTHROPIC_API_KEY, so this can never pick up the key from a shell that
    # happened to have one exported and call the gate met by accident.
    said=$(printf 'reply with the single word: forged\n' |
        sandbox --share-net --ro-bind /etc/resolv.conf /etc/resolv.conf \
            --setenv ANTHROPIC_API_KEY "$CRUCIBLE_SMOKE_KEY" \
            /opt/crucible/crucible 2>&1)
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
            /opt/crucible/crucible 2>&1)
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
