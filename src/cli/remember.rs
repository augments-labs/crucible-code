//! Writing one answer into a configuration file.
//!
//! Into two files: the rule an answer of `always` leaves behind goes into the
//! layer a project keeps out of git, and what `/model` and `/effort` pick goes
//! into the one at home, because which model to ask and how hard to think are
//! facts about who is running crucible rather than about the checkout.
//!
//! The crate below decides what a file may say and what one more answer leaves
//! it looking like. This opens it, and puts the answer back.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crucible_config::ConfigError;
use crucible_core::{Effort, Minted};

/// What can stop an answer of `always` from lasting.
///
/// None of these ends the turn. The call the user allowed still runs and the
/// engine still remembers it for the session; what is lost is the part that
/// outlives the process, and the report says which rule that was so it can be
/// written by hand.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RememberError {
    #[error("{file} could not be written: {source}")]
    Unwritable { file: Box<str>, source: io::Error },

    #[error(transparent)]
    Unusable(#[from] ConfigError),
}

/// Adds `rule` to the `allow` list in `file`.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the rule and nothing else.
pub(crate) fn allowing(file: &Path, rule: &Minted) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::allowing(text, named, rule)
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

    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        // Nothing there yet, which is what most projects look like. The empty
        // text is what the crate below reads as "write a whole file".
        Err(source) if source.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(unwritable(source)),
    };

    let written = splice(&text, &named)?;

    put(file, &written).map_err(unwritable)
}

/// Replaces the file, or leaves whatever is there untouched.
fn put(file: &Path, text: &str) -> io::Result<()> {
    let directory = file.parent().unwrap_or_else(|| Path::new(""));

    // Made rather than required: a project nobody has configured has no
    // `.crucible` at all, and this is the first thing to go in it.
    if !directory.as_os_str().is_empty() {
        fs::create_dir_all(directory)?;
    }

    // What the file is held at now, before it is replaced. A rename puts a new
    // file where the old one was rather than new bytes inside it, so whatever
    // the user narrowed this to would be widened back to what the account
    // creates a file as — and this is the file that says what may run without
    // being asked about, so widening who can write to it is the last thing
    // answering `always` should do. `None` where there is nothing there yet,
    // which is the case the account's own default is right for.
    let held = fs::metadata(file).map(|found| found.permissions()).ok();

    let beside = Beside::new(directory);
    beside.write(text)?;

    if let Some(held) = held {
        beside.hold(held)?;
    }

    beside.over(file)
}

/// The document, written where it does not belong yet.
///
/// Written beside the file and renamed over it, because a write that stops
/// part-way through leaves half a document, and half a document is a file
/// crucible refuses to start from — so the failure would cost the user their
/// whole configuration rather than one rule.
///
/// The rename is what makes the replacement whole, so every step before it is
/// work that can fail with this file already on disk, holding the entire
/// permission document under a name nothing will ever look at again: the next
/// crucible reuses this process id only by coincidence. Removing it falls to a
/// guard rather than to an arm on each failure, because the steps between the
/// write and the rename are the kind that get added to, and a guard covers the
/// next one without being told it is there.
struct Beside {
    path: PathBuf,
    landed: bool,
}

impl Beside {
    /// A name beside the file to be replaced. The process id in it is what
    /// keeps two crucibles in one checkout off each other's half-written file.
    fn new(directory: &Path) -> Self {
        Self {
            path: directory.join(format!(".writing.{}", std::process::id())),
            landed: false,
        }
    }

    fn write(&self, text: &str) -> io::Result<()> {
        fs::write(&self.path, text)
    }

    /// Held at what the file being replaced was held at.
    fn hold(&self, permissions: fs::Permissions) -> io::Result<()> {
        fs::set_permissions(&self.path, permissions)
    }

    /// Over the real file, which is the step that makes this the real file.
    fn over(mut self, file: &Path) -> io::Result<()> {
        fs::rename(&self.path, file)?;
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
        // part they need: the rule was not remembered. A tidy-up that failed
        // has nowhere to go that would not be in front of that, so it goes
        // nowhere. The same silence covers the write that never made a file.
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests;
