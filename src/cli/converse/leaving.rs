//! Standing the list of commands that outlived their calls.
//!
//! The other half of the count on the row under the box. That row says how many
//! there are; this is where they are named, how long each has been running, and
//! what a key does about one.
//!
//! It stands where the box was and hands the room straight back, for the reason
//! nothing else standing here is written down: a command still running is a thing
//! that is happening, and the record is what has happened. The list is drawn from
//! a copy of the facts rather than from the registry itself, because the registry
//! is behind a lock that a thread drawing a frame must not be holding.
//!
//! Two keys act on a command and one leaves. `enter` stands what it has printed,
//! in the view a finished result is stood in — the same reader asking the same
//! question one layer further out. `x` ends it, with no confirmation: the command
//! was started by a call somebody allowed, and stopping it is the reason this is
//! reachable at all. `esc`, and the key that opened it, close it.

use crucible_tools::{Background, Standing};
use crucible_tui::{
    Command, Expanded, Glyphs, Key, Pressed, Renderer, Row, Running, Shown, Terminal,
};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::region::{self, Ended, Moved};

/// What the list is showing, between one key and the next.
#[derive(Debug, Default)]
pub(super) struct Leaving {
    /// Which row a key acts on.
    at: usize,
    /// The command whose output is standing over the list, where one is.
    ///
    /// A number rather than the text, because the text is read again for every
    /// frame it is drawn in: a command being watched goes on printing, and a copy
    /// taken when the key was pressed would be a picture that stopped moving the
    /// moment somebody asked to look at it.
    shown: Option<usize>,
    /// How far down that output the window has been scrolled.
    from: usize,
    /// How far down it may be scrolled, which only the layout knows.
    end: usize,
}

impl Leaving {
    /// Stands the list, and answers with whether it ended by taking a row.
    ///
    /// # Errors
    ///
    /// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
    pub(super) fn stand<T: Terminal>(
        &mut self,
        renderer: &mut Renderer<T>,
        style: Style,
        left: &Background,
    ) -> Result<Ended, Fatal> {
        // Taken once per frame rather than once per press: a command ending while
        // the list is open is a row that has to go, and the list is the one place
        // a reader is looking at it.
        region::stand(
            renderer,
            style,
            self,
            |leaving, columns, rows| leaving.rows(left, columns, rows, style.glyphs()),
            |arrived, leaving| leaving.against(arrived, left),
        )
    }

    /// The rows for this frame, at this size.
    fn rows(&mut self, left: &Background, columns: usize, rows: usize, glyphs: Glyphs) -> Vec<Row> {
        let running = left.running();

        // A command that ended while this was open takes its row with it, and the
        // mark comes back inside the list rather than pointing past the end of it.
        self.at = self.at.min(running.len().saturating_sub(1));

        match self.shown {
            Some(_) => self.watching(left, columns, rows, glyphs),
            None => listed(&running, self.at, columns, rows, glyphs),
        }
    }

    /// One command's output, stood whole.
    fn watching(
        &mut self,
        left: &Background,
        columns: usize,
        rows: usize,
        glyphs: Glyphs,
    ) -> Vec<Row> {
        // Read off the state rather than passed in beside it: the caller has just
        // matched on the same field, and a number carried alongside could be a
        // different one from the number being shown.
        let Some(text) = self.shown.and_then(|number| left.wrote(number)) else {
            // It ended while it was being read. Back to the list, which is where
            // the reader would have gone next anyway.
            self.shown = None;
            return self.rows(left, columns, rows, glyphs);
        };

        let called = left
            .running()
            .into_iter()
            .find(|standing| Some(standing.number) == self.shown)
            .map_or_else(String::new, |standing| standing.called.to_string());

        let shown = [Shown {
            called: &called,
            text: &text,
        }];
        let expanded = Expanded {
            shown: &shown,
            from: self.from,
        };

        // Written down here because only the layout knows it: how far the window
        // may open depends on how many rows the output came to at this width.
        self.end = expanded.end(columns, rows);
        self.from = self.from.min(self.end);

        expanded.within(columns, rows, glyphs)
    }

    /// What one key does to it.
    ///
    /// Every key is named rather than caught by a rest arm, for the reason every
    /// other standing component names its own: a key arriving at something on
    /// screen is either something it does or something it has decided to ignore,
    /// and a key added later should have to be told which.
    fn against(&mut self, arrived: Pressed, left: &Background) -> Moved {
        // The output of one command is standing. Its keys are that view's.
        if self.shown.is_some() {
            return match arrived {
                Pressed::Up => {
                    let back = self.from.checked_sub(1);
                    region::step(&mut self.from, back)
                }
                Pressed::Down => {
                    let on = (self.from < self.end).then(|| self.from + 1);
                    region::step(&mut self.from, on)
                }
                // Back to the list rather than out of it: this was opened from
                // there, and the way back is where the reader came from.
                Pressed::Escape | Pressed::Background | Pressed::Key(Key::Enter) => {
                    self.shown = None;
                    self.from = 0;
                    Moved::Redraw
                }
                Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
                Pressed::Resized => Moved::Redraw,
                Pressed::Key(_)
                | Pressed::Cycle
                | Pressed::Explain
                | Pressed::Expand
                | Pressed::Plan
                | Pressed::Clicked { .. }
                | Pressed::Ignored => Moved::Still,
            };
        }

        let running = left.running();

        match arrived {
            Pressed::Up => {
                let back = self.at.checked_sub(1);
                region::step(&mut self.at, back)
            }
            Pressed::Down => {
                let on = (self.at + 1 < running.len()).then(|| self.at + 1);
                region::step(&mut self.at, on)
            }

            // Shows what it has printed. Nothing is taken and the list is still
            // behind it, so this is a step further in rather than the way out.
            Pressed::Key(Key::Enter) => match running.get(self.at) {
                Some(standing) => {
                    self.shown = Some(standing.number);
                    self.from = 0;
                    Moved::Redraw
                }
                None => Moved::Still,
            },

            // Ends it. No confirmation: the command was started by a call
            // somebody allowed, and this is the only key that can end one.
            Pressed::Key(Key::Char('x')) => match running.get(self.at) {
                Some(standing) => {
                    left.stop(standing.number);

                    // The last one going takes the list with it — there is nothing
                    // left to stand, and a frame of empty chrome is worse than the
                    // row under the box that opened this.
                    if running.len() <= 1 {
                        Moved::Left
                    } else {
                        Moved::Redraw
                    }
                }
                None => Moved::Left,
            },

            // The key that opened it closes it, which is what every other
            // `ctrl+` key here does.
            Pressed::Escape | Pressed::Background | Pressed::Key(Key::Interrupt | Key::Eof) => {
                Moved::Left
            }
            Pressed::Resized => Moved::Redraw,
            Pressed::Key(_)
            | Pressed::Cycle
            | Pressed::Explain
            | Pressed::Expand
            | Pressed::Plan
            | Pressed::Clicked { .. }
            | Pressed::Ignored => Moved::Still,
        }
    }
}

/// The list itself, laid out for this frame.
fn listed(
    running: &[Standing],
    at: usize,
    columns: usize,
    rows: usize,
    glyphs: Glyphs,
) -> Vec<Row> {
    let shown: Vec<Command<'_>> = running
        .iter()
        .map(|standing| Command {
            number: standing.number,
            called: &standing.called,
            running: standing.running,
            lines: standing.lines,
            bytes: standing.bytes,
        })
        .collect();

    Running { shown: &shown, at }.rows(columns, rows, glyphs)
}

#[cfg(test)]
mod tests;
