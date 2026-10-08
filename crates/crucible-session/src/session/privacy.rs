//! Holding the session tree out of every other account's reach.
//!
//! A log holds what was typed, what the model said, the contents of the files
//! that were read and everything a command printed. Left at what the system
//! creates a file as, that is a transcript any account on a shared machine can
//! read.
//!
//! Two things are wanted, and neither is the lesser worry. Only this account may
//! read the tree, or the session is public. And only this account may write to
//! it — `--continue` replays a log, so an account that can append a line to one
//! can put words in the user's mouth and have the model act on them.
//!
//! What differs between platforms is only how that is said. Unix has a file
//! mode. Windows has an access control list, and the call that writes one is
//! FFI: `windows`, which only a Windows build has, opts out of `unsafe_code`
//! for that call, an allowance confined to this module so that nothing else
//! in the crate has to carry it.
//!
//! What is read back by name is read through [`opened`], whatever the file:
//! a log, the index and its mark, the prompt history or a deferred call's
//! result. Anything that can write to the tree can put a link or a pipe under
//! one of those names, and each reader refuses either the way it refuses a
//! file of its own that will not open. The one exception is a running session
//! reading back the log it created and is still writing, by the name it made.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub(super) use unix::{directory, fresh, log, mark};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(super) use windows::{directory, fresh, log, mark};

/// The file at `path`, opened to read, where it is one ordinary file.
///
/// A pipe would hold `--continue`, the welcome screen, a listing or the start
/// itself until something wrote to it, and a link would read wherever it
/// leads, out of the sessions directory or into another of its files — and a
/// session continued through one is then cut and appended to there. So the
/// name is opened without following a final link and, on Unix, without
/// waiting for a writer, and the proof that it is an ordinary file is taken on
/// the handle that opened, before a byte is read. On Windows a final reparse
/// point is opened as itself and refused by the same proof; a pipe cannot sit
/// in a directory there.
///
/// A second hard name is accepted. Nothing is followed to reach a file with
/// one, so what is read is what this name opened, and a backup made with hard
/// links gives every file one: refusing it would leave every session that
/// backup touched impossible to continue.
///
/// The privacy crate's opener rather than a workspace path: the sessions
/// directory is not a root the agent was pointed at, and a workspace proof
/// follows any link that stays inside its root, where this follows none.
///
/// # Errors
///
/// What the open said, a missing file still [`io::ErrorKind::NotFound`], or
/// that what opened is not one ordinary file; a caller reports either against
/// the file as it reports one that will not open.
pub(super) fn opened(path: &Path) -> Result<File, io::Error> {
    crucible_privacy::open_read_ordinary(path).map_err(crucible_privacy::PrivacyError::into_io)
}

#[cfg(test)]
mod tests;
