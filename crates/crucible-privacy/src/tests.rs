use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use crate::{append, sync_parent, tighten};
use crate::{
    create_append, create_write, directory, lock, open_read, open_read_append, open_read_ordinary,
    replace, replace_in,
};

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
    assert_eq!(
        open_read_ordinary(&link).unwrap_err().kind(),
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

/// The one open that takes a file however it is named still reads it, and
/// through the name it was handed.
#[test]
fn an_ordinary_file_with_another_hard_name_is_read_where_its_names_are_not_asked() {
    use std::io::Read as _;

    let scratch = Scratch::new("hard-name-ordinary");
    directory(&scratch.0).unwrap();
    let source = scratch.0.join("source");
    let alias = scratch.0.join("alias");
    fs::write(&source, "under two names").unwrap();
    fs::hard_link(&source, &alias).unwrap();

    let mut read = String::new();
    open_read_ordinary(&alias)
        .unwrap()
        .read_to_string(&mut read)
        .unwrap();

    assert_eq!(read, "under two names");
}

/// A pipe where an ordinary file is read waits for a writer that is not
/// coming, so an open that waited would never come back.
#[cfg(unix)]
#[test]
fn a_pipe_where_an_ordinary_file_is_read_is_refused_without_waiting_for_a_writer() {
    let scratch = Scratch::new("ordinary-pipe");
    fs::create_dir_all(&scratch.0).unwrap();
    let at = scratch.0.join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&at)
        .status()
        .expect("mkfifo is available on Unix");
    assert!(made.success());

    let opened = answered("reading a pipe standing where a file is read", move || {
        open_read_ordinary(&at).map(drop)
    });

    assert_eq!(
        opened.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput,
        "a pipe standing where a file is read was opened"
    );
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

/// Another crucible reading the file at the moment it is replaced, which is
/// how a renewed sign-in used to go unwritten on Windows: the rotated tokens
/// were refused their place and the spent ones stayed on disk.
#[test]
fn a_destination_open_for_reading_is_replaced_all_the_same() {
    use std::io::Read as _;

    let scratch = Scratch::new("replace-read");
    directory(&scratch.0).unwrap();
    let partial = scratch.0.join("partial");
    let destination = scratch.0.join("destination");
    fs::write(&destination, "old").unwrap();
    let mut reading = fs::File::open(&destination).unwrap();
    let mut prepared = create_write(&partial).unwrap();
    prepared.write_all(b"new").unwrap();
    prepared.sync_all().unwrap();
    drop(prepared);

    replace(&partial, &destination).unwrap();

    assert_eq!(fs::read_to_string(&destination).unwrap(), "new");
    let mut kept = String::new();
    reading.read_to_string(&mut kept).unwrap();
    assert_eq!(kept, "old", "the reader lost the file it opened");
}

/// A file read and not trusted, such as a session index, is one another
/// crucible replaces whole while this one reads it, and the read must not cost
/// that replace.
#[test]
fn a_destination_open_as_an_ordinary_file_is_replaced_all_the_same() {
    use std::io::Read as _;

    let scratch = Scratch::new("replace-read-ordinary");
    directory(&scratch.0).unwrap();
    let partial = scratch.0.join("partial");
    let destination = scratch.0.join("destination");
    fs::write(&destination, "old").unwrap();
    let mut reading = open_read_ordinary(&destination).unwrap();
    let mut prepared = create_write(&partial).unwrap();
    prepared.write_all(b"new").unwrap();
    prepared.sync_all().unwrap();
    drop(prepared);

    replace(&partial, &destination).unwrap();

    assert_eq!(fs::read_to_string(&destination).unwrap(), "new");
    let mut kept = String::new();
    reading.read_to_string(&mut kept).unwrap();
    assert_eq!(kept, "old", "the reader lost the file it opened");
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

/// A pipe standing where a lock is taken is no file to lock, and opening one
/// for writing can wait on a reader that is not coming.
#[cfg(unix)]
#[test]
fn a_pipe_where_a_lock_is_taken_is_refused_without_waiting() {
    let scratch = Scratch::new("lock-pipe");
    fs::create_dir_all(&scratch.0).unwrap();
    let at = scratch.0.join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&at)
        .status()
        .expect("mkfifo is available on Unix");
    assert!(made.success());

    let opened = answered("locking a pipe standing where a lock is kept", move || {
        lock(&at).map(drop)
    });

    assert_eq!(
        opened.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput,
        "a pipe standing where a lock is kept was opened"
    );
}

/// A link to nothing where a lock is taken would have the lock made wherever
/// it leads.
#[cfg(unix)]
#[test]
fn a_dangling_link_where_a_lock_is_taken_makes_nothing_where_it_leads() {
    let scratch = Scratch::new("lock-dangling");
    fs::create_dir_all(&scratch.0).unwrap();
    let nowhere = scratch.0.join("nowhere");
    let link = scratch.0.join("link");
    std::os::unix::fs::symlink(&nowhere, &link).unwrap();

    assert!(lock(&link).is_err());
    assert!(!nowhere.exists());
}

/// The names in `directory`, sorted.
fn names(directory: &Path) -> Vec<OsString> {
    let mut names: Vec<OsString> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    names
}

#[test]
fn a_file_replaced_in_its_directory_holds_what_was_written_last_and_no_draft_is_left() {
    let scratch = Scratch::new("replace-in");
    let held = scratch.0.join("held");
    let file = held.join("settings.json");

    replace_in(&held, OsStr::new("settings.json"), b"first").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&held, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    }
    replace_in(&held, OsStr::new("settings.json"), b"second").unwrap();

    assert_eq!(fs::read(&file).unwrap(), b"second");
    assert_eq!(names(&held), ["settings.json"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&held).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn a_file_replaced_in_its_directory_is_named_by_one_name_in_it() {
    let scratch = Scratch::new("replace-in-names");
    let held = scratch.0.join("held");
    directory(&held).unwrap();

    for name in ["", ".", "..", "inner/settings.json", "../settings.json"] {
        assert!(replace_in(&held, OsStr::new(name), b"new").is_err());
    }
    assert!(names(&held).is_empty());
    assert_eq!(names(&scratch.0), ["held"]);
}

/// A rename that cannot land, over a directory with something in it, is a
/// failure after the draft was written.
#[test]
fn a_file_that_cannot_be_replaced_in_its_directory_leaves_no_draft() {
    let scratch = Scratch::new("replace-in-refused");
    let held = scratch.0.join("held");
    let occupied = held.join("settings.json");
    fs::create_dir_all(occupied.join("inside")).unwrap();

    assert!(replace_in(&held, OsStr::new("settings.json"), b"new").is_err());
    assert_eq!(names(&held), ["settings.json"]);
    assert_eq!(names(&occupied), ["inside"]);
}

#[cfg(unix)]
#[test]
fn a_link_where_the_directory_is_named_refuses_the_replace_and_writes_nothing_where_it_leads() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("replace-in-directory-link");
    fs::create_dir_all(&scratch.0).unwrap();
    let outside = scratch.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    let link = scratch.0.join("held");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let refused = replace_in(&link, OsStr::new("settings.json"), b"new").unwrap_err();
    assert!(
        refused.kind() == std::io::ErrorKind::InvalidInput
            && refused.to_string() == "private state cannot be a symbolic link",
        "the refusal does not say a link stands where the directory is named"
    );
    assert!(names(&outside).is_empty());
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
}

#[cfg(unix)]
#[test]
fn a_link_where_the_file_is_named_refuses_the_replace_and_changes_nothing_where_it_leads() {
    let scratch = Scratch::new("replace-in-file-link");
    let held = scratch.0.join("held");
    directory(&held).unwrap();
    let outside = scratch.0.join("outside");
    fs::write(&outside, "kept").unwrap();
    let link = held.join("settings.json");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    assert!(replace_in(&held, OsStr::new("settings.json"), b"new").is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "kept");
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(names(&held), ["settings.json"]);
}

/// A process in the directory's parent exchanging the directory for a link to
/// another one, as fast as it can, while the file is replaced: every write
/// lands in the directory that was opened, and none where the link leads.
#[cfg(target_os = "linux")]
#[test]
fn a_directory_exchanged_for_a_link_while_its_file_is_replaced_keeps_every_write() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread::JoinHandle;

    use rustix::fs::{CWD, RenameFlags};

    /// Stops and joins the exchanging thread however the test ends.
    struct Exchanging {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for Exchanging {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    let scratch = Scratch::new("replace-in-exchanged");
    let held = scratch.0.join("profile").join("held");
    directory(&held).unwrap();
    let outside = scratch.0.join("outside");
    fs::create_dir(&outside).unwrap();
    let link = scratch.0.join("profile").join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let exchanged = Arc::new(AtomicUsize::new(0));
    let exchanging = Exchanging {
        stop: Arc::clone(&stop),
        thread: Some(std::thread::spawn({
            let (held, link, exchanged) = (held.clone(), link.clone(), Arc::clone(&exchanged));
            move || {
                while !stop.load(Ordering::Relaxed) {
                    if rustix::fs::renameat_with(
                        CWD,
                        held.as_path(),
                        CWD,
                        link.as_path(),
                        RenameFlags::EXCHANGE,
                    )
                    .is_ok()
                    {
                        exchanged.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        })),
    };
    for _ in 0..2_000 {
        let _ = replace_in(&held, OsStr::new("settings.json"), b"new");
    }
    drop(exchanging);

    assert!(
        exchanged.load(Ordering::Relaxed) > 0,
        "the directory was never exchanged"
    );
    assert!(
        names(&outside).is_empty(),
        "a write reached the directory the link leads to"
    );
}
