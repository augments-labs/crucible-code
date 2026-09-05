# What to install, and building it

## What every host needs

Three things, and the list ends there.

**git and rustup.** The toolchain itself is pinned in `rust-toolchain.toml`, so
rustup fetches the right one on the first cargo command in this directory —
including `rustfmt` and `clippy`, which the gate runs. There is no version to
choose and no second toolchain to keep current.

**A C compiler and a linker.** One dependency compiles C: `ring`, the
cryptography under the TLS the HTTP client speaks. It ships its assembly
pregenerated, so no assembler is needed and no C++ is compiled anywhere in the
tree — the packages below carry a C++ compiler because that is how they are
shipped, not because a build asks for one.

**A POSIX shell, to run the gate.** `scripts/check.sh` is bash. On Windows that
means Git Bash or a Windows Subsystem for Linux shell; the build itself needs no
shell.

Nothing else: no OpenSSL, no `pkg-config`, no cmake, no Python, no node. A
dependency that wanted one would need the justification comment every entry in
`Cargo.toml` carries, and this list is part of what that comment is weighed
against.

## Linux

```bash
sudo apt install build-essential     # Debian, Ubuntu
sudo dnf install gcc gcc-c++ make    # Fedora, RHEL
sudo pacman -S base-devel            # Arch
```

Then rustup, if the distribution's own Rust package is not the pinned version —
it usually is not, and a distribution toolchain that shadows rustup is the most
common reason a first build fails in a way that has nothing to do with crucible:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## macOS

```bash
xcode-select --install
```

That is clang, the linker and the macOS SDK. Full Xcode works too; nothing here
needs it. Then rustup, the same way as above — Homebrew's `rust` is a toolchain
that does not read `rust-toolchain.toml`.

## Windows

Rust's default host toolchain on Windows targets MSVC, so that is what the
compiler comes from:

- **Visual Studio Build Tools** (or Visual Studio itself) with the **Desktop
  development with C++** workload. That is `cl.exe`, `link.exe` and the Windows
  SDK.
- **rustup** from [rustup.rs](https://rustup.rs), which offers to install the
  Build Tools if they are missing.

crucible's own `bash` tool looks for a POSIX shell at runtime as well, and finds
the one [Git for Windows](https://git-scm.com/download/win) installs — see
[getting started](../getting-started/getting-started.md).

## Build it

```bash
git clone https://github.com/augments-labs/crucible-code
cd crucible-code
cargo build
cargo run -- --help
```

`cargo build --release` is what a release ships: fat link-time optimization, one
codegen unit, symbols stripped. It is several times slower to link and is the
only build the performance budgets are measured against, so use the debug build
while working and the release build to check a number.

## Run the gate

```bash
scripts/check.sh
```

The compatibility command runs the deterministic Rust and repository checks
expected on a contributor machine. CI calls those named gates independently,
runs Rust tests on Intel and Apple silicon macOS plus Windows, and supplies
dependency and performance jobs of its own;
[workflow ownership](../../.github/workflows/README.md) has the map. You can
also [build for another platform yourself](cross-compiling.md).
