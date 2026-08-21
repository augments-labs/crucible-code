//! Escape sequences, recognised as the one thing a terminal reads them as.
//!
//! A terminal reads `ESC` and what follows it as a single instruction. Anything
//! walking characters one at a time sees the parameters instead: it draws `[2J`
//! and counts three columns for it. Both halves of that are wrong, and the
//! second half is the one that does damage — the count is what decides whether
//! a row fits its band, so a row measured three columns too wide wraps on its
//! own and pushes everything below it a row down the screen.
//!
//! What arrives here is text this process did not write: streamed model output
//! and tool results. So a sequence is recognised in order to be *dropped* —
//! left undrawn and uncounted — because an instruction from there could move
//! the cursor into a band it was not aimed at, or leave an attribute set for
//! everything after it. Colour crucible writes for itself never travels as bytes inside a
//! string; it belongs to a row and is applied as the row is drawn.

/// The byte that opens a sequence.
const ESCAPE: char = '\x1b';

/// What ends a command string, in the older of its two spellings.
const BELL: char = '\x07';

/// Where a stream of characters sits inside an escape sequence.
///
/// Held across calls because a sequence is not delivered whole: streamed output
/// is split wherever the wire split it, and a run cut between two deltas has to
/// go on being a run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Escapes {
    /// Outside a sequence. What arrives is text.
    #[default]
    Text,
    /// `ESC` has arrived, and the character after it says what it opened.
    Opened,
    /// Inside `ESC [ …`: parameter bytes, then one byte that ends it and says
    /// what it was.
    Control,
    /// Inside `ESC ] …`: a string, ended by [`BELL`] or by `ESC \`.
    Command,
}

impl Escapes {
    /// Whether `character` belongs to a sequence rather than to the text, and
    /// so is neither drawn nor counted.
    pub(crate) fn holds(&mut self, character: char) -> bool {
        let held = self.step(character);

        // No answer means the sequence was abandoned rather than finished: the
        // byte that broke it is one a terminal draws, so it is text here too
        // and the caller counts it.
        *self = held.unwrap_or(Self::Text);
        held.is_some()
    }

    /// Where `character` leaves the machine, or `None` if it is text.
    fn step(self, character: char) -> Option<Self> {
        // From text this opens a sequence, and inside an unfinished one it
        // abandons that and opens another — which is what a terminal does with
        // the pair. It is also how `ESC \` ends a command string: the escape
        // reopens, and the backslash finishes a two-character sequence.
        if character == ESCAPE {
            return Some(Self::Opened);
        }

        match self {
            Self::Text => None,
            Self::Opened => Self::opened(character),
            Self::Control => Self::control(character),
            Self::Command => Self::command(character),
        }
    }

    /// What the `ESC` opened, which is said by the character after it.
    fn opened(character: char) -> Option<Self> {
        match character {
            '[' => Some(Self::Control),
            ']' => Some(Self::Command),
            // Everything else drawable is a sequence two characters long,
            // finished by the one that just arrived.
            '\u{20}'..='\u{7e}' => Some(Self::Text),
            _ => None,
        }
    }

    /// Inside `ESC [ …`, where parameters run until one byte ends the run.
    fn control(character: char) -> Option<Self> {
        match character {
            // Parameters, and the intermediate bytes that may follow them.
            '\u{20}'..='\u{3f}' => Some(Self::Control),
            // The byte that ends the sequence, and says what it was.
            '\u{40}'..='\u{7e}' => Some(Self::Text),
            _ => None,
        }
    }

    /// Inside `ESC ] …`, where a string runs until a terminator.
    ///
    /// Only [`BELL`] is matched here: the other spelling is `ESC \`, and the
    /// escape at the top of [`Self::step`] has already taken it.
    fn command(character: char) -> Option<Self> {
        match character {
            BELL => Some(Self::Text),
            // A control character is not string content. Ending the string here
            // is what stops one sequence nobody terminated from swallowing
            // everything a tool printed after it.
            _ if !character.is_control() => Some(Self::Command),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What survives `text` — the characters that are text rather than
    /// instruction, in order.
    fn kept(text: &str) -> String {
        let mut escapes = Escapes::default();

        text.chars()
            .filter(|character| !escapes.holds(*character))
            .collect()
    }

    /// The same, but fed in two pieces, so a sequence is split where a delta
    /// would split it.
    fn kept_split(text: &str, at: usize) -> String {
        let mut escapes = Escapes::default();
        let (head, rest) = text.split_at(at);

        [head, rest]
            .into_iter()
            .flat_map(str::chars)
            .filter(|character| !escapes.holds(*character))
            .collect()
    }

    #[test]
    fn a_control_sequence_goes_whole_rather_than_by_its_parameters() {
        // The bug this module exists for: walking characters keeps `[2J`, which
        // is three columns of rubbish on a row measured three columns short.
        assert_eq!(kept("a\x1b[2Jb"), "ab");
        assert_eq!(kept("\x1b[38;5;214mwarn\x1b[0m"), "warn");
    }

    #[test]
    fn a_sequence_split_between_two_deltas_is_still_one_sequence() {
        // Streamed output is cut wherever the wire cut it, and every one of
        // these cuts falls inside the run.
        for at in 1..="a\x1b[31mb".len() {
            assert_eq!(kept_split("a\x1b[31mb", at), "ab", "split at {at}");
        }
    }

    #[test]
    fn a_command_string_ends_at_either_of_its_two_terminators() {
        assert_eq!(kept("\x1b]0;a title\x07after"), "after");
        assert_eq!(kept("\x1b]0;a title\x1b\\after"), "after");
    }

    #[test]
    fn a_command_string_nobody_terminated_ends_at_the_line() {
        // Otherwise one stray sequence in a tool's output takes everything
        // printed after it with it.
        assert_eq!(kept("\x1b]0;a title\nafter"), "\nafter");
    }

    #[test]
    fn a_sequence_a_byte_abandons_gives_that_byte_back() {
        // The terminal draws the byte that broke the sequence, so it is counted
        // here rather than swallowed.
        assert_eq!(kept("\x1b[3\u{7f}x"), "\u{7f}x");
    }

    #[test]
    fn a_two_character_sequence_takes_exactly_two_characters() {
        // `ESC M` scrolls the screen down; the `M` is the sequence, not text.
        assert_eq!(kept("a\x1bMb"), "ab");
    }

    #[test]
    fn an_escape_inside_an_unfinished_sequence_opens_a_new_one() {
        assert_eq!(kept("a\x1b\x1b[2Jb"), "ab");
    }

    #[test]
    fn text_with_no_escape_in_it_is_left_alone() {
        assert_eq!(
            kept("plain text, 日本語, e\u{301}"),
            "plain text, 日本語, e\u{301}"
        );
    }
}
