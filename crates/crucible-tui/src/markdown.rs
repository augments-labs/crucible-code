//! Markdown, read as an answer streams.
//!
//! A model answers in markdown whether or not anything asked it to, so the
//! choice is between showing the markers and reading them. This reads them: a
//! marker is recognised, dropped, and the run it covered is handed on wearing a
//! [`Slot`] instead. What it does *not* do is turn the model's text into escape
//! sequences — the run and its slot arrive separately and the slot is applied as
//! the row is drawn, which is what keeps the rule that no byte from a model is
//! ever written through as an instruction.
//!
//! The scan is one character at a time and its state outlives a delta, because a
//! delta is a piece of the wire rather than a piece of the answer: a marker
//! arrives split across two of them as often as not. Nothing looks ahead past
//! the single character that ends a run of markers, and nothing is buffered
//! waiting for a closing marker that may never come — an answer that stopped
//! mid-sentence has already been drawn up to where it stopped.

use crate::color::Slot;

/// The longest run of one marker character read as a marker.
///
/// Six because that is the deepest heading. Past it the run is capped rather
/// than rejected, so `#######` is a heading — the alternative is spending a
/// counter on a line nobody writes.
const RUN: usize = 6;

/// What a run is written back as, where it turned out not to be a marker.
///
/// Static, so putting one back allocates nothing and does not borrow the delta
/// it arrived in — which matters, since a run can start in one delta and be
/// settled by the next.
const HASHES: &str = "######";
const STARS: &str = "******";
const SCORES: &str = "______";

/// A run of one marker character, waiting for the character that says what it
/// was.
#[derive(Debug, Clone, Copy)]
struct Held {
    mark: char,
    count: usize,
}

/// What is open on the line being read.
///
/// Both end where the line does. Markdown lets emphasis cross a line break and
/// this does not: an unclosed marker then costs one line rather than the rest of
/// the answer, and a model that opened one by accident is the case that actually
/// happens.
#[derive(Debug, Default, Clone, Copy)]
struct Line {
    heading: bool,
    code: bool,
    strong: bool,
}

/// Reads markdown out of a stream of deltas.
#[derive(Debug, Default)]
pub struct Markdown {
    /// A run of markers whose meaning the next character decides.
    held: Option<Held>,
    /// Something other than a space has arrived on this line.
    ///
    /// A heading is only one at the start of a line; anywhere else it is a hash.
    started: bool,
    /// The character before the run being held, for the one rule that needs it.
    previous: char,
    line: Line,
}

impl Markdown {
    /// Reads `delta`, handing each run of text to `say` under its slot.
    ///
    /// Runs rather than characters: the text between two markers is one call,
    /// so a delta with no markers in it is one call for the whole delta.
    /// Markers themselves are never handed on.
    pub fn read(&mut self, delta: &str, say: &mut dyn FnMut(Slot, &str)) {
        // Where the text not yet handed on begins.
        let mut run = 0;

        for (at, character) in delta.char_indices() {
            let next = at.saturating_add(character.len_utf8());

            match self.held {
                // More of the run being held. Not text, and not decided yet.
                Some(held) if held.mark == character => {
                    self.held = Some(Held {
                        count: held.count.saturating_add(1).min(RUN),
                        ..held
                    });
                    run = next;
                    continue;
                }
                // The run is over, and this character says what it was.
                Some(held) => {
                    if self.settle(held, character, say) {
                        run = next;
                        continue;
                    }
                    run = at;
                }
                None => {}
            }

            if character == '\n' {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.end_line(say);
                run = next;
            } else if self.marks(character) {
                self.say(delta.get(run..at).unwrap_or_default(), say);
                self.held = Some(Held {
                    mark: character,
                    count: 1,
                });
                run = next;
            } else {
                self.started |= character != ' ';
                self.previous = character;
            }
        }

        self.say(delta.get(run..).unwrap_or_default(), say);
    }

    /// Whether `character` starts a run of markers where the scan stands.
    fn marks(&self, character: char) -> bool {
        match character {
            '*' | '_' | '`' => true,
            '#' => !self.started,
            _ => false,
        }
    }

    /// Decides what a finished run of markers was, given the character that
    /// ended it. Answers whether that character was part of the marker.
    fn settle(&mut self, held: Held, next: char, say: &mut dyn FnMut(Slot, &str)) -> bool {
        self.held = None;
        self.started = true;

        match held.mark {
            // The hashes and the one space after them are the marker, and what
            // is left of the line is the heading itself.
            '#' if next == ' ' => {
                self.line.heading = true;
                true
            }
            '`' => {
                self.line.code = !self.line.code;
                false
            }
            '*' | '_' if self.emphasises(held.mark, next) => {
                self.line.strong = !self.line.strong;
                false
            }
            _ => {
                self.previous = held.mark;
                self.say(written(held), say);
                false
            }
        }
    }

    /// Whether a run of `mark` followed by `next` turns emphasis on or off.
    fn emphasises(&self, mark: char, next: char) -> bool {
        if mark == '_' {
            return if self.line.strong {
                // Closes at the end of a word rather than inside one, so
                // `_borrowed_ from` closes and `_borrowed_from` does not.
                !next.is_alphanumeric()
            } else {
                // Opens only where a word is not already under way:
                // `read_to_string` is a function far more often than it is a
                // sentence with emphasis in the middle of it.
                !self.previous.is_alphanumeric() && !next.is_whitespace()
            };
        }

        // Something has to follow on the same line for a run to open. That is
        // what leaves `* item` a bullet instead of turning the rest of the
        // paragraph bold.
        self.line.strong || !next.is_whitespace()
    }

    /// Ends the line, and with it everything markdown ends at one.
    fn end_line(&mut self, say: &mut dyn FnMut(Slot, &str)) {
        *self = Self::default();

        // After the reset, so the row that ends carries no slot into the row
        // that follows it. An unclosed marker costs its own line and no more.
        say(self.slot(), "\n");
    }

    /// Hands on a run of text, if there is any.
    fn say(&self, text: &str, say: &mut dyn FnMut(Slot, &str)) {
        if !text.is_empty() {
            say(self.slot(), text);
        }
    }

    /// The slot everything read right now is written under.
    fn slot(&self) -> Slot {
        if self.line.code {
            Slot::Quiet
        } else if self.line.heading || self.line.strong {
            Slot::Strong
        } else {
            Slot::Plain
        }
    }
}

/// The text a run is put back as, where it meant nothing.
fn written(held: Held) -> &'static str {
    let whole = match held.mark {
        '#' => HASHES,
        '*' => STARS,
        _ => SCORES,
    };

    whole.get(..held.count).unwrap_or(whole)
}

#[cfg(test)]
mod tests;
