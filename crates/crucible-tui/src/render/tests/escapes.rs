//! A backslash before a line break and the references that stand for a
//! character: what reaches the terminal when a model writes them, in the
//! screen mode that redraws.

use super::*;

/// References to characters no screen draws as themselves, each written the
/// way a model might to put an escape, a bell, a bidirectional override or a
/// nothing on the terminal.
const HOSTILE_REFERENCES: [&str; 5] = [
    "&#27;[31mred&#27;[0m and &#27;]0;t&#7;\n",
    "&#x1b; and &#x1B;]8;;https://evil.invalid&#x7;\n",
    "&#x202E;gnirts and &#8238;\n",
    "&#7; bell, &#0; nothing and &#x0; nothing\n",
    "a &#x10EEEE; b and &#1109742; c\n",
];

#[test]
fn a_hostile_entity_writes_no_escape_nor_a_picture_cell_and_is_drawn_as_written() {
    for answer in HOSTILE_REFERENCES {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let mut drawn = reading(64, 12);
            drawn.render.images(kitty_in(9, 18));
            drawn.render.draws(glyphs);
            drawn.answer(answer);

            let written = drawn.render.terminal.written();
            for hostile in [
                "\u{1b}]",
                "\u{1b}[31",
                "\u{1b}P",
                "\u{1b}_",
                "\u{7}",
                "\u{8}",
                "\u{85}",
                "\u{9c}",
                "\u{202e}",
                "\u{0}",
            ] {
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
    let rows = drawn_at_thirty(
        "&#27;[31m and &#x1b; and &#x202E;gnirts and &#7; and &#0;\n",
        Glyphs::Unicode,
    )
    .join(" ");
    for words in ["&#27;[31m", "&#x1b;", "&#x202E;", "&#7;", "&#0;"] {
        assert!(rows.contains(words), "{rows:?} lost {words:?}");
    }
}

#[test]
fn an_entity_is_drawn_as_the_character_it_names() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = drawn_at_thirty("a &amp; b &lt;c&gt; &#169; &#x41;\n", glyphs);
        assert_eq!(rows, ["a & b <c> © A"], "{glyphs:?}");
    }
}

#[test]
fn a_backslash_before_a_line_break_breaks_the_line_and_draws_no_backslash() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = drawn_at_thirty("first line\\\nsecond line\n", glyphs);
        assert_eq!(rows, ["first line", "second line"], "{glyphs:?}");

        let rows = drawn_at_thirty("ends on it\\\n\nnext\n", glyphs);
        assert_eq!(rows, ["ends on it\\", "next"], "{glyphs:?}");
    }
}
