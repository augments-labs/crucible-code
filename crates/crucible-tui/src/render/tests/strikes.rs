//! A phrase between single tildes, a path that keeps its tilde, and weight
//! with emphasis: what reaches the screen in the mode that redraws, and what
//! the live foot shows while a phrase is held.

use super::*;
use crate::markdown::{Markdown, Said};

/// The text of every struck run the reader says for a whole `answer`.
fn struck(answer: &str) -> String {
    let mut markdown = Markdown::default();
    let mut said = String::new();
    let mut say = |told: Said<'_>| {
        if let Said::Run { slot, text, .. } = told
            && slot == Slot::Struck
        {
            said.push_str(text);
        }
    };
    markdown.read(answer, 80, &mut say);
    markdown.finish(80, &mut say);
    said
}

/// Every row the record keeps, as words.
fn recorded(drawn: &Drawn) -> Vec<String> {
    let record = &drawn.render.record;
    (0..record.lines())
        .flat_map(|line| record.folded(line))
        .map(|row| row.text().trim_end().to_owned())
        .collect()
}

#[test]
fn single_tildes_strike_and_paths_stay_as_written() {
    let mut drawn = reading(80, 4);
    drawn.answer("~one~ and ~/Projects and cd ~/a and ~/b\n");

    assert_eq!(
        drawn.screen().row(0),
        "one and ~/Projects and cd ~/a and ~/b"
    );
}

#[test]
fn a_tilde_with_no_closer_in_its_paragraph_strikes_nothing() {
    for prose in [
        "edit ~/.config/crucible/config.json then ~/bin/run",
        "between ~/a and ~/b",
        "about ~5 minutes or ~10",
        "an opener ~here\n\nand a closer~ after a blank line",
    ] {
        assert_eq!(
            struck(&format!("{prose}\n")),
            "",
            "{prose:?} struck something"
        );
        let mut drawn = reading(80, 4);
        drawn.answer(&format!("{prose}\n"));
        // The picture leaves blank rows out, so the blank line between two
        // paragraphs is compared by leaving it out of the prose as well.
        let expected: Vec<&str> = prose.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(drawn.rows(), expected, "{prose:?} changed");
    }
}

#[test]
fn a_struck_phrase_wears_the_strike_and_a_path_does_not() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let mut drawn = reading(80, 4);
        drawn.render.draws(glyphs);
        drawn.answer("~one~ and ~/Projects\n");

        let written = drawn.render.terminal.written();
        let worn = colourful().open(Slot::Struck);
        let struck = worn.as_str();
        assert!(
            written.contains(&format!("{struck}one")),
            "{glyphs:?}: {written:?}"
        );
        assert!(
            !written.contains(&format!("{struck}/Projects")),
            "{glyphs:?}: {written:?}"
        );
    }
}

#[test]
fn three_markers_are_drawn_bold_and_italic_together() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let mut drawn = reading(80, 4);
        drawn.render.draws(glyphs);
        drawn.answer("a ***both*** b and **bold _italic_ bold**\n");

        assert_eq!(
            drawn.screen().row(0),
            "a both b and bold italic bold",
            "{glyphs:?}"
        );
        let written = drawn.render.terminal.written();
        assert!(
            written.contains("\u{1b}[1;3mboth"),
            "{glyphs:?}: {written:?}"
        );
        assert!(
            written.contains("\u{1b}[1;3mitalic"),
            "{glyphs:?}: {written:?}"
        );
    }
}

#[test]
fn a_held_phrase_is_told_of_in_the_live_foot_and_joins_the_record_once_settled() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let mut drawn = reading(80, 8);
        drawn.render.draws(glyphs);
        drawn.render.stream("look in ~/Projects").unwrap();

        assert!(
            !recorded(&drawn)
                .iter()
                .any(|row| row.contains("~/Projects")),
            "{glyphs:?}: the held phrase reached the record"
        );
        assert_eq!(
            drawn.render.holding(80).map(|row| row.text()),
            Some("~/Projects".to_owned()),
            "{glyphs:?}"
        );
        assert!(
            drawn.rows().iter().any(|row| row.contains("~/Projects")),
            "{glyphs:?}: {:#?}",
            drawn.rows()
        );

        drawn.render.stream("\n\nnext\n").unwrap();
        drawn.render.settle().unwrap();

        assert_eq!(drawn.render.holding(80), None, "{glyphs:?}");
        assert!(
            recorded(&drawn)
                .iter()
                .any(|row| row == "look in ~/Projects"),
            "{glyphs:?}: {:#?}",
            recorded(&drawn)
        );
        assert_eq!(drawn.rows(), ["look in ~/Projects", "next"], "{glyphs:?}");
    }
}

#[test]
fn a_held_phrase_told_of_in_the_foot_writes_no_escape_it_holds() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let mut drawn = reading(80, 8);
        drawn.render.draws(glyphs);
        drawn
            .render
            .stream("a ~b \u{1b}[31mred\u{7} \u{202e}c")
            .unwrap();

        assert!(drawn.render.holding(80).is_some(), "{glyphs:?}");
        let written = drawn.render.terminal.written();
        for hostile in ["\u{1b}[31", "\u{7}", "\u{202e}"] {
            assert!(
                !written.contains(hostile),
                "{hostile:?} reached the terminal with {glyphs:?}"
            );
        }
    }
}
