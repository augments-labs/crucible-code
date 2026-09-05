//! A line typed where the screen says how much of it there is and nothing else.
//!
//! The box stands where the prompt box stood, one mark per character instead of
//! what was typed, under a breadcrumb saying whose key it is and a sentence
//! saying where the key goes. Nothing of the prompt is on the screen with it —
//! no window reading, no model, no door to the commands — because a key being
//! pasted in is not a turn, and a fact about the next turn drawn over it would
//! say it was about to be sent somewhere.
//!
//! There is no cursor to move, so the keys that move one do nothing. A secret
//! is a value being handed over rather than a sentence being composed, and an
//! arrow key against a line nobody can read moves an insertion point somebody
//! would then have to guess the position of. Rubbing out from the end is the
//! one edit that needs no sight of the line.
//!
//! What is typed or pasted lives in a `String` for as long as it takes to reach
//! the store and is never drawn, committed or put in an error. The marks are
//! counted from it, and the count is the only thing about it that reaches the
//! panel. At sixteen KiB the box refuses more, whole and silently: no supported
//! credential is close to that boundary, so what did not fit was not a key.

use crucible_tui::{Caret, Glyphs, Key, KeyPanel, Pressed, Renderer, Row, Terminal};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::region::{self, Ended, Moved};

/// More than any supported credential needs, and small enough that a pasted
/// file cannot become secret-shaped process memory.
const MAX_BYTES: usize = 16 * 1024;

/// Reads a key for `provider`, drawing a mark per character where the prompt
/// draws a line.
///
/// `provider` is the display name the breadcrumb and the frame's label spell.
/// `None` comes back from a box that was left, and from one there was no room
/// to stand — a key was asked for and none was given, either way. Enter with
/// nothing in the box is not a way out of it: there is nothing to hand over, so
/// the box goes on standing.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn ask<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    provider: &str,
) -> Result<Option<String>, Fatal> {
    let glyphs = style.glyphs();
    let mut held = String::new();

    let ended = region::stand(
        renderer,
        |_| style,
        &mut held,
        |held, columns, room| standing(provider, held, columns, room, glyphs),
        typing,
    )?;

    Ok(match ended {
        Ended::Took => taken(&held),
        Ended::Left | Ended::Cramped => None,
    })
}

/// The rows the box stands as while `held` is what it holds.
///
/// The count is all the panel is given: the characters stay here.
fn standing(
    provider: &str,
    held: &str,
    columns: usize,
    room: usize,
    glyphs: Glyphs,
) -> (Vec<Row>, Option<Caret>) {
    KeyPanel {
        provider,
        held: held.chars().count(),
    }
    .within(columns, room, glyphs)
}

/// What a finished line comes to.
///
/// Trimmed, because a copied key commonly carries spaces around it, which every
/// provider would otherwise refuse with a sentence about the key being wrong.
fn taken(held: &str) -> Option<String> {
    let trimmed = held.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// What a pasted line comes to before it is held: the spaces and the newline a
/// clipboard carries around a key are dropped, and so is every control
/// character, which no key contains and no box should retain.
pub(super) fn pasted(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// Appends `text` whole, or not at all where it would take the line past the
/// bound. Says whether the picture changed.
///
/// Whole, because half a key is worth less than no key; silently, because
/// nothing a reader would paste as a key comes near the bound, and the marks
/// not growing is the whole of the answer.
fn appended(held: &mut String, text: &str) -> Moved {
    if text.is_empty() || text.len() > MAX_BYTES.saturating_sub(held.len()) {
        return Moved::Still;
    }
    held.push_str(text);
    Moved::Redraw
}

/// What `arrived` does to a line of `held` characters.
///
/// Everything that is not one of these does nothing at all. The arrows and the
/// word keys are in that set on purpose: see the prose above.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn typing(arrived: Pressed, held: &mut String) -> Moved {
    match arrived {
        Pressed::Key(Key::Char(typed)) if !typed.is_control() => {
            appended(held, typed.encode_utf8(&mut [0; 4]))
        }
        Pressed::Pasted(text) => appended(held, &pasted(&text)),
        Pressed::Key(Key::Backspace) => match held.pop() {
            Some(_) => Moved::Redraw,
            None => Moved::Still,
        },
        Pressed::Key(Key::Enter) => match taken(held) {
            Some(_) => Moved::Took,
            None => Moved::Still,
        },
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

#[cfg(test)]
mod tests;
