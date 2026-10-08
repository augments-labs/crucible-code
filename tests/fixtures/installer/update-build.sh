#!/usr/bin/env bash
# One more release of the checked-out tree, built for the cells that stage
# `crucible update`, under a version number of the cell's choosing.
#
# The release workflow's build jobs call this once they have built and
# packaged the staged release, and hand it the build command they ran. It
# copies the checkout into a scratch directory, sets `[workspace.package]
# version` in that copy's root Cargo.toml, lets cargo bring the lock file's
# own entries up to that number without reaching the network, and runs the
# same command there, so the build is locked and makes the same binaries the
# job's own build does. The target directory is the checkout's, so nothing
# the job already compiled is compiled again but the workspace's own crates.
# The binaries are packaged the way the job packages them, under OUT/VERSION
# with a SHA256SUMS beside the archive. The copy is never pushed, and nothing
# built here is ever uploaded to a release.
#
# Usage: update-build.sh --version VERSION --stem STEM --target TARGET
#            --binaries 'NAME…' [--exe .exe] --out DIR -- BUILD COMMAND…
set -euo pipefail

usage() {
    echo "usage: update-build.sh --version VERSION --stem STEM --target TARGET --binaries 'NAME...' [--exe .exe] --out DIR -- COMMAND..." >&2
    exit 2
}

exe=
while (($#)); do
    case $1 in
        --) shift; break ;;
        --version | --stem | --target | --binaries | --exe | --out)
            [[ $# -ge 2 ]] || usage
            declare "${1#--}=$2"
            shift 2
            ;;
        *) usage ;;
    esac
done
[[ $# -gt 0 && -n ${version:-} && -n ${stem:-} && -n ${target:-} && -n ${binaries:-} && -n ${out:-} ]] || usage
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
    echo "update-build: '$version' is not a release version" >&2
    exit 2
}

checkout=$PWD
mkdir -p "$out"
out=$(cd "$out" && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/update-build.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

# Cargo is a native program on Windows, and is handed the target directory as
# one; Git Bash's own spelling of the path is kept for reading the result.
target_dir=$checkout/target
cargo_target_dir=$target_dir
if command -v cygpath >/dev/null; then
    cargo_target_dir=$(cygpath -w "$target_dir")
fi

tar --exclude=./target --exclude=./.git --exclude=./generated -cf - . | tar -xf - -C "$scratch"
(
    cd "$scratch"
    awk -v version="$version" '
        /^\[workspace\.package\]$/ { inside = 1 }
        /^\[/ && !/^\[workspace\.package\]$/ { inside = 0 }
        inside && !done && /^version = "/ { sub(/"[^"]*"/, "\"" version "\""); done = 1 }
        { print }
        END { if (!done) exit 1 }
    ' Cargo.toml >Cargo.toml.next || {
        echo 'update-build: the root Cargo.toml has no [workspace.package] version' >&2
        exit 1
    }
    mv Cargo.toml.next Cargo.toml
    cargo update --workspace --offline
    CARGO_TARGET_DIR=$cargo_target_dir "$@"
)

built=$target_dir/$target/release
said=$("$built/crucible$exe" --version)
if [[ $said != "crucible $version" ]]; then
    echo "update-build: --version said '$said', expected 'crucible $version'" >&2
    exit 1
fi

name=crucible-$version-$stem
mkdir -p "$out/$version/$name"
for binary in $binaries; do
    cp "$built/$binary$exe" "$out/$version/$name/"
done
cp README.md LICENSE "$out/$version/$name/"
cp scripts/sh/install.sh scripts/sh/uninstall.sh "$out/$version/$name/"
cd "$out/$version"
tar czf "$name.tar.gz" "$name"
rm -rf "$name"
if command -v sha256sum >/dev/null; then
    sha256sum "$name.tar.gz" >SHA256SUMS
elif command -v shasum >/dev/null; then
    shasum -a 256 "$name.tar.gz" >SHA256SUMS
else
    sha256 -r "$name.tar.gz" >SHA256SUMS
fi
cat SHA256SUMS
