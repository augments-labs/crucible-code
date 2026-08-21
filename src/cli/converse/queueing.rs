//! The prompts waiting behind a turn, stood open to be gone over.
//!
//! Ctrl+Q stands the whole queue where the panel above the box named the first
//! few of it. Up and down walk it, `x` takes the marked line back into the box
//! to be edited or sent sooner, and `esc` — or the key that opened it — closes
//! it again.
//!
//! While it stands, the queue is held, and that is the point of it. The turn
//! above goes on writing and takes none of these lines: a line the reader is
//! still going over is not one the agent should be reading, and one taken
//! mid-edit is in the transcript, where it cannot be taken back. Closing the
//! view releases the whole batch at once — the lines that were edited and the
//! ones that were not — and the turn works them in at its next pass boundary.
//!
//! Nothing above it stops for that. The turn writes into the tail as it always
//! does; a held queue answers the exchange loop the way an empty one does, which
//! is what it meets at almost every pass anyway. What the reader sees is their
//! own lines sitting still while the answer above them goes on arriving.
//!
//! Where it stands depends on whether a turn is running, and on nothing else —
//! the shape [`super::expanding`] sets. Between turns it takes the region the
//! box was in and reads keys of its own, because nothing else is reading; while
//! a turn runs it stands under the tail, in the rows the box has, and every
//! frame draws it again beneath whatever arrived above it. The keys are the same
//! either way, and a view opened under a turn is still open when the turn ends —
//! which is what stops the queue being committed out from under a reader who was
//! halfway through it.

use crucible_core::Steer;
use crucible_tui::{Caret, Editor, Key, Pressed, Renderer, Row, Terminal};

use crate::cli::Fatal;
use crate::cli::draw;
use crate::cli::style::Style;

use super::Prompts;
use super::region::{self, Moved};

/// What the view acts on, held together so that one call carries all of it.
///
/// Three references rather than three arguments at each call, for the reason
/// [`super::Held`] is one value: the list, the box a line taken back returns
/// to, and the queue the turn reads are one subject, and a key press is
/// answered against all three or against none of them.
pub(super) struct Reading<'a> {
    /// The list being read, which is also the panel above the box.
    pub(super) queue: &'a mut Prompts,
    /// The box a line taken back returns to.
    pub(super) editor: &'a mut Editor,
    /// The offer the running turn reads, held while this stands.
    pub(super) steer: &'a Steer,
}

/// Whether the queue is standing open, and which line the keys act on.
///
/// Held by the session rather than by either of the loops that draw it, for the
/// reason the other view here is: one opened while a turn ran is still open when
/// that turn ends, and the reader who opened it is still reading.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum Standing {
    /// Nothing is standing, and the turn may take the queue.
    #[default]
    Closed,
    /// The list is standing, with the mark this far down it.
    Open(usize),
}

impl Standing {
    /// Whether the list is standing.
    pub(super) fn is_open(&self) -> bool {
        matches!(self, Self::Open(_))
    }

    /// Opens the list, and holds the queue while it stands.
    ///
    /// Nothing opens on an empty queue: the key is offered by the panel that
    /// names what is waiting, so a session with nothing waiting has made no
    /// offer, and a frame put up in answer to a press nobody meant is one that
    /// took the box away for no reason.
    pub(super) fn open(&mut self, queue: &Prompts, steer: &Steer) {
        if queue.waiting_count() == 0 {
            return;
        }

        steer.hold();
        *self = Self::Open(0);
    }

    /// Gives one key to the list, and answers whether a frame is owed.
    ///
    /// That it closed is read off [`Standing::is_open`] afterwards rather than
    /// reported here: the caller draws something either way, and which of the
    /// two it draws is a question about the state and not about the key.
    pub(super) fn against(&mut self, arrived: &Pressed, reading: Reading<'_>) -> bool {
        let Self::Open(at) = self else {
            return false;
        };

        let mut open = Open { at: *at, reading };

        match moving(arrived, &mut open) {
            Moved::Redraw => {
                *self = Self::Open(open.at);
                true
            }
            Moved::Still => false,

            // Nothing here is committed, so the two ways out are one way out.
            Moved::Took | Moved::Left => {
                open.reading.steer.release();
                *self = Self::Closed;
                true
            }
        }
    }
}

/// The list and the mark on it, which is what a key is answered against.
///
/// One value so that the two closures [`region::stand`] drives can each borrow
/// the whole of it.
struct Open<'a> {
    /// Which line a key acts on.
    at: usize,
    /// What it acts on it with.
    reading: Reading<'a>,
}

/// Stands the list where the box was, and reads keys until it is closed.
///
/// Between turns, which is the half of this that has the keyboard to itself.
/// Reached where a turn ended under an open view: the queue is still the
/// reader's until they close it, and committing it out from under them is the
/// one thing this whole view exists to stop. Returns as soon as it is called if
/// nothing is open.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    reading: Reading<'_>,
    standing: &mut Standing,
) -> Result<(), Fatal> {
    let Standing::Open(at) = *standing else {
        return Ok(());
    };

    let mut open = Open { at, reading };
    let picture =
        |open: &mut Open<'_>, columns: usize, rows: usize| (laid(open, columns, rows, style), None);

    region::stand(
        renderer,
        |_| style,
        &mut open,
        picture,
        |arrived, open| moving(&arrived, open),
    )?;

    // Every way out is the same way out, a window with no room among them: the
    // region goes back, the box comes up under it, and the lines the reader left
    // in the queue are the turn's again.
    open.reading.steer.release();
    *standing = Standing::Closed;
    Ok(())
}

/// Stands the list under the tail, in the rows the box has while a turn runs.
///
/// Answers whether it stood, which is the caller's question rather than this
/// one's: the box and the list take the same rows, so exactly one of them is
/// drawn per frame and the caller draws the other.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn under<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    queue: &Prompts,
    standing: &mut Standing,
    steer: &Steer,
) -> Result<bool, Fatal> {
    let Standing::Open(at) = *standing else {
        return Ok(false);
    };

    // One row of tail is kept whatever stands under it, so a list as tall as the
    // window would leave a live region a row taller than the screen. The row it
    // gives up is the one the turn goes on writing into.
    let room = renderer.rows().saturating_sub(1);
    let rows = rows(queue, at, renderer.columns(), room, style);

    // Nothing left to stand: the reader took the last line back, or the window
    // has no room for the list at all. Either way the box comes back in this
    // same frame, and the queue is the turn's again.
    let Some(row) = rows.len().checked_sub(1) else {
        steer.release();
        *standing = Standing::Closed;
        return Ok(false);
    };

    renderer.under(&rows, Some(Caret { row, column: 0 }), style.palette())?;
    Ok(true)
}

/// One key against the list, and what it owes the picture.
///
/// The one handler for both halves of the view, so a key that does one thing
/// under a turn cannot come to do another between them.
fn moving(arrived: &Pressed, open: &mut Open<'_>) -> Moved {
    let at = open.at;

    match arrived {
        Pressed::Up => region::step(&mut open.at, at.checked_sub(1)),
        Pressed::Down => region::step(
            &mut open.at,
            (at + 1 < open.reading.queue.waiting_count()).then(|| at + 1),
        ),

        // The one key that changes the queue: the marked line is taken back into
        // the box, where it can be edited or sent ahead of the rest. Out of both
        // places it sits in, because the panel and the turn's own offer hold the
        // same line — one dropped from the panel alone is a prompt the reader
        // deleted that the turn works in anyway. With one line it is also the way
        // out, since the list it was read from is then empty.
        Pressed::Key(Key::Char('x')) => match open.reading.queue.drop(at) {
            Some(line) => {
                open.reading.steer.forget(&line);
                open.reading.editor.paste(&line);
                open.at = at.min(open.reading.queue.waiting_count().saturating_sub(1));

                if open.reading.queue.waiting_count() == 0 {
                    Moved::Left
                } else {
                    Moved::Redraw
                }
            }
            None => Moved::Still,
        },

        Pressed::Escape | Pressed::Queue => Moved::Left,
        _ => Moved::Still,
    }
}

/// The rows of the list as [`region::stand`] asks for them.
fn laid(open: &mut Open<'_>, columns: usize, rows: usize, style: Style) -> Vec<Row> {
    self::rows(open.reading.queue, open.at, columns, rows, style)
}

/// The queue laid out as a titled list, with the marked line standing out.
///
/// Each line is led by the mark a line is typed after, and the one the mark is
/// on is drawn in the accent so a key's target is never a guess. No rows at all
/// where there is nothing left to name, which both callers read as the view
/// closing.
fn rows(queue: &Prompts, at: usize, columns: usize, rows: usize, style: Style) -> Vec<Row> {
    use crucible_tui::Slot;

    let waiting = queue.waiting_count();
    let room = rows.saturating_sub(3);
    if waiting == 0 || room == 0 {
        return Vec::new();
    }

    let glyphs = style.glyphs();
    let mark = glyphs.caret();

    let caption = format!("{waiting} queued — x to take back, esc to close");
    let title = Row::new().then(Slot::Strong, draw::clipped(&caption, columns, glyphs));

    let mut laid = vec![title, Row::new()];

    for (place, said) in queue.waiting_all().enumerate().take(room) {
        let tone = if place == at {
            Slot::Accent
        } else {
            Slot::Plain
        };

        laid.push(
            Row::new()
                .then(Slot::Accent, mark)
                .then(Slot::Plain, " ")
                .then(tone, draw::clipped(said, columns.saturating_sub(2), glyphs)),
        );
    }

    laid.push(Row::new());
    laid
}

#[cfg(test)]
mod tests;
