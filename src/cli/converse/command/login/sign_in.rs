//! The panel a sign-in run as a command stands in: what the command says over
//! the line that goes to it, for as long as the agents door runs it.
//!
//! The door runs the command and owns everything about how it went: this shows
//! what the door carried, sends it what is typed, and raises the door's cancel.
//! Nothing here says the sign-in worked, or that it was stopped: the panel
//! stands until the door answers, and the line `/login` then draws is the door's.
//!
//! What the command says comes from the one bounded store [`Heard`], read here
//! on the drawing thread as it is drawn, so the panel keeps none of it. Each
//! line is hostile, however much of it the door redacted: it passes through
//! [`escaped`], is folded to the width, and only ever fills rows of this panel.
//!
//! A line is shown as it is typed, because the panel cannot tell a password
//! prompt from a question, and withholding a short answer would garble what the
//! command is told. What is secret is the person's to say: the key the footer
//! names puts the next line in the secret box, where the panel draws one mark
//! for each character and nothing else, sends it as [`Typing::Secret`] and goes
//! back to showing lines. The hidden text is not `Debug`, reaches no error, and
//! is gone from here once it is sent.
//!
//! Either box takes one line. A paste holding a line ending is refused here,
//! whole, with nothing sent: splitting it would give the command lines nobody
//! typed, and the door refuses such a secret anyway.
//!
//! This is a region like the panels around it, so it stands in both
//! `output.screen` modes.

use std::mem;
use std::sync::{Arc, Mutex, PoisonError};

use crucible_app::client::agents::{Typed, Typing};
use crucible_runtime::Cancel;
use crucible_tui::{Caret, Glyphs, Key, Pressed, Renderer, Row, Slot, Terminal, columns, fold};
use crucible_types::shown::escaped;
use tokio::sync::mpsc;

use crate::cli::Fatal;
use crate::cli::client::{Carrying, Heard};
use crate::cli::converse::region::{self, Ended, Moved};
use crate::cli::style::Style;

use super::{BEAT, pressed};

/// The longest line the box holds, as the key box bounds what it holds: enough
/// for any answer a sign-in asks for, and too small for a pasted file.
const LINE_BYTES: usize = 16 * 1024;

/// The rows drawn around what the command said: the rule and a blank row over
/// the lead, and under them the row a refusal stands in, the line and the footer.
const CHROME: usize = 6;

/// What the panel says of a paste it would not take.
const PASTE_REFUSED: &str = "that paste holds a line ending: nothing was sent";

/// What the panel says of a line longer than it holds.
const TOO_LONG: &str = "that is longer than one line can be: nothing was added";

/// What the panel says where lines wait to be read and there is no place for
/// one more.
const NOT_READING: &str = "the command is not reading yet: press enter again";

/// What the panel says of a line the door cut.
const CUT: &str = "the command wrote more of this line than crucible keeps";

/// This session's terminal, where it has one to lend a sign-in the agent runs
/// at a real one.
///
/// All three streams have to be terminals: a sign-in at a real terminal is the
/// route for a person at one, and a child whose input or either output is a
/// pipe has no terminal to be handed. The modes are the ones the session's
/// guards hold, taken from the same decision that took the screen and the
/// pointer, so nothing is left that this process is still holding.
pub(super) fn handover<T: Terminal>(renderer: &Renderer<T>) -> Option<crate::cli::client::Suspend> {
    use std::io::IsTerminal as _;

    handover_if(
        renderer,
        [
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
        ],
    )
}

/// [`handover`] against the three stream answers it read, so every combination
/// can be asked without a terminal to ask.
fn handover_if<T: Terminal>(
    renderer: &Renderer<T>,
    streams: [bool; 3],
) -> Option<crate::cli::client::Suspend> {
    if !renderer.is_terminal() || !streams.into_iter().all(|terminal| terminal) {
        return None;
    }
    Some(crate::cli::client::Suspend::holding(
        crate::cli::converse::holds(renderer.screen()).held(),
        || {},
    ))
}

/// Stands the panel under `title` over the sign-in `doing` says is under way
/// until the door stops to ask something or is done.
///
/// Where the window has no room for it the door goes on, and a beat is waited
/// for the one key that matters without the panel: the one that stops it.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stood<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    title: &'static str,
    doing: &str,
    carrying: &mut Carrying<'_>,
) -> Result<(), Fatal> {
    if carrying.handed() {
        // The terminal is a child's: say this thread has left the read loop,
        // then wait rather than draw over it or read the keys it is typing.
        carrying.left();
        std::thread::sleep(BEAT);
        return Ok(());
    }
    if !carrying.working() {
        return Ok(());
    }
    let glyphs = style.glyphs();
    let mut panel = SigningIn::new(
        title,
        doing,
        carrying.heard(),
        carrying.typing(),
        carrying.cancel().clone(),
    );
    let ended = region::stand_watching(
        renderer,
        |_| style,
        &mut panel,
        |panel: &mut SigningIn, width, room| panel.laid(width, room, glyphs),
        |pressed, panel: &mut SigningIn| panel.pressed(pressed),
        BEAT,
        |panel: &mut SigningIn| {
            // Asked before the loop reads a key, as the panel over a door
            // that may hand its terminal to a child asks: a read there would
            // be a read by a background process group.
            if carrying.handed() {
                return Moved::Took;
            }
            if !carrying.working() {
                return Moved::Took;
            }
            if panel.told() {
                Moved::Redraw
            } else {
                Moved::Still
            }
        },
    )?;
    if ended == Ended::Cramped
        && renderer.waiting(BEAT)?
        && let Some(pressed) = renderer.took(pressed()?)?
        && stops(&pressed)
    {
        panel.stop();
    }
    Ok(())
}

/// Whether `pressed` is what stops the command.
const fn stops(pressed: &Pressed) -> bool {
    matches!(
        pressed,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof)
    )
}

/// The panel. It has no `Debug`, since what it holds may be a hidden line.
struct SigningIn {
    /// The few words the panel's first row opens with.
    title: &'static str,
    /// What is under way.
    doing: String,
    /// What the command has said, as the door carried it.
    heard: Arc<Mutex<Heard>>,
    /// How many lines the command had said when this last drew them.
    seen: u64,
    /// Where a finished line goes.
    typing: mpsc::Sender<Typing>,
    /// What stops the command.
    cancel: Cancel,
    /// Whether it has been asked to stop.
    stopping: bool,
    /// The line being typed, drawn only where `hidden` is not set.
    held: String,
    /// Whether the line being typed goes in the secret box.
    hidden: bool,
    /// What it said of the last key, until the next.
    notice: Option<&'static str>,
}

impl SigningIn {
    fn new(
        title: &'static str,
        doing: &str,
        heard: Arc<Mutex<Heard>>,
        typing: mpsc::Sender<Typing>,
        cancel: Cancel,
    ) -> Self {
        let seen = heard.lock().unwrap_or_else(PoisonError::into_inner).told();
        Self {
            title,
            doing: doing.to_owned(),
            heard,
            seen,
            typing,
            stopping: cancel.requested(),
            cancel,
            held: String::new(),
            hidden: false,
            notice: None,
        }
    }

    /// Whether the command has said more since this last looked.
    fn told(&mut self) -> bool {
        let told = self
            .heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .told();
        let changed = told != self.seen;
        self.seen = told;
        changed
    }

    /// Raises the door's cancel, once.
    fn stop(&mut self) {
        self.cancel.request();
        self.stopping = true;
    }

    /// What a key does. Once the command is being stopped there is nothing left
    /// to send it, and the panel only waits for the door.
    // An event token is handed over, not lent, as the key box takes one.
    #[allow(clippy::needless_pass_by_value)]
    fn pressed(&mut self, pressed: Pressed) -> Moved {
        if matches!(pressed, Pressed::Resized) {
            return Moved::Redraw;
        }
        if self.stopping {
            return Moved::Still;
        }
        match pressed {
            pressed if stops(&pressed) => {
                self.stop();
                self.held.clear();
            }
            Pressed::Tab => {
                // Text typed in one box is not carried into the other: what was
                // typed to be shown would be hidden, and what was typed to be
                // hidden would be drawn.
                self.hidden = !self.hidden;
                self.held.clear();
                self.notice = None;
            }
            Pressed::Key(Key::Char(typed)) if !typed.is_control() => {
                self.appended(typed.encode_utf8(&mut [0; 4]));
            }
            Pressed::Pasted(text) => {
                if text.contains(['\n', '\r']) {
                    self.notice = Some(PASTE_REFUSED);
                } else {
                    let kept: String = text.chars().filter(|one| !one.is_control()).collect();
                    self.appended(&kept);
                }
            }
            Pressed::Key(Key::Backspace) => {
                self.held.pop();
                self.notice = None;
            }
            Pressed::Key(Key::Enter) => return self.send(),
            _ => return Moved::Still,
        }
        Moved::Redraw
    }

    /// Appends `text` whole, or says it did not fit.
    fn appended(&mut self, text: &str) {
        if text.len() > LINE_BYTES.saturating_sub(self.held.len()) {
            self.notice = Some(TOO_LONG);
        } else {
            self.held.push_str(text);
            self.notice = None;
        }
    }

    /// Sends the line typed: a line in the secret box goes as a secret, and the
    /// next is shown again. An empty line is an answer, where it can be seen to
    /// be one; an empty secret is nothing yet. A line with nowhere to go is
    /// kept, so nothing typed is lost to a command slow to read.
    fn send(&mut self) -> Moved {
        if self.hidden && self.held.is_empty() {
            return Moved::Still;
        }
        let Ok(place) = self.typing.try_reserve() else {
            self.notice = Some(NOT_READING);
            return Moved::Redraw;
        };
        let line = mem::take(&mut self.held);
        place.send(if self.hidden {
            Typing::Secret(Typed::new(line))
        } else {
            Typing::Line(line)
        });
        self.hidden = false;
        self.notice = None;
        Moved::Redraw
    }

    /// The panel at `width` in `room` rows, each cut at the width: nothing where
    /// not even the newest row of what was said fits, and what cannot be seen
    /// said by a row of its own.
    fn laid(&self, width: usize, room: usize, glyphs: Glyphs) -> (Vec<Row>, Option<Caret>) {
        let Some(left) = room.checked_sub(CHROME).filter(|left| *left > 0) else {
            return (Vec::new(), None);
        };
        let Some(said) = self.said(width, left) else {
            return (Vec::new(), None);
        };
        let mut laid = Vec::with_capacity(said.len().saturating_add(CHROME));
        laid.push(Row::new().then(Slot::Accent, glyphs.horizontal().repeat(width)));
        laid.push(Row::new());
        laid.push(Row::new().then(
            Slot::Strong,
            format!("{} {} {}", self.title, glyphs.dot(), self.doing),
        ));
        laid.extend(said);
        laid.push(match self.notice {
            Some(notice) => Row::new().then(Slot::Quiet, notice),
            None => Row::new(),
        });
        let (line, column) = self.line(width, glyphs);
        let caret = Caret {
            row: laid.len(),
            column,
        };
        laid.push(line);
        laid.push(Row::new().then(Slot::Quiet, self.footer(width, glyphs)));
        let laid = laid.iter().map(|row| row.clipped(width)).collect();
        (laid, (!self.stopping).then_some(caret))
    }

    /// What the command said, folded to `width` in at most `left` rows, the
    /// newest kept, and a row saying what cannot be seen. Nothing where that
    /// row would be the only one.
    fn said(&self, width: usize, left: usize) -> Option<Vec<Row>> {
        let heard = self.heard.lock().unwrap_or_else(PoisonError::into_inner);
        let mut rows = Vec::new();
        for line in heard.lines() {
            let text = escaped(line.as_str());
            rows.extend(
                fold(&text, width.max(1))
                    .into_iter()
                    .map(|row| Row::plain(row.to_owned())),
            );
            if line.incomplete() {
                rows.push(Row::new().then(Slot::Quiet, CUT));
            }
        }
        let dropped = heard.dropped();
        drop(heard);
        let fits = rows.len() <= left && (dropped == 0 || rows.len() < left);
        if fits && dropped == 0 {
            return Some(rows);
        }
        let room = left.saturating_sub(1);
        if room == 0 {
            return None;
        }
        let more = rows.len().saturating_sub(room);
        let notice = match (dropped, more) {
            (0, more) => format!("{more} earlier rows; make the window taller to see them"),
            (dropped, 0) => format!("{dropped} earlier lines dropped"),
            (dropped, more) => format!("{dropped} earlier lines dropped, and {more} rows above"),
        };
        let mut kept = vec![Row::new().then(Slot::Quiet, notice)];
        kept.extend(rows.into_iter().skip(more));
        Some(kept)
    }

    /// The row a line is typed on, and the column its caret stands at: a mark
    /// for each character of a hidden line, and the end of a shown one, which
    /// goes through what any hostile line does since a paste can hold anything.
    fn line(&self, width: usize, glyphs: Glyphs) -> (Row, usize) {
        let mark = if self.hidden {
            format!("hidden {} ", glyphs.caret())
        } else {
            format!("{} ", glyphs.caret())
        };
        let taken = columns(&mark);
        let room = width.saturating_sub(taken.saturating_add(1));
        let shown = if self.hidden {
            glyphs.hidden().repeat(self.held.chars().count().min(room))
        } else {
            tail(&escaped(&self.held), room)
        };
        let column = taken
            .saturating_add(columns(&shown))
            .min(width.saturating_sub(1));
        let row = Row::new().then(Slot::Accent, mark).then(Slot::Plain, shown);
        (row, column)
    }

    /// The footer that fits `width`, the keys always named before anything
    /// about them.
    fn footer(&self, width: usize, glyphs: Glyphs) -> String {
        if self.stopping {
            return "stopping the command…".to_owned();
        }
        let dot = glyphs.dot();
        let long = if self.hidden {
            format!("enter sends it hidden {dot} tab shows the line {dot} esc stops it")
        } else {
            format!("enter sends {dot} tab hides the next line {dot} esc stops it")
        };
        if columns(&long) <= width {
            return long;
        }
        if self.hidden {
            format!("tab shows {dot} esc stops")
        } else {
            format!("tab hides {dot} esc stops")
        }
    }
}

/// As much of the end of `text` as fits in `room` columns: the end of a line is
/// the part being typed.
fn tail(text: &str, room: usize) -> String {
    let mut used = 0_usize;
    let mut kept: Vec<char> = Vec::new();
    for character in text.chars().rev() {
        used = used.saturating_add(columns(character.encode_utf8(&mut [0; 4])));
        if used > room {
            break;
        }
        kept.push(character);
    }
    kept.into_iter().rev().collect()
}

#[cfg(test)]
mod tests;
