//! Standing the whole of what the transcript had to cut down to a row.
//!
//! Every result a row could not say, newest first, in the region the prompt box
//! was in. It stands there rather than being written into the transcript for the
//! reason nothing else standing there is written down: the results are already
//! in the record as the rows that could not fit them, and committing the text a
//! second time would be a session saying everything twice. So the way out of it
//! is the way out of everything else here — the region is given back, and the
//! screen reads afterwards as though nothing had been opened.
//!
//! Which is also what makes Ctrl+O a toggle rather than a door. It is the key
//! the rows themselves name, and pressing it against what it opened closes it
//! again; there is nothing to undo afterwards because there was never anything
//! to undo.
//!
//! Between turns only. A turn writes its results into the transcript as they
//! arrive and every frame of that reaches over whatever is standing underneath,
//! so a view opened while one runs would be taken back by the next tool that
//! answered — which is the tool the reader most likely opened it to read about.

use crucible_tui::{Expanded, Key, Pressed, Renderer, Shown, Terminal};

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
struct Standing {
    /// How far down the whole of it the window is open.
    from: usize,
    /// The furthest down it may go, as of the last frame drawn.
    end: usize,
}

/// Opens what was cut and reads keys until it is closed.
///
/// Nothing is drawn where nothing was cut. The key is offered by the rows that
/// were cut, so a session with none of them has made no offer, and a frame put
/// up in answer to a press nobody meant is one that took the prompt away for no
/// reason.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    kept: &Kept,
) -> Result<(), Fatal> {
    if kept.is_empty() {
        return Ok(());
    }

    let shown: Vec<Shown<'_>> = kept
        .newest()
        .map(|whole| Shown {
            called: whole.called(),
            text: whole.text(),
        })
        .collect();

    let glyphs = style.glyphs();
    let mut standing = Standing::default();

    let laid = |standing: &mut Standing, columns: usize, rows: usize| {
        let expanded = Expanded {
            shown: &shown,
            from: standing.from,
        };

        // Written before the rows are asked for, so the key pressed against this
        // picture is clamped to what this picture could reach.
        standing.end = expanded.end(columns, rows);
        standing.from = standing.from.min(standing.end);

        expanded.within(columns, rows, glyphs)
    };

    // Nothing is taken here and nothing is committed, so every way out is the
    // same way out: the region goes back and the box comes up under it.
    region::stand(renderer, style, &mut standing, laid, moving)?;
    Ok(())
}

/// What one key does to the view.
///
/// Every key is named rather than caught by a rest arm, for the reason the
/// permission panel names every one of its own: a key arriving at something
/// standing is either something it does or something it has decided to ignore,
/// and a new [`Pressed`] must be decided about here rather than quietly join the
/// second group.
fn moving(arrived: Pressed, standing: &mut Standing) -> Moved {
    match arrived {
        Pressed::Up => {
            let next = standing.from.checked_sub(1);
            region::step(&mut standing.from, next)
        }
        Pressed::Down => {
            let next = Some(standing.from.saturating_add(1)).filter(|next| *next <= standing.end);
            region::step(&mut standing.from, next)
        }

        // Ctrl+O closes what Ctrl+O opened, which is the whole of what the rows
        // offering it say the key does. Esc is the way out of whatever is
        // standing everywhere else in a session, and Ctrl-C and Ctrl-D reach the
        // line under this one — so the view goes first and the line gets the key
        // it was always going to get.
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
