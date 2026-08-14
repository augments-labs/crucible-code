//! Something standing where the prompt box was, until one of its entries is
//! taken or it is left.
//!
//! The box and the row under it are what the live region holds while a line is
//! being typed; this puts a panel or a ladder there instead and reads keys
//! against it. That is the whole of what "it replaces the prompt" means —
//! nothing is drawn over anything, the region is simply given different rows,
//! and the next frame after this returns is the box again.
//!
//! Nothing about the *contents* is decided here. What is being chosen, what
//! each entry says and what the footer names are the caller's, which is what
//! lets `/login`, `/logout` and a first run that opens with the same list share
//! one loop rather than one string.
//!
//! Two loops rather than one, because the two shapes are read by different
//! keys: a list is walked down and a ladder is walked along, and a component
//! whose arrows disagree with its picture is one nobody trusts twice. What they
//! share is what happens around the keys — the resize, the answer, and the box
//! coming back.
//!
//! The one promise this module makes about the keys is that the mark stops at
//! each end instead of wrapping. A ring puts the first entry one key past the
//! last, so the key that went too far is the key that goes further.

use crucible_tui::{Caret, Key, Ladder, Panel, Pressed, Renderer, Row, Terminal, pressed};

use crate::cli::Fatal;
use crate::cli::style::Style;

/// What came back off a panel.
///
/// Three answers rather than two, because what a caller owes differs in each.
/// A panel that was left has already been answered — the person who pressed
/// escape asked for the screen they had before it, and the list underneath
/// would be this program insisting on the question a second time. What is owed
/// there is one line saying the question was dropped, in the words of whatever
/// asked it. A panel there was no room for was never drawn and read no key, so
/// the answer still has to come from somewhere, and the list is it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Picked {
    /// The entry at this index was taken. It is an index into the slice that
    /// was handed in.
    Took(usize),
    /// The panel was left with nothing taken. What is owed is the caller's
    /// line saying so.
    Left,
    /// There was no room to stand one. [`Panel::within`] gives up rows rather
    /// than overflowing, and its last rung is nothing at all.
    Cramped,
}

/// [`Picked`], with the index already looked up in what the panel was built
/// from — which is the shape every caller wants, none of them having a use for
/// a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Taken<T> {
    /// This entry was taken off the panel.
    Took(T),
    /// The panel was left. What is owed is the caller's line saying so.
    Left,
    /// Nothing was drawn and nothing read. The caller still owes an answer.
    Cramped,
}

impl Picked {
    /// Looks a taken index up in `all`, the slice the panel was built from.
    ///
    /// The index cannot miss — it came back out of a list this same slice was
    /// mapped into. Written as a lookup that can rather than as an assertion
    /// nobody would read again, and a miss falls to the answer a caller has for
    /// a panel it could not stand, which is the one that draws something.
    pub(super) fn of<T: Copy>(self, all: &[T]) -> Taken<T> {
        match self {
            Self::Took(at) => all.get(at).copied().map_or(Taken::Cramped, Taken::Took),
            Self::Left => Taken::Left,
            Self::Cramped => Taken::Cramped,
        }
    }
}

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
/// `panel.chosen` is where the mark starts, and [`Picked`] says which of the
/// three ways it ended.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn pick<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    mut panel: Panel<'_>,
) -> Result<Picked, Fatal> {
    let count = panel.shown.len();
    if count == 0 {
        return Ok(Picked::Cramped);
    }

    let mut at = panel.chosen.min(count - 1);
    let mut changed = true;

    loop {
        if changed {
            panel.chosen = at;
            let rows = panel.within(renderer.columns(), renderer.rows(), style.glyphs());
            if !standing(renderer, style, &rows)? {
                return Ok(Picked::Cramped);
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
            Moved::Took => return leaving(renderer, Picked::Took(at)),
            Moved::Left => return leaving(renderer, Picked::Left),
        }
    }
}

/// Stands `ladder` where the prompt box was and reads keys until a rung is
/// taken.
///
/// `ladder.chosen` is where the mark starts, and the index that comes back is
/// into the rungs it was handed. Otherwise [`pick`], for a component read along
/// rather than down.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn adjust<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    mut ladder: Ladder<'_>,
) -> Result<Picked, Fatal> {
    let count = ladder.rungs.len();
    if count == 0 {
        return Ok(Picked::Cramped);
    }

    let mut at = ladder.chosen.min(count - 1);
    let mut changed = true;

    loop {
        if changed {
            ladder.chosen = at;
            let rows = ladder.rows(renderer.columns(), style.glyphs());
            if !standing(renderer, style, &rows)? {
                return Ok(Picked::Cramped);
            }
        }

        let arrived = pressed()?;

        if arrived == Pressed::Resized {
            renderer.resized()?;
        }

        match sliding(arrived, &mut at, count) {
            Moved::Redraw => changed = true,
            Moved::Still => changed = false,
            Moved::Took => return leaving(renderer, Picked::Took(at)),
            Moved::Left => return leaving(renderer, Picked::Left),
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
fn standing<T: Terminal>(
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

/// Takes the panel off the screen and answers with `picked`.
///
/// It is not written down. A panel is a question, and what belongs in the
/// record is the answer to it — which is the caller's to commit, in the words
/// that say what the answer meant.
fn leaving<T: Terminal>(renderer: &mut Renderer<T>, picked: Picked) -> Result<Picked, Fatal> {
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

/// What `arrived` does to a ladder of `count` rungs with `at` marked.
///
/// The arrows that walk it are the ones drawn under it, and they are the across
/// pair: a ladder is one row of rungs, so up and down point at nothing. They are
/// left alone rather than aliased onto the across pair, because a key that moves
/// the mark in a direction the picture does not have is how somebody learns not
/// to trust the picture.
fn sliding(arrived: Pressed, at: &mut usize, count: usize) -> Moved {
    match arrived {
        Pressed::Key(Key::Left) => step(at, at.checked_sub(1)),
        Pressed::Key(Key::Right) => step(at, Some(*at + 1).filter(|next| *next < count)),
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
