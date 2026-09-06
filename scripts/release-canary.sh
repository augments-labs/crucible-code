#!/usr/bin/env bash
# Install the newest published release into a disposable prefix, run it, remove
# it through the shipped uninstaller, and prove neither executable remains.
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(mktemp -d)
trap 'rm -rf -- "$root"' EXIT
mkdir -p "$root/home" "$root/bin"

HOME="$root/home" CRUCIBLE_INSTALL_DIR="$root/bin" scripts/install.sh
version=$(HOME="$root/home" "$root/bin/crucible" --version)
[[ $version == 'crucible '* ]] || {
    printf 'installed release reported an unexpected version: %q\n' "$version" >&2
    exit 1
}
[[ -x $root/bin/crucible && -L $root/bin/cru ]] || {
    echo 'latest release did not install the binary and owned alias' >&2
    exit 1
}

HOME="$root/home" CRUCIBLE_CODE_HOME="$root/home/.crucible" \
    CRUCIBLE_INSTALL_DIR="$root/bin" scripts/uninstall.sh
for path in crucible crucible-sandbox-broker cru; do
    if [[ -e $root/bin/$path || -L $root/bin/$path ]]; then
        printf 'latest release uninstall left %s behind\n' "$path" >&2
        exit 1
    fi
done

printf '%s installed, ran and uninstalled cleanly\n' "$version"
