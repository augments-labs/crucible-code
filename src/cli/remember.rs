//! Writing one answer into a configuration file.
//!
//! `/model` and `/effort` write into the file at home, because which model to
//! ask and how hard to think are facts about who is running crucible rather
//! than about the checkout.
//!
//! The crate below decides what a file may say and what one more answer leaves
//! it looking like. This opens it, and puts the answer back.

use std::fs::{self, File, TryLockError};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crucible_config::ConfigError;
use crucible_core::Effort;

#[cfg(test)]
use crucible_core::Minted;

/// What can stop a configuration choice from lasting.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RememberError {
    #[error("{file} could not be written: {source}")]
    Unwritable { file: Box<str>, source: io::Error },

    #[error("{file} is being changed by another crucible; try again")]
    Busy { file: Box<str> },

    #[error(transparent)]
    Unusable(#[from] ConfigError),
}

/// Adds `rule` to the `allow` list in `file`.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the rule and nothing else.
#[cfg(test)]
fn allowing(file: &Path, rule: &Minted) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::allowing(text, named, rule)
    })
}

/// Writes `theme` down as the table to draw with from now on.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the theme and nothing else.
pub(crate) fn drawing(file: &Path, theme: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::drawing(text, named, theme)
    })
}

/// Writes `provider` down as the one to ask from now on.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the name and nothing else.
pub(crate) fn asking(file: &Path, provider: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::asking(text, named, provider)
    })
}

/// Writes `model` down as the one to ask `provider` for.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the choice and nothing else.
pub(crate) fn choosing(file: &Path, provider: &str, model: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::choosing(text, named, provider, model)
    })
}

/// Writes `effort` down as how hard to think for everything asked of
/// `provider`.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the rung and nothing else.
pub(crate) fn thinking(file: &Path, provider: &str, effort: Effort) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::thinking(text, named, provider, effort)
    })
}

/// Reads the file, hands what it holds to `splice`, and puts back what comes
/// out.
///
/// The three above differ in that one call and in nothing else — which file is
/// opened, what a missing one means, and what a half-written one would cost are
/// one answer for all of them, and three copies of it would be three places to
/// fix the day the answer changes.
fn answering(
    file: &Path,
    splice: impl FnOnce(&str, &str) -> Result<String, ConfigError>,
) -> Result<(), RememberError> {
    // Named the way the user would name it, because that is what a refusal
    // from below tells them to open.
    let named = file.display().to_string();
    let unwritable = |source| RememberError::Unwritable {
        file: named.clone().into(),
        source,
    };

    let directory = file.parent().unwrap_or_else(|| Path::new(""));
    if !directory.as_os_str().is_empty() {
        crucible_privacy::directory(directory)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(&unwritable)?;
    }
    let _held = Held::take(file).map_err(|problem| match problem {
        TakeError::Io(source) => unwritable(source),
        TakeError::Busy => RememberError::Busy {
            file: named.clone().into(),
        },
    })?;

    match crucible_privacy::tighten(file) {
        Ok(_) => {}
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => {}
        Err(problem) => return Err(unwritable(problem.into_io())),
    }

    let opened = match File::open(file) {
        Ok(opened) => Some(opened),
        // Nothing there yet, which is what most projects look like. The empty
        // text is what the crate below reads as "write a whole file".
        Err(source) if source.kind() == io::ErrorKind::NotFound => None,
        Err(source) => return Err(unwritable(source)),
    };
    let mut text = String::new();
    if let Some(opened) = opened {
        opened
            .take((crucible_config::MAX_DOCUMENT_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(unwritable)?;
        if text.len() > crucible_config::MAX_DOCUMENT_BYTES {
            return Err(ConfigError::TooLarge {
                file: named.clone().into(),
                maximum: crucible_config::MAX_DOCUMENT_BYTES,
            }
            .into());
        }
    }

    let written = splice(&text, &named)?;

    put(file, &written).map_err(unwritable)
}

/// Replaces the file, or leaves whatever is there untouched.
fn put(file: &Path, text: &str) -> io::Result<()> {
    let directory = file.parent().unwrap_or_else(|| Path::new(""));

    // Made rather than required: a user nobody has configured has no crucible
    // directory yet, and this is the first thing to go in it.
    if !directory.as_os_str().is_empty() {
        crucible_privacy::directory(directory).map_err(crucible_privacy::PrivacyError::into_io)?;
    }

    let mut beside = Beside::new(directory)?;
    beside.write(text)?;
    beside.over(file)
}

/// The file all configuration replacements contend for.
struct Held {
    _file: File,
}

/// Why a lock could not be taken.
enum TakeError {
    Io(io::Error),
    Busy,
}

impl Held {
    fn take(file: &Path) -> Result<Self, TakeError> {
        let lock = lock_name(file);
        let held = crucible_privacy::lock(&lock)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(TakeError::Io)?;

        for _ in 0..250 {
            match held.try_lock() {
                Ok(()) => return Ok(Self { _file: held }),
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(TryLockError::Error(problem)) => return Err(TakeError::Io(problem)),
            }
        }

        Err(TakeError::Busy)
    }
}

fn lock_name(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// The document, written where it does not belong yet.
///
/// Written beside the file and renamed over it, because a write that stops
/// part-way through leaves half a document, and half a document is a file
/// crucible refuses to start from — so the failure would cost the user their
/// whole configuration rather than one setting.
///
/// The rename is what makes the replacement whole, so every step before it is
/// work that can fail with this file already on disk, holding the entire
/// configuration document under a name nothing will ever look at again: the next
/// crucible reuses this process id only by coincidence. Removing it falls to a
/// guard rather than to an arm on each failure, because the steps between the
/// write and the rename are the kind that get added to, and a guard covers the
/// next one without being told it is there.
#[derive(Debug)]
struct Beside {
    path: PathBuf,
    file: Option<fs::File>,
    landed: bool,
}

impl Beside {
    /// An exclusively-created name beside the file to be replaced.
    fn new(directory: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        for _ in 0..32 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(".writing.{}.{sequence}", std::process::id()));
            match Self::at(path) {
                Ok(beside) => return Ok(beside),
                Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {}
                Err(problem) => return Err(problem),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not find a free sibling name for the configuration replacement",
        ))
    }

    fn at(path: PathBuf) -> io::Result<Self> {
        let file = crucible_privacy::create_write(&path)
            .map_err(crucible_privacy::PrivacyError::into_io)?;
        Ok(Self {
            path,
            file: Some(file),
            landed: false,
        })
    }

    fn write(&mut self, text: &str) -> io::Result<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("configuration temporary is already closed"))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    }

    /// Over the real file, which is the step that makes this the real file.
    fn over(mut self, file: &Path) -> io::Result<()> {
        drop(self.file.take());
        crucible_privacy::replace(&self.path, file)
            .map_err(crucible_privacy::PrivacyError::into_io)?;
        self.landed = true;

        Ok(())
    }
}

impl Drop for Beside {
    fn drop(&mut self) {
        if self.landed {
            return;
        }

        // Whatever went wrong is already on its way to the user, and it is the
        // part they need: the setting was not remembered. A tidy-up that failed
        // has nowhere to go that would not be in front of that, so it goes
        // nowhere. The same silence covers the write that never made a file.
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests;
