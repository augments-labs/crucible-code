//! A phrase between two single tildes, the hold that waits for the closing
//! one, and a run under weight and emphasis at once: what the reader draws,
//! and what it leaves as written.

use super::*;

/// Every word of `answer`, read whole and ended, that is drawn struck through,
/// with what a line taken back said left out.
fn struck(answer: &str) -> String {
    standing(ended(answer))
        .into_iter()
        .filter(|(slot, _)| *slot == Slot::Struck)
        .map(|(_, text)| text)
        .collect()
}

/// What is left standing of `said`, as the text it draws and the slots it
/// draws under.
fn picture(said: Vec<(Slot, String)>) -> (String, Vec<Slot>) {
    let said = standing(said);
    (drawn(&said), slots(&said))
}

/// What an answer draws, and under which slots, cut into deltas at `cuts`.
fn cut_at(answer: &str, cuts: &[usize]) -> (String, Vec<Slot>) {
    let mut pieces = Vec::new();
    let mut from = 0;
    for &cut in cuts {
        pieces.push(answer.get(from..cut).unwrap_or_default());
        from = cut;
    }
    pieces.push(answer.get(from..).unwrap_or_default());
    picture(ended_in(&pieces))
}

#[test]
fn single_tildes_strike_the_phrase_between_them() {
    assert_eq!(struck("~one~ and ~/Projects and cd ~/a and ~/b\n"), "one");
    assert_eq!(
        struck("takes 2~3 seconds, then 5~10\n"),
        "3 seconds, then 5"
    );
    assert_eq!(struck("see ~/a~ here\n"), "/a");
    assert_eq!(
        struck("an opener ~here\nand a closer~ on the next line\n"),
        "here\nand a closer"
    );
}

#[test]
fn a_struck_phrase_draws_without_its_tildes() {
    for (answer, drawn_text) in [
        (
            "~one~ and ~/Projects and cd ~/a and ~/b\n",
            "one and ~/Projects and cd ~/a and ~/b\n",
        ),
        (
            "takes 2~3 seconds, then 5~10\n",
            "takes 23 seconds, then 510\n",
        ),
        ("see ~/a~ here\n", "see /a here\n"),
        (
            "an opener ~here\nand a closer~ on the next line\n",
            "an opener here\nand a closer on the next line\n",
        ),
    ] {
        assert_eq!(drawn(&standing(ended(answer))), drawn_text, "{answer:?}");
    }
}

#[test]
fn a_single_tilde_with_no_closer_in_its_paragraph_is_drawn_as_written() {
    for answer in [
        "edit ~/.config/crucible/config.json then ~/bin/run\n",
        "between ~/a and ~/b\n",
        "about ~5 minutes or ~10\n",
        "an opener ~here\n\nand a closer~ after a blank line\n",
        "a lone closer~ and nothing after\n",
        "ends on a tilde ~\n",
        "a ~ standing alone ~ twice\n",
    ] {
        let said = standing(ended(answer));
        assert_eq!(drawn(&said), answer, "{answer:?}");
        assert!(
            !said.iter().any(|(slot, _)| *slot == Slot::Struck),
            "{answer:?}: {said:?}"
        );
    }
}

#[test]
fn a_tilde_path_inside_emphasis_stays_as_written_under_the_emphasis() {
    let said = standing(ended("*see ~/a and ~/b*\n"));

    assert_eq!(drawn(&said), "see ~/a and ~/b\n");
    assert_eq!(slots(&said), [Slot::Emphasis, Slot::Plain]);
}

#[test]
fn a_closer_closes_the_innermost_opener_and_drops_the_ones_inside_it() {
    // The rule GitHub's emphasis is resolved by: a closer pairs with the
    // nearest opener still open, and an opener between them is a character.
    assert_eq!(struck("~a ~b~ c~ d\n"), "a b c");
    assert_eq!(drawn(&standing(ended("~a ~b~ c~ d\n"))), "a b c d\n");
    assert_eq!(struck("~a ~b~ c\n"), "b");
    assert_eq!(drawn(&standing(ended("~a ~b~ c\n"))), "~a b c\n");
    assert_eq!(struck("~a ~/b c~ d\n"), "/b c");
    assert_eq!(drawn(&standing(ended("~a ~/b c~ d\n"))), "~a /b c d\n");
}

#[test]
fn two_tildes_and_three_read_as_they_did() {
    assert_eq!(struck("a ~~loud and wrong~~ b\n"), "loud and wrong");
    assert_eq!(struck("~~a ~b~ c~~\n"), "a b c");
    assert_eq!(drawn(&standing(ended("~~a ~b~ c~~\n"))), "a b c\n");
    assert_eq!(struck("~a ~~b~~ c~\n"), "a b c");
    assert_eq!(
        ended("~~~b~~~ c\n"),
        [
            (Slot::Quiet, "┌─ b~~~".to_owned()),
            (Slot::Quiet, "\n".to_owned())
        ]
    );
}

#[test]
fn a_single_pair_wears_what_a_double_pair_wears_where_it_stands() {
    // One source for what a struck phrase outranks and what outranks it: the
    // slots a pair of two tildes gives the same words, around the same text.
    for around in [
        "a {} b\n",
        "**{}** b\n",
        "*{}* b\n",
        "***{}*** b\n",
        "# {} b\n",
        "> {} b\n",
        "- {} b\n",
        "- [x] {} b\n",
        "1. {} b\n",
        "<details><summary>{}</summary>\n",
    ] {
        for phrase in ["{t}word{t}", "{t}`code` and [a link](https://h){t}"] {
            let phrase_in = |tilde: &str| around.replace("{}", &phrase.replace("{t}", tilde));
            assert_eq!(
                picture(ended(&phrase_in("~"))),
                picture(ended(&phrase_in("~~"))),
                "{:?}",
                phrase_in("~")
            );
        }
    }
}

#[test]
fn a_single_tilde_changes_nothing_where_text_is_drawn_as_written() {
    // Code, a key, a script, a link's address and words, a bare address, a
    // block of code and a block shown as written: each draws the tildes it
    // was written with, and none of it is struck.
    for (answer, drawn_text) in [
        ("a `~b~` c\n", "a ~b~ c\n"),
        ("a <kbd>~b~</kbd> c\n", "a ~b~ c\n"),
        ("a <sub>~b~</sub> c\n", "a ~b~ c\n"),
        ("a <sup>~b~</sup> c\n", "a ~b~ c\n"),
        ("a [x](https://h/~b~) c\n", "a x c\n"),
        ("[~b~](https://h) c\n", "~b~ c\n"),
        ("a <https://h/~b~> c\n", "a https://h/~b~ c\n"),
        ("a https://h/~b~ c\n", "a https://h/~b~ c\n"),
        ("a www.h.com/~b~ c\n", "a www.h.com/~b~ c\n"),
        ("a\\~b\\~ c\n", "a~b~ c\n"),
        ("$$\n~b~ and ~c~\n$$\n", "$$\n~b~ and ~c~\n$$\n"),
        ("```\n~b~ and ~c~\n```\n", "┌─\n│ ~b~ and ~c~\n└─\n"),
        (
            "```mermaid\n~b~ and ~c~\n```\n",
            "┌─ mermaid\n│ ~b~ and ~c~\n└─\n",
        ),
        (
            "```diff\n~b~ and ~c~\n```\n",
            "┌─ diff\n│ ~b~ and ~c~\n└─\n",
        ),
        (
            "para\n\n    ~b~ and ~c~\n",
            "para\n\n┌─\n│ ~b~ and ~c~\n└─\n",
        ),
    ] {
        let said = standing(ended(answer));
        assert_eq!(drawn(&said), drawn_text, "{answer:?}");
        assert!(
            !said.iter().any(|(slot, _)| *slot == Slot::Struck),
            "{answer:?}: {said:?}"
        );
    }
}

#[test]
fn a_held_phrase_crosses_exactly_the_lines_a_double_pair_crosses() {
    // One source for where a paragraph ends: whatever a pair of two tildes
    // carries over, a pair of one strikes the same words across, and whatever
    // stops the one stops the other, which is then drawn as written.
    for written in [
        "a {t}b\nc{t} d\n",
        "a {t}b\n\nc{t} d\n",
        "a {t}b\n- c{t} d\n",
        "a {t}b\n> c{t} d\n",
        "a {t}b\n# c{t} d\n",
        "a {t}b\n```\nc{t} d\n```\n",
        "a {t}b\n$$\nc{t} d\n$$\n",
        "a {t}b\n    c{t} d\n",
        "a {t}b\n1. c{t} d\n",
        "a {t}b\n---\nc{t} d\n",
        "a {t}b\nc\n===\nd{t} e\n",
        "a {t}b\n| x | y |\n|---|---|\n| c{t} | d |\n",
        "a {t}b\nx | y\n--|--\nc{t} | d\n",
        "- a {t}b\n- c{t} d\n",
        "> a {t}b\n> c{t} d\n",
    ] {
        let single = written.replace("{t}", "~");
        let double = written.replace("{t}", "~~");
        let carried = struck(&double);
        if carried.contains(['c', 'd']) {
            let without_breaks = |text: String| text.replace('\n', "");
            assert_eq!(
                without_breaks(struck(&single)),
                without_breaks(carried),
                "{single:?}"
            );
            assert_eq!(
                picture(ended(&single)).0,
                picture(ended(&double)).0,
                "{single:?}"
            );
        } else {
            assert_eq!(struck(&single), "", "{single:?}");
            assert_eq!(
                picture(ended(&single)),
                picture(ended(
                    &written.replacen("{t}", "\\~", 1).replace("{t}", "~")
                )),
                "{single:?}"
            );
        }
    }
}

/// An answer whose struck phrase is `length` bytes between its tildes.
fn phrase_of(length: usize) -> String {
    format!("a ~{}~ b\n", "x".repeat(length))
}

#[test]
fn a_phrase_strikes_only_when_its_closer_is_within_the_bound() {
    let below = STRIKE_CAP - 1;
    assert_eq!(struck(&phrase_of(below)), "x".repeat(below));
    for length in [STRIKE_CAP, STRIKE_CAP + 1] {
        assert_eq!(struck(&phrase_of(length)), "", "{length}");
        assert_eq!(
            drawn(&standing(ended(&phrase_of(length)))),
            phrase_of(length),
            "{length}"
        );
    }
}

#[test]
fn a_phrase_at_the_bound_draws_the_same_wherever_it_is_cut() {
    for length in [STRIKE_CAP - 1, STRIKE_CAP, STRIKE_CAP + 1] {
        let answer = phrase_of(length);
        let whole = picture(ended(&answer));
        // The opener is at byte 2 and the closer at `length + 3`.
        let closer = length + 3;
        let mut cuts: Vec<usize> = vec![1, 2, 3, 4];
        cuts.extend((STRIKE_CAP..=STRIKE_CAP + 5).filter(|cut| *cut < answer.len()));
        cuts.extend([closer - 1, closer, closer + 1, closer + 2]);
        for cut in cuts {
            assert_eq!(cut_at(&answer, &[cut]), whole, "{length} cut at {cut}");
        }
        // Reaching the bound in one delta and the closer in the next.
        assert_eq!(
            cut_at(&answer, &[3, STRIKE_CAP + 2, closer]),
            whole,
            "{length}"
        );
    }
}

#[test]
fn an_answer_with_single_tildes_draws_the_same_wherever_it_is_cut() {
    for answer in [
        "~one~ and ~/Projects and cd ~/a and ~/b\n",
        "takes 2~3 seconds, then 5~10\n",
        "see ~/a~ here\n",
        "an opener ~here\nand a closer~ on the next line\n",
        "an opener ~here\n\nand a closer~ after a blank line\n",
        "edit ~/.config/crucible/config.json then ~/bin/run\n",
        "~a ~b~ c~ d\n",
        "a ~b\n- c~ d\n",
        "a ~b\nx | y\n--|--\nc~ | d\n",
        "a ~b\nc\n===\nd~ e\n",
        "**bold _italic_ bold** and ***both***\n",
    ] {
        let whole = picture(ended(answer));
        for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
            assert_eq!(cut_at(answer, &[cut]), whole, "{answer:?} cut at {cut}");
        }
    }
}

#[test]
fn an_unended_opener_holds_what_follows_it_out_of_the_record() {
    let mut markdown = Markdown::default();
    let said = read(&mut markdown, "look in ~/Projects");

    assert_eq!(drawn(&said), "look in ");
    assert!(markdown.holds_a_phrase());
    assert_eq!(
        markdown.striking(ROOM).map(|row| row.text()),
        Some("~/Projects".to_owned())
    );

    let mut rest = Vec::new();
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        rest.push((slot, text.to_owned()));
    });
    assert_eq!(drawn(&rest), "~/Projects");
    assert!(!markdown.holds_a_phrase());
    assert!(markdown.striking(ROOM).is_none());
}

#[test]
fn the_held_phrase_is_shown_by_its_last_line_clipped_to_the_room() {
    let mut markdown = Markdown::default();
    read(&mut markdown, "an opener ~here\nand a clo");

    assert_eq!(
        markdown.striking(ROOM).map(|row| row.text()),
        Some("and a clo".to_owned())
    );
    assert_eq!(
        markdown.striking(5).map(|row| width::columns(&row.text())),
        Some(5)
    );

    read(&mut markdown, "ser~ on the next line\n");
    assert!(!markdown.holds_a_phrase());
    assert!(markdown.striking(ROOM).is_none());
}

#[test]
fn the_held_phrase_is_shown_with_no_control_character_in_it() {
    let mut markdown = Markdown::default();
    read(&mut markdown, "a ~b \u{1b}[31mred\u{7} \u{202e}c");

    let shown = markdown
        .striking(ROOM)
        .map(|row| row.text())
        .unwrap_or_default();
    assert!(
        !shown.contains(['\u{1b}', '\u{7}', '\u{202e}']),
        "{shown:?}"
    );
}

#[test]
fn a_held_phrase_lets_go_once_the_bound_is_reached() {
    let mut markdown = Markdown::default();
    let opened = read(&mut markdown, "a ~");
    let held = read(&mut markdown, &"x".repeat(STRIKE_CAP - 1));
    assert_eq!(drawn(&opened) + &drawn(&held), "a ");
    assert!(markdown.holds_a_phrase());

    let let_go = read(&mut markdown, "xy");
    assert!(!markdown.holds_a_phrase());
    assert_eq!(drawn(&let_go), format!("~{}y", "x".repeat(STRIKE_CAP)));
}

#[test]
fn weight_and_emphasis_together_show_both() {
    assert_eq!(
        slots(&whole("**bold _italic_ bold**")),
        [Slot::Bold, Slot::BoldEmphasis, Slot::Bold]
    );
    assert_eq!(slots(&whole("***both***")), [Slot::BoldEmphasis]);
    assert_eq!(
        slots(&whole("*leant **and raised** again*")),
        [Slot::Emphasis, Slot::BoldEmphasis, Slot::Emphasis]
    );
    // Nothing else stacks: a strike and code still outrank both.
    assert_eq!(slots(&whole("***~~both~~***")), [Slot::Struck]);
    assert_eq!(
        slots(&whole("a ***`both`*** b")),
        [Slot::Plain, Slot::Code, Slot::Plain]
    );
}
