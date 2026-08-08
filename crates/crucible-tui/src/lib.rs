//! Inline terminal rendering: the prompt, the streaming transcript, and the
//! permission prompt.
//!
//! Rendering is inline rather than full-screen: output is written into the
//! terminal's own scrollback instead of an alternate screen, so the scroll
//! buffer belongs to the terminal and not to this process. That is what keeps
//! memory flat as a transcript grows, and it is the reason a pane layout is not
//! a drop-in change later.
//!
//! Depends on `crucible-core` alone. It renders domain types and never calls a
//! tool or a provider.

mod title;

pub use title::{TITLE, Title, TitleError};
