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
//! FFI: [`windows`] is the single module in this tree that opts out of
//! `unsafe_code`, and it does so there precisely so that nothing else has to.

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub(super) use unix::{directory, log, mark};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(super) use windows::{directory, log, mark};

#[cfg(test)]
mod tests;
