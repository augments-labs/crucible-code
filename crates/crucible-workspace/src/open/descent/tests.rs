use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use crate::{PathError, Workspace};

/// A directory of its own that is removed when the test ends.
struct Directory(PathBuf);

impl Directory {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-descent-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn workspace(&self) -> Workspace {
        Workspace::open(&self.0).unwrap()
    }

    /// A file at `name` holding `content`, left at exactly `mode`.
    fn file(&self, name: &str, content: &str, mode: u32) {
        let path = self.0.join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn mode(&self, name: &str) -> u32 {
        fs::metadata(self.0.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The mode a file made with the ordinary creation request ends up with here,
/// probed in a directory named for `test` so tests running together share none.
///
/// The tests below look at group and other bits, so a process umask that
/// already clears them would let a defect pass unseen; a test that cannot tell
/// says so rather than passing.
fn ordinary_creation_mode(test: &str) -> u32 {
    let directory = Directory::new(&format!("probe-{test}"));
    let probe = directory.0.join("probe");
    drop(File::create_new(&probe).unwrap());
    fs::metadata(&probe).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_replaced_files_preparation_file_is_owner_only_from_the_moment_it_is_made() {
    let ordinary = ordinary_creation_mode("owner-only");
    assert!(
        ordinary & 0o077 != 0,
        "this process creates files as {ordinary:o}, which already has no group or other \
         bit, so the test cannot tell an owner-only file from an ordinary one: run it under \
         `umask 022`"
    );

    let directory = Directory::new("owner-only");
    directory.file("secret.txt", "private\n", 0o600);
    let path = directory.workspace().existing("secret.txt").unwrap();
    let original = path.open_regular().unwrap();
    let permissions = original.metadata().unwrap().permissions();

    let mut seen = None;
    path.replace_with(
        Some(permissions),
        Some(&original),
        |file| {
            seen = Some(file.metadata()?.permissions().mode() & 0o777);
            Ok(())
        },
        |_| true,
    )
    .unwrap();

    let seen = seen.expect("the write callback ran");
    assert_eq!(
        seen & 0o077,
        0,
        "the preparation file was {seen:o} when the write callback ran"
    );
}

#[test]
fn a_replaced_file_keeps_the_mode_it_had() {
    for mode in [0o600, 0o640, 0o644, 0o755] {
        let directory = Directory::new(&format!("keeps-{mode:o}"));
        directory.file("kept.txt", "before\n", mode);
        let path = directory.workspace().existing("kept.txt").unwrap();
        let original = path.open_regular().unwrap();
        let permissions = original.metadata().unwrap().permissions();

        path.replace_with(
            Some(permissions),
            Some(&original),
            |file| std::io::Write::write_all(file, b"after\n"),
            |_| true,
        )
        .unwrap();

        assert_eq!(directory.mode("kept.txt"), mode);
        assert_eq!(
            fs::read_to_string(directory.0.join("kept.txt")).unwrap(),
            "after\n"
        );
    }
}

#[test]
fn a_new_file_is_made_with_the_ordinary_creation_mode() {
    let directory = Directory::new("new-file");
    let path = directory.workspace().creatable("fresh.txt").unwrap();

    path.replace_with(
        None,
        None,
        |file| std::io::Write::write_all(file, b"new\n"),
        |_| true,
    )
    .unwrap();

    assert_eq!(
        directory.mode("fresh.txt"),
        ordinary_creation_mode("new-file")
    );
}

#[test]
fn the_same_file_whose_content_the_caller_no_longer_accepts_is_not_replaced() {
    // Identity alone does not see a file rewritten in place: the caller is
    // asked about the content of the file at the name, and its no keeps the
    // file and leaves nothing beside it.
    let directory = Directory::new("content-refused");
    directory.file("kept.txt", "rewritten in place\n", 0o644);
    let path = directory.workspace().existing("kept.txt").unwrap();
    let original = path.open_regular().unwrap();
    let permissions = original.metadata().unwrap().permissions();

    let mut asked = None;
    let problem = path
        .replace_with(
            Some(permissions),
            Some(&original),
            |file| std::io::Write::write_all(file, b"replacement\n"),
            |current| {
                let mut text = String::new();
                asked = std::io::Read::read_to_string(current, &mut text)
                    .ok()
                    .map(|_| text);
                false
            },
        )
        .expect_err("the caller refused the content");

    assert!(matches!(problem, PathError::Changed { .. }), "{problem:?}");
    assert_eq!(asked.as_deref(), Some("rewritten in place\n"));
    assert_eq!(
        fs::read_to_string(directory.0.join("kept.txt")).unwrap(),
        "rewritten in place\n"
    );
    let names: Vec<_> = fs::read_dir(&directory.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, [std::ffi::OsString::from("kept.txt")]);
}
