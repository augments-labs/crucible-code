//! A panel standing where the prompt box was, until one of its entries is
//! taken or it is left.
//!
//! The box and the row under it are what the live region holds while a line is
//! being typed; this puts a panel there instead and reads keys against it. That
//! is the whole of what "the panel replaces the prompt" means — nothing is
//! drawn over anything, the region is simply given different rows, and the next
//! frame after this returns is the box again.
//!
//! Nothing about the panel's *contents* is decided here. What is being chosen,
//! what each entry says and what the footer names are the caller's, which is
//! what lets `/login`, `/logout` and a first run that opens with the same list
//! share one loop rather than one string.
//!
//! The one promise this module makes about the keys is that the mark stops at
//! each end instead of wrapping. A ring puts the first entry one key past the
//! last, so the key that went too far is the key that goes further.

use crucible_tui::{Caret, Key, Panel, Pressed, Renderer, Terminal, pressed};

use crate::cli::Fatal;
use crate::cli::style::Style;

/// What one key does to a panel that is standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moved {
    /// The picture no longer matches the mark, so the next frame is owed.
    Redraw,
    /// Nothing changed, so nothing is drawn.
    Still,
    /// The entry under the mark was taken.
    Took,
    /// The panel was left with nothing taken.
    Left,
}

/// Stands `panel` where the prompt box was and reads keys until one is chosen.
///
/// The index that comes back is into the slice that was handed in, and
/// `panel.chosen` is where the mark starts. `None` is a panel that was left and
/// a window with no room to draw one in alike — [`Panel::within`] gives up rows
/// rather than overflowing, and its last rung is nothing at all. One answer,
/// because to a caller they are one fact: nothing was taken. Not one to this
/// function, which reads no key in the second case and cannot be looped on.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn pick<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    mut panel: Panel<'_>,
) -> Result<Option<usize>, Fatal> {
    let count = panel.shown.len();
    if count == 0 {
        return Ok(None);
    }

    let mut at = panel.chosen.min(count - 1);
    let mut changed = true;

    loop {
        if changed {
            panel.chosen = at;
            if !standing(renderer, style, panel)? {
                return Ok(None);
            }
        }

        let arrived = pressed()?;

        // The rows on screen were laid out for a window that is no longer this
        // one. Taking them back is the renderer's; saying the picture no longer
        // matches is the match below — and the panel is measured against the
        // new height as well as the new width, since height is what it gives
        // rows up for.
        if arrived == Pressed::Resized {
            renderer.resized()?;
        }

        match moving(arrived, &mut at, count) {
            Moved::Redraw => changed = true,
            Moved::Still => changed = false,
            Moved::Took => return leaving(renderer, Some(at)),
            Moved::Left => return leaving(renderer, None),
        }
    }
}

/// Draws the panel, and says whether there was room for one.
///
/// The cursor is parked on the last row it drew. Nothing here hides it — this
/// program never does — and the footer is the one row of a panel where it
/// stands beside a key rather than inside a name somebody is reading.
fn standing<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    panel: Panel<'_>,
) -> Result<bool, Fatal> {
    let rows = panel.within(renderer.columns(), renderer.rows(), style.glyphs());
    let Some(last) = rows.len().checked_sub(1) else {
        return Ok(false);
    };

    let caret = Caret {
        row: last,
        column: 0,
    };

    renderer.live(&rows, caret, style.palette())?;
    Ok(true)
}

/// Takes the panel off the screen and answers with `picked`.
///
/// It is not written down. A panel is a question, and what belongs in the
/// record is the answer to it — which is the caller's to commit, in the words
/// that say what the answer meant.
fn leaving<T: Terminal>(
    renderer: &mut Renderer<T>,
    picked: Option<usize>,
) -> Result<Option<usize>, Fatal> {
    renderer.settle()?;
    Ok(picked)
}

/// What `arrived` does to a panel of `count` entries with `at` marked.
///
/// Every key that is not one of the five is a key that moved nothing. A panel
/// is not a line, so a letter is not a character being typed, and a frame for
/// each of them would be a frame per keystroke for somebody typing at a list.
fn moving(arrived: Pressed, at: &mut usize, count: usize) -> Moved {
    match arrived {
        Pressed::Up => step(at, at.checked_sub(1)),
        Pressed::Down => step(at, Some(*at + 1).filter(|next| *next < count)),
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

/// Moves the mark to `next`, where there is one to move to.
fn step(at: &mut usize, next: Option<usize>) -> Moved {
    match next {
        Some(next) => {
            *at = next;
            Moved::Redraw
        }
        None => Moved::Still,
    }
}

#[cfg(test)]
mod tests;
