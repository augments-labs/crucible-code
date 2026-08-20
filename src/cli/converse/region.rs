//! What every component standing where the prompt box was does the same way.
//!
//! Draw, read a key, draw again if the key moved something, and hand the region
//! back when it is over. That much is the same for a list, a ladder and a
//! question, and what differs between them is three things: what the component
//! keeps between frames, how the rows are laid out at the current size, and
//! which keys move any of it. All three arrive as arguments, so a new component
//! brings its own state and its own picture and its own keys and not its own
//! loop.
//!
//! Where the cursor goes is the same kind of answer and arrives the same way. A
//! component that draws a line somebody is typing into is the only party that
//! knows which of its rows that is and how far along it the cursor sits, and it
//! finds out while it is laying the rows out. So the layout hands it back beside
//! them, and `None` is *wherever you have always parked it* — which is what
//! every component that draws no line hands back.
//!
//! What is kept is the component's own type rather than an index, because a mark
//! is not all a component has to remember. A panel whose prose can be scrolled
//! holds where the window over it opens as well as which answer is marked, and
//! the two move on different keys. Both are laid out at once and neither is
//! this module's business.
//!
//! The loop itself cannot be driven from a test — the keyboard it reads is the
//! process's own. That is the reason the two arguments are separate from it:
//! what a key does is a function of the key, and it is testable where it is
//! written rather than here.
//!
//! Nothing that stands is written down. A component is a question, and what
//! belongs in the record is the answer to it, in the words of whatever asked —
//! which is the caller's to commit after this returns.

use crucible_tui::{Caret, Pressed, Renderer, Row, Terminal, caret, pressed};

use crate::cli::Fatal;
use crate::cli::style::Style;

/// How a component that was standing stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ended {
    /// Whatever the mark stood on was taken. Where it stood is in the state the
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
/// `laid` is given the state and the size of the window and answers with the
/// rows to draw; `keys` is given a key and the state and answers with what that
/// key did. The state is the caller's, so where it finished is readable there
/// once this returns — which is what a component whose answer is an index
/// needs, and what one with three fixed answers ignores.
///
/// `laid` takes it by mutable reference rather than by value, because laying the
/// rows out is where a component finds out things a key cannot know. How far
/// down a panel's prose the window may open is one: it depends on how many rows
/// the paragraphs folded to at this width, which is the layout's answer and not
/// the keyboard's. So the frame that discovers it is the frame that writes it
/// down, and the next key acts on a state the picture has already agreed with.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stand<T: Terminal, S>(
    renderer: &mut Renderer<T>,
    style: impl Fn(&S) -> Style,
    state: &mut S,
    mut laid: impl FnMut(&mut S, usize, usize) -> (Vec<Row>, Option<Caret>),
    keys: impl Fn(Pressed, &mut S) -> Moved,
) -> Result<Ended, Fatal> {
    let mut changed = true;

    loop {
        if changed {
            let (rows, caret) = laid(state, renderer.columns(), renderer.rows());
            // Asked per frame rather than taken once, because one caller
            // changes it between frames: `/theme` draws its specimen in
            // whatever the mark is standing on, which is the whole of how a
            // theme is chosen by seeing rather than by reading a name.
            if !drawn(renderer, style(state), &rows, caret)? {
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

        // A click is reported against the whole screen, and a component thinks
        // in the rows it drew. This is the one place both are known — the
        // region's origin from where the cursor is parked, the click's row from
        // the press — so the click is rewritten to a row of the region here,
        // and one that landed outside it is nothing the component is asked
        // about. Asked of the terminal per click and never per frame, for the
        // reason typing.rs's `cursor` gives: it costs a round trip.
        let arrived = match arrived {
            Pressed::Clicked { row, column } => {
                let Some(cursor) = caret().ok().map(|(row, _)| row) else {
                    continue;
                };
                match renderer.within(row, cursor) {
                    Some(row) => Pressed::Clicked { row, column },
                    None => continue,
                }
            }
            other => other,
        };

        match keys(arrived, state) {
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
/// The cursor is parked where `caret` says, and on the last row it drew where
/// that is `None`. Nothing here hides it — this program never does — and the
/// last row is the one place it can stand without sitting inside a name
/// somebody is reading.
fn drawn<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    rows: &[Row],
    caret: Option<Caret>,
) -> Result<bool, Fatal> {
    let Some(last) = rows
        .len()
        .checked_sub(1)
        .filter(|_| rows.len() <= renderer.rows())
    else {
        return Ok(false);
    };

    let caret = caret.unwrap_or(Caret {
        row: last,
        column: 0,
    });

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
