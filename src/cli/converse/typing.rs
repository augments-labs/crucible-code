//! Reading a prompt as it is typed, inside the box it is typed into.
//!
//! The other way to read one is [`std::io::BufRead::read_line`], which the loop
//! still uses wherever there is no terminal. What this adds is everything that
//! needs a keystroke rather than a line: the box around what is being written,
//! the mode under it, and a window that follows the cursor along a line longer
//! than the screen.
//!
//! Raw mode is held for exactly as long as a line is being typed and handed
//! back before the turn starts. That is what keeps the rest of the session
//! working the way it did: a permission question is still answered by a line
//! the terminal collects, and Ctrl-C during a turn is still the signal the
//! terminal sends rather than a byte nothing is waiting to read.
//!
//! It is also the only place the mode changes, which is why the engine is a
//! parameter here. The mode is a fact about the session and lives in the
//! engine; this reads it to draw the row under the box and steps it when the
//! key that steps it arrives. Nothing else is looking at it while a prompt is
//! being typed, so there is one copy of it and no lock over it.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use crucible_core::Mode;
use crucible_runner::Runner;
use crucible_tui::{
    Editor, Glyphs, Key, Menu, Pressed, Prompt, Raw, Renderer, Row, Slot, Terminal, Typed, pressed,
};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::command;
use super::mode::tone;

/// What the row under the box says after the mode, when pressing the key again
/// is all there is to do with it.
///
/// The one place keys are printed beside the thing they act on. It earns that
/// by being the only way anybody finds out the row is a control at all.
const CYCLE: &str = "(shift+tab to cycle)";

/// What the row under the status says once Ctrl-C has been pressed against an
/// empty line.
///
/// On screen from the first press rather than after the window has lapsed. The
/// press is the moment somebody is asking to leave, so it is the moment worth
/// answering — and by the time waiting too long could be noticed, the thing to
/// say would be that the offer had already gone.
const LEAVING: &str = "press ctrl-c again to leave";

/// How long the second press has to arrive in.
///
/// Long enough to be a pair of presses somebody meant and short enough that it
/// cannot span a thought. What it is defending against is a Ctrl-C aimed at a
/// turn that had already finished: the terminal sends the key here instead, and
/// a session ended by it is one nobody asked to end.
const TOGETHER: Duration = Duration::from_secs(2);

/// What reading a prompt produced.
pub(crate) enum Asked {
    /// A line was typed and finished.
    Said(String),
    /// The session is over: Ctrl-D on an empty line, or Ctrl-C twice against
    /// one.
    Ended,
    /// There is nothing here to type into. The caller reads a line instead.
    Untyped,
}

/// Reads one prompt, drawing it as it arrives.
///
/// The guard is taken for this call and dropped on the way out of it, including
/// on the `?` below, so no path out of here leaves a terminal that shows
/// nothing as the user types.
pub(crate) fn ask<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    runner: &mut Runner,
) -> Result<Asked, Fatal> {
    let Some(_raw) = Raw::enter()? else {
        return Ok(Asked::Untyped);
    };

    let glyphs = style.glyphs();
    let mut editor = Editor::new();

    // A mode put on screen and waiting to be agreed to. `None` is the ordinary
    // state, where what the box is drawn in is the mode the engine is in.
    let mut proposed: Option<Mode> = None;
    let mut says = saying(runner, proposed, glyphs);

    // When Ctrl-C was last pressed against an empty line, if it is still the
    // last key pressed. Taken by every other key, so the pair has to be two
    // presses in a row rather than two with a session between them.
    let mut leaving: Option<Instant> = None;

    draw(renderer, &editor, style, &says, None)?;

    loop {
        let arrived = pressed()?;

        // A mode waiting to be agreed to is modal, and the row under the box
        // names the two keys that answer it. Everything else waits, including a
        // character that would otherwise be typed into the line: a key that
        // both typed and left the question standing would have somebody
        // agreeing to one thing while looking at another.
        if proposed.is_some() {
            match arrived {
                Pressed::Resized => renderer.resized()?,
                Pressed::Key(Key::Enter) => {
                    runner.cycle();
                    proposed = None;
                }
                Pressed::Escape => proposed = None,
                _ => continue,
            }

            says = saying(runner, proposed, glyphs);
            draw(renderer, &editor, style, &says, None)?;
            continue;
        }

        // Whatever arrived, the offer to leave was made to the key after the
        // one that made it, and this is that key.
        let offered = leaving.take();

        match arrived {
            // Redrawn rather than re-wrapped: the box was laid out for a width
            // the window no longer has, and the rows it left on screen are the
            // renderer's to take back before the new ones go down.
            Pressed::Resized => {
                renderer.resized()?;
                draw(renderer, &editor, style, &says, None)?;
            }

            // Nothing is standing, so there is nothing to back out of — except
            // the offer above, which is on screen and has just been taken back.
            Pressed::Escape | Pressed::Ignored => {
                if offered.is_some() {
                    draw(renderer, &editor, style, &says, None)?;
                }
            }

            // Stepping the mode on. One that has to be agreed to goes on screen
            // first and is entered by the key that agrees to it; every other
            // one takes effect on the press.
            Pressed::Cycle => {
                let next = runner.mode().next();

                if agreed_first(next) {
                    proposed = Some(next);
                } else {
                    runner.cycle();
                }

                says = saying(runner, proposed, glyphs);
                draw(renderer, &editor, style, &says, None)?;
            }

            Pressed::Key(key) => match editor.press(key) {
                // A key that moved nothing costs no frame, unless the offer
                // above is on screen and now stale. An arrow held down against
                // the end of a line is what the first half of that saves.
                Typed::Ignored => {
                    if offered.is_some() {
                        draw(renderer, &editor, style, &says, None)?;
                    }
                }
                Typed::Changed => draw(renderer, &editor, style, &says, None)?,
                Typed::Submitted => return said(renderer, &mut editor, style),

                // Ctrl-C against a line with nothing on it. The first press
                // says what a second one would do; the second does it, so long
                // as it is the very next key and soon enough to be one gesture
                // with the first.
                Typed::Interrupted => {
                    if together(offered, Instant::now()) {
                        renderer.settle()?;
                        return Ok(Asked::Ended);
                    }

                    leaving = Some(Instant::now());
                    draw(renderer, &editor, style, &says, Some(LEAVING))?;
                }

                Typed::Ended => {
                    renderer.settle()?;
                    return Ok(Asked::Ended);
                }
            },
        }
    }
}

/// Whether a press at `now` is the second half of the one `offered`.
///
/// The clock is an argument rather than read here, which is what lets the
/// window be tested at all: the case worth pinning is the press that arrived
/// too late, and waiting for it in a test would put [`TOGETHER`] into how long
/// the suite takes.
fn together(offered: Option<Instant>, now: Instant) -> bool {
    offered.is_some_and(|since| now.duration_since(since) < TOGETHER)
}

/// What the row under the box says, and the colour the box says it in.
///
/// Built when the mode changes rather than when the box is drawn. The box is
/// redrawn on every keystroke and this is the same row until a key changes the
/// mode, so formatting it per frame would be work done to produce the bytes
/// that were already there.
struct Says {
    /// The mode, in the words somebody reads rather than the ones they type.
    mode: Cow<'static, str>,
    /// The keys that act on it, said quietly after.
    keys: &'static str,
    /// What the border and the sentence are both drawn in.
    tone: Slot,
}

/// The row for whichever mode the box is showing.
fn saying(runner: &Runner, proposed: Option<Mode>, glyphs: Glyphs) -> Says {
    // The colour goes on first and the sentence says what the colour means,
    // which is what the step is worth a keystroke for: *nothing will be asked*
    // is something somebody can agree to, where `fullAccess` is something they
    // can only recognise.
    let Some(mode) = proposed else {
        let mode = runner.mode();

        return Says {
            mode: Cow::Borrowed(mode.sentence()),
            keys: CYCLE,
            tone: tone(mode),
        };
    };

    let (means, keys) = confirming(glyphs);

    Says {
        mode: Cow::Owned(format!("{} {means}", mode.sentence())),
        keys,
        tone: tone(mode),
    }
}

/// What the row says after the mode while one is waiting to be agreed to, and
/// the keys that answer it.
///
/// A pair per set of characters rather than one sentence with its marks swapped
/// out. What a terminal with no box-drawing font gets is a different sentence
/// rather than the same one punctuated differently, and keeping the two side by
/// side is what makes that a decision instead of a fallback.
const fn confirming(glyphs: Glyphs) -> (&'static str, &'static str) {
    match glyphs {
        Glyphs::Unicode => ("— nothing will be asked", "(⏎ confirm · esc cancels)"),
        Glyphs::Ascii => ("- nothing will be asked", "(enter confirms, esc cancels)"),
    }
}

/// Whether stepping into this mode is agreed to before it takes effect.
///
/// One mode, one step, and that is the whole of the case for it: a confirm on a
/// key somebody pressed on purpose is exactly the shape that teaches people to
/// dismiss confirms unread, and this one survives because it is the only one. A
/// second landing on this key is a reason to reconsider this one rather than to
/// join it.
const fn agreed_first(mode: Mode) -> bool {
    match mode {
        Mode::Ask | Mode::AllowEdits => false,
        Mode::FullAccess => true,
    }
}

/// Puts the box on screen with the cursor where the line was typed to.
///
/// The box is the last rows of the region and the list, when there is one, is
/// the first. Drawn in that order for the reason the list opens upwards at all:
/// the box and the row under it are what the eye is resting on, and rows added
/// above them leave both exactly where they were.
fn draw<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    style: Style,
    says: &Says,
    asking: Option<&str>,
) -> Result<(), Fatal> {
    let columns = renderer.columns();

    let prompt = Prompt {
        said: editor.text(),
        column: editor.column(),
        mode: says.mode.as_ref(),
        tone: says.tone,
        hint: says.keys,
        asking,
    };

    let mut boxed = prompt.rows(columns, style.glyphs());
    let mut caret = prompt.caret(columns);

    // What is left for a list once the box and the blank row that keeps it off
    // the box have taken theirs.
    let room = renderer.rows().saturating_sub(boxed.len() + 1);

    let mut rows = opened(editor.text(), columns, room, style.glyphs());
    caret.row += rows.len();
    rows.append(&mut boxed);

    renderer.live(&rows, caret, style.palette())?;
    Ok(())
}

/// The command list to open above the box, and the blank row under it.
///
/// Empty in the ordinary case, where the line is a prompt: nothing is
/// allocated, and the region is the rows the box has always been.
///
/// A list with no room for it is not opened. The live region is taken back by
/// moving the cursor up over it, so one taller than the screen is one whose top
/// has already scrolled beyond reach — the next rewind would move back over
/// rows the terminal has taken. Drawing what fits instead would be worse than
/// drawing nothing: a list cut off at the top reads as the whole list.
fn opened(said: &str, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
    let shown = command::filtering(said, glyphs);
    if shown.is_empty() || shown.len() > room {
        return Vec::new();
    }

    let mut rows = Menu {
        shown: &shown,
        chosen: None,
    }
    .rows(columns, glyphs);
    rows.push(Row::new());
    rows
}

/// Takes the finished line, leaving it in the record where the box was.
///
/// The box goes and the line stays: what was asked belongs in the transcript
/// beside the answer to it, and the box is chrome around a line that is no
/// longer being changed.
fn said<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &mut Editor,
    style: Style,
) -> Result<Asked, Fatal> {
    let said = editor.take();

    renderer.settle()?;
    renderer.present(&[Prompt::committed(&said, style.glyphs())], style.palette())?;

    Ok(Asked::Said(said))
}

#[cfg(test)]
mod tests;
