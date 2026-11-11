//! A backslash before a line break and the named and numeric references that
//! stand for a character: what the reader draws for them, and what it leaves
//! as written.

use super::*;

/// What is left standing of an answer read whole and ended.
fn shown(answer: &str) -> String {
    standing_text(&drawn(&ended(answer)))
}

/// What an answer draws and the slots it draws under, cut at `cut` into two
/// deltas and ended.
fn cut_at(answer: &str, cut: usize) -> (String, Vec<Slot>) {
    let (first, second) = answer.split_at(cut);
    let said = standing(ended_in(&[first, second]));
    (drawn(&said), slots(&said))
}

/// Holds `answer` to drawing what it draws whole, wherever it is cut in two.
fn drawn_the_same_cut_anywhere(answer: &str) {
    let whole = standing(ended(answer));
    for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
        assert_eq!(
            cut_at(answer, cut),
            (drawn(&whole), slots(&whole)),
            "{answer:?} cut at {cut}"
        );
    }
}

/// Every character of `answer` as a delta of its own, ended.
fn drawn_a_character_at_a_time(answer: &str) -> String {
    let pieces: Vec<String> = answer.chars().map(String::from).collect();
    let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
    standing_text(&drawn(&ended_in(&pieces)))
}

#[test]
fn a_backslash_hard_break_draws_no_backslash() {
    for (answer, drawn_text) in [
        ("first line\\\nsecond line\n", "first line\nsecond line\n"),
        ("first line\\\nsecond line", "first line\nsecond line"),
        ("a\\\nb\\\nc\n", "a\nb\nc\n"),
        (
            "first line\\\n   second line\n",
            "first line\n   second line\n",
        ),
        ("- item\\\n  goes on\n", "• item\n  goes on\n"),
        ("> quoted\\\n> goes on\n", "│ quoted\n│ goes on\n"),
        ("*lean\\\nstill*\n", "lean\nstill\n"),
    ] {
        assert_eq!(shown(answer), drawn_text, "{answer:?}");
    }
}

#[test]
fn a_backslash_that_breaks_no_line_is_kept() {
    for answer in [
        // The message ends on it, or on its break.
        "first line\\",
        "first line\\\n",
        // A blank line follows, one with spaces in it among them.
        "first line\\\n\nsecond line\n",
        "first line\\\n   \nsecond line\n",
        // A heading, code and a span are not prose a break is read in.
        "# heading\\\nsecond line\n",
        "`code\\\nspan`\n",
        "```\ncode\\\nmore\n```\n",
        "\n\n    code\\\n    more\n",
    ] {
        let said = shown(answer);
        assert!(
            said.contains('\\'),
            "{answer:?} was drawn without its backslash as {said:?}"
        );
    }
}

#[test]
fn a_backslash_inside_display_math_is_kept() {
    let (blocks, _) = blocks("$$\nx\\\ny\n$$\n");
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.source.as_str())
            .collect::<Vec<_>>(),
        ["$$\nx\\\ny\n$$\n"]
    );
}

#[test]
fn a_backslash_before_a_blank_line_stays_where_it_was() {
    assert_eq!(
        shown("first line\\\n\nsecond line\n"),
        "first line\\\n\nsecond line\n"
    );
    assert_eq!(shown("first line\\\n"), "first line\\\n");
}

#[test]
fn a_backslash_hard_break_is_the_same_cut_anywhere() {
    for answer in [
        "first line\\\nsecond line\n",
        "first line\\\n   second line\n",
        "first line\\\n\nsecond line\n",
        "first line\\\n   \nsecond line\n",
        "first line\\\n",
        "- item\\\n  goes on\n",
        "> quoted\\\n> goes on\n",
        "a\\\nb\\\nc",
    ] {
        drawn_the_same_cut_anywhere(answer);
        assert_eq!(
            drawn_a_character_at_a_time(answer),
            shown(answer),
            "{answer:?}"
        );
    }
}

/// What a whole answer says in the entities below, the way it is written and
/// the way it is drawn.
const ENTITIES: [(&str, &str); 7] = [
    ("a &amp; b", "a & b"),
    ("&lt;tag&gt; &quot;q&quot; &apos;s&apos;", "<tag> \"q\" 's'"),
    (
        "&copy; &reg; &trade; &hellip; &mdash; &ndash;",
        "© ® ™ … — –",
    ),
    ("&#169; &#x41; &#X41; &#0000065;", "© A A A"),
    (
        "&larr;&rarr;&uarr;&darr;&harr;&rArr;&lArr;&hArr;",
        "←→↑↓↔⇒⇐⇔",
    ),
    (
        "&ne; &le; &ge; &infin; &check; &deg; &plusmn;",
        "≠ ≤ ≥ ∞ ✓ ° ±",
    ),
    ("&euro;1 &pound;2 &yen;3 &cent;4", "€1 £2 ¥3 ¢4"),
];

#[test]
fn entities_are_the_characters_they_name() {
    for (written, drawn_text) in ENTITIES {
        assert_eq!(shown(written), drawn_text, "{written:?}");
    }
    assert_eq!(
        shown("a &amp; b &lt;tag&gt; &copy; &#169; &#x41;\n"),
        "a & b <tag> © © A\n"
    );
}

#[test]
fn an_entity_is_drawn_where_a_heading_a_quote_a_item_and_a_link_say_it() {
    for (written, drawn_text) in [
        ("# a &amp; b\n", "a & b\n"),
        ("> a &amp; b\n", "│ a & b\n"),
        ("- a &amp; b\n", "• a & b\n"),
        ("*a &amp; b*\n", "a & b\n"),
        ("[a &amp; b](https://x.test)\n", "a & b\n"),
        ("a &amp; b\n---\n", "a & b\n"),
    ] {
        let said = drawn(&standing(ended(written)));
        assert!(
            said.starts_with(drawn_text),
            "{written:?} drew {said:?}, not {drawn_text:?}"
        );
    }
}

#[test]
fn what_an_entity_names_is_text_and_is_never_read_again() {
    // `&#42;` emphasises nothing, so the stars that follow it stand alone.
    let said = ended("&#42;a&#42; and *b*");
    assert_eq!(shown("&#42;a&#42; and *b*"), "*a* and b");
    assert_eq!(
        slots(&said),
        [Slot::Plain, Slot::Emphasis],
        "only the stars written as stars lean: {said:?}"
    );

    for (written, drawn_text) in [
        // A tag the entities spell opens nothing.
        ("&lt;details&gt;\ninside\n", "<details>\ninside\n"),
        ("&lt;br&gt;x", "<br>x"),
        ("&lt;kbd&gt;K&lt;/kbd&gt;", "<kbd>K</kbd>"),
        // A heading's hash, a bullet and a quote mark do not start a block.
        ("&#35; not a heading\n", "# not a heading\n"),
        ("&#45; not an item\n", "- not an item\n"),
        ("&gt; not a quote\n", "> not a quote\n"),
        // Nor does one tie a reference to its definition.
        (
            "[x]: https://x.test\n&#91;x&#93; and [x]\n",
            "https://x.test\n[x] and x\n",
        ),
        // Nor a code span is opened by the marks that make one.
        ("&#96;not code&#96;", "`not code`"),
    ] {
        let said = ended(written);
        assert_eq!(shown(written), drawn_text, "{written:?}");
        let first = usize::from(written.starts_with("[x]"));
        assert!(
            said.iter()
                .skip(first)
                .filter(|(_, text)| !text.is_empty())
                .all(|(slot, _)| matches!(slot, Slot::Plain | Slot::Link)),
            "{written:?} was read again: {said:?}"
        );
    }
}

#[test]
fn an_entity_inside_code_stays_as_written() {
    for answer in [
        "`&amp;` stays\n",
        "``a &lt; b`` stays\n",
        "```\n&copy; &#169;\n```\n",
        "~~~rust\n&amp;\n~~~\n",
        "\n    &amp; &#x41;\n",
        "<kbd>&amp; &lt;</kbd>\n",
    ] {
        let said = shown(answer);
        for entity in ["&amp;", "&lt;", "&copy;", "&#169;", "&#x41;"] {
            assert_eq!(
                answer.contains(entity),
                said.contains(entity),
                "{answer:?} drew {said:?}"
            );
        }
    }
    assert_eq!(shown("`&amp;` stays\n"), "&amp; stays\n");
}

#[test]
fn an_entity_that_is_not_one_is_shown_as_written() {
    for written in [
        // No closing `;`, or a space where one belongs.
        "&amp",
        "&amp and",
        "&copy 2026",
        "& amp;",
        "a & b",
        "AT&T",
        "R&D;",
        // A name the table does not hold, and one in the wrong case.
        "&bogus;",
        "&AMP;",
        "&Copy;",
        "&frac12;",
        // A reference that holds no digit, or the wrong kind.
        "&#;",
        "&#x;",
        "&#xg1;",
        "&#12a;",
        "&#-1;",
        // More digits than a reference may hold.
        "&#00000065;",
        "&#x00000041;",
    ] {
        assert_eq!(shown(written), written, "{written:?}");
    }
}

#[test]
fn a_reference_that_names_what_no_screen_draws_as_itself_is_shown_as_written() {
    for written in [
        // Nothing, a control character, an escape and a bell.
        "&#0;",
        "&#x0;",
        "&#7;",
        "&#27;[31m",
        "&#x1b;",
        "&#x1B;]0;t",
        "&#127;",
        "&#x85;",
        // A format character: a bidirectional override, a zero-width space, a
        // byte order mark and a soft hyphen.
        "&#x202E;",
        "&#8238;",
        "&#x200B;",
        "&#xFEFF;",
        "&#173;",
        // A surrogate and a value past the last scalar.
        "&#xD800;",
        "&#55296;",
        "&#x110000;",
        "&#1114112;",
        // The character a picture is placed with.
        "&#x10EEEE;",
        "&#1109742;",
    ] {
        assert_eq!(shown(written), written, "{written:?}");
    }
}

#[test]
fn an_entity_is_the_same_cut_anywhere() {
    for answer in [
        "a &amp; b &lt;tag&gt; &copy; &#169; &#x41;\n",
        "&co",
        "&copy",
        "&#x41;",
        "&#4",
        "&bogus; &#0; &#x202E; &amp",
        "# &amp; heading\n",
        "- &lt;a&gt; item\n> &gt; quote\n",
        "[a &amp; b](https://x.test) and *&#42;*\n",
        "`&amp;` &amp; ```&amp;```\n",
    ] {
        drawn_the_same_cut_anywhere(answer);
        assert_eq!(
            drawn_a_character_at_a_time(answer),
            shown(answer),
            "{answer:?}"
        );
    }
}

#[test]
fn a_held_ampersand_is_given_up_after_the_longest_name_a_number_or_a_space() {
    // Not one of them is waiting for a `;` that may never come: the reader has
    // drawn all of it before the line ends, or the answer does.
    for delta in [
        "&copyyyyy",
        "&hellipx",
        "&nosuchx",
        "& ",
        "x & y",
        "&#12345678 ",
        "&#x12345678 ",
    ] {
        let said = read(&mut Markdown::default(), delta);
        assert_eq!(drawn(&said), delta, "{delta:?} was held back as {said:?}");
    }
}

#[test]
fn a_decoded_entity_wears_the_slot_of_the_words_around_it() {
    let said = ended("**a &amp; b** and ~~&lt;c&gt;~~\n");
    assert_eq!(
        shown("**a &amp; b** and ~~&lt;c&gt;~~\n"),
        "a & b and <c>\n"
    );
    assert_eq!(
        slots(&said),
        [Slot::Bold, Slot::Plain, Slot::Struck, Slot::Plain]
    );
}

#[test]
fn the_longest_name_bound_is_the_longest_name_in_the_table() {
    assert_eq!(
        super::super::escapes::NAMED
            .iter()
            .map(|(name, _)| name.len())
            .max(),
        Some(super::super::escapes::LONGEST)
    );
}

#[test]
fn an_entity_is_drawn_in_a_table_cell() {
    let said = shown("| a | b |\n|---|---|\n| &amp; | &lt; |\n");
    assert!(said.contains('&') && said.contains('<'), "{said:?}");
    assert!(
        !said.contains("&amp;") && !said.contains("&lt;"),
        "{said:?}"
    );
}
