//! Full-screen terminal rendering: the prompt, the streaming transcript, and
//! the permission prompt.
//!
//! A session takes the alternate screen and every cell on it is this process's.
//! The window is cut into bands once — a head, the transcript, what stands
//! over the box, the box, and a foot — and each row is addressed by its number,
//! so a frame writes the rows whose text is not already there and touches
//! nothing else.
//!
//! What that screen has no room for is the job the terminal used to do. The
//! transcript above the viewport is this crate's now, held by the record, which
//! is why it is bounded: nothing here may be proportional to how long the
//! session has run, and a store that grew with it would spend the whole
//! process's peak-RSS budget on rows nobody is looking at. Only the lines the
//! viewport covers are folded and painted.
//!
//! Which is also what makes a pane layout an ordinary change rather than a
//! rewrite: a band is a rectangle of a screen this process already owns.
//!
//! Depends on no other crate in this workspace. What reaches it is already
//! text, so it never names a domain type, calls a tool or asks a provider for
//! anything.

mod asked;
mod asking;
mod bands;
mod clipboard;
mod color;
#[cfg(test)]
mod dump;
mod editor;
mod escape;
mod expanded;
#[cfg(test)]
mod fits;
mod glyphs;
pub mod ground;
mod ladder;
pub mod markdown;
mod menu;
mod notice;
mod panel;
mod plan;
mod prompt;
mod record;
mod render;
mod row;
mod running;
pub mod syntax;
mod terminal;
mod title;
mod welcome;
mod width;
mod working;

pub use asked::{Asked, Choice, Given, Stop, Writing};
pub use asking::Question;
pub use color::{Palette, Sequence, Slot, Theme, Worn};
pub use editor::{Editor, Key, Sending, Typed};
pub use expanded::{Expanded, Shown};
pub use glyphs::Glyphs;
pub use ground::{Ground, is_light};
pub use ladder::Ladder;
pub use markdown::Markdown;
pub use menu::{Listed, Menu};
pub use notice::Notice;
pub use panel::{Offered, Panel};
pub use plan::{Plan, State, Task};
pub use prompt::Prompt;
pub use render::{Aimed, Caret, Renderer};
pub use row::Row;
pub use running::{Command, Running};
pub use terminal::ground::asked;
pub use terminal::keyboard::{Pasting, Spelling};
pub use terminal::keys::{Characters, Pressed, characters, pressed, waiting};
pub use terminal::mouse::Reporting;
pub use terminal::raw::{Raw, RawError};
pub use terminal::screen::{Screen, ScreenError};
pub use terminal::system::SystemTerminal;
pub use terminal::{Picture, Recording, Size, Terminal, TerminalError};
pub use title::{TITLE, Title, TitleError};
pub use welcome::{Recent, Welcome};
pub use width::{clip, columns, cut, fold};
pub use working::Working;
