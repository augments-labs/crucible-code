//! Writing one answer into a configuration file.
//!
//! `/model` and `/effort` write into the file at home, because which model to
//! ask and how hard to think are facts about who is running crucible rather
//! than about the checkout; so does a yes to a route whose vendor uses what is
//! sent, which only that file may hold.
//!
//! The crate below decides what a file may say and what one more answer leaves
//! it looking like. This opens it, and puts the answer back. A writer that
//! changes the installed agents or how a provider is reached hands back both
//! as the file then states them, which the run reads for the rest of it; one
//! the agents door calls, which no run reads after, hands back what the door
//! reports: the instances left as they were, and the one an add wrote.

use std::fs::{self, File, TryLockError};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crucible_config::ConfigError;
use crucible_models::{Effort, Speed};

#[cfg(test)]
use crucible_tools::Minted;

/// What can stop a configuration choice from lasting.
#[derive(Debug, thiserror::Error)]
pub enum RememberError {
    /// The file could not be opened, replaced or locked.
    #[error("{file} could not be written: {source}")]
    Unwritable {
        /// Which file, as the reader would spell it.
        file: Box<str>,
        /// What the platform refused.
        source: io::Error,
    },

    /// Another crucible held the file for longer than this one waited.
    #[error("{file} is being changed by another crucible; try again")]
    Busy {
        /// Which file, as the reader would spell it.
        file: Box<str>,
    },

    /// The file does not say what a configuration file may say.
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
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn drawing(file: &Path, theme: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::drawing(text, named, theme)
    })
}

/// Writes `word` down as what `row` is set to.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the setting and nothing else.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration
/// or the setting is not one the next start would read.
pub fn setting(file: &Path, row: &crucible_config::Row, word: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::setting(text, named, row, word)
    })
}

/// Persists the sandbox choice in the user configuration.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn sandboxing(file: &Path, enabled: bool) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::sandboxing(text, named, enabled)
    })
}

/// Writes down that the installed agent `instance` takes no new work and that
/// no provider is reached through it any longer, and changes nothing else in
/// `file`.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration
/// or names no such instance.
pub fn disabling(file: &Path, instance: &str) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        crucible_config::disabling(text, named, instance)
    })
}

/// Writes down that the installed agent `instance` is started on `model` at
/// `effort`, in one update of `file`, and changes nothing else in it.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration,
/// names no such instance, or would not be with the model written.
pub fn choosing_agent(
    file: &Path,
    instance: &str,
    model: &str,
    effort: Option<Effort>,
) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        crucible_config::choosing_agent_model(text, named, instance, Some(model))
            .and_then(|text| crucible_config::thinking_agent(&text, named, instance, effort))
    })
}

/// Writes down that the installed agent `instance` is started at `effort`,
/// and changes nothing else in `file`.
///
/// # Errors
///
/// As [`choosing_agent`].
pub fn thinking_agent(
    file: &Path,
    instance: &str,
    effort: Effort,
) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        crucible_config::thinking_agent(text, named, instance, Some(effort))
    })
}

/// Writes down that every installed agent in `instances` takes no new work and
/// reaches no provider, in one update of `file`, and changes nothing else in
/// it.
///
/// # Errors
///
/// As [`disabling`], where any of them would be refused, and then none is
/// written.
pub fn disabling_every(
    file: &Path,
    instances: &[&str],
) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        instances
            .iter()
            .try_fold(text.to_owned(), |text, instance| {
                crucible_config::disabling(&text, named, instance)
            })
    })
}

/// Writes into `file` that `provider` is reached through the installed agent
/// `instance`, and that the instance takes new work, the second last, in one
/// update; and changes nothing else in it.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration
/// or names no such instance.
pub fn connecting(
    file: &Path,
    provider: &str,
    instance: &str,
) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        crucible_config::connecting(text, named, provider, instance)
    })
}

/// Writes into `file` that `provider` is reached as `access` says and is the
/// one asked, in one update; and changes nothing else in it.
///
/// # Errors
///
/// As [`connecting`], and [`RememberError::Unusable`] where the file cannot
/// say so.
pub fn reaching(
    file: &Path,
    provider: &str,
    access: &crucible_config::Access,
) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        crucible_config::reaching(text, named, provider, access)
    })
}

/// [`reaching`], with `default`'s model and rung written in the same update
/// where the file names no model for `provider`.
///
/// # Errors
///
/// As [`reaching`].
pub fn reaching_defaulted(
    file: &Path,
    provider: &str,
    access: &crucible_config::Access,
    default: Option<(&str, Option<Effort>)>,
) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        let reached = crucible_config::reaching(text, named, provider, access)?;
        match default {
            Some((model, effort)) => {
                crucible_config::defaulting(&reached, named, provider, model, effort)
            }
            None => Ok(reached),
        }
    })
}

/// Writes `model` down as the one to ask `provider` for, at `effort`, where
/// the file names no model for `provider`; and changes nothing else in it.
///
/// # Errors
///
/// As [`choosing`].
pub fn defaulting(
    file: &Path,
    provider: &str,
    model: &str,
    effort: Option<Effort>,
) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::defaulting(text, named, provider, model, effort)
    })
}

/// The model and the rung `file` names for `provider` as it stands now; neither
/// where it cannot be read.
#[must_use]
pub fn chosen(file: &Path, provider: &str) -> (Option<String>, Option<Effort>) {
    let named = file.display().to_string();
    text_of(file, &named).map_or((None, None), |text| {
        crucible_config::chosen(&text, provider)
    })
}

/// Writes into `file` the installed agent `agent`, that `provider` is
/// reached through it, and that it takes new work, the last, in one update;
/// and changes nothing else in it.
///
/// # Errors
///
/// As [`connecting`], and [`RememberError::Unusable`] where the file already
/// names an instance called as `agent` is.
pub fn installing(
    file: &Path,
    provider: &str,
    agent: &crucible_config::ExternalAgent,
) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        crucible_config::installing(text, named, provider, agent)
    })
}

/// Writes into `file` that each installed agent `moved` names is started as
/// the last program beside it, where it still names the first, in one update,
/// and changes nothing else in it; gives the instances left as they are
/// because they name something else by now.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration
/// or names no such instance.
pub fn pointing(file: &Path, moved: &[(&str, &str, &str)]) -> Result<Vec<String>, RememberError> {
    answered(file, |text, named| {
        crucible_config::pointing(text, named, moved)
    })
}

/// Writes into `file`, in one update, what [`pointing`] writes for `moved`
/// and one more instance of the catalog agent `agent`, started as
/// `executable`, under the first name `file` leaves free when it is read under
/// its lock, and changes nothing else in it; gives the instances left as they
/// are because they name something else by now, and the name written. Where
/// either is refused, neither is written.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration
/// or names no such instance, or would not be configuration with the instance
/// in it.
pub fn adding(
    file: &Path,
    moved: &[(&str, &str, &str)],
    agent: &crucible_types::AgentKey,
    executable: &str,
) -> Result<(Vec<String>, String), RememberError> {
    answered(file, |text, named| {
        // Nothing to move in a file that may not be there yet.
        let (moved, left) = if moved.is_empty() {
            (text.to_owned(), Vec::new())
        } else {
            crucible_config::pointing(text, named, moved)?
        };
        let (written, instance) = crucible_config::adding(&moved, named, agent, executable)?;
        Ok((written, (left, instance)))
    })
}

/// Takes the installed agent `instance` out of `file`, and changes nothing
/// else in it.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration,
/// names no such instance, or still reaches a provider through it.
pub fn removing(file: &Path, instance: &str) -> Result<crucible_config::Routes, RememberError> {
    routing(file, |text, named| {
        crucible_config::removing(text, named, instance)
    })
}

/// Whether [`removing`] would take the installed agent `instance` out of
/// `file` as it stands now, writing nothing.
///
/// # Errors
///
/// As [`removing`] would, but for the file being held by another crucible,
/// which is not looked at.
pub fn removable(file: &Path, instance: &str) -> Result<(), RememberError> {
    let named = file.display().to_string();
    let text = text_of(file, &named)?;
    crucible_config::removing(&text, &named, instance)?;
    Ok(())
}

/// Writes down that picking a session up should stop asking about its size.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn unasked(file: &Path) -> Result<(), RememberError> {
    answering(file, crucible_config::unasked)
}

/// Writes `theme` down as the syntax theme fenced code is read in.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn syntax(file: &Path, theme: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::reading(text, named, theme)
    })
}

/// Writes down the yes to sending on `route`.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the yes and nothing else.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn accepting(file: &Path, route: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::accepting(text, named, route)
    })
}

/// Takes the yes to every route `gone` picks out of the file.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn forgetting(file: &Path, gone: impl Fn(&str) -> bool) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::forgetting(text, named, &gone)
    })
}

/// Writes `provider` down as the one to ask from now on.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the name and nothing else.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn asking(file: &Path, provider: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::asking(text, named, provider)
    })
}

/// Writes `model` down as the one to ask `provider` for.
///
/// The rung is taken out, and the speed too where the file named another
/// model; everything else already in the file stays where it was, byte for
/// byte. A file that is
/// not there yet becomes one holding the choice and nothing else.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn choosing(file: &Path, provider: &str, model: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::choosing(text, named, provider, model)
    })
}

/// Writes `effort` down as how hard to think for everything asked of
/// `provider`.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the rung and nothing else.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn thinking(file: &Path, provider: &str, effort: Effort) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::thinking(text, named, provider, effort)
    })
}

/// Writes down that `provider` asks `model` fast: the model and the speed
/// together, since the speed was chosen at that model's price. Everything
/// else already in the file stays where it was, byte for byte.
///
/// # Errors
///
/// [`RememberError::Busy`] when another crucible holds the file,
/// [`RememberError::Unwritable`] when it cannot be opened or replaced, and
/// [`RememberError::Unusable`] when what it already says is not configuration.
pub fn hastening(file: &Path, provider: &str, model: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::hastening(text, named, provider, model)
    })
}

/// Takes `provider`'s speed out of `file`, so it asks at standard.
///
/// # Errors
///
/// As [`hastening`].
pub fn slowing(file: &Path, provider: &str) -> Result<(), RememberError> {
    answering(file, |text, named| {
        crucible_config::slowing(text, named, provider)
    })
}

/// Reads the file, hands what it holds to `splice`, and puts back what comes
/// out.
///
/// Every writer above differs in that one call and in nothing else — which
/// file is opened, what a missing one means, and what a half-written one
/// would cost are one answer for all of them, and a copy of it per writer
/// would be that many places to fix the day the answer changes.
fn answering(
    file: &Path,
    splice: impl FnOnce(&str, &str) -> Result<String, ConfigError>,
) -> Result<(), RememberError> {
    answered(file, |text, named| Ok((splice(text, named)?, ())))
}

/// [`answering`], handing back the installed agents and how each provider is
/// reached as the file states them with the answer in it: what a writer that
/// changes either gives the run, which reads them for the rest of it in place
/// of what its launch read.
fn routing(
    file: &Path,
    splice: impl FnOnce(&str, &str) -> Result<String, ConfigError>,
) -> Result<crucible_config::Routes, RememberError> {
    answered(file, |text, named| {
        let written = splice(text, named)?;
        let routes = crucible_config::Routes::of(&written, named)?;
        Ok((written, routes))
    })
}

/// [`answering`], with what `splice` gives beside the text handed back once
/// the text is in place.
fn answered<T>(
    file: &Path,
    splice: impl FnOnce(&str, &str) -> Result<(String, T), ConfigError>,
) -> Result<T, RememberError> {
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

    let text = text_of(file, &named)?;

    let (written, answer) = splice(&text, &named)?;
    // An answer already written, or a yes taken out of a file that holds
    // none, changes nothing, and a file nothing changed is not written: one
    // that was not there would otherwise be left empty, which is not
    // configuration.
    if written == text {
        return Ok(answer);
    }

    put(file, &written).map_err(unwritable)?;
    Ok(answer)
}

/// What `file`, named `named`, holds: the empty text where nothing is there
/// yet, which is what most projects look like and what the crate below reads
/// as "write a whole file".
fn text_of(file: &Path, named: &str) -> Result<String, RememberError> {
    let unwritable = |source| RememberError::Unwritable {
        file: named.into(),
        source,
    };
    let opened = match File::open(file) {
        Ok(opened) => opened,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(source) => return Err(unwritable(source)),
    };
    let mut text = String::new();
    opened
        .take((crucible_config::MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(unwritable)?;
    if text.len() > crucible_config::MAX_DOCUMENT_BYTES {
        return Err(ConfigError::TooLarge {
            file: named.into(),
            maximum: crucible_config::MAX_DOCUMENT_BYTES,
        }
        .into());
    }
    Ok(text)
}

/// The speed `file` asks `provider` for now: what [`hastening`] last wrote,
/// read without waiting on the lock, since a write replaces the file whole.
///
/// # Errors
///
/// [`RememberError::Unwritable`] when the file cannot be read, and
/// [`RememberError::Unusable`] when what it says is not configuration.
pub fn hastened(file: &Path, provider: &str, model: &str) -> Result<Speed, RememberError> {
    let named = file.display().to_string();
    let text = text_of(file, &named)?;
    Ok(crucible_config::hastened(&text, &named, provider, model)?)
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
