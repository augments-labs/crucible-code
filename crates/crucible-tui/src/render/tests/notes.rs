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

/// Reference definitions, angle addresses and `www.` words as a model might
/// write them to carry an escape or a picture, in the address and in the words.
const HOSTILE_ADDRESSES: [&str; 5] = [
    "[a]: https://x.test/\u{1b}]0;t\u{7}\nsee [a] and [a\u{1b}]0;t\u{7}][a]\n",
    "<https://x.test/\u{1b}]8;;https://evil.invalid\u{1b}\\> and <a\u{1b}]0;t\u{7}@b.test>\n",
    "www.x.test/\u{1b}]0;t\u{7} and www.\u{1b}_Gi=1;AAAA\u{1b}\\x.test and www.y.test\u{8}\u{85}\n",
    "[a\u{1b}]0;t\u{7}]: https://x.test/a\nsee [a\u{1b}]0;t\u{7}] and [w\u{8}][a\u{1b}]0;t\u{7}]\n",
    concat!(
        "[\u{10eeee}\u{305}x]: https://x.test\n",
        "see [words][\u{10eeee}\u{305}x]\n",
    ),
];

/// What `written` says with the hyperlink sequences it carries taken out, each
/// of which must hold printable ASCII and nothing that ends it early.
fn without_links(written: &str) -> String {
    let mut rest = written;
    let mut kept = String::new();
    while let Some(at) = rest.find("\u{1b}]8;") {
        kept.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at.saturating_add(4)..).unwrap_or_default();
        let end = after.find("\u{1b}\\").unwrap_or(after.len());
        let sequence = after.get(..end).unwrap_or_default();
        assert!(
            sequence.bytes().all(|byte| byte.is_ascii_graphic()),
            "a link sequence carried {sequence:?} in {written:?}"
        );
        rest = after.get(end.saturating_add(2)..).unwrap_or_default();
    }
    kept.push_str(rest);
    kept
}

#[test]
fn a_hostile_reference_angle_or_www_address_writes_no_escape_nor_a_picture_cell() {
    for answer in HOSTILE_ADDRESSES {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let mut drawn = Drawn::new(64, 12);
            drawn.wears(colourful().addressing(true));
            drawn.render.images(kitty_in(9, 18));
            drawn.render.draws(glyphs);
            drawn.answer(answer);

            let written = without_links(drawn.render.terminal.written());
            for hostile in [
                "\u{1b}]", "\u{1b}P", "\u{1b}_", "\u{7}", "\u{8}", "\u{85}", "\u{9c}",
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
}

#[test]
fn an_entity_inside_a_key_is_drawn_as_written() {
    let rows = drawn_at_thirty("<kbd>&amp; &lt;</kbd>\n", Glyphs::Unicode);
    assert_eq!(rows, ["&amp; &lt;"]);
}

/// The rows of `answer` in `glyphs` that are not blank, at thirty columns:
/// once drawn there and once after a window of sixty narrowed to it.
fn at_thirty_and_narrowed(answer: &str, glyphs: Glyphs) -> [Vec<String>; 2] {
    [30, 60].map(|columns| {
        let mut drawn = reading(columns, 24);
        drawn.render.draws(glyphs);
        drawn.answer(answer);
        drawn.resize(30, 24);
        drawn
            .rows()
            .into_iter()
            .map(|row| row.trim_end().to_owned())
            .filter(|row| !row.is_empty())
            .collect()
    })
}

/// A `<details>` block whose body line is wider than thirty columns, once
/// written as one line and once broken onto a second row by a `<br>`.
fn wide_details() -> [String; 2] {
    [LONG.to_owned(), format!("the body<br>{LONG}")]
        .map(|body| format!("<details><summary>Why</summary>\n{body}\n</details>\nafter\n"))
}

#[test]
fn a_details_body_wider_than_the_window_is_two_columns_in_on_every_row_drawn_or_narrowed() {
    for answer in wide_details() {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for rows in at_thirty_and_narrowed(&answer, glyphs) {
                let body = rows
                    .get(1..rows.len().saturating_sub(1))
                    .unwrap_or_default();
                assert!(body.len() > 2, "{glyphs:?} drew {rows:#?}");
                for row in body {
                    assert_eq!(indent(row), 2, "{glyphs:?} drew {row:?} in {rows:#?}");
                }
                assert_eq!(rows.last().map(String::as_str), Some("after"), "{rows:#?}");
            }
        }
    }
}

/// A bare table inside a `<details>` block, the tag that closes the block
/// alone on the line after its last row.
const TABLE_IN_DETAILS: &str =
    "<details>\n<summary>S</summary>\n\na | b\n--|--\n1 | 2\n</details>\nafter\n";

#[test]
fn a_closing_details_tag_after_a_bare_table_is_no_row_of_it_and_the_line_after_is_at_the_margin() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = drawn_at_thirty(TABLE_IN_DETAILS, glyphs);
        assert!(
            rows.iter().all(|row| !row.contains("details")),
            "{glyphs:?} drew {rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.contains('2')),
            "{glyphs:?} drew {rows:#?}"
        );
        assert_eq!(rows.last().map(String::as_str), Some("after"), "{rows:#?}");
    }
}
