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

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

use crate::escape::Escapes;

/// How far a tab advances, matching what terminals do.
const TAB_STOP: usize = 8;

/// The selector that asks for the emoji rendering of the character before it.
pub(crate) const EMOJI_PRESENTATION: char = '\u{FE0F}';

/// `said` with everything a terminal would read as an instruction dropped.
///
/// What arrives from a model or a tool is text this process did not write, and a
/// terminal reads an escape as an instruction rather than as characters. Walking
/// one at a time measures *around* a sequence, which is right for the arithmetic
/// and wrong for the screen: the bytes are still in the slice, and a terminal
/// sent them would move a cursor this process believes it is tracking, or leave
/// an attribute set for every row after it.
///
/// So this drops what may not be drawn — every character that costs no column,
/// which is a sequence's parameters, the escape that opened it, and any control
/// byte that arrived on its own. Nothing here changes a width, because nothing
/// dropped was ever counted.
///
/// Colour crucible writes for itself never travels as bytes inside a string; it
/// belongs to a [`crate::Row`] and is applied as the row is drawn. So there is
/// no case where dropping these loses something this program meant.
pub(crate) fn spoken(said: &str) -> String {
    let mut escapes = Escapes::default();

    said.chars()
        .filter(|character| !escapes.holds(*character) && advance(*character).is_some())
        .collect()
}

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

/// How far `character` moves the cursor along from `column`, after `base`.
///
/// The three answers a bare [`advance`] gets wrong, in the one place every walk
/// over a string asks for them: a tab lands on the next stop rather than moving
/// one column, a selector takes the column it widened its base by although it
/// asks for none itself, and anything a terminal does not draw moves nothing.
///
/// `None` is that last case, and it is not zero: a character that is dropped is
/// a character the caller neither counts nor keeps, and one that costs no
/// columns and is still drawn — a combining mark — is a `Some(0)` that has to
/// stay with the character it marks.
pub(crate) fn along(column: usize, character: char, base: Option<char>) -> Option<usize> {
    match character {
        '\t' => Some(tab_stop(column).saturating_sub(column)),
        EMOJI_PRESENTATION if widens(base) => Some(1),
        _ => advance(character),
    }
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

/// `text` with at most `columns` display columns of it kept.
///
/// [`cut`] with the offset already applied, for a caller that wants the text
/// rather than the place it stops. Every component in this crate that lays a
/// row out ends up wanting exactly this, and so does anything outside it
/// composing rows of its own — one row is one row wherever it was built.
#[must_use]
pub fn clip(text: &str, columns: usize) -> &str {
    match cut(text, columns) {
        Some(at) => text.get(..at).unwrap_or_default(),
        None => text,
    }
}

/// Where in `text` the rows of [`fold`] are.
///
/// The same walk, answering in offsets rather than in slices, because a row of
/// spans has to cut each span at the break and a `&str` cannot say where it was
/// taken from. Whitespace at a break is dropped as [`fold`] drops it, so these
/// ranges are the rows and not a partition of `text`.
pub(crate) fn folds(text: &str, columns: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    if columns == 0 {
        return rows;
    }

    let mut rest = text.trim();
    let mut base = text.len() - text.trim_start().len();

    while !rest.is_empty() {
        let Some(over) = cut(rest, columns) else {
            rows.push(base..base + rest.len());
            break;
        };

        // What stopped the row decides where it breaks. A space there means the
        // row filled to the column on a whole word and what came next was the
        // gap after it, so the row is already whole words — looking further
        // back for a space would move a word that fitted down to the next row
        // and leave a column of every such row empty.
        //
        // Otherwise the cut fell inside a word, so the last space before it is
        // the break. Failing that the row is one long word and the cut stands.
        // Never zero: a character wider than the whole row would take no bytes
        // off the front and this would not end.
        let at = if rest[over..].starts_with(char::is_whitespace) {
            over
        } else {
            match rest[..over].rfind(' ') {
                Some(space) if space > 0 => space,
                _ => over.max(step(rest)),
            }
        };

        rows.push(base..base + rest[..at].trim_end().len());
        let after = rest[at..].trim_start();
        base += rest.len() - after.len();
        rest = after;
    }

    rows
}

/// Where to wrap editable text without discarding any source.
///
/// Unlike `folds`, these ranges partition the text: whitespace and indentation
/// remain represented so a displayed caret or click can map back to the exact
/// source. A word moves whole when it can, and only a word wider than the row
/// is hard-broken.
pub(crate) fn wraps(text: &str, columns: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    if columns == 0 {
        return rows;
    }

    let mut base = 0;
    while base < text.len() {
        let rest = text.get(base..).unwrap_or_default();
        let Some(over) = cut(rest, columns) else {
            rows.push(base..text.len());
            break;
        };

        let at = if over == 0 {
            step(rest)
        } else if rest
            .get(over..)
            .and_then(|after| after.chars().next())
            .is_some_and(char::is_whitespace)
        {
            over
        } else {
            rest.get(..over)
                .unwrap_or_default()
                .char_indices()
                .filter(|(_, character)| character.is_whitespace())
                .map(|(at, character)| at + character.len_utf8())
                .next_back()
                .unwrap_or(over)
        };

        rows.push(base..base + at);
        base += at;
    }

    rows
}

/// `text` broken into rows no wider than `columns`, at the spaces where it can
/// be.
///
/// For a sentence composed here rather than one that arrived: a component with a
/// paragraph to draw has to know how many rows it drew, and this is where the
/// walk that answers that already lives. The streamed tail wraps at the column
/// instead, because it is fed a character at a time and cannot see the end of
/// the word it is in.
///
/// A word too long for a row is cut rather than left to overflow, which is what
/// keeps every row back no wider than asked for however narrow the terminal is.
/// Borrowed rather than allocated: the rows are pieces of `text`.
#[must_use]
pub fn fold(text: &str, columns: usize) -> Vec<&str> {
    folds(text, columns)
        .into_iter()
        .map(|row| text.get(row).unwrap_or_default())
        .collect()
}

/// The offset one character in, for the row too narrow to hold even that.
fn step(text: &str) -> usize {
    text.char_indices()
        .nth(1)
        .map_or(text.len(), |(offset, _)| offset)
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

        if character == '\n' {
            return (column, Some(offset));
        }

        let base = last.map(|(_, character)| character);
        let Some(step) = along(column, character, base) else {
            continue;
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

/// What a one-line field `room` columns wide shows of `text`, and the column
/// its caret stands in, with the caret `at` characters into the line.
///
/// A field is not a row of prose: prose that outgrows its width is cut and the
/// reader has read it, while a line being typed into outgrows its width in
/// front of the hands that are typing it. Cut, it answers every keystroke with
/// the same picture — and the caret, placed past the cut, stands somewhere the
/// field is not. So the window slides instead: it ends at the caret once the
/// text in front of the caret is wider than the field, and the caret keeps the
/// last column for itself, which is the column a terminal parks a cursor in.
///
/// The window is worked out from the caret alone rather than remembered
/// between frames. A field that remembered where it had scrolled to would need
/// somewhere to keep it, and every party that draws one would have to keep it
/// the same way; this asks the two facts a caller already has.
#[must_use]
pub(crate) fn windowed(text: &str, at: usize, room: usize) -> (&str, usize) {
    if room == 0 {
        return ("", 0);
    }

    let caret = text
        .char_indices()
        .nth(at)
        .map_or(text.len(), |(offset, _)| offset);

    // Walked back from the caret rather than forward from the start: what has
    // to be on screen is the caret, and everything else is what happens to fit
    // in front of it.
    let last = room - 1;
    let mut start = caret;
    let mut column = 0;
    let mut base = None;
    for (offset, character) in text.get(..caret).unwrap_or_default().char_indices().rev() {
        let Some(step) = along(column, character, base) else {
            continue;
        };
        if column + step > last {
            break;
        }
        column += step;
        start = offset;
        base = Some(character);
    }

    (clip(text.get(start..).unwrap_or_default(), room), column)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One column as text, two once the selector follows it. Spelled out
    /// because a selector is invisible in a source file.
    const WARNING: &str = "\u{26A0}\u{FE0F}";

    #[test]
    fn a_word_ending_exactly_on_the_last_column_stays_on_that_row() {
        // The row is full and the word is whole, so there is nothing to move
        // down. Breaking at the space before it instead loses a column of every
        // row it happens on, and it happens on every row whose last word lands
        // on the edge — a paragraph in a narrow terminal reads ragged for no
        // reason a reader can see.
        assert_eq!(fold("ab cd ef", 5), vec!["ab cd", "ef"]);
        assert_eq!(fold("one two three", 7), vec!["one two", "three"]);
    }

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
    fn a_row_does_not_end_in_the_space_it_broke_at() {
        // A run of spaces at a break has one of them ending the row and one
        // starting the next, and neither belongs to either. It reads as
        // correct at every width and is not: a trailing space is a column the
        // row was padded to and painted, so a row that keeps one carries its
        // colour a column further than the words did.
        assert_eq!(fold("aa  bb", 4), ["aa", "bb"]);
        assert_eq!(fold("aa   bb", 4), ["aa", "bb"]);
        assert_eq!(fold("one  two  three", 5), ["one", "two", "three"]);
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

    #[test]
    fn editable_wraps_keep_whitespace_and_move_words_whole() {
        let text = "  additional  vertical";
        let ranges = wraps(text, 14);
        let rows: Vec<_> = ranges
            .iter()
            .map(|range| text.get(range.clone()).unwrap_or_default())
            .collect();

        assert_eq!(rows, ["  additional  ", "vertical"]);
        assert_eq!(rows.concat(), text);
    }

    #[test]
    fn editable_wraps_preserve_combining_and_wide_source_when_a_word_cannot_fit() {
        for text in ["e\u{301}e\u{301}e\u{301}", "日本語"] {
            let ranges = wraps(text, 3);
            let rows: Vec<_> = ranges
                .iter()
                .map(|range| text.get(range.clone()).unwrap_or_default())
                .collect();

            assert_eq!(rows.concat(), text);
            assert!(rows.iter().all(|row| columns(row) <= 3));
        }
    }

    #[test]
    fn a_field_narrower_than_what_was_typed_shows_where_the_caret_is() {
        // The end of the line, because that is where a caret at the end of it
        // is: a field that went on showing the beginning would answer every
        // keystroke with the same picture, and typing into it looks broken
        // rather than full.
        let typed = "please tell me everything about the fox";
        assert_eq!(windowed(typed, typed.chars().count(), 10), ("t the fox", 9));

        // And back at the front, where the whole of it fits in front of the
        // caret, the window is the front of the line.
        assert_eq!(windowed(typed, 3, 10), ("please tel", 3));
    }

    #[test]
    fn a_field_wider_than_what_was_typed_is_the_whole_of_it() {
        assert_eq!(windowed("hello", 5, 20), ("hello", 5));
        assert_eq!(windowed("", 0, 20), ("", 0));
    }
}
