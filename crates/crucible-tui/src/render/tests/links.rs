//! Links, addresses and forge references on the screen: what a row draws for
//! each, what a drag copies off it, and what reaches the terminal when a
//! model writes one to carry an escape.

use super::*;
use crate::Forge;

/// The repository this checkout was cloned from, as a session finds it.
fn here() -> Forge {
    Forge::new(
        "https://github.com",
        "augments-labs/crucible-code",
        "/issues/",
    )
}

#[test]
fn a_pull_request_address_is_drawn_as_its_reference() {
    let answer = "opened https://github.com/augments-labs/crucible-code/pull/1324 today\n";

    let mut drawn = reading(100, 4);
    drawn.answer(answer);
    let rows = drawn.rows();
    assert!(
        rows.iter()
            .any(|row| row.contains("opened augments-labs/crucible-code#1324 today")),
        "{rows:#?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("https://github.com")),
        "{rows:#?}"
    );

    let mut drawn = reading(100, 4);
    drawn.render.counts(Some(here()));
    drawn.answer(answer);
    let rows = drawn.rows();
    assert!(
        rows.iter().any(|row| row.contains("opened #1324 today")),
        "{rows:#?}"
    );
}

#[test]
fn an_image_reference_keeps_its_words_and_drops_its_marker() {
    let answer = "look at ![the screenshot](out/shot.png) here\n";
    // Cut anywhere, the bang and its bracket included, the row is the same.
    for cut in (0..=answer.len()).filter(|&cut| answer.is_char_boundary(cut)) {
        let mut drawn = reading(100, 4);
        drawn.answer_in_pieces(answer, &[cut]);
        let rows = drawn.rows();
        assert!(
            rows.iter()
                .any(|row| row.contains("look at [image: the screenshot] (out/shot.png) here")),
            "cut at {cut}: {rows:#?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains('!')),
            "cut at {cut}: {rows:#?}"
        );
    }
}

#[test]
fn a_drag_over_a_worded_link_copies_its_words_and_not_its_quiet_address() {
    let mut drawn = reading(60, 6);
    drawn.answer("see [the guide](https://example.com/docs/guide/intro) now\n");
    let rows = window(&drawn, 6);
    let row = row_containing(&rows, "see the guide (example.com/docs/…) now");
    drawn.take();

    let wrote = drag(&mut drawn, (row, 0), (row, 59));

    assert!(
        wrote.contains(&onto_the_clipboard("see the guide now")),
        "{rows:#?} gave {wrote:?}"
    );
}

#[test]
fn every_address_drawn_shorter_is_listed_whole_newest_first_after_the_blocks() {
    let home = "https://github.com/augments-labs/crucible-code";
    let mut drawn = reading(100, 20);
    drawn.render.counts(Some(here()));
    drawn.answer(&format!(
        "a {home}/pull/1324 b #7 c [d](https://example.com/docs/guide/intro) \
         e [f](https://example.com/x)\n\n{WRITTEN_TABLE}\nafter\n"
    ));

    // A reference the answer wrote short itself, and a quiet address shown
    // whole, were drawn as written: neither is listed.
    let listed: Vec<(&str, &str)> = drawn
        .sources()
        .map(|source| (source.word, source.text))
        .collect();
    let pull = format!("{home}/pull/1324");
    assert_eq!(
        listed,
        [
            ("table", WRITTEN_TABLE),
            ("address", "https://example.com/docs/guide/intro"),
            ("address", pull.as_str()),
        ]
    );

    // The bound a list taken now is held to covers them.
    let bound = drawn.sourced();
    assert!(drawn.sources().all(|source| source.line < bound));
}

/// Forge addresses, worded links, images and a table cell as a model might
/// write them to carry an escape, a control, a reordering or an invisible
/// character, in the words, the address and the path.
const HOSTILE_LINKS: [&str; 6] = [
    "https://github.com/augments-labs/crucible-code/pull/1\u{1b}]0;t\u{7} and https://github.com/o/r/commit/abcdef1\u{9b}31m\n",
    "[fix\u{1b}]0;t\u{7}](https://github.com/o/r/pull/12) and [x](https://github.com/o/r/pull/12\u{85})\n",
    "[guide](https://example.com/\u{1b}]8;;https://evil.invalid\u{1b}\\/a/b) and [g](https://example.com/\u{202e}\u{200b}x/y/z)\n",
    "![alt\u{1b}]0;t\u{7}](out/\u{1b}_Gi=1;AAAA\u{1b}\\shot.png) and ![a](https://x.test/\u{202e}gnp.exe\u{9c})\n",
    "[write](mailto:a\u{1b}]0;t\u{7}@b.test) and [w](mailto:a@b.test\u{8}\u{9b}2J)\n",
    "| a |\n|---|\n| [g](https://example.com/\u{1b}]0;t\u{7}/x) https://github.com/o/r/pull/3\u{85} |\n",
];

#[test]
fn a_hostile_forge_address_worded_link_or_image_writes_no_escape() {
    for answer in HOSTILE_LINKS {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let mut drawn = Drawn::new(64, 12);
            drawn.wears(colourful().addressing(true));
            drawn.render.images(kitty_in(9, 18));
            drawn.render.draws(glyphs);
            drawn.render.counts(Some(here()));
            drawn.answer(answer);

            let (written, sequences) = super::notes::without_links(drawn.render.terminal.written());
            for sequence in sequences {
                assert!(
                    sequence.bytes().all(|byte| byte.is_ascii_graphic()),
                    "a link sequence carried {sequence:?} for {answer:?} with {glyphs:?}"
                );
            }
            for hostile in [
                "\u{1b}]",
                "\u{1b}P",
                "\u{1b}_",
                "\u{1b}[31",
                "\u{1b}[2J",
                "\u{7}",
                "\u{8}",
                "\u{85}",
                "\u{9b}",
                "\u{9c}",
                "\u{202e}",
                "\u{200b}",
            ] {
                assert!(
                    !written.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
        }
    }
}

#[test]
fn every_shape_of_link_cut_anywhere_is_drawn_as_it_is_whole() {
    // Each shape the reader settles, and the rows it is drawn as: a cut at
    // any character boundary, inside the address, the brackets or the cell,
    // is drawn as the answer streamed whole is.
    let shapes: [(&str, &[&str]); 6] = [
        (
            "a https://github.com/augments-labs/crucible-code/pull/1324 b\n",
            &["a #1324 b"],
        ),
        (
            "a <https://github.com/someone/else/issues/12> b\n",
            &["a someone/else#12 b"],
        ),
        (
            "a [the guide](https://example.com/docs/guide/intro) b\n",
            &["a the guide (example.com/docs/…) b"],
        ),
        (
            "a [the fix](https://gitlab.com/group/sub/repo/-/merge_requests/3) b\n",
            &["a the fix (group/sub/repo!3) b"],
        ),
        (
            "a [write](mailto:someone.with.a.long.name@example.com) b\n",
            &["a write (someone.with.a.long.name…) b"],
        ),
        (
            "| what | where |\n|---|---|\n| fix | https://github.com/augments-labs/crucible-code/commit/abcdef1234567 |\n",
            &["what │ where", "─────┼────────", "fix  │ abcdef1"],
        ),
    ];
    for (answer, expected) in shapes {
        for cut in (0..=answer.len()).filter(|&cut| answer.is_char_boundary(cut)) {
            let mut drawn = reading(100, 6);
            drawn.render.counts(Some(here()));
            drawn.answer_in_pieces(answer, &[cut]);
            let rows: Vec<String> = drawn
                .rows()
                .iter()
                .map(|row| row.trim_end().to_owned())
                .filter(|row| !row.is_empty())
                .collect();
            assert_eq!(rows, expected, "{answer:?} cut at {cut}");
        }
    }
}
