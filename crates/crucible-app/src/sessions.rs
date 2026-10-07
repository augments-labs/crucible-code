//! What `crucible sessions list` says: the sessions recorded for the directory
//! it was started in, read without starting, resuming or writing to any of
//! them.
//!
//! What is read is [`crucible_session::discovered`]'s, which reads the session
//! index and each listed log's first line and nothing after it, takes no lock
//! and writes nothing. So a list says when each session started, how many
//! messages it has, the branch it was started on and the title somebody saved,
//! and never a word anybody wrote in it: there is no first prompt in a list,
//! because finding one would mean reading past the header.
//!
//! The sessions are the ones the interactive opening and `--continue` would
//! consider here: recorded in this directory, newest first, as many as the
//! client contract's list holds and the rest counted. The terminal's text and
//! the `--json` document are both written from the same
//! [`crucible_client_api::sessions::Report`], translated here by hand, so
//! neither can say what the other does not. Whatever kept the list from being
//! every session there is is said under the text and named in the document.
//!
//! The title and the branch are a file's words, so the text writes what a
//! terminal would act on in them as its escape and the document carries them
//! as they are, for the JSON writer to escape. A failure's document names the
//! step that stopped and no path, as `crucible sandbox inspect --json` does,
//! and the whole sentence goes to standard error.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crucible_client_api::bounds::ITEMS;
use crucible_client_api::sessions::{Listing, Report, Session};
use crucible_client_api::{Refusal, Text};
use crucible_config::Home;
use crucible_session::{Discovered, Discovery, Roots};
use crucible_types::shown::escaped;
use crucible_workspace::Workspace;

use crate::AppError;

/// The sessions recorded for one directory, as a list says them.
#[derive(Debug)]
pub struct Listed {
    at: PathBuf,
    listing: Listing,
}

/// The sessions recorded for `here` in `home`'s session directory.
///
/// # Errors
///
/// `here` is not a directory crucible can work in, or the session index is
/// there and cannot be read.
pub fn list(here: &Path, home: &Home) -> Result<Listed, AppError> {
    listing(here, home.sessions())
}

/// [`list`], from the session directory `directory`.
pub(crate) fn listing(here: &Path, directory: &Path) -> Result<Listed, AppError> {
    let workspace = Workspace::open(here)?;
    let found = crucible_session::discovered(directory, Roots::These(&[workspace.root()]), ITEMS)?;
    Ok(Listed {
        at: workspace.root().to_owned(),
        listing: translated(&found),
    })
}

/// What the session store found, as the client contract carries it.
fn translated(found: &Discovery) -> Listing {
    let counted = |count: usize| u64::try_from(count).unwrap_or(u64::MAX);
    Listing {
        sessions: found
            .sessions()
            .iter()
            .map(|session: &Discovered| Session {
                id: session.id().clone(),
                branch: session.branch().map(Text::cut),
                messages: counted(session.messages()),
                title: session.title().map(Text::cut),
            })
            .collect(),
        omitted: counted(found.omitted()),
        unreadable: counted(found.unreadable()),
        index_full: found.full(),
        unindexed: found.unindexed(),
    }
}

impl Listed {
    /// The list as text, one session a line, ending in a newline; `ago` says
    /// how long before now an instant was.
    #[must_use]
    pub fn human(&self, ago: &dyn Fn(SystemTime) -> String) -> String {
        human(&self.at, &self.listing, ago)
    }

    /// The list as `crucible sessions list --json` writes it: one JSON
    /// document on one line, ending in a newline.
    ///
    /// # Errors
    ///
    /// [`Unwritten`] for a list the document cannot carry.
    pub fn json(&self) -> Result<Vec<u8>, Unwritten> {
        Ok(self.contract().encode()?)
    }

    /// The list, as the document's value.
    pub(crate) fn contract(&self) -> Report {
        Report::Listed(self.listing.clone())
    }
}

/// `listing`, made for the directory `at`, as text.
pub(crate) fn human(at: &Path, listing: &Listing, ago: &dyn Fn(SystemTime) -> String) -> String {
    let at = escaped(&at.display().to_string());
    let mut said = String::new();
    let _ = match listing.sessions.len() {
        0 => writeln!(said, "no sessions recorded for {at}"),
        1 => writeln!(said, "1 session recorded for {at}"),
        many => writeln!(said, "{many} sessions recorded for {at}"),
    };
    for session in &listing.sessions {
        let mut row = format!(
            "  {}  {}  {}",
            session.id,
            ago(session.id.started()),
            match session.messages {
                1 => "1 message".to_owned(),
                many => format!("{many} messages"),
            }
        );
        if let Some(branch) = &session.branch {
            let _ = write!(row, "  on {}", escaped(branch.as_str()));
        }
        match &session.title {
            Some(title) => {
                let _ = write!(row, "  {}", escaped(title.as_str()));
            }
            None => row.push_str("  untitled"),
        }
        let _ = writeln!(said, "{row}");
    }

    let mut short = Vec::new();
    if listing.omitted > 0 {
        short.push(format!(
            "{} older {} recorded here {} not listed",
            listing.omitted,
            if listing.omitted == 1 {
                "session"
            } else {
                "sessions"
            },
            if listing.omitted == 1 { "is" } else { "are" },
        ));
    }
    if listing.unreadable > 0 {
        short.push(format!(
            "{} session {} could not be read and {} not listed",
            listing.unreadable,
            if listing.unreadable == 1 {
                "log"
            } else {
                "logs"
            },
            if listing.unreadable == 1 { "is" } else { "are" },
        ));
    }
    if listing.index_full {
        short.push(
            "the session index holds as many sessions as it keeps, so older ones may be missing"
                .to_owned(),
        );
    }
    if listing.unindexed {
        short.push(
            "no session index has been written yet, so the sessions there are not listed; the \
             next session started or continued here writes one"
                .to_owned(),
        );
    }
    if !short.is_empty() {
        said.push('\n');
        for line in short {
            let _ = writeln!(said, "{line}");
        }
    }
    if !listing.sessions.is_empty() {
        let _ = writeln!(said, "\n`crucible --resume ID` carries one on");
    }
    said
}

/// Why no list could be made, as far as the failed document says it.
#[derive(Debug, Clone, Copy)]
pub enum Unmade<'a> {
    /// The directory crucible was started in could not be read.
    Here,
    /// [`list`] refused.
    Listing(&'a AppError),
    /// A list was made and the document could not carry it.
    Unwritten(&'a Unwritten),
}

impl Unmade<'_> {
    /// The step that stopped, in words that name no file.
    ///
    /// Each error a list can stop on leads with the file it is about, so the
    /// document says which step stopped and leaves the rest to standard error,
    /// where the run ends with the whole sentence.
    fn said(self) -> String {
        match self {
            Self::Here => "the directory crucible was started in could not be read".to_owned(),
            Self::Listing(AppError::Workspace(_)) => {
                "this directory is not one crucible can work in; standard error says why".to_owned()
            }
            Self::Listing(AppError::Config(_)) => {
                "crucible's home directory could not be found; standard error says why".to_owned()
            }
            Self::Listing(AppError::Session(_)) => {
                "the session index could not be read; standard error says why".to_owned()
            }
            // Nothing else is returned by a list today, and a sentence not
            // read here cannot be promised to carry no path.
            Self::Listing(_) => {
                "the sessions could not be listed; standard error says why".to_owned()
            }
            Self::Unwritten(unwritten) => unwritten.to_string(),
        }
    }
}

/// The document a failure to list at all is written as, naming the step that
/// stopped, so a script reading standard output is never left with nothing.
///
/// # Errors
///
/// [`Unwritten`] if even that could not be framed; the sentence is cut to its
/// bound first, so this is the refusal said rather than one expected.
pub fn failure(problem: Unmade<'_>) -> Result<Vec<u8>, Unwritten> {
    Ok(Report::Failed(Text::cut(&problem.said())).encode()?)
}

/// A list that was made and could not be written as its document.
///
/// It carries the contract's refusal, which names a code and nothing of the
/// list, so it can be said wherever the list itself could have been.
#[derive(Debug, thiserror::Error)]
#[error("the sessions list could not be written: {0}")]
pub struct Unwritten(#[from] Refusal);

#[cfg(test)]
mod tests;
