//! Standing the whole of what the transcript had to cut down to a row.
//!
//! Every result a row could not say, newest first. It stands rather than being
//! written into the transcript for the reason nothing else standing here is
//! written down: the results are already in the record as the rows that could
//! not fit them, and committing the text a second time would be a session
//! saying everything twice. So the way out of it is the way out of everything
//! else here — what it stood in is given back, and the screen reads afterwards
//! as though nothing had been opened.
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
use crate::cli::kept::Kept;
use crate::cli::style::Style;

use super::region::{self, Moved};

/// Where the window over the whole of it is open.
///
/// `end` is the layout's answer rather than the keyboard's — how far down the
/// window may go depends on how many rows the results came to at this width,
/// which is not known until they are laid out. So the frame that discovers it
/// writes it here, and the next key acts on a number the picture agrees with.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Window {
    /// How far down the whole of it the window is open.
    from: usize,
    /// The furthest down it may go, as of the last frame drawn.
    end: usize,
    /// How many results had been cut when it opened.
    ///
    /// A count rather than the results themselves, so that nothing standing
    /// here borrows what a running turn is writing into.
    cut: usize,
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
    Open(Window),
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
    pub(super) fn open(&mut self, kept: &Kept) {
        if kept.is_empty() {
            return;
        }

        *self = Self::Open(Window {
            cut: kept.cut(),
            ..Window::default()
        });
    }

    /// Gives one key to the view, and answers whether a frame is owed.
    ///
    /// That it closed is read off [`Standing::is_open`] afterwards rather than
    /// reported here: the caller draws something either way, and which of the
    /// two it draws is a question about the state and not about the key.
    pub(super) fn against(&mut self, arrived: Pressed) -> bool {
        let Self::Open(window) = self else {
            return false;
        };

        match moving(arrived, window) {
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
    let Standing::Open(window) = standing else {
        return Ok(());
    };

    let glyphs = style.glyphs();
    let picture = |window: &mut Window, columns: usize, rows: usize| {
        laying(kept, window, glyphs, columns, rows)
    };

    // Nothing is taken here and nothing is committed, so every way out is the
    // same way out: the region goes back and the box comes up under it. A
    // window with no room to stand it in is one of them — a component that
    // never drew and read no key, which here is a view the reader never saw.
    region::stand(renderer, style, window, picture, moving)?;
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
    let Standing::Open(window) = standing else {
        return Ok(false);
    };

    // One row of tail is kept whatever stands under it, so a view as tall as
    // the window would leave a live region a row taller than the screen — and
    // the top of that has already scrolled past the reach of the next rewind.
    // The row it gives up is the one the turn goes on writing into.
    let room = renderer.rows().saturating_sub(1);
    let rows = laying(kept, window, style.glyphs(), renderer.columns(), room);

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
fn laying(
    kept: &Kept,
    window: &mut Window,
    glyphs: Glyphs,
    columns: usize,
    rows: usize,
) -> Vec<Row> {
    // Stepped over rather than counted up to, because the end that gives is the
    // other one: what arrived after the view opened is at the front of `newest`,
    // and what was dropped to stay under the ceiling has gone from its back.
    let since = kept.cut().saturating_sub(window.cut);
    let shown: Vec<Shown<'_>> = kept
        .newest()
        .skip(since)
        .map(|whole| Shown {
            called: whole.called(),
            text: whole.text(),
        })
        .collect();

    let expanded = Expanded {
        shown: &shown,
        from: window.from,
    };

    // Written before the rows are asked for, so the key pressed against this
    // picture is clamped to what this picture could reach.
    window.end = expanded.end(columns, rows);
    window.from = window.from.min(window.end);

    expanded.within(columns, rows, glyphs)
}

/// What one key does to the view.
///
/// Every key is named rather than caught by a rest arm, for the reason the
/// permission panel names every one of its own: a key arriving at something
/// standing is either something it does or something it has decided to ignore,
/// and a new [`Pressed`] must be decided about here rather than quietly join the
/// second group.
fn moving(arrived: Pressed, window: &mut Window) -> Moved {
    match arrived {
        Pressed::Up => {
            let next = window.from.checked_sub(1);
            region::step(&mut window.from, next)
        }
        Pressed::Down => {
            let next = Some(window.from.saturating_add(1)).filter(|next| *next <= window.end);
            region::step(&mut window.from, next)
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
        // the box the moment somebody meant to scroll.
        Pressed::Key(_)
        | Pressed::Cycle
        | Pressed::Explain
        | Pressed::Clicked { .. }
        | Pressed::Ignored => Moved::Still,
    }
}

#[cfg(test)]
mod tests;
