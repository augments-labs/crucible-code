//! What every component standing where the prompt box was does the same way.
//!
//! Draw, read a key, draw again if the key moved something, and hand the region
//! back when it is over. That much is the same for a list, a ladder and a
//! question, and what differs between them is two things: how the rows are laid
//! out at the current size, and which keys move the mark. Both arrive as
//! arguments, so a new component brings its own picture and its own keys and
//! not its own loop.
//!
//! The loop itself cannot be driven from a test — the keyboard it reads is the
//! process's own. That is the reason the two arguments are separate from it:
//! what a key does is a function of the key, and it is testable where it is
//! written rather than here.
//!
//! Nothing that stands is written down. A component is a question, and what
//! belongs in the record is the answer to it, in the words of whatever asked —
//! which is the caller's to commit after this returns.

use crucible_tui::{Caret, Pressed, Renderer, Row, Terminal, pressed};

use crate::cli::Fatal;
use crate::cli::style::Style;

/// How a component that was standing stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ended {
    /// Whatever the mark stood on was taken. Where it stood is the `at` the
    /// caller handed in.
    Took,
    /// It was left with nothing taken.
    Left,
    /// There was no room to stand it. It was never drawn and read no key, so
    /// the caller still owes whatever it was going to ask.
    Cramped,
}

/// What one key does to something that is standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Moved {
    /// The picture no longer matches the mark, so the next frame is owed.
    Redraw,
    /// Nothing changed, so nothing is drawn.
    Still,
    /// What the mark stood on was taken.
    Took,
    /// It was left with nothing taken.
    Left,
}

/// Stands something where the prompt box was and reads keys until it ends.
///
/// `laid` is given the mark and the size of the window and answers with the
/// rows to draw; `keys` is given a key and the mark and answers with what that
/// key did. The mark is the caller's, so where it finished is readable there
/// once this returns — which is what a component whose answer is an index
/// needs, and what one with three fixed answers ignores.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    at: &mut usize,
    mut laid: impl FnMut(usize, usize, usize) -> Vec<Row>,
    keys: impl Fn(Pressed, &mut usize) -> Moved,
) -> Result<Ended, Fatal> {
    let mut changed = true;

    loop {
        if changed {
            let rows = laid(*at, renderer.columns(), renderer.rows());
            if !drawn(renderer, style, &rows)? {
                return Ok(Ended::Cramped);
            }
        }

        let arrived = pressed()?;

        // The rows on screen were laid out for a window that is no longer this
        // one. Taking them back is the renderer's; saying the picture no longer
        // matches is `keys` below — and the rows are laid out again against the
        // new height as well as the new width, since height is what a component
        // gives rows up for.
        if arrived == Pressed::Resized {
            renderer.resized()?;
        }

        match keys(arrived, at) {
            Moved::Redraw => changed = true,
            Moved::Still => changed = false,
            Moved::Took => return over(renderer, Ended::Took),
            Moved::Left => return over(renderer, Ended::Left),
        }
    }
}

/// Draws `rows` in the live region, and says whether there was room for them.
///
/// A component gives up rows rather than overflowing the width, and its last
/// rung is nothing at all; the height is checked here, since a live region
/// taller than the window is a frame that cannot be rewound over.
///
/// The cursor is parked on the last row it drew. Nothing here hides it — this
/// program never does — and the footer is the one row where it stands beside a
/// key rather than inside a name somebody is reading.
fn drawn<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    rows: &[Row],
) -> Result<bool, Fatal> {
    let Some(last) = rows
        .len()
        .checked_sub(1)
        .filter(|_| rows.len() <= renderer.rows())
    else {
        return Ok(false);
    };

    let caret = Caret {
        row: last,
        column: 0,
    };

    renderer.live(rows, caret, style.palette())?;
    Ok(true)
}

/// Takes the live region back and answers with `ended`.
fn over<T: Terminal>(renderer: &mut Renderer<T>, ended: Ended) -> Result<Ended, Fatal> {
    renderer.settle()?;
    Ok(ended)
}

/// Moves the mark to `next`, where there is one to move to.
pub(super) fn step(at: &mut usize, next: Option<usize>) -> Moved {
    match next {
        Some(next) => {
            *at = next;
            Moved::Redraw
        }
        None => Moved::Still,
    }
}
