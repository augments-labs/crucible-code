# Building for another platform

Every published artifact is built natively, on a runner of its own
architecture. Nothing a release ships is cross-compiled, and this page does not
change that — what it is for is the other question: whether a change compiles
for the six platforms that are not the one in front of you, answered in a minute
rather than in a round trip through CI.

## The target, and what links it

A target has two halves. Rust's own half is one command:

```bash
rustup target add aarch64-apple-darwin
```

The other half is a linker and a C compiler that produce objects for that
platform, because `ring` compiles C wherever crucible is built. Two tools cover
every target crucible ships:

```bash
cargo install --locked cargo-zigbuild cargo-xwin
```

`cargo-zigbuild` drives the compiler and linker inside `zig`, which carries the
headers and stub libraries for Linux, macOS and FreeBSD. Install zig itself from
[ziglang.org](https://ziglang.org/download/), your package manager, or
`pip install ziglang`. `cargo-xwin` fetches the Microsoft headers and import
libraries on first use and links with `lld`, so a Windows build needs `clang`
and `lld` on the host and nothing from Microsoft installed.

## From a Linux x86-64 host

| Target | Command |
| --- | --- |
| `aarch64-unknown-linux-gnu` | `cargo zigbuild --target aarch64-unknown-linux-gnu` |
| `aarch64-apple-darwin` | `cargo zigbuild --target aarch64-apple-darwin` |
| `x86_64-apple-darwin` | `cargo zigbuild --target x86_64-apple-darwin` |
| `x86_64-unknown-freebsd` | `cargo zigbuild --target x86_64-unknown-freebsd` |
| `x86_64-pc-windows-msvc` | `cargo xwin build --target x86_64-pc-windows-msvc` |

The macOS builds print a warning that `xcrun` could not be found. That is zig
saying it has no Xcode installation to read an SDK version out of; it links
against its own copy and the binary is a real Mach-O executable either way.

Linux ARM64 has a second route that needs no zig, if the distribution packages a
cross toolchain — `gcc-aarch64-linux-gnu` on Debian and Ubuntu — and is told
which linker to use:

```bash
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --target aarch64-unknown-linux-gnu
```

`cargo zigbuild` is the one to reach for anyway, because it also chooses which
glibc to build against — `aarch64-unknown-linux-gnu.2.34` targets the floor the
releases hold rather than whatever the host happens to have.

**Windows ARM64 does not cross-build from Linux today.** `cargo-xwin` hands
`ring`'s C sources to a Unix `clang` with Microsoft-style include flags, which
that compiler reads as file names, and the build stops in the build script. The
target is built natively on a Windows ARM64 runner when a release is cut, and
that build is what proves it.

## What a cross build does not answer

It compiles and links; it does not run. The tests for a target are run on that
platform — CI runs them on Linux, macOS and Windows for every pull request — so
a cross build catches a platform-specific compile error and nothing that
happens afterwards. `scripts/check.sh` is still the gate.
