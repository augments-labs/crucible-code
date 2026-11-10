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
//! - **Bounded before it is kept.** [`read()`] counts bytes and lines against
//!   [`MOST_BYTES`] and [`MOST_LINES`] before it parses a byte, and refuses a
//!   diff that names more than [`MOST_FILES`] files however they open. Past a
//!   cap the block is shown as written, and says why. A [`File`] keeps at most
//!   [`MOST_LINES`] lines and a [`Change`] at most [`MOST_FILES`] files however
//!   it was built, and the box counts what either left out.
//! - **Hostile text drawn safe.** A path, a section name and every line are
//!   text this process did not write: a path has its control, bidirectional
//!   and private-use characters replaced before it is kept.
//! - **Nothing folded is lost.** A file of more than [`FOLD`] rows draws its
//!   first [`HEAD`] and last [`TAIL`] with a quiet row between that counts the
//!   rest; the [`Change`] keeps every one of them.
//! - **Bounded when it is drawn.** One change reads at most [`SYNTAX_LINES`]
//!   lines and [`SYNTAX_BYTES`] bytes for their colours, compares at most
//!   [`PAIRS`] pairs of lines of at most [`TOKENS`] words each, and lays out no
//!   more of a line than the room shows. Past a cap its rows draw plain, never
//!   wrong.
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
pub use draw::{Changes, SYNTAX_BYTES, SYNTAX_LINES};
pub use read::{MOST_BYTES, NotADiff, read};
pub use words::{PAIRS, TOKENS};
