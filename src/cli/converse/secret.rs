//! A line typed where the screen says how much of it there is and nothing else.
//!
//! The box is the one the prompt uses, given dots instead of what was typed.
//! That is deliberate: what somebody is looking at while they paste a key is
//! the same shape as what they were looking at a moment before, so the one
//! thing that changed is the one thing that matters — the characters are not
//! coming back.
//!
//! There is no cursor to move, so the keys that move one do nothing. A secret
//! is a value being handed over rather than a sentence being composed, and an
//! arrow key against a line nobody can read moves an insertion point somebody
//! would then have to guess the position of. Rubbing out from the end is the
//! one edit that needs no sight of the line.
//!
//! What is typed lives in a `String` for as long as it takes to reach the store
//! and is never drawn, committed or put in an error. The dots are counted from
//! it and are the only thing about it that reaches the terminal. At sixteen KiB
//! the box refuses another character and says why beneath itself; no supported
//! credential is close to that boundary.

use crucible_tui::{Key, Pressed, Prompt, Renderer, Slot, Terminal, pressed};

use crate::cli::Fatal;
use crate::cli::style::Style;

/// What stands in for one character.
const DOT: &str = "•";

/// The row under the box, which says the one key that is not Enter.
const CANCEL: &str = "esc to cancel";

/// What a key box says after refusing input it cannot retain.
const LIMITED: &str = "key is limited to 16 KiB";

/// More than any supported credential needs, and small enough that a pasted
/// file cannot become secret-shaped process memory.
const MAX_BYTES: usize = 16 * 1024;

/// What one key does to a secret being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Typing {
    /// The dots no longer match what is held.
    Redraw,
    /// Nothing changed, so nothing is drawn.
    Still,
    /// The edit would retain more secret input than the box accepts.
    Refused,
    /// The line is finished.
    Done,
    /// The box was left with nothing given.
    Left,
}

/// Reads one, drawing dots where the prompt draws a line.
///
/// `asking` stands where the mode stands on the prompt, because it is the same
/// kind of fact: what this box is for, true until it closes. `None` comes back
/// from a box that was left and from one finished with nothing in it — a key was
/// asked for and none was given, either way.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn ask<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    asking: &str,
) -> Result<Option<String>, Fatal> {
    let mut held = String::new();
    let mut changed = true;
    let mut limited = false;

    loop {
        if changed {
            standing(renderer, style, asking, held.chars().count(), limited)?;
        }

        let arrived = pressed()?;

        // The rows on screen were laid out for a window that is no longer this
        // one. Taking them back is the renderer's; saying the picture no longer
        // matches is the match below.
        if arrived == Pressed::Resized {
            renderer.resized()?;
        }

        match typing(arrived, &mut held) {
            Typing::Redraw => {
                changed = true;
                limited = false;
            }
            Typing::Still => changed = false,
            Typing::Refused => {
                changed = true;
                limited = true;
            }
            Typing::Done => {
                renderer.settle()?;
                return Ok(taken(&held));
            }
            Typing::Left => {
                renderer.settle()?;
                return Ok(None);
            }
        }
    }
}

/// Draws the box with `dots` characters accounted for.
fn standing<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    asking: &str,
    dots: usize,
    limited: bool,
) -> Result<(), Fatal> {
    let said = DOT.repeat(dots);
    let prompt = Prompt {
        said: &said,
        // A dot is one column wide, so the cursor sits after as many columns as
        // there are characters — which is the one thing the box knows about the
        // line and the only thing it is allowed to know.
        column: dots,
        mode: asking,
        // The border says what the box is for. Not a mode's colour: no mode is
        // in force in here, and borrowing one would say a permission had
        // changed because a key was being asked for.
        tone: Slot::Accent,
        hint: CANCEL,
        // Nothing about the vendor, the model or the rung, which are facts
        // about the next turn. This box is not one: what is typed into it goes
        // to no provider, and naming one over a key being pasted in would say
        // it was about to be sent somewhere.
        model: "",
        provider: "",
        effort: None,
        asking: limited.then_some(LIMITED),
        room: Prompt::room(renderer.rows()),
    };

    let rows = prompt.rows(renderer.columns(), style.glyphs());

    renderer.live(&rows, prompt.caret(renderer.columns()), style.palette())?;
    Ok(())
}

/// What a finished line comes to.
///
/// Trimmed, because a copied key commonly carries spaces around it, which every
/// provider would otherwise refuse with a sentence about the key being wrong.
/// A pasted newline arrives as Enter and submits before this function is called.
fn taken(held: &str) -> Option<String> {
    let trimmed = held.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// What `arrived` does to a line of `held` characters.
///
/// Everything that is not one of these does nothing at all. The arrows and the
/// word keys are in that set on purpose: see the prose above.
fn typing(arrived: Pressed, held: &mut String) -> Typing {
    match arrived {
        Pressed::Key(Key::Char(typed)) => {
            if typed.len_utf8() > MAX_BYTES.saturating_sub(held.len()) {
                return Typing::Refused;
            }
            held.push(typed);
            Typing::Redraw
        }
        Pressed::Key(Key::Backspace) => match held.pop() {
            Some(_) => Typing::Redraw,
            None => Typing::Still,
        },
        Pressed::Key(Key::Enter) => Typing::Done,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Typing::Left,
        Pressed::Resized => Typing::Redraw,
        _ => Typing::Still,
    }
}

#[cfg(test)]
mod tests;
