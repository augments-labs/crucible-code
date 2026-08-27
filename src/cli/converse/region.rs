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
//! The wheel is the one press this loop answers on a component's behalf. A
//! component that is a window over more text than it has rows for walks it like
//! an arrow and says so by moving; every other one stands over a transcript
//! that is still on screen above it, and that is what the wheel moves. Which of
//! the two a component is, is the component's answer and arrives the same way
//! every other key's does.
//!
//! Nothing that stands is written down. A component is a question, and what
//! belongs in the record is the answer to it, in the words of whatever asked —
//! which is the caller's to commit after this returns.

use crucible_tui::{Aimed, Caret, Pressed, Renderer, Row, Terminal};

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
    laid: impl FnMut(&mut S, usize, usize) -> (Vec<Row>, Option<Caret>),
    keys: impl Fn(Pressed, &mut S) -> Moved,
) -> Result<Ended, Fatal> {
    stand_while(renderer, style, state, laid, keys, |_| Ok(()))
}

/// [`stand`], with something to do on each pass before a key is waited on.
///
/// Between turns there is nothing to do — nothing moves while nobody types —
/// so `stand` passes a no-op. Mid-turn there is: the worker goes on reporting
/// while a panel stands, and the transcript would freeze over the box it opened
/// from if nothing drained it. The hook is that drain, run once per pass so a
/// turn's text keeps arriving behind what it is standing under.
#[allow(clippy::too_many_arguments)]
// Six is the one over clippy's limit, and each is a distinct thing the loop
// owns: the terminal, the style under the mark, the state, the layout, the
// keys, and the drain a mid-turn panel runs to keep the transcript moving.
// Bundling them into a struct would name them once and force every caller to
// build it — several callers, one argument — for a type that exists only to
// satisfy the count.
pub(super) fn stand_while<T: Terminal, S>(
    renderer: &mut Renderer<T>,
    style: impl Fn(&S) -> Style,
    state: &mut S,
    mut laid: impl FnMut(&mut S, usize, usize) -> (Vec<Row>, Option<Caret>),
    keys: impl Fn(Pressed, &mut S) -> Moved,
    while_waiting: impl FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Ended, Fatal> {
    let mut changed = true;
    let mut while_waiting = while_waiting;
    let mut resting = None;

    loop {
        while_waiting(renderer)?;
        if changed {
            let (rows, caret) = laid(state, renderer.columns(), renderer.room());
            // Asked per frame rather than taken once, because one caller
            // changes it between frames: `/theme` draws its specimen in
            // whatever the mark is standing on, which is the whole of how a
            // theme is chosen by seeing rather than by reading a name.
            if !drawn(renderer, style(state), &rows, caret)? {
                return Ok(Ended::Cramped);
            }
        }

        // Offered to the selection first, wherever a key is read in this
        // session. A drag is answered there and comes back as nothing,
        // which is what lets a reader select across a component that has
        // never heard of one.
        let Some(arrived) = renderer.pressed()? else {
            // A hover is one of the things answered there, and a component
            // that lights a row under the pointer is the only party that
            // knows which row that is. So where the pointer has come to rest
            // somewhere inside what this component drew, it is asked -- and
            // only where that place is not the one it was already told about,
            // since a terminal reports the pointer once per cell and every
            // report would otherwise be a frame.
            let now = pointing(renderer);
            if now != resting {
                resting = now;
                // A pointer that has left is news too: the row it was lighting
                // has to go out. There is no press for *nowhere* and none is
                // wanted -- a place past every row a component answered with is
                // one it draws nothing at, and it reads that off the same
                // number it reads every other place off.
                let (row, column) = now.unwrap_or((usize::MAX, usize::MAX));
                if keys(Pressed::Hovered { row, column }, state) == Moved::Redraw {
                    changed = true;
                }
            }
            continue;
        };

        // The rows on screen were laid out for a window that is no longer this
        // one. Taking them back is the renderer's; saying the picture no longer
        // matches is `keys` below — and the rows are laid out again against the
        // new height as well as the new width, since height is what a component
        // gives rows up for.
        if arrived == Pressed::Resized {
            renderer.resized()?;
        }

        // A click is reported against the whole window, and a component thinks
        // in the rows it drew. The renderer is what knows both, so the click is
        // rewritten to a row of the region here, and one that landed anywhere
        // else — the transcript above, a band nothing is standing in — is
        // nothing this component is asked about.
        let arrived = match arrived {
            Pressed::Clicked { row, column } => match renderer.aimed(row) {
                Some(Aimed::Stood(row)) => Pressed::Clicked { row, column },
                _ => continue,
            },
            other => other,
        };

        // Read before the key is handed over, because it is handed over: a
        // component takes the press, and this loop still has to know what it
        // was to answer for it below.
        let wheel = match arrived {
            Pressed::Scrolled { back } => Some(back),
            _ => None,
        };

        match keys(arrived, state) {
            Moved::Redraw => changed = true,
            Moved::Still => {
                changed = false;

                // The wheel, where the component did nothing with it. A
                // component the wheel ought to walk — a view over more text
                // than its rows hold — says so by moving; everything else is
                // standing over a transcript, and the transcript is what a
                // reader reaching for a wheel meant to move. The renderer draws
                // its own frame and the rows this loop put down are in a band
                // it is not touching, so nothing is laid out again for it.
                if let Some(back) = wheel {
                    renderer.notched(back)?;
                }
            }
            Moved::Took => return over(renderer, Ended::Took),
            Moved::Left => return over(renderer, Ended::Left),
        }
    }
}

/// Where the pointer rests inside what a standing component drew.
///
/// The row is the component's own, counted from the first row it answered with,
/// so a component can read it against the rows it laid out without knowing
/// which band of the window they went into. `None` for a pointer resting
/// anywhere else -- the transcript above, the head, a band nothing is standing
/// in -- which a component reads as *nothing of mine is under it*.
fn pointing<T: Terminal>(renderer: &Renderer<T>) -> Option<(usize, usize)> {
    let (row, column) = renderer.pointer()?;
    match renderer.aimed(row) {
        Some(Aimed::Stood(row)) => Some((row, column)),
        _ => None,
    }
}

/// Draws `rows` where the box was, and says whether there was room for them.
///
/// A component gives up rows rather than overflowing the width, and its last
/// rung is nothing at all; the height is checked here, since a component
/// taller than the window is one the reader would only see part of.
///
/// The box is taken off first and the rows stand in the band above it. A
/// component here is not a prompt, so the share a prompt is held to is not
/// its — a list of themes may take the window it needs, and there is nothing
/// underneath it for that to push off the screen.
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
        .filter(|_| rows.len() <= renderer.room())
    else {
        return Ok(false);
    };

    let caret = caret.unwrap_or(Caret {
        row: last,
        column: 0,
    });

    // The row beside the box is the panel's too while the panel stands: the
    // box is covered, and the `transcript map` door on it reports on a screen
    // the panel owns, so the band goes blank with the box. Up on the way in,
    // back on the way out — covered and uncovered in the same place the box is.
    renderer.cover_map()?;
    renderer.live(&[], Caret::default(), style.palette())?;
    renderer.under(rows, Some(caret), style.palette())?;
    Ok(true)
}

/// Takes back the rows this was standing in and answers with `ended`.
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
