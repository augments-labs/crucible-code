//! Standing what the transcript had to cut down to a row.
//!
//! Two ways in and one picture. Ctrl+O names no result, so it stands every one
//! of them, newest first; a click names one by landing on the row that made the
//! offer, so it stands that one. Everything after that is the same — the same
//! rows, the same arrows through them, the same keys out.
//!
//! It stands rather than being written into the transcript for the reason
//! nothing else standing here is written down: the results are already in the
//! record as the rows that could not fit them, and committing the text a second
//! time would be a session saying everything twice. So the way out of it is the
//! way out of everything else here — what it stood in is given back, and the
//! screen reads afterwards as though nothing had been opened.
//!
//! Which is also what makes Ctrl+O a toggle rather than a door. It is the key
//! the rows themselves name, and pressing it against what it opened closes it
//! again; there is nothing to undo afterwards because there was never anything
//! to undo.
//!
//! Where it stands depends on whether a turn is running, and on nothing else.
//! Between turns it takes the region the prompt box was in and reads keys of
//! its own until it is closed. While a turn runs it stands under the tail
//! instead, in the rows the box has: results go on arriving above it, every
//! frame draws it again underneath them, and the turn never reaches down into
//! it. The keys are the same either way and so is the picture, which is the
//! point — a reader pressing Ctrl+O is not asked to know which of the two they
//! are in, and a view opened under a turn is still open when the turn ends.
//!
//! What it stands over is what had been cut when it opened. A turn writing
//! underneath it goes on cutting results, and letting those in would slide the
//! rows being read down the screen as each one arrived; they are there the next
//! time it is opened, which is one press away.

use crucible_tui::{Caret, Expanded, Glyphs, Key, Pressed, Renderer, Row, Shown, Terminal};

use crate::cli::Fatal;
use crate::cli::kept::{Kept, Whole};
use crate::cli::style::Style;

use super::region::{self, Moved};
use super::typing::Asked;

/// What the view is standing over.
///
/// Which of the two it is depends on what asked for it, and on nothing after
/// that: the key names no result, so it stands them all, and a click names one
/// by landing on the row that offered it. Both are the same picture with a
/// different number of results in it, and both are closed by the same keys.
#[derive(Debug, PartialEq, Eq)]
enum Over {
    /// Everything that had been cut when it opened, newest first.
    ///
    /// A count of them rather than the results themselves, so that nothing
    /// standing here borrows what a running turn is writing into.
    Everything(usize),
    /// The one result whose offer was written on this row of the record.
    ///
    /// The row rather than the result, for the same reason. It is also what
    /// makes the view close on its own when the ceiling drops that result:
    /// there is then no row to find, and nothing to stand.
    One(usize),
}

/// Where the window over what is standing is open.
///
/// `end` is the layout's answer rather than the keyboard's — how far down the
/// window may go depends on how many rows the results came to at this width,
/// which is not known until they are laid out. So the frame that discovers it
/// writes it here, and the next key acts on a number the picture agrees with.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct View {
    /// How far down the whole of it the window is open.
    from: usize,
    /// The furthest down it may go, as of the last frame drawn.
    end: usize,
    /// What it is a window over.
    over: Over,
}

impl View {
    /// A window at the top of `over`, before a frame has said how far it goes.
    fn onto(over: Over) -> Self {
        Self {
            from: 0,
            end: 0,
            over,
        }
    }
}

/// Whether the whole of what was cut is standing, and where over it.
///
/// Held by the session rather than by either of the loops that draw it, for one
/// reason: a view opened while a turn ran is still open when that turn ends,
/// and the reader who opened it is still reading. So it outlives the loop that
/// took the key, and the loop that comes next picks the same window up where
/// this one left it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum Standing {
    /// Nothing is standing.
    #[default]
    Closed,
    /// The view is standing, with the window this far down it.
    Open(View),
}

impl Standing {
    /// Whether the view is standing.
    pub(super) fn is_open(&self) -> bool {
        matches!(self, Self::Open(_))
    }

    /// Opens the view over everything cut so far.
    ///
    /// Nothing opens where nothing was cut. The key is offered by the rows that
    /// were cut, so a session with none of them has made no offer, and a frame
    /// put up in answer to a press nobody meant is one that took the prompt
    /// away for no reason.
    /// Whether this is what the box was answered with, opening it if it is.
    ///
    /// Asked before the answer is read as a line, because the two keys that reach
    /// this are answered by the state that holds what they stand over rather than
    /// by the loop that read them.
    pub(super) fn asked(&mut self, asked: &Asked, kept: &Kept) -> bool {
        match asked {
            Asked::Expand => self.open(kept),
            Asked::Clicked(at) => self.one(kept, *at),

            // Not this one's. A line, a turn nobody typed and the end of a
            // session are the loop's.
            Asked::Said(_) | Asked::Woke(_) | Asked::Ended | Asked::Untyped => return false,
        }

        true
    }

    pub(super) fn open(&mut self, kept: &Kept) {
        if kept.is_empty() {
            return;
        }

        *self = Self::Open(View::onto(Over::Everything(kept.cut())));
    }

    /// Opens the view over the one result the record row `at` offered.
    ///
    /// Nothing opens where that row offered nothing, which is most rows: a
    /// pointer lands wherever it lands, and the answer to a click on a line of
    /// an answer, a blank row or the shell's own output is the screen the
    /// reader was already looking at.
    pub(super) fn one(&mut self, kept: &Kept, at: usize) {
        if !kept.offered(at) {
            return;
        }

        *self = Self::Open(View::onto(Over::One(at)));
    }

    /// Gives one key to the view, and answers whether a frame is owed.
    ///
    /// That it closed is read off [`Standing::is_open`] afterwards rather than
    /// reported here: the caller draws something either way, and which of the
    /// two it draws is a question about the state and not about the key.
    pub(super) fn against(&mut self, arrived: Pressed) -> bool {
        let Self::Open(view) = self else {
            return false;
        };

        match moving(arrived, view) {
            Moved::Redraw => true,
            Moved::Still => false,

            // Nothing here is ever taken, so the two ways out are one way out.
            Moved::Took | Moved::Left => {
                *self = Self::Closed;
                true
            }
        }
    }
}

/// Stands the view where the box was, and reads keys until it is closed.
///
/// Between turns, which is the half of this that has the keyboard to itself:
/// nothing else is reading, so the loop can block on a press. Returns as soon
/// as it is called if nothing is open.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    kept: &Kept,
    standing: &mut Standing,
) -> Result<(), Fatal> {
    let Standing::Open(view) = standing else {
        return Ok(());
    };

    let glyphs = style.glyphs();
    let picture = |view: &mut View, columns: usize, rows: usize| {
        (laying(kept, view, glyphs, columns, rows), None)
    };

    // Nothing is taken here and nothing is committed, so every way out is the
    // same way out: the region goes back and the box comes up under it. A
    // window with no room to stand it in is one of them — a component that
    // never drew and read no key, which here is a view the reader never saw.
    region::stand(renderer, |_| style, view, picture, moving)?;
    *standing = Standing::Closed;
    Ok(())
}

/// Stands the view under the tail, in the rows the box has while a turn runs.
///
/// Answers whether it stood, which is the caller's question rather than this
/// one's: the box and the view take the same rows, so exactly one of them is
/// drawn per frame and the caller draws the other. A window with no room for
/// the view closes it here rather than standing its chrome with no text under
/// it — the same answer the region gives between turns.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn under<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    kept: &Kept,
    standing: &mut Standing,
) -> Result<bool, Fatal> {
    let Standing::Open(view) = standing else {
        return Ok(false);
    };

    // One row is left to the transcript whatever stands here, and it is the row
    // the turn goes on writing into: a view that asked for the whole window
    // would be a reader looking back through what was said while what is being
    // said now has nowhere at all to appear.
    let room = renderer.rows().saturating_sub(1);
    let rows = laying(kept, view, style.glyphs(), renderer.columns(), room);

    let Some(row) = rows.len().checked_sub(1) else {
        *standing = Standing::Closed;
        return Ok(false);
    };

    // On the view's last row rather than up in the tail, which is where the box
    // parks it too: the cursor sits at the bottom of whatever is standing, and
    // a cursor left in the transcript above reads as a place text is about to
    // appear.
    renderer.under(&rows, Some(Caret { row, column: 0 }), style.palette())?;
    Ok(true)
}

/// The rows of the view at this size, and the state the picture agrees with.
///
/// No rows where there is nothing left to stand, which both callers read as the
/// view closing. That is a result the ceiling dropped while somebody was
/// reading it — rare, and the honest answer to it is the screen coming back
/// rather than a frame of chrome with nothing under it.
fn laying(kept: &Kept, view: &mut View, glyphs: Glyphs, columns: usize, rows: usize) -> Vec<Row> {
    let shown: Vec<Shown<'_>> = match view.over {
        // Stepped over rather than counted up to, because the end that gives is
        // the other one: what arrived after the view opened is at the front of
        // `newest`, and what was dropped to stay under the ceiling has gone
        // from its back.
        // The call still out first, where there is one, because it is the newest
        // thing there is and it is what a reader pressing this while a command
        // runs is asking about. Chained rather than folded into `newest`: the
        // skip below steps over results that arrived since the view opened, and a
        // call that has not answered has not been counted among them.
        Over::Everything(cut) => kept
            .writing()
            .chain(kept.newest().skip(kept.cut().saturating_sub(cut)))
            .map(showing)
            .collect(),

        // Every result that row offered, and usually that is one. A row
        // counting a folded run is the exception and the reason this filters
        // rather than finds: several calls were shown as one line, so the line
        // opens on all of them — a reader who was given a count is owed
        // everything it counted.
        Over::One(at) => kept
            .newest()
            .filter(|whole| whole.at() == Some(at))
            .map(showing)
            .collect(),
    };

    if shown.is_empty() {
        return Vec::new();
    }

    let expanded = Expanded {
        shown: &shown,
        from: view.from,
    };

    // Written before the rows are asked for, so the key pressed against this
    // picture is clamped to what this picture could reach.
    view.end = expanded.end(columns, rows);
    view.from = view.from.min(view.end);

    expanded.within(columns, rows, glyphs)
}

/// One held result, as the view shows it.
fn showing(whole: &Whole) -> Shown<'_> {
    Shown {
        called: whole.called(),
        text: whole.text(),
    }
}

/// What one key does to the view.
///
/// Every key is named rather than caught by a rest arm, for the reason the
/// permission panel names every one of its own: a key arriving at something
/// standing is either something it does or something it has decided to ignore,
/// and a new [`Pressed`] must be decided about here rather than quietly join the
/// second group.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn moving(arrived: Pressed, view: &mut View) -> Moved {
    match arrived {
        // The wheel walks this view rather than the transcript under it, which
        // is why it arrives here beside the arrows: what is standing is a window
        // over more text than its rows hold, and that is exactly the thing a
        // reader turning a wheel is pointing at. At either end it moves nothing,
        // and the loop that reads this takes that as the transcript's turn.
        Pressed::Up | Pressed::Scrolled { back: true } => {
            let next = view.from.checked_sub(1);
            region::step(&mut view.from, next)
        }
        Pressed::Down | Pressed::Scrolled { back: false } => {
            let next = Some(view.from.saturating_add(1)).filter(|next| *next <= view.end);
            region::step(&mut view.from, next)
        }

        // Ctrl+O closes what Ctrl+O opened, which is the whole of what the rows
        // offering it say the key does. Esc is the way out of whatever is
        // standing everywhere else in a session — including out of a view
        // standing under a turn, which it closes rather than stopping the turn
        // behind it — and Ctrl-C and Ctrl-D reach the line under this one, so
        // the view goes first and the line gets the key it was always going to
        // get.
        Pressed::Expand | Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,

        Pressed::Resized => Moved::Redraw,

        // Nothing here is typed into or picked from, so a letter, a click and a
        // mode step have nothing to act on. Return is among them: the line under
        // this is not being read, and closing on it would send whatever is in
        // the box the moment somebody meant to scroll. Ctrl+T for the reason
        // that is nearly the opposite: the plan it opens is in the rows this
        // view is standing over, so the key would move a panel nobody can see.
        // The key about what is running among them: those have their own list,
        // opened from the row under the box, and reached from in here it would be
        // two things standing in one region. The key that copies the line for the
        // plainest reason of all — the line is under this view, not in it. The
        // key that crosses regions for a reason of the same shape: a view is
        // one window over one thing, and one region is all there is here.
        Pressed::Key(_)
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Tab
        | Pressed::Explain
        | Pressed::Plan
        | Pressed::Clicked { .. }
        | Pressed::Pasted(_)
        | Pressed::Queue
        | Pressed::Copy
        | Pressed::PasteImage
        | Pressed::Rename
        | Pressed::Dragged { .. }
        | Pressed::Hovered { .. }
        | Pressed::Released { .. }
        | Pressed::Ignored => Moved::Still,
    }
}

#[cfg(test)]
mod tests;
