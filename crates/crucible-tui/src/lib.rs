//! Inline terminal rendering: the prompt, the streaming transcript, and the
//! permission prompt.
//!
//! Rendering is inline rather than full-screen: output is written into the
//! terminal's own scrollback instead of an alternate screen, so the scroll
//! buffer belongs to the terminal and not to this process. That is what keeps
//! *this crate* flat as a transcript grows — the live tail is bounded and rows
//! that fall off it are written out once and forgotten, so nothing here is
//! proportional to how long the session has run. The transcript itself is held
//! whole elsewhere, and it is what the peak-RSS budget is set to cover.
//!
//! It is also the reason a pane layout is not a drop-in change later: taking
//! the alternate screen makes this process the owner of scrollback, which is a
//! job the terminal is doing today.
//!
//! Depends on `crucible-core` alone. It renders domain types and never calls a
//! tool or a provider.

mod frame;
mod plain;
mod render;
mod screen;
mod system;
mod tail;
mod terminal;
mod title;

pub use render::Renderer;
pub use system::SystemTerminal;
pub use tail::Tail;
pub use terminal::{Recording, Size, Terminal, TerminalError};
pub use title::{TITLE, Title, TitleError};
