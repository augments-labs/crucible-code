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

use crucible_tui::{Editor, Pressed, Prompt, Raw, Renderer, Slot, Terminal, Typed, pressed};

use crate::cli::Fatal;
use crate::cli::style::Style;

/// What reading a prompt produced.
pub(crate) enum Asked {
    /// A line was typed and finished.
    Said(String),
    /// The session is over: Ctrl-D on an empty line, or Ctrl-C against one.
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
    mode: &str,
) -> Result<Asked, Fatal> {
    let Some(_raw) = Raw::enter()? else {
        return Ok(Asked::Untyped);
    };

    let mut editor = Editor::new();
    draw(renderer, &editor, style, mode)?;

    loop {
        match pressed()? {
            // Redrawn rather than re-wrapped: the box was laid out for a width
            // the window no longer has, and the rows it left on screen are the
            // renderer's to take back before the new ones go down.
            Pressed::Resized => {
                renderer.resized()?;
                draw(renderer, &editor, style, mode)?;
            }

            Pressed::Ignored => {}

            Pressed::Key(key) => match editor.press(key) {
                // A key that moved nothing costs no frame. An arrow held down
                // against the end of a line is what that saves.
                Typed::Ignored => {}
                Typed::Changed => draw(renderer, &editor, style, mode)?,
                Typed::Submitted => return said(renderer, &mut editor, style),
                Typed::Ended => {
                    renderer.settle()?;
                    return Ok(Asked::Ended);
                }
            },
        }
    }
}

/// Puts the box on screen with the cursor where the line was typed to.
fn draw<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    style: Style,
    mode: &str,
) -> Result<(), Fatal> {
    let columns = renderer.columns();

    let prompt = Prompt {
        said: editor.text(),
        column: editor.column(),
        mode,
        tone: Slot::Accent,
        hint: "",
    };

    let rows = prompt.rows(columns, style.glyphs());
    renderer.live(&rows, prompt.caret(columns), style.palette())?;
    Ok(())
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
