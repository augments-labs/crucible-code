//! The receipt read by both of its readers, which must agree.
//!
//! The installer that writes the receipt is shell, and reads it back on an
//! upgrade, while crucible reads it on its own. Each receipt under
//! `tests/fixtures/installer/receipts/` is given to the shell reader beside
//! them, under `/bin/sh` and `LC_ALL=C`, and to [`Receipt::parse`]. Both must
//! accept the ones in `accepted/` with the same values, byte for byte, and
//! both must refuse the ones in `refused/`.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crucible_update::Receipt;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/installer")
}

fn receipts(verdict: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(fixtures().join("receipts").join(verdict))
        .expect("the fixture directory")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    found.sort();
    assert!(!found.is_empty(), "no {verdict} receipts");
    found
}

/// A value's bytes, escaped so that a failure shows them and two different
/// byte strings never print alike.
fn shown(bytes: &[u8]) -> String {
    bytes.escape_ascii().to_string()
}

/// What the shell reader made of `receipt`: the values it set, in the order
/// the receipt holds them, or `None` when it refused.
fn shell(receipt: &Path) -> Option<Vec<String>> {
    shell_with(receipt, "/usr/bin:/bin")
}

/// [`shell`], with commands found on `path`.
fn shell_with(receipt: &Path, path: &str) -> Option<Vec<String>> {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(
            r#". "$1" || exit 2
            crucible_receipt_read "$2" 2>/dev/null || exit 1
            printf '%s\n' "$receipt_installation" "$receipt_target" "$receipt_prefix" \
                "$receipt_version" "$receipt_crucible" "$receipt_broker""#,
        )
        .arg("sh")
        .arg(fixtures().join("receipt.sh"))
        .arg(receipt)
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", path)
        .output()
        .expect("/bin/sh runs");
    match output.status.code() {
        Some(0) => Some(
            output
                .stdout
                .strip_suffix(b"\n")
                .expect("the last value's newline")
                .split(|byte| *byte == b'\n')
                .map(shown)
                .collect(),
        ),
        Some(1) => None,
        _ => panic!(
            "the shell reader failed on {}: {output:?}",
            receipt.display()
        ),
    }
}

/// The same values, as the Rust reader gives them.
fn rust(receipt: &Path) -> Option<Vec<String>> {
    let parsed = Receipt::parse(&fs::read(receipt).expect("the receipt")).ok()?;
    Some(vec![
        shown(parsed.installation().as_str().as_bytes()),
        shown(parsed.target().as_str().as_bytes()),
        shown(parsed.prefix().as_os_str().as_bytes()),
        shown(parsed.version().as_str().as_bytes()),
        shown(parsed.crucible().to_string().as_bytes()),
        shown(
            parsed
                .broker()
                .map(ToString::to_string)
                .unwrap_or_default()
                .as_bytes(),
        ),
    ])
}

#[test]
fn both_readers_take_each_accepted_receipt_to_the_same_values() {
    for receipt in receipts("accepted") {
        let name = receipt.display();
        let shell = shell(&receipt).unwrap_or_else(|| panic!("the shell refused {name}"));
        let rust = rust(&receipt).unwrap_or_else(|| panic!("the parser refused {name}"));
        assert_eq!(shell, rust, "{name}");
    }
}

#[test]
fn both_readers_refuse_each_refused_receipt() {
    for receipt in receipts("refused") {
        let name = receipt.display();
        assert_eq!(shell(&receipt), None, "the shell took {name}");
        assert_eq!(rust(&receipt), None, "the parser took {name}");
    }
}

/// The commands the shell reader runs that are not built into the shell.
const COMMANDS: [&str; 3] = ["tail", "tr", "wc"];

#[test]
fn the_shell_reader_refuses_a_receipt_it_cannot_check_whole() {
    let accepted = receipts("accepted");
    let receipt = accepted.first().expect("an accepted receipt");
    for missing in COMMANDS {
        let path = std::env::temp_dir().join(format!(
            "crucible-receipt-path-{missing}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("a directory for the commands");
        for command in COMMANDS.into_iter().filter(|command| *command != missing) {
            let found = ["/usr/bin", "/bin"]
                .into_iter()
                .map(|dir| Path::new(dir).join(command))
                .find(|at| at.exists())
                .unwrap_or_else(|| panic!("{command} is installed"));
            std::os::unix::fs::symlink(found, path.join(command)).expect("the command linked");
        }

        let read = shell_with(receipt, path.to_str().expect("a UTF-8 path"));

        let _ = fs::remove_dir_all(&path);
        assert_eq!(read, None, "read without {missing}");
    }
}
