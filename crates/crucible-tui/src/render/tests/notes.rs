//! Footnotes and the six tags a screen reads: what reaches the terminal when
//! a model writes them to carry an escape or a picture, and what a key holds.

use super::*;

/// A tag, a key and a footnote's label as a model might write them to be read
/// as an instruction. The image placeholder is written into a tag's attributes
/// and a label only: between `<kbd>` and `</kbd>` it is text, drawn as written
/// as it is in inline code.
const HOSTILE_SMALL: [&str; 5] = [
    "<kbd\u{1b}]0;a title\u{7}>K</kbd> and <details \u{1b}_Gi=1;AAAA\u{1b}\\>\n",
    "<kbd>&amp; \u{1b}]8;;https://example.invalid\u{1b}\\ \u{1b}_Gi=1;AAAA\u{1b}\\</kbd>\n",
    "a[^\u{1b}]0;t\u{7}] and [^\u{1b}_Gi=1;AAAA\u{1b}\\]\n\n[^\u{1b}P1$r\u{1b}\\]: a note\n",
    "H<sub>\u{1b}]0;t\u{7}</sub>O and x<sup>\u{8}2\u{85}</sup>\n",
    concat!(
        "<kbd \u{10eeee}\u{305}>K</kbd> a[^\u{10eeee}\u{305}x] H<sub \u{10eeee}\u{305}>2</sub>",
        " <details \u{10eeee}\u{305}>\n\n[^\u{10eeee}\u{305}x]: n\n",
    ),
];

#[test]
fn a_hostile_tag_key_or_footnote_label_writes_no_escape_nor_a_tag_or_label_a_picture_cell() {
    for answer in HOSTILE_SMALL {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let mut drawn = reading(64, 12);
            drawn.render.images(kitty_in(9, 18));
            drawn.render.draws(glyphs);
            drawn.answer(answer);

            let written = drawn.render.terminal.written();
            for hostile in ["\u{1b}]", "\u{1b}P", "\u{1b}_", "\u{7}", "\u{8}", "\u{85}"] {
                assert!(
                    !written.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
            assert!(
                !written.contains('\u{10eeee}'),
                "a picture cell was placed for {answer:?} with {glyphs:?}"
            );
        }
    }
}

#[test]
fn an_entity_inside_a_key_is_drawn_as_written() {
    let rows = drawn_at_thirty("<kbd>&amp; &lt;</kbd>\n", Glyphs::Unicode);
    assert_eq!(rows, ["&amp; &lt;"]);
}
