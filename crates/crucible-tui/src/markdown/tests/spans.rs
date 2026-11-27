//! A span of code: opened by a run of backticks in prose and closed only by a
//! run of exactly as many, each end of what it holds giving up one space where
//! both have one, and the run that opened it the backticks it was where
//! nothing closes it. Read on the held path and the streamed one, and however
//! the answer is cut into deltas.

use super::*;

/// What `answer` says once it has ended, read whole, with what was taken back
/// gone and the runs beside each other in one slot joined; and the same answer
/// cut in two at every byte a cut can fall on, each of which must say the same.
fn spoken(answer: &str) -> Vec<(Slot, String)> {
    let whole = joined(standing(ended(answer)));
    for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
        let (first, second) = answer.split_at(cut);
        assert_eq!(
            joined(standing(ended_in(&[first, second]))),
            whole,
            "{answer:?} cut at {cut}"
        );
    }
    whole
}

/// What `answer` draws once it has ended, read whole; and the same answer cut
/// in two at every byte a cut can fall on, each of which must draw the same.
fn streamed(answer: &str) -> String {
    let whole = standing_text(&drawn(&ended(answer)));
    for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
        let (first, second) = answer.split_at(cut);
        assert_eq!(
            standing_text(&drawn(&ended_in(&[first, second]))),
            whole,
            "{answer:?} cut at {cut}"
        );
    }
    whole
}

/// `said` with every run joined to the one before it where both wear one
/// slot, and the empty ones left out: where a run is cut says nothing.
fn joined(said: Vec<(Slot, String)>) -> Vec<(Slot, String)> {
    let mut joined: Vec<(Slot, String)> = Vec::new();
    for (slot, text) in said {
        match joined.last_mut() {
            _ if text.is_empty() => {}
            Some((last, words)) if *last == slot => words.push_str(&text),
            _ => joined.push((slot, text)),
        }
    }
    joined
}

fn plain(text: &str) -> (Slot, String) {
    (Slot::Plain, text.to_owned())
}

fn code(text: &str) -> (Slot, String) {
    (Slot::Code, text.to_owned())
}

#[test]
fn a_span_closes_only_on_a_run_as_long_as_the_one_that_opened_it() {
    assert_eq!(
        spoken("Inline: ``a `tick` inside`` and `` ` `` alone."),
        vec![
            plain("Inline: "),
            code("a `tick` inside"),
            plain(" and "),
            code("`"),
            plain(" alone."),
        ]
    );
    // A longer run is no more a closer than a shorter one.
    assert_eq!(
        spoken("x `a``b` y"),
        vec![plain("x "), code("a``b"), plain(" y")]
    );
}

#[test]
fn a_run_of_backticks_that_opened_no_block_opens_a_span_as_long() {
    // The item in front of it is the item it would be in front of any span.
    let mut lead = spoken("- a");
    lead.pop();
    lead.extend([code("``a `tick` inside``"), plain(" came out as text.")]);

    assert_eq!(
        spoken("- ``` ``a `tick` inside`` ``` came out as text."),
        lead
    );
}

#[test]
fn one_space_comes_off_each_end_of_a_span_where_both_have_one() {
    for (answer, held) in [
        ("x `` `a` `` y", "`a`"),
        ("x ` a ` y", "a"),
        ("x `  a  ` y", " a "),
        ("x `a ` y", "a "),
        // Nothing but spaces stays as it was.
        ("x ` ` y", " "),
        ("x `  ` y", "  "),
    ] {
        assert_eq!(
            spoken(answer),
            vec![plain("x "), code(held), plain(" y")],
            "{answer:?}"
        );
    }
}

#[test]
fn a_run_nothing_closes_is_the_backticks_it_was() {
    for answer in ["a ``b` c", "a `b", "``", "x `a\nb` y"] {
        assert_eq!(spoken(answer), vec![plain(answer)], "{answer:?}");
    }
    // What comes after it is read as it would have been, a later span too.
    assert_eq!(
        spoken("``a `b` c"),
        vec![plain("``a "), code("b"), plain(" c")]
    );
    assert_eq!(spoken("``a`` `b"), vec![code("a"), plain(" `b")]);
    // A backslash is a character in a span like any other.
    assert_eq!(
        spoken("x `a\\`b` y"),
        vec![plain("x "), code("a\\"), plain("b` y")]
    );
    // And one a span ends its line on is drawn, not read as a break.
    assert_eq!(streamed("`code\\\nspan`\n"), "`code\\\nspan`\n");
}

#[test]
fn a_span_longer_than_its_bound_is_the_backticks_it_was() {
    let answer = format!("`{}` b", "a".repeat(MOST));

    assert_eq!(joined(ended(&answer)), vec![plain(&answer)]);
}

#[test]
fn a_span_in_a_block_laid_out_again_closes_where_it_does_in_prose() {
    // A table's cell is as wide as what it draws, the backtick a span holds
    // included.
    assert_eq!(
        streamed("| code |\n| --- |\n| ``a `b` c`` |\n\n"),
        "code   \n───────\na `b` c\n\n"
    );

    // So is a heading's rule, which is as wide as its words.
    let heading = streamed("# a ``b`c`` d\n");
    let mut rows = heading.lines();
    assert_eq!(rows.next(), Some("a b`c d"), "{heading:?}");
    assert_eq!(
        rows.next().map(|rule| rule.chars().count()),
        Some(7),
        "{heading:?}"
    );
}

#[test]
fn a_span_being_read_is_shown_at_the_foot_as_written() {
    let mut markdown = Markdown::default();

    let first = read(&mut markdown, "Inline: ``a `ti");
    assert_eq!(drawn(&first), "Inline: ");
    assert!(markdown.holds_a_span());
    assert_eq!(
        markdown.spanning(ROOM).map(|row| row.text()),
        Some("``a `ti".to_owned())
    );

    let second = read(&mut markdown, "ck` inside`` and");
    assert_eq!(drawn(&second), "a `tick` inside and");
    assert!(!markdown.holds_a_span());
    assert!(markdown.spanning(ROOM).is_none());
}
