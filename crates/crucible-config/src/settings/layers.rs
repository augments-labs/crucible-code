//! The files a setting can come from, and the order they are read in.
//!
//! Three, nearest last, so that merging them in order leaves the nearest layer
//! holding what it set. The command line is the fourth and nearest layer and is
//! not a file, so it is applied by the wiring above rather than here.
//!
//! A file that is not there is not an error — most machines have none of these
//! and crucible has to run anyway. A file that *is* there and cannot be read is
//! a different thing and says so: silently skipping it would turn a permissions
//! mistake into settings that mysteriously stopped applying.
//!
//! All three are read before the first frame is drawn, so a layer is opened in
//! a way that cannot wait: what is not there is skipped, and what is there is
//! opened without waiting for a peer and then asked, through the handle, whether
//! it is an ordinary file. A pipe left at one of these names is refused, and the
//! error names the file, rather than holding the run.
//!
//! A symbolic link is followed rather than refused, deliberately: a settings file
//! is a person's own, and pointing one at a checkout of dotfiles is an ordinary
//! thing to have done. That is the open below at every path, and what differs is
//! what reached it. A run opens the home file as private state before these
//! layers are read, and that refuses a link there, so on that path a link is
//! followed only at the two project files. The two report flags read with
//! nothing before them — `--extensions` at the home file alone and `--sandbox` at
//! all three — and follow a link at whichever of them is planted.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use crate::MAX_DOCUMENT_BYTES;
use crate::document::{Document, Origin};
use crate::error::ConfigError;
use crate::home::Home;

use super::Settings;

/// What a configuration file is called, in the home directory and in a project.
const FILE: &str = "config.json";

/// The directory a project keeps crucible's files in.
const PROJECT: &str = ".crucible";

/// The nearer project file for ordinary non-authority overrides.
const LOCAL: &str = "config.local.json";

impl Settings {
    /// Reads whichever of the three files exist and resolves them.
    ///
    /// `workspace` is the directory crucible was started in, which is what makes
    /// a project's settings a property of the checkout rather than of the shell
    /// that launched it.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Unreadable`] for a file that is there and cannot be read
    /// as settings — it will not open, or what opened is not an ordinary file —
    /// and one of the document errors for a file that opens and is not a
    /// configuration document.
    pub fn read(home: &Home, workspace: &Path) -> Result<Self, ConfigError> {
        let mut documents = Vec::new();

        for (path, origin) in files(home, workspace) {
            documents.extend(read_one(&path, origin)?);
        }

        Self::resolve_checked(documents)
    }

    /// Reads the home file alone, whatever checkout crucible was started in.
    ///
    /// For a question the two project layers are not allowed to answer. Those
    /// keys widen, so a value in a committed file is refused at the boundary
    /// anyway — but reading a project file only to be told it may not speak
    /// lets one that will not parse withhold an answer it was never going to
    /// contribute to. This opens the one file that decides.
    ///
    /// # Errors
    ///
    /// As [`Settings::read`], for that one file.
    pub fn read_home(home: &Home) -> Result<Self, ConfigError> {
        let document = read_one(&user(home), Origin::User)?;

        Self::resolve_checked(document.into_iter().collect())
    }
}

/// One layer, or nothing where that file is not on this machine.
fn read_one(path: &Path, origin: Origin) -> Result<Option<Document>, ConfigError> {
    // Named by its whole path. Two of these live in the project and one does
    // not, so `config.json` alone would leave the reader working out which file
    // the message is about.
    let file: Box<str> = path.display().to_string().into();

    let opened = match open_layer(path) {
        Ok(opened) => opened,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ConfigError::Unreadable { file, source }),
    };
    let mut text = String::new();
    opened
        .take((MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|source| ConfigError::Unreadable {
            file: file.clone(),
            source,
        })?;
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(ConfigError::TooLarge {
            file,
            maximum: MAX_DOCUMENT_BYTES,
        });
    }

    Document::parse(&text, &file, origin).map(Some)
}

/// Opens one layer as a handle that can be asked what it is, without waiting
/// for a peer that may never come.
///
/// The proof is taken on the descriptor this returns, never on the name it was
/// given: asking the name again is a second lookup, and what it would answer is
/// about whatever now sits there rather than about the file the read is about to
/// come from. `metadata` on a `File` is one `fstat` against the descriptor, so
/// the answer belongs to the same object the bytes come from.
#[cfg(unix)]
fn open_layer(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    // Opening a pipe for reading waits until somebody writes, so the descriptor
    // is asked for without waiting and the ordinary-file proof is what refuses
    // what opened. On a regular file the flag bears only on the open, which
    // returns before any read happens.
    //
    // `O_NOFOLLOW` is deliberately absent, though the private-state opens in
    // `crucible-privacy` take it. Those files are private state, where the file
    // has to be the one that was named; a file at any of these paths is a
    // person's own configuration, which they may have pointed wherever they like
    // — a link into a checkout of dotfiles is an ordinary way to keep one.
    // Adding the flag would refuse a pipe promptly and that along with it, at
    // every one of the call sites named above, which is the worse breakage of
    // the two: it is silent, and it only reaches a machine whose settings were
    // working.
    //
    // `std` will carry a raw flag through `custom_flags` and names none of them,
    // so the one value is asked for by name instead. `libc` is here for that
    // alone: it brings no open of its own, and the open below is `std`'s, which
    // supplies it. `custom_flags` adds to what `read(true)` already asked for
    // rather than replacing it, and leaves the close-on-exec to `std`, which
    // sets it on every open it makes, so this is the whole of the request.
    //
    // Writing the number here would have been a platform branch: this bit is
    // not in the same place on every platform, so the literal has to be chosen
    // per target, and a target left out of the set has to fail rather than fall
    // back to one that opens with a bit meaning something else. Naming it
    // settles both — the value is wherever the pinned release puts it, and a
    // platform that release does not name it for does not compile here.
    //
    // The pipe test below is the other half of that: it fails by name if the
    // value on the platform running it is not the one that opens without
    // waiting.
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;

    ordinary(&file)?;
    Ok(file)
}

/// Opens one layer as a handle that can be asked what it is.
///
/// Windows has no flag that stops this open waiting on a peer: a name under a
/// directory is not opened as a pipe, so the plain open is the whole of what the
/// platform has to give, and the same proof as elsewhere is what refuses a
/// directory that opened. It is a narrower question here than the one the Unix
/// arm asks, because `is_file` answers on this platform whether the descriptor
/// is neither a directory nor a reparse point: a device that opened is not
/// turned away by it.
///
/// A reparse point is followed rather than refused, for the reason the Unix arm
/// does not take `O_NOFOLLOW`. Opening resolves it, so the handle the proof
/// reads is the file at the far end, which is the one the person meant.
#[cfg(not(unix))]
fn open_layer(path: &Path) -> io::Result<File> {
    let file = File::open(path)?;

    ordinary(&file)?;
    Ok(file)
}

/// Refuses a handle that is not one ordinary file, whatever opened it.
fn ordinary(file: &File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a settings file is not an ordinary file",
        ));
    }
    Ok(())
}

/// Where a project conventionally keeps its nearer non-authority settings.
///
/// Named here beside the layers it is read from so there is one answer to which
/// filename has that precedence. An ignore rule is a convention, not trust:
/// repositories can commit this file, so it cannot carry authority.
#[must_use]
pub fn local(workspace: &Path) -> PathBuf {
    workspace.join(PROJECT).join(LOCAL)
}

/// Where this machine keeps the settings that follow the person rather than the
/// checkout.
///
/// The other layer crucible itself writes to, and named here for the same
/// reason: which file a model chosen at the prompt lands in is the same fact as
/// which file it is read back from, and two answers to it would be a choice
/// written where nothing looks.
#[must_use]
pub fn user(home: &Home) -> PathBuf {
    home.path().join(FILE)
}

/// The three files, in the order they merge: furthest first.
fn files(home: &Home, workspace: &Path) -> [(PathBuf, Origin); 3] {
    let project = workspace.join(PROJECT);

    [
        (home.path().join(FILE), Origin::User),
        (project.join(FILE), Origin::Project),
        (project.join(LOCAL), Origin::ProjectLocal),
    ]
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::time::Duration;

    use super::*;
    use crate::sample::Scratch;

    /// A home directory inside the scratch tree, found the way the binary finds
    /// one.
    fn home(scratch: &Scratch) -> Home {
        let named = scratch.text("home");
        Home::find(&move |wanted| (wanted == crate::HOME).then(|| OsString::from(named.clone())))
            .expect("an absolute path")
    }

    /// A symbolic link at `link` pointing at `target`, which is a file.
    ///
    /// Windows has a call per kind and no way to make one for a target that is
    /// not there yet, which is why the kind is in the name rather than read off
    /// the target. Making one there is a privilege — developer mode, or an
    /// elevated shell — and failing loudly is right: a link that was not made
    /// turns this into a test that passes because there was nothing behind it.
    fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);

        made.expect("a symbolic link: on Windows this needs developer mode");
    }

    /// How long a read of planted settings is given to answer before the test
    /// says what it was still waiting for.
    ///
    /// Not a deadline anything is measured against: opening a name and asking a
    /// handle what it is is a few syscalls, and the channel hands the answer
    /// over the moment it arrives. The harness reports elapsed time to a
    /// hundredth of a second, so a passing run is recorded as `0.00s` and what
    /// the evidence bounds is the run under that reported resolution, not a
    /// millisecond. It is the point at which an open that has stopped answering
    /// gives up and names itself, which is the whole reason it is here. Thirty
    /// seconds rather than two, because a shared runner hands a thread out when
    /// it feels like it, and a short window buys a suite that failed on a busy
    /// machine and passed on a quiet one.
    #[cfg(unix)]
    const PATIENCE: Duration = Duration::from_secs(30);

    /// Runs `read` where this test can leave it behind, and fails by name if it
    /// has not answered within [`PATIENCE`].
    ///
    /// An open waiting for a writer nobody is going to send never comes back, so
    /// the thread running it stays where it is; abandoning it is what turns the
    /// wait into a reported failure instead of a suite that never finishes. The
    /// process ends it with the test binary, and it holds no descriptor, so
    /// nothing it is waiting on is kept open by it.
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

    #[test]
    fn a_machine_with_no_configuration_files_at_all_is_not_an_error() {
        // The common case, and the one that must not need a file to exist:
        // crucible is usable before anybody has configured anything.
        let scratch = Scratch::new("layers-none");

        let settings = Settings::read(&home(&scratch), scratch.root()).expect("nothing to read");

        assert_eq!(settings.model("anthropic"), None);
    }

    #[test]
    fn each_layer_takes_the_one_nearer_the_work() {
        let scratch = Scratch::new("layers-order");
        scratch.write(
            "home/config.json",
            r#"{"providers":{"a":{"model":"user"}}}"#,
        );

        let settings = Settings::read(&home(&scratch), scratch.root()).expect("one file");
        assert_eq!(settings.model("a"), Some("user"));

        scratch.write(
            ".crucible/config.json",
            r#"{"providers":{"a":{"model":"project"}}}"#,
        );
        let settings = Settings::read(&home(&scratch), scratch.root()).expect("two files");
        assert_eq!(settings.model("a"), Some("project"));

        scratch.write(
            ".crucible/config.local.json",
            r#"{"providers":{"a":{"model":"local"}}}"#,
        );
        let settings = Settings::read(&home(&scratch), scratch.root()).expect("three files");
        assert_eq!(settings.model("a"), Some("local"));
    }

    #[test]
    fn the_local_file_still_holds_overrides_that_carry_no_authority() {
        // The local filename remains a nearer layer for ordinary preferences;
        // only settings that widen authority or select secrets are refused.
        let scratch = Scratch::new("layers-local");
        let path = local(scratch.root());

        fs::create_dir_all(path.parent().expect("a directory to write into"))
            .expect("a writable temporary directory");
        fs::write(&path, r#"{"providers": {"a": {"model": "local"}}}"#)
            .expect("a writable temporary directory");

        let settings = Settings::read(&home(&scratch), scratch.root()).expect("a layer it reads");

        assert_eq!(settings.model("a"), Some("local"));
    }

    #[test]
    fn a_file_that_is_not_a_document_is_refused_by_the_name_it_has_on_disk() {
        let scratch = Scratch::new("layers-bad");
        scratch.write(".crucible/config.json", r#"{"providers": 1}"#);

        let err = Settings::read(&home(&scratch), scratch.root()).unwrap_err();

        // The whole path, because two of the three files are called
        // `config.json` and one of them is somewhere else entirely. Joined
        // rather than written out, so this asks for the path the reader will be
        // shown rather than for the separator Unix happens to use.
        let named = Path::new(".crucible").join("config.json");
        let said = err.to_string();
        assert!(said.contains(&named.display().to_string()), "got {said}");
        assert!(said.contains("providers"), "got {said}");
    }

    #[test]
    fn a_file_that_is_there_and_will_not_open_is_reported_rather_than_skipped() {
        // A directory where a file should be: present, so not the missing-file
        // case, and unreadable for a reason nobody would guess from settings
        // that simply stopped applying.
        let scratch = Scratch::new("layers-shut");
        scratch.make(".crucible/config.local.json");

        let err = Settings::read(&home(&scratch), scratch.root()).unwrap_err();

        assert!(matches!(err, ConfigError::Unreadable { .. }), "got {err:?}");
    }

    #[test]
    fn a_configuration_file_over_the_byte_bound_is_refused_before_parsing() {
        let scratch = Scratch::new("layers-too-large");
        scratch.write(".crucible/config.json", &" ".repeat(MAX_DOCUMENT_BYTES + 1));

        let err = Settings::read(&home(&scratch), scratch.root()).unwrap_err();

        assert!(matches!(
            err,
            ConfigError::TooLarge {
                maximum: MAX_DOCUMENT_BYTES,
                ..
            }
        ));
    }

    /// A pipe standing where a project's settings file is read waits for a writer
    /// that is not coming, and the layers are read before the first frame is
    /// drawn, so one planted there by anything able to write in that directory
    /// would hold the run with no bound. It is refused as soon as the handle is
    /// asked what it opened, and the error names the file it was asked about.
    #[cfg(unix)]
    #[test]
    fn a_pipe_where_settings_are_read_is_refused_without_waiting_for_a_writer() {
        let scratch = Scratch::new("layers-pipe");
        let project = scratch.make(".crucible");
        let named = project.join(LOCAL);
        let made = std::process::Command::new("mkfifo")
            .arg(&named)
            .status()
            .expect("mkfifo is available on Unix");
        assert!(made.success());

        // Asked on a thread of its own because the read under test is the one
        // thing here that may never come back, and a test that waits for it on
        // this thread waits for it forever.
        let wanted = scratch.text("home");
        let root = scratch.root().to_path_buf();
        let read = answered(
            "reading a pipe standing where settings are read",
            move || {
                let home = Home::find(&move |name| {
                    (name == crate::HOME).then(|| OsString::from(wanted.clone()))
                })
                .expect("an absolute path");

                Settings::read(&home, &root)
            },
        );

        let err = read.unwrap_err();
        assert!(matches!(err, ConfigError::Unreadable { .. }), "got {err:?}");
        // The whole path, because two of the three files are called
        // `config.json` and one of them is somewhere else entirely.
        assert!(
            err.to_string().contains(&named.display().to_string()),
            "got {err}"
        );
    }

    /// A project's settings file a person pointed at with a symbolic link is
    /// still read.
    ///
    /// A project file is ordinary configuration a user writes, and a link into a
    /// checkout of dotfiles is a normal way to keep one. This is the other half
    /// of the refusal above, and the reason the open does not take the flag the
    /// private-state opens take: a change that reads a pipe promptly and refuses
    /// this has broken a setup that works.
    ///
    /// Only a project layer is what this covers, and the home file is a different
    /// case with a different answer that is not decided here: a run reaches it
    /// after opening that path as private state, which refuses a link, while
    /// `--extensions` and `--sandbox` read it with nothing before them and follow
    /// one.
    #[test]
    fn a_projects_settings_file_reached_through_a_symbolic_link_is_still_read() {
        let scratch = Scratch::new("layers-link");
        scratch.write("kept.json", r#"{"providers":{"a":{"model":"linked"}}}"#);
        let project = scratch.make(".crucible");
        symlink(scratch.at("kept.json"), project.join(LOCAL));

        let settings = Settings::read(&home(&scratch), scratch.root()).expect("a linked layer");

        assert_eq!(settings.model("a"), Some("linked"));
    }
}
