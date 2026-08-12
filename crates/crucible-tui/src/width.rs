//! Display width, counted in one place so that everything counts it the same.
//!
//! A terminal wraps by columns rather than by characters, and the count that
//! decides where it wraps is the one the live tail uses. Anything measuring a
//! string against the Unicode tables directly would disagree with the screen
//! somewhere else rather than agree with it, so this is the only module that
//! names `unicode-width`, the tail asks it what a character costs, and [`cut`]
//! is how a caller outside this crate asks the same question the tail asks.
//!
//! What is *not* text is [`crate::escape`]'s to recognise. Width and escapes are
//! two questions and this module answers only the first, but every walk over a
//! string has to ask both — a sequence is instruction, so it takes no columns
//! and reaches no screen.

use unicode_width::UnicodeWidthChar;

use crate::escape::Escapes;

/// How far a tab advances, matching what terminals do.
const TAB_STOP: usize = 8;

/// The selector that asks for the emoji rendering of the character before it.
pub(crate) const EMOJI_PRESENTATION: char = '\u{FE0F}';

/// The columns `character` moves the cursor along by.
///
/// `None` for one that is not drawn at all — a control character is dropped
/// rather than counted, because a stray escape byte from a tool would move a
/// cursor this process believes it is tracking.
pub(crate) fn advance(character: char) -> Option<usize> {
    character.width()
}

/// Whether [`EMOJI_PRESENTATION`] after `base` makes the pair two columns.
///
/// The selector takes no column of its own, so counting one character at a
/// time misses this and leaves the row a column short — which is a row the
/// terminal wraps itself, leaving the cursor a row below where the next frame
/// expects it. Only a base of exactly one column widens: after a wide
/// character, a combining mark or nothing at all, the selector asks for
/// nothing.
pub(crate) fn widens(base: Option<char>) -> bool {
    base.and_then(advance) == Some(1)
}

/// The column a tab moves to from `column`.
pub(crate) fn tab_stop(column: usize) -> usize {
    (column / TAB_STOP + 1) * TAB_STOP
}

/// Where to cut `text` so that what is kept is one row of at most `columns`
/// display columns, counted the way the live tail counts them.
///
/// `None` when the whole of `text` is already such a row, which is the case
/// worth not allocating for. A caller that puts something in the space it
/// saves — an ellipsis, a count — asks for the columns it will not use.
///
/// A byte offset rather than a string, so that what to do with either side of
/// it stays the caller's. The offset never falls inside a character, and never
/// between a character and the selector that widens it: parted from its base a
/// selector stops asking for anything, and the base would then draw narrow
/// where a column had already been counted for it. It never falls inside an
/// escape sequence either — a sequence costs nothing, so the walk is never
/// stopped part way through one. A newline is a cut too, because what is kept
/// is one row.
#[must_use]
pub fn cut(text: &str, columns: usize) -> Option<usize> {
    walk(text, columns).1
}

/// How many display columns one row of `text` costs.
///
/// The same walk as [`cut`] rather than a second count of the same string: a
/// component that pads a row out to a width and the tail that then measures the
/// result have to agree about how wide it already was, and two walks are two
/// answers waiting to differ.
///
/// One row, so a newline ends the count as it ends a row. Escape sequences cost
/// nothing, which is what lets a row be measured after the palette has coloured
/// it as well as before.
#[must_use]
pub fn columns(text: &str) -> usize {
    walk(text, usize::MAX).0
}

/// The one walk both questions are answered from: how many columns were
/// counted, and where the row left `ceiling` behind if it did.
///
/// The count is the whole row's only when nothing was cut, which is the only
/// case that asks for it — a caller passing a real ceiling wants the offset.
fn walk(text: &str, ceiling: usize) -> (usize, Option<usize>) {
    let mut column = 0;
    // The last character counted and where it starts, so a selector that will
    // not fit can take its base down with it.
    let mut last: Option<(usize, char)> = None;
    // One string is one walk, so the machine starts fresh and is dropped with
    // it. The tail keeps its own across deltas; this has no across.
    let mut escapes = Escapes::default();

    for (offset, character) in text.char_indices() {
        if escapes.holds(character) {
            continue;
        }

        let base = last.map(|(_, character)| character);
        let step = match character {
            '\n' => return (column, Some(offset)),
            '\t' => tab_stop(column).saturating_sub(column),
            EMOJI_PRESENTATION if widens(base) => 1,
            _ => match advance(character) {
                Some(step) => step,
                None => continue,
            },
        };

        if column + step > ceiling {
            let at = match (character, last) {
                (EMOJI_PRESENTATION, Some((at, _))) => at,
                _ => offset,
            };
            return (column, Some(at));
        }

        column += step;
        last = Some((offset, character));
    }

    (column, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One column as text, two once the selector follows it. Spelled out
    /// because a selector is invisible in a source file.
    const WARNING: &str = "\u{26A0}\u{FE0F}";

    #[test]
    fn text_that_already_fits_is_not_cut() {
        assert_eq!(cut("hello", 20), None);
        assert_eq!(cut("hello", 5), None, "exactly the columns asked for");
    }

    #[test]
    fn a_cut_keeps_the_columns_asked_for_and_no_more() {
        assert_eq!(cut("hello", 4), Some(4));
    }

    #[test]
    fn a_wide_character_costs_two_columns() {
        // Counting characters would keep three of these where two fit.
        assert_eq!(cut("日本語", 5), Some(6));
    }

    #[test]
    fn a_combining_mark_costs_none() {
        // "e" plus a combining acute is two characters and one column, so the
        // cut must not fall between them.
        assert_eq!(cut("e\u{301}x", 2), None);
    }

    #[test]
    fn a_selector_is_never_cut_from_its_base() {
        // The pair is two columns and one glyph. Cutting between them keeps a
        // base that draws narrow where a column was counted for it.
        assert_eq!(cut(&WARNING.repeat(2), 3), Some(WARNING.len()));
    }

    #[test]
    fn a_selector_that_fits_takes_the_column_it_asks_for() {
        // Summed per character these are two columns and would both be kept.
        assert_eq!(cut(&WARNING.repeat(2), 4), None);
        assert_eq!(cut(&WARNING.repeat(3), 4), Some(WARNING.len() * 2));
    }

    #[test]
    fn a_tab_is_cut_when_its_stop_is_past_the_edge() {
        // The tail drops a tab whose stop it cannot reach and starts a row, so
        // a cut lands in the same place.
        assert_eq!(cut("ab\tcd", 4), Some(2));
        assert_eq!(cut("ab\tcd", 10), None);
    }

    #[test]
    fn a_control_character_is_not_counted() {
        // Dropped by the tail rather than drawn, so it costs no column here
        // either -- and it is not a base a selector could widen.
        assert_eq!(cut("a\u{7f}\u{7f}bc", 3), None);
    }

    #[test]
    fn an_escape_sequence_costs_no_columns_at_all() {
        // Counting its parameters is what put `[2J` on screen and made the row
        // three columns wider than the terminal ever drew.
        assert_eq!(cut("\x1b[31mred\x1b[0m", 3), None);
        assert_eq!(
            cut("\x1b[31mred\x1b[0m!", 3),
            Some("\x1b[31mred\x1b[0m".len())
        );
    }

    #[test]
    fn a_newline_is_a_cut_because_what_is_kept_is_one_row() {
        assert_eq!(cut("one\ntwo", 40), Some(3));
    }

    #[test]
    fn what_a_row_costs_is_what_a_cut_at_that_width_would_leave_uncut() {
        // The two answers come from one walk, and this is the property that
        // says so: a row padded to its own measured width is a row the tail
        // will not wrap. Anything that costs more than it counted is a row the
        // terminal breaks somewhere this process did not predict.
        for text in [
            "hello",
            "日本語",
            "e\u{301}x",
            &WARNING.repeat(3),
            "ab\tcd",
            "a\u{7f}bc",
            "\x1b[31mred\x1b[0m",
            "",
        ] {
            let wide = columns(text);
            assert_eq!(cut(text, wide), None, "{text:?} is wider than {wide}");
            assert!(
                wide == 0 || cut(text, wide - 1).is_some(),
                "{text:?} fits in {} columns, so {wide} was an overcount",
                wide - 1
            );
        }
    }

    #[test]
    fn a_row_stops_being_counted_where_it_stops_being_a_row() {
        assert_eq!(columns("one\ntwo"), 3);
    }
}
