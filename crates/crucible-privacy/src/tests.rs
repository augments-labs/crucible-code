use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Duration;

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

/// How long a read of planted state is given to answer before the test says
/// what it was still waiting for.
///
/// Not a deadline anything is measured against: opening a name and asking for
/// its mode is three syscalls, and the channel hands the answer over the moment
/// it arrives. How far inside the window that happens is not something this
/// suite knows. The harness reports elapsed time to a hundredth of a second, so
/// a passing run is recorded as `0.00s` and what the evidence bounds is the run
/// under that reported resolution, not a millisecond. It is the point at which
/// an open that has stopped answering gives up and names itself, which is the
/// whole reason it is here. Thirty seconds rather than two, because a shared
/// runner hands a thread out when it feels like it, and a short window buys a
/// suite that failed on a busy machine and passed on a quiet one.
#[cfg(unix)]
const PATIENCE: Duration = Duration::from_secs(30);

/// Runs `read` where this test can leave it behind, and fails by name if it has
/// not answered within [`PATIENCE`].
///
/// An open waiting for a writer nobody is going to open never comes back, so
/// the thread running it stays where it is; abandoning it is what turns the
/// wait into a reported failure instead of a suite that never finishes. The
/// process ends it with the test binary, and it holds no descriptor, so nothing
/// it is waiting on is kept open by it.
#[cfg(unix)]
fn answered<T: Send + 'static>(what: &str, read: impl FnOnce() -> T + Send + 'static) -> T {
    let (said, inbox) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = said.send(read());
    });

    match inbox.recv_timeout(PATIENCE) {
        Ok(answered) => answered,
        Err(timed_out) => panic!("{what}: {timed_out} within {PATIENCE:?}"),
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

/// A pipe standing where a private file is tightened waits for a writer that is
/// not coming, and tightening private state is on the startup path, so one
/// planted there by a process able to write in this directory would hold the
/// run before it drew anything. It is refused instead: the name is opened
/// without waiting for a peer, and the ordinary-file proof is what then refuses
/// what opened.
///
/// Waited for under [`PATIENCE`] rather than joined. A blocked open is a
/// syscall that nothing outside it can interrupt, so the bound is the only
/// thing that ends one, and a bound held outside this test would leave the
/// suite waiting instead of reporting.
#[cfg(unix)]
#[test]
fn a_pipe_where_private_state_is_tightened_is_refused_without_waiting_for_a_writer() {
    let scratch = Scratch::new("tighten-pipe");
    fs::create_dir_all(&scratch.0).unwrap();
    let at = scratch.0.join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&at)
        .status()
        .expect("mkfifo is available on Unix");
    assert!(made.success());

    let tightened = answered(
        "tightening a pipe standing where private state is kept",
        move || tighten(&at),
    );

    assert_eq!(
        tightened.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput,
        "a pipe standing where private state is kept was tightened"
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
