#!/bin/sh
# One disposable native compatibility experiment; no project/config mutations.
set -eu
test "$(uname -s)" = Darwin || { echo 'Requires native macOS' >&2; exit 77; }
probe_rustc=$(rustup which --toolchain stable rustc)
probe_cargo=$(rustup which --toolchain stable cargo)
probe_clang=$(/usr/bin/xcrun --find clang)
probe_sdk=$(/usr/bin/xcrun --sdk macosx --show-sdk-path)
probe_python=$(command -v python3)
probe_root=$(mktemp -d /tmp/crucible-macos-tools.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
mkdir "$probe_root/home" "$probe_root/tmp" "$probe_root/cargo-home" "$probe_root/fixtures"
printf 'Evidence retained at %s\n' "$probe_root"

cat > "$probe_root/fixtures/hello.c" <<'C'
int main(void) { return 0; }
C
cat > "$probe_root/fixtures/hello.rs" <<'RS'
fn main() { println!("RUST-BINARY-RAN"); }
RS
cat > "$probe_root/fixtures/command.rs" <<'RS'
fn main() {
    match std::process::Command::new("/usr/bin/true").status() {
        Ok(status) if status.success() => println!("RUST-COMMAND-RAN"),
        other => { eprintln!("RUST-COMMAND-FAILED: {other:?}"); std::process::exit(1); }
    }
}
RS
cat > "$probe_root/fixtures/python.py" <<'PY'
import os, subprocess, sys
print("PYTHON", sys.version, "USE_POSIX_SPAWN", subprocess._USE_POSIX_SPAWN, flush=True)
def audit(event, args):
    if event == "os.posix_spawn":
        print("PYTHON-PATH=posix_spawn", flush=True)
sys.addaudithook(audit)
mode = sys.argv[1]
try:
    if mode == "python-spawn":
        pid = os.posix_spawn("/usr/bin/true", ["true"], os.environ)
        _, status = os.waitpid(pid, 0)
        assert os.waitstatus_to_exitcode(status) == 0
    else:
        kwargs = {}
        if mode == "python-close-fds-false": kwargs["close_fds"] = False
        if mode == "python-cwd": kwargs["cwd"] = "."
        subprocess.run(["/usr/bin/true"], check=True, timeout=5, **kwargs)
    print("PYTHON-CHILD-RAN", mode)
except Exception as exc:
    print("PYTHON-FAILED", mode, type(exc).__name__, "errno", getattr(exc, "errno", None), str(exc))
    sys.exit(1)
PY

# Compile only this trusted measurement fixture before confinement; its runtime
# behavior, and separate fresh compiler/linker work, are tested below.
SDKROOT="$probe_sdk" "$probe_rustc" --edition=2021 \
    "$probe_root/fixtures/command.rs" -o "$probe_root/fixtures/rust-command"

cat > "$probe_root/control.sb" <<'SB'
(version 1)
(allow default)
(deny network*)
SB
cat > "$probe_root/all-spawn.sb" <<'SB'
(version 1)
(allow default)
(deny network*)
(deny syscall-unix (syscall-number SYS_setsid SYS_setpgid SYS_posix_spawn))
SB

cat > "$probe_root/run-case.sh" <<'SH'
set -eu
case_name=$1; work=$2; fixture=$3; rustc_bin=$4; cargo_bin=$5; clang_bin=$6; python_bin=$7
cd "$work"
case "$case_name" in
    rust-command) "$fixture/rust-command" ;;
    rustc-link)
        "$rustc_bin" --edition=2021 "$fixture/hello.rs" -o rust-output
        ./rust-output ;;
    clang-object)
        "$clang_bin" -c "$fixture/hello.c" -o c-output.o
        test -s c-output.o ;;
    clang-link)
        "$clang_bin" "$fixture/hello.c" -o c-output
        ./c-output ;;
    cargo-test)
        mkdir src
        cat > Cargo.toml <<'TOML'
[package]
name = "native-scope-probe"
version = "0.0.0"
edition = "2021"
TOML
        cat > build.rs <<'RS'
fn main() {
    assert!(std::process::Command::new("/usr/bin/true").status().expect("build child").success());
    println!("cargo:warning=BUILD-CHILD-RAN");
}
RS
        cat > src/lib.rs <<'RS'
#[test]
fn native_child() {
    assert!(std::process::Command::new("/usr/bin/true").status().expect("test child").success());
    println!("TEST-CHILD-RAN");
}
RS
        "$cargo_bin" test --offline --jobs 1 --target-dir target -- --nocapture ;;
    python-*) "$python_bin" -I "$fixture/python.py" "$case_name" ;;
    *) exit 77 ;;
esac
printf 'CASE-OK %s\n' "$case_name"
SH

{
    /usr/bin/sw_vers
    /usr/bin/uname -mrv
    "$probe_rustc" -Vv
    "$probe_cargo" --version
    "$probe_clang" --version
    "$probe_python" --version
    /usr/bin/shasum -a 256 "$probe_rustc" "$probe_cargo" "$probe_clang" \
        "$probe_python" "$probe_root/fixtures/"* "$probe_root/"*.sb "$probe_root/run-case.sh"
} > "$probe_root/environment.txt" 2>&1
/bin/cat "$probe_root/environment.txt"
probe_path="$(dirname "$probe_rustc"):$(dirname "$probe_clang"):/usr/bin:/bin:/usr/sbin:/sbin"
for profile in control all-spawn; do
    for case_name in rust-command rustc-link clang-object clang-link cargo-test \
        python-default python-close-fds-false python-cwd python-spawn; do
        work="$probe_root/$profile-$case_name"
        mkdir "$work"
        set +e
        /usr/bin/env -i HOME="$probe_root/home" CARGO_HOME="$probe_root/cargo-home" \
            TMPDIR="$probe_root/tmp/" PATH="$probe_path" SDKROOT="$probe_sdk" \
            RUSTC="$probe_rustc" RUSTDOC="$(dirname "$probe_rustc")/rustdoc" \
            /usr/bin/sandbox-exec -f "$probe_root/$profile.sb" \
            /bin/sh "$probe_root/run-case.sh" "$case_name" "$work" "$probe_root/fixtures" \
            "$probe_rustc" "$probe_cargo" "$probe_clang" "$probe_python" \
            > "$work.log" 2>&1
        result=$?
        set -e
        printf 'RESULT profile=%s case=%s status=%s\n' "$profile" "$case_name" "$result" >> "$work.log"
        /bin/cat "$work.log"
    done
done
printf 'Retained evidence: %s\n' "$probe_root"
# No aggregate approval: compare each strict result against its passing control.
