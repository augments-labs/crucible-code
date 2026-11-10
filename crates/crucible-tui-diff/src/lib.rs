//! A change to files drawn as boxed rows, read from a unified diff or handed over by a tool.
//!
//! One [`Change`] is drawn one way by [`Changes::draw`], whoever made it: a
//! fence the model wrote (`diff` or `patch`), read by [`read()`], and a file a
//! tool edited, turned into a [`Change`] by the command line. Both arrive at
//! the same box, so a reader who has learned what one edit looks like has
//! learned the other.
//!
//! What this crate holds to:
//!
//! - **Bounded before it is read.** [`read()`] counts bytes, lines and files
//!   against [`MOST_BYTES`], [`MOST_LINES`] and [`MOST_FILES`] before it parses
//!   a byte, and a [`File`] keeps at most [`MOST_LINES`] lines however it was
//!   built. Past a cap the block is shown as written, and says why.
//! - **Hostile text drawn safe.** A path, a section name and every line are
//!   text this process did not write: a path has its control, bidirectional
//!   and private-use characters replaced before it is kept.
//! - **Nothing folded is lost.** A file of more than [`FOLD`] rows draws its
//!   first [`HEAD`] and last [`TAIL`] with a quiet row between that counts the
//!   rest; the [`Change`] keeps every one of them.
//! - **Every row fits.** Each row is clipped to the room it was given, and a
//!   header that cannot hold its counts leaves them off rather than cut them.

mod change;
mod draw;
mod read;
mod words;

#[cfg(test)]
mod fits;
#[cfg(test)]
mod tests;

pub use change::{Change, FOLD, File, HEAD, Line, MOST_FILES, MOST_LINES, MOST_PATH, Side, TAIL};
pub use draw::Changes;
pub use read::{MOST_BYTES, NotADiff, read};
