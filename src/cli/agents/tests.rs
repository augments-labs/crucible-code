//! What `crucible agents install` and `crucible agents unpack-check` write.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_external_harnesses::release::proof;

use super::*;

/// The file list `unpack-check` prints for the release [`proof::catalog_release`]
/// publishes, committed for the catalog's intake to read.
const SURVEYED: &str =
    include_str!("../../../tests/fixtures/agent-catalog/unpack-check/binary.json");

/// A directory of the test's own, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let at = std::env::temp_dir().join(format!(
            "crucible-cli-{name}-{unique}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&at).expect("a directory");
        Self(at)
    }

    /// The artifact `described` and its download `archive`, written here.
    fn published(&self, described: &[u8], archive: &[u8]) -> (PathBuf, PathBuf) {
        let entry = self.0.join("entry.json");
        let download = self.0.join("agent.tar.gz");
        fs::write(&entry, described).expect("the artifact");
        fs::write(&download, archive).expect("the download");
        (entry, download)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the intake records for a release is what `unpack-check` prints, byte
/// for byte the committed list, and the release is left unpacked where the
/// command was told to put it.
#[test]
fn unpack_check_prints_the_file_list_committed_for_the_intake() {
    let scratch = Scratch::new("unpack-check");
    let (archive, described) = proof::catalog_release();
    let (entry, download) = scratch.published(&described, &archive);
    let out = scratch.0.join("out");

    let printed = checked(&entry, &download, &out);

    assert_eq!(printed.as_deref(), Ok(SURVEYED));
    assert_eq!(
        fs::read(out.join("lib").join("harness")).ok(),
        Some(proof::COMPANION.to_vec())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(&out).map_or(0, |metadata| metadata.permissions().mode() & 0o777);
        assert_eq!(mode, 0o700, "only its owner reaches what was unpacked");
    }
}

/// The artifact `described` spells, pinned instead at `archive`.
fn repinned(described: &[u8], archive: &[u8]) -> Vec<u8> {
    use sha2::{Digest as _, Sha256};

    let mut artifact: serde_json::Value = serde_json::from_slice(described).expect("an artifact");
    let sha256 = Sha256::digest(archive)
        .iter()
        .fold(String::new(), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        });
    if let Some(fields) = artifact.as_object_mut() {
        fields.insert("sha256".to_owned(), sha256.into());
        fields.insert("size".to_owned(), archive.len().into());
    }
    artifact.to_string().into_bytes()
}

/// A refusal says what was wrong with what it was given, as the reader that
/// refused it found it, and not only that it was refused.
#[test]
fn unpack_check_says_the_cause_of_a_refusal() {
    let scratch = Scratch::new("unpack-check-cause");
    let (mut archive, described) = proof::catalog_release();
    archive.extend_from_slice(b"one byte more and then some");
    let described = repinned(&described, &archive);
    let (entry, download) = scratch.published(&described, &archive);
    let out = scratch.0.join("out");

    let refused = checked(&entry, &download, &out).expect_err("bytes after the stream");

    assert!(refused.contains("could not be read: "), "{refused}");
    assert!(refused.contains("past its gzip stream"), "{refused}");
    assert_eq!(refused.lines().count(), 1, "{refused}");
    assert!(!out.exists(), "nothing is left where the release would go");
}

/// A download that is not the artifact's, and a directory already where the
/// release would go, are each refused in one line, with nothing unpacked.
#[test]
fn unpack_check_refuses_in_one_line_and_leaves_nothing_unpacked() {
    let scratch = Scratch::new("unpack-check-refused");
    let (archive, described) = proof::catalog_release();
    let (entry, download) = scratch.published(&described, &archive);
    fs::write(&download, b"another release").expect("the download");
    let out = scratch.0.join("out");

    let refused = checked(&entry, &download, &out).expect_err("another release is refused");

    assert!(refused.starts_with("crucible: "), "{refused}");
    assert_eq!(refused.lines().count(), 1, "{refused}");
    assert!(!out.exists(), "nothing is left where the release would go");

    fs::write(&download, &archive).expect("the download");
    fs::create_dir(&out).expect("a directory in the way");
    let refused = checked(&entry, &download, &out).expect_err("a directory in the way");
    assert!(
        refused.contains("could not be made to unpack into: "),
        "{refused}"
    );
    assert!(
        refused.contains("(os error"),
        "the system's own words: {refused}"
    );
}

#[test]
fn a_driver_is_named_as_externalagents_names_it() {
    assert_eq!(driver("antigravity"), Ok(Driver::Antigravity));
    assert_eq!(driver("claude"), Ok(Driver::Claude));
    assert_eq!(
        driver("Antigravity"),
        Err("expected one of claude, antigravity".to_owned())
    );
}

/// A catalog agent is not installed by its driver, which names no agent of
/// its own, so `catalog` is refused with the command that adds one, and only
/// the built-in drivers are listed.
#[test]
fn a_catalog_agent_is_added_rather_than_installed() {
    let refused = driver("catalog").expect_err("catalog is not installed by its driver");
    assert!(refused.contains("crucible agents add"), "{refused}");
    assert!(refused.contains("claude, antigravity"), "{refused}");
}

#[test]
fn an_install_says_where_the_release_is_and_how_to_use_it() {
    let installed = Installed {
        version: "1.3.0",
        executable: PathBuf::from("/home/me/.crucible/programs/antigravity/1.3.0/agy"),
        kept: false,
    };
    assert_eq!(
        answered(Driver::Antigravity, &installed),
        "Installed antigravity 1.3.0 at /home/me/.crucible/programs/antigravity/1.3.0/agy. \
         /login offers it under Use an installed coding agent.\n"
    );
    let kept = Installed {
        kept: true,
        ..installed
    };
    assert_eq!(
        answered(Driver::Antigravity, &kept),
        "antigravity 1.3.0 is already installed at \
         /home/me/.crucible/programs/antigravity/1.3.0/agy.\n"
    );
}

#[test]
fn a_terminal_is_told_each_new_percent_on_one_line() {
    let mut progress = Progress::new(Driver::Antigravity, true);
    assert_eq!(progress.ended(), "");
    assert_eq!(
        progress.heard(1_000_000, 4_000_000),
        "\rDownloading antigravity (4 MB):  25%"
    );
    assert_eq!(progress.heard(1_000_001, 4_000_000), "");
    assert_eq!(
        progress.heard(4_000_000, 4_000_000),
        "\rDownloading antigravity (4 MB): 100%"
    );
    assert_eq!(progress.ended(), "\n");
}

#[test]
fn anything_else_is_told_once_that_the_download_began() {
    let mut progress = Progress::new(Driver::Antigravity, false);
    assert_eq!(
        progress.heard(1_000_000, 4_000_000),
        "Downloading antigravity (4 MB)…\n"
    );
    assert_eq!(progress.heard(4_000_000, 4_000_000), "");
    assert_eq!(progress.ended(), "");
}
