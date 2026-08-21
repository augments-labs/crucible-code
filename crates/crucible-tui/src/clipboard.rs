//! Asking the terminal to put text on the clipboard.
//!
//! The clipboard a reader means is the one beside the screen they are looking
//! at, which over a link is not the machine this process runs on. So the
//! request goes to the terminal rather than to this host: `OSC 52` is the
//! sequence a terminal reads as *put this on the clipboard*, and it travels
//! the same wire the drawing does — which is what makes a copy taken over ssh
//! land where the reader is rather than where the agent is.
//!
//! **Nothing here reads the clipboard back.** The other direction of the same
//! sequence asks the terminal to hand its contents over, and a terminal that
//! answered it would let anything printing to the screen read what somebody
//! copied. Terminals disable that half by default and this asks for it nowhere.
//!
//! A terminal that does not implement the sequence drops it, and the ceiling
//! below is why one that does cannot be handed something it will not take.

use std::fmt::Write as _;

use base64::Engine as _;

/// How much text one request may carry, in bytes before encoding.
///
/// A terminal reads a command string into a buffer of its own, and the ones
/// that bound it bound it around this — so a longer request is not a longer
/// copy, it is a truncated one, or a stream of parameter bytes drawn on the
/// screen. What is refused here is refused whole: half a prompt on somebody's
/// clipboard is worse than none, because nothing says which half.
const CEILING: usize = 64 * 1024;

/// The sequence that puts `text` on the reader's clipboard, or nothing.
///
/// `None` for the empty string — a copy of nothing would clear what is already
/// there, which is not what any key was pressed for — and for text past
/// [`CEILING`].
#[must_use]
pub(crate) fn copying(text: &str) -> Option<String> {
    if text.is_empty() || text.len() > CEILING {
        return None;
    }

    // `c` is the selection every terminal understands as the clipboard proper,
    // as against the primary selection X11 fills from a drag. A reader who
    // pressed a key meant the one they paste from.
    let mut said = String::from("\x1b]52;c;");
    base64::engine::general_purpose::STANDARD.encode_string(text, &mut said);

    // The older terminator, for the reason the background query uses it: it is
    // the one every terminal that implements the sequence at all agrees on.
    said.write_char('\x07').ok()?;
    Some(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_encoded_into_the_sequence_the_terminal_reads() {
        assert_eq!(copying("hi"), Some("\x1b]52;c;aGk=\x07".to_owned()));
    }

    #[test]
    fn a_copy_of_nothing_is_not_a_request_to_empty_the_clipboard() {
        assert_eq!(copying(""), None);
    }

    #[test]
    fn more_than_a_terminal_will_take_is_refused_whole() {
        // Rather than truncated: a prompt cut at a buffer boundary pastes as
        // something the reader never wrote and nothing on screen says so.
        assert_eq!(copying(&"x".repeat(CEILING + 1)), None);
        assert!(copying(&"x".repeat(CEILING)).is_some());
    }
}
