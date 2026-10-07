//! Text a checkout or a vendor chose, written where a terminal will read it
//! without the renderer between them.
//!
//! A terminal acts on a control character rather than drawing it: ESC and the
//! C1 controls, U+009B among them as ESC `[`, can retitle the window or clear
//! the screen, and a line break starts a line the text was never given, as
//! the line and paragraph separators do in some terminals and viewers. A
//! Unicode format character, a right-to-left override among them, reorders or
//! hides what is drawn around it. So text that is quoted rather than written
//! by crucible leaves with each of these as an escape a person can read, the
//! two joiners that only shape a word aside. A JSON document leaves with every
//! one as a JSON escape, `\n` for a line break and `\u202e` for an override,
//! which reads back as the same character, so nothing a reader decodes
//! changes.
//!
//! Owned here because the two writers of such text, a configuration report and
//! a client-contract document, may not name each other.

#[cfg(test)]
mod tests;

use std::io;

use serde_json::ser::{CompactFormatter, Formatter};

/// Whether `character` is a control character; a Unicode format character
/// (general category `Cf`), the bidi marks, embeddings, overrides and
/// isolates, the zero-width characters and the byte order mark among them,
/// any of which reorders or hides the text around it when drawn; or the line
/// or paragraph separator, which some terminals and viewers end a line at.
/// U+2065, unassigned between the invisible operators and the isolates, is
/// taken with them.
///
/// The terminal drops the same format characters from everything it draws,
/// except the zero-width non-joiner and joiner, which join characters on
/// screen, and it draws the two separators, a column each. Neither crate may
/// name the other, so a test in the command line holds the two lists to each
/// other.
#[must_use]
pub const fn unshown(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{890}'..='\u{891}'
                | '\u{8e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

/// `text` with every [`unshown`] character but the two joiners below, a line
/// break among them, written as its escape: `\u{1b}` for ESC, `\n` for a line
/// break, `\u{2028}` for a line separator, `\u{202e}` for a right-to-left
/// override. Escaped, it is still the name, and the person who sees it can
/// tell which directory or key it was.
///
/// The zero-width non-joiner and joiner, U+200C and U+200D, are kept, as the
/// terminal keeps them: they shape Persian and Indic script and join an emoji
/// sequence, and drawn they move and hide nothing, so escaping them would only
/// break the word they stand in.
#[must_use]
pub fn escaped(text: &str) -> String {
    text.chars()
        .fold(String::with_capacity(text.len()), |mut shown, character| {
            if character.is_control() {
                shown.extend(character.escape_debug());
            } else if unshown(character) && !JOINERS.contains(&character) {
                shown.extend(character.escape_unicode());
            } else {
                shown.push(character);
            }
            shown
        })
}

/// The zero-width non-joiner and joiner, which [`escaped`] keeps.
const JOINERS: [char; 2] = ['\u{200c}', '\u{200d}'];

/// Compact JSON whose strings carry every [`unshown`] character as a `\u`
/// escape: DEL, the C1 controls, the format characters and the line and
/// paragraph separators as well as the C0 controls `serde_json` escapes
/// already. One beyond the Basic Multilingual Plane is written as its two
/// UTF-16 halves, as the JSON grammar spells it.
#[derive(Debug, Clone, Copy)]
pub struct Escaping;

impl Formatter for Escaping {
    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut rest = fragment;
        while let Some((at, hidden)) = rest
            .char_indices()
            .find(|&(_, character)| unshown(character))
        {
            let (before, after) = rest.split_at(at);
            CompactFormatter.write_string_fragment(writer, before)?;
            for half in hidden.encode_utf16(&mut [0; 2]) {
                write!(writer, "\\u{half:04x}")?;
            }
            rest = after.get(hidden.len_utf8()..).unwrap_or_default();
        }
        CompactFormatter.write_string_fragment(writer, rest)
    }
}
