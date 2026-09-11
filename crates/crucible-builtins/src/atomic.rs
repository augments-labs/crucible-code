//! Whole-file replacement without an interval containing half a file.
//!
//! A replacement is prepared under a fresh name in the destination directory,
//! flushed, and renamed over the destination. The original name changes in one
//! operation and is untouched on every failure before it. Unix performs the
//! walk and rename against the proven parent descriptor in `crucible-core` and
//! flushes that directory after commit. Windows validates and holds the
//! destination directory, then renames the prepared file relative to that
//! handle and flushes the renamed file; its handle-relative rename API has no
//! write-through flag that proves the directory entry durable. Its public path
//! can become a directory reparse point meanwhile without changing where the
//! commit lands.

use std::fs::{File, Permissions};
use std::io::{self, Write as _};

use crucible_core::{PathError, WorkspacePath};

#[cfg(windows)]
mod windows;

/// Replaces `path` with `content`, preserving `permissions` when it existed.
pub(crate) fn replace(
    path: &WorkspacePath,
    content: &[u8],
    permissions: Option<Permissions>,
    expected: Option<&File>,
) -> Result<(), PathError> {
    replace_with(path, permissions, expected, |file| file.write_all(content))
}

#[cfg(unix)]
fn replace_with(
    path: &WorkspacePath,
    permissions: Option<Permissions>,
    expected: Option<&File>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), PathError> {
    path.replace_with(permissions, expected, write)
}

#[cfg(windows)]
fn replace_with(
    path: &WorkspacePath,
    permissions: Option<Permissions>,
    expected: Option<&File>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), PathError> {
    windows::replace_with(path, permissions, expected, write)
}

#[cfg(not(any(unix, windows)))]
fn replace_with(
    path: &WorkspacePath,
    permissions: Option<Permissions>,
    expected: Option<&File>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), PathError> {
    let _ = (permissions, expected, write);
    Err(PathError::Unreplaced {
        at: path.to_string().into(),
        source: io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic replacement is not implemented on this platform",
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;

    #[test]
    fn a_failed_temporary_write_leaves_the_original_and_no_residue() {
        let sample = Sample::new("atomic-rollback");
        sample.write("one.txt", "original\n");
        let path = sample.workspace().existing("one.txt").unwrap();
        let permissions = path.open().unwrap().metadata().unwrap().permissions();

        let original = path.open().unwrap();
        let problem = replace_with(&path, Some(permissions), Some(&original), |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("injected write failure"))
        })
        .expect_err("the injected failure");

        assert!(problem.to_string().contains("injected write failure"));
        assert_eq!(
            std::fs::read_to_string(sample.root().join("one.txt")).unwrap(),
            "original\n"
        );
        let names: Vec<_> = std::fs::read_dir(sample.root())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("one.txt")]);
    }

    #[test]
    fn a_new_name_claimed_before_commit_is_never_overwritten() {
        let sample = Sample::new("atomic-create-race");
        let path = sample.workspace().creatable("one.txt").unwrap();
        let planted = sample.root().join("one.txt");

        let problem = replace_with(&path, None, None, |file| {
            file.write_all(b"ours")?;
            std::fs::write(&planted, "theirs")
        })
        .expect_err("the destination was claimed first");

        assert!(problem.to_string().contains("could not be replaced"));
        assert_eq!(std::fs::read_to_string(&planted).unwrap(), "theirs");
        let names: Vec<_> = std::fs::read_dir(sample.root())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("one.txt")]);
    }

    #[test]
    fn an_existing_file_changed_before_commit_is_not_overwritten() {
        let sample = Sample::new("atomic-identity-race");
        sample.write("one.txt", "original\n");
        let path = sample.workspace().existing("one.txt").unwrap();
        let original = path.open_regular().unwrap();
        let permissions = original.metadata().unwrap().permissions();
        let moved = sample.root().join("moved.txt");
        let planted = sample.root().join("one.txt");

        let problem = replace_with(&path, Some(permissions), Some(&original), |file| {
            file.write_all(b"replacement\n")?;
            std::fs::rename(&planted, &moved)?;
            std::fs::write(&planted, "concurrent\n")
        })
        .expect_err("the destination changed identity");

        assert!(matches!(problem, PathError::Changed { .. }), "{problem:?}");
        assert_eq!(
            problem.to_string(),
            format!("{path} changed while its replacement was prepared, so it was not replaced")
        );
        assert_eq!(std::fs::read_to_string(&planted).unwrap(), "concurrent\n");
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "original\n");
    }

    #[cfg(windows)]
    #[test]
    fn an_ancestor_changed_to_a_reparse_point_cannot_redirect_the_replacement() {
        let sample = Sample::new("atomic-reparse-race");
        sample.write("sub/one.txt", "original");
        let outside = sample.root().parent().unwrap().join("outside");
        std::fs::write(outside.join("one.txt"), "outside").unwrap();
        let path = sample.workspace().existing("sub/one.txt").unwrap();
        let original = path.open_regular().unwrap();
        let permissions = original.metadata().unwrap().permissions();
        drop(original);

        std::fs::rename(sample.root().join("sub"), sample.root().join("moved")).unwrap();
        std::os::windows::fs::symlink_dir(&outside, sample.root().join("sub"))
            .expect("creating this directory link needs Windows developer mode");

        let problem = replace(&path, b"replacement", Some(permissions), None).unwrap_err();

        assert!(matches!(problem, PathError::Swapped { .. }), "{problem:?}");
        assert_eq!(
            std::fs::read_to_string(outside.join("one.txt")).unwrap(),
            "outside"
        );
        assert_eq!(
            std::fs::read_to_string(sample.root().join("moved/one.txt")).unwrap(),
            "original"
        );
    }
}
