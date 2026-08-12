//! Making text fit a column that is narrower than it is.
//!
//! Borrowed back unchanged when it already fits, which is the usual case and
//! the one worth not allocating for.

use std::borrow::Cow;

use crate::width;

use crate::glyphs::Glyphs;

/// `text`, kept to at most `columns` display columns, with a mark standing
/// where the rest of it was.
///
/// The mark is inside the budget rather than beside it: a row that fits "except
/// for the ellipsis" is a row the terminal wraps.
pub(super) fn elide(text: &str, columns: usize, glyphs: Glyphs) -> Cow<'_, str> {
    if fits(text, columns) {
        return Cow::Borrowed(text);
    }

    let mark = glyphs.ellipsis();
    let room = columns.saturating_sub(width::columns(mark));

    // Narrower than the mark itself. Saying that something was dropped would
    // cost more columns than saying any of it, so the text is simply cut.
    if room == 0 {
        return Cow::Owned(kept(text, columns).to_owned());
    }

    // The trim is what stops a space arriving in front of the mark, which reads
    // as a gap in the text rather than as the end of it.
    Cow::Owned(format!("{}{mark}", kept(text, room).trim_end()))
}

/// `path`, kept to at most `columns` display columns by dropping the
/// directories between the root it starts from and the one it names.
///
/// Those two ends are what answer *where*: a route through somebody's home
/// directory is the part that grew, and it is the part they already know. A
/// path with no room even for the two ends falls back to [`elide`], which keeps
/// the root — by then the row is narrower than a directory name and no
/// shortening rescues it.
pub(super) fn shorten(path: &str, columns: usize, glyphs: Glyphs) -> Cow<'_, str> {
    if fits(path, columns) {
        return Cow::Borrowed(path);
    }

    if let (Some(first), Some(last)) = (path.find(separator), path.rfind(separator))
        && first < last
    {
        // Inclusive of the first separator and from the last, so what is left
        // still reads as a path rather than as two names pushed together.
        let short = format!("{}{}{}", &path[..=first], glyphs.ellipsis(), &path[last..]);
        if fits(&short, columns) {
            return Cow::Owned(short);
        }
    }

    elide(path, columns, glyphs)
}

/// Whether the whole of `text` is one row of at most `columns` columns.
fn fits(text: &str, columns: usize) -> bool {
    width::cut(text, columns).is_none()
}

/// As much of `text` as `columns` holds.
fn kept(text: &str, columns: usize) -> &str {
    match width::cut(text, columns) {
        Some(at) => &text[..at],
        None => text,
    }
}

/// Whether a character parts one directory from the next.
///
/// Both, on either kind of machine. A path drawn here came from the operating
/// system this is running on, and Windows hands back either one.
fn separator(character: char) -> bool {
    character == '/' || character == '\\'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_fits_is_handed_straight_back() {
        assert!(matches!(
            elide("crucible", 20, Glyphs::Unicode),
            Cow::Borrowed("crucible")
        ));
    }

    #[test]
    fn the_mark_is_inside_the_budget_and_not_beside_it() {
        // The failure this exists to stop: a row measured without its own
        // ellipsis is a row one column too wide for the column it went in.
        for columns in 1..12 {
            let short = elide("a search that stops partway", columns, Glyphs::Unicode);
            assert!(
                fits(&short, columns),
                "{short:?} is wider than {columns} columns"
            );
        }
    }

    #[test]
    fn a_font_without_an_ellipsis_spends_three_columns_saying_the_same_thing() {
        assert_eq!(elide("crucible", 6, Glyphs::Ascii), "cru...");
        assert_eq!(elide("crucible", 6, Glyphs::Unicode), "cruci…");
    }

    #[test]
    fn no_space_is_left_standing_in_front_of_the_mark() {
        assert_eq!(elide("one two three", 8, Glyphs::Unicode), "one two…");
    }

    #[test]
    fn a_path_gives_up_its_middle_before_either_of_its_ends() {
        assert_eq!(
            shorten("~/code/vendor/crucible-code", 20, Glyphs::Unicode),
            "~/…/crucible-code"
        );
    }

    #[test]
    fn a_path_that_fits_keeps_every_directory_it_names() {
        assert!(matches!(
            shorten("~/code/crucible-code", 40, Glyphs::Unicode),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_windows_path_is_parted_at_the_separator_windows_wrote() {
        assert_eq!(
            shorten(r"C:\Users\someone\src\crucible-code", 24, Glyphs::Unicode),
            r"C:\…\crucible-code"
        );
    }

    #[test]
    fn a_path_with_no_room_for_both_ends_still_fits_the_column() {
        for columns in 0..24 {
            let short = shorten("~/code/vendor/crucible-code", columns, Glyphs::Unicode);
            assert!(
                fits(&short, columns),
                "{short:?} is wider than {columns} columns"
            );
        }
    }
}
