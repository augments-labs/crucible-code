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

mod clear;
mod color;
mod editor;
mod escape;
mod glyphs;
mod markdown;
mod menu;
mod notice;
mod prompt;
mod render;
mod row;
mod terminal;
mod title;
mod welcome;
mod width;

pub use clear::{CLEARED, clear};
pub use color::{Palette, Slot};
pub use editor::{Editor, Key, Typed};
pub use glyphs::Glyphs;
pub use menu::{Listed, Menu};
pub use notice::Notice;
pub use prompt::Prompt;
pub use render::{Caret, Renderer};
pub use row::Row;
pub use terminal::keys::{Pressed, caret, pressed, waiting};
pub use terminal::mouse::Reporting;
pub use terminal::raw::{Raw, RawError};
pub use terminal::system::SystemTerminal;
pub use terminal::{Recording, Size, Terminal, TerminalError};
pub use title::{TITLE, Title, TitleError};
pub use welcome::{Recent, Welcome};
pub use width::{clip, columns, cut, fold};
