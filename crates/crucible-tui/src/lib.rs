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
//! Depends on no other crate in this workspace. What reaches it is already
//! text, so it never names a domain type, calls a tool or asks a provider for
//! anything.

mod color;
mod escape;
mod render;
mod terminal;
mod title;
mod width;

pub use color::{Palette, Slot};
pub use render::Renderer;
pub use terminal::system::SystemTerminal;
pub use terminal::{Recording, Size, Terminal, TerminalError};
pub use title::{TITLE, Title, TitleError};
pub use width::cut;
