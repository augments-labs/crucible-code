use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

#[cfg(unix)]
use crate::{append, sync_parent, tighten};
use crate::{create_append, create_write, directory, lock, open_read, open_read_append, replace};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-privacy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn every_created_kind_and_its_directory_are_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("created");
    directory(&scratch.0).unwrap();
    drop(append(&scratch.0.join("append")).unwrap());
    drop(create_append(&scratch.0.join("fresh")).unwrap());
    drop(create_write(&scratch.0.join("write")).unwrap());
    drop(lock(&scratch.0.join("lock")).unwrap());

    assert_eq!(
        fs::metadata(&scratch.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["append", "fresh", "write", "lock"] {
        assert_eq!(
            fs::metadata(scratch.0.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "{name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn existing_open_permissions_are_tightened() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("tightened");
    fs::create_dir_all(&scratch.0).unwrap();
    fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o755)).unwrap();
    directory(&scratch.0).unwrap();

    let file = scratch.0.join("file");
    fs::write(&file, "secret").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(tighten(&file).unwrap());
    assert!(!tighten(&file).unwrap());
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn live_file_symlinks_are_refused_without_tightening_their_target() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let scratch = Scratch::new("file-link");
    fs::create_dir_all(&scratch.0).unwrap();
    let target = scratch.0.join("target");
    fs::write(&target, "outside").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let link = scratch.0.join("link");
    symlink(&target, &link).unwrap();

    assert!(append(&link).is_err());
    assert!(lock(&link).is_err());
    assert_eq!(
        open_read(&link).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(
        open_read_append(&link).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert!(tighten(&link).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "outside");
    assert_eq!(
        fs::metadata(target).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn an_existing_file_with_another_hard_name_is_not_opened_as_private_state() {
    let scratch = Scratch::new("hard-name");
    directory(&scratch.0).unwrap();
    let source = scratch.0.join("source");
    let alias = scratch.0.join("alias");
    let mut file = create_append(&source).unwrap();
    file.write_all(b"unchanged").unwrap();
    drop(file);
    fs::hard_link(&source, &alias).unwrap();

    assert_eq!(
        open_read(&alias).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(
        open_read_append(&alias).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(fs::read(source).unwrap(), b"unchanged");
}

#[cfg(unix)]
#[test]
fn live_directory_symlinks_are_refused_without_tightening_their_target() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let scratch = Scratch::new("directory-link");
    fs::create_dir_all(&scratch.0).unwrap();
    let target = scratch.0.join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    let link = scratch.0.join("link");
    symlink(&target, &link).unwrap();

    assert!(directory(&link).is_err());
    assert_eq!(
        fs::metadata(target).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn a_renamed_file_can_sync_the_directory_that_names_it() {
    let scratch = Scratch::new("parent-sync");
    directory(&scratch.0).unwrap();
    let partial = scratch.0.join("partial");
    let final_path = scratch.0.join("final");
    fs::write(&partial, "durable").unwrap();
    fs::rename(partial, &final_path).unwrap();

    sync_parent(&final_path).unwrap();
    assert_eq!(fs::read_to_string(final_path).unwrap(), "durable");
}

#[test]
fn replacement_consumes_the_prepared_file_and_changes_the_destination_whole() {
    let scratch = Scratch::new("replace");
    directory(&scratch.0).unwrap();
    let partial = scratch.0.join("partial");
    let destination = scratch.0.join("destination");
    let mut prepared = create_write(&partial).unwrap();
    prepared.write_all(b"new").unwrap();
    prepared.sync_all().unwrap();
    drop(prepared);
    fs::write(&destination, "old").unwrap();

    replace(&partial, &destination).unwrap();

    assert_eq!(fs::read_to_string(destination).unwrap(), "new");
    assert!(!partial.exists());
}

#[cfg(windows)]
#[test]
fn every_created_kind_remains_reachable_by_its_owner() {
    use std::io::Write as _;

    let scratch = Scratch::new("windows-owner");
    directory(&scratch.0).unwrap();
    let mut partial = create_write(&scratch.0.join("partial")).unwrap();
    partial.write_all(b"secret").unwrap();
    drop(partial);
    drop(lock(&scratch.0.join("lock")).unwrap());

    assert_eq!(fs::read(scratch.0.join("partial")).unwrap(), b"secret");
    assert!(fs::read_dir(&scratch.0).unwrap().count() >= 2);
}
