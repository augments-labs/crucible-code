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
        // A line that only starts like a block is more of the paragraph.
        ("first\\\n#hashtag\n", "first\n#hashtag\n"),
        ("first\\\n-5 degrees\n", "first\n-5 degrees\n"),
        ("first\\\n1.5 times\n", "first\n1.5 times\n"),
        ("first\\\n12)maybe\n", "first\n12)maybe\n"),
        ("first\\\n**bold** word\n", "first\nbold word\n"),
        ("first\\\n$5 and $6\n", "first\n$5 and $6\n"),
        ("first\\\n>no space\n", "first\n>no space\n"),
        (
            "first\\\n<b>not a block</b>\n",
            "first\n<b>not a block</b>\n",
        ),
        ("first\\\n`span` here\n", "first\nspan here\n"),
        ("first\\\n~~gone~~ here\n", "first\ngone here\n"),
    ] {
        assert_eq!(shown(answer), drawn_text, "{answer:?}");
    }
}

/// A first line and the line under it that opens a block of its own, so that a
/// backslash at the end of the first is no break.
const OPENERS: [(&str, &str, &str); 17] = [
    ("first", "first", "# heading\n"),
    ("first", "first", "## heading\n"),
    ("first", "first", "- next\n"),
    ("first", "first", "* next\n"),
    ("first", "first", "+ next\n"),
    ("- item", "item", "- next\n"),
    ("1. one", "one", "2. two\n"),
    ("first", "first", "  - nested\n"),
    ("first", "first", "```\ncode\n```\n"),
    ("first", "first", "~~~\ncode\n~~~\n"),
    ("first", "first", "> quote\n"),
    ("first", "first", "***\n"),
    ("first", "first", "___\n"),
    ("first", "first", "| a | b |\n|---|---|\n| 1 | 2 |\n"),
    ("first", "first", "$$\nx\n$$\n"),
    ("first", "first", "\\[\nx\n\\]\n"),
    (
        "first",
        "first",
        "<details>\n<summary>s</summary>\nbody\n</details>\n",
    ),
];

#[test]
fn a_backslash_before_a_line_that_opens_a_block_is_kept_and_the_block_opens() {
    for (head, word, opener) in OPENERS {
        let with = format!("{head}\\\n{opener}");
        let without = format!("{head}\n{opener}");
        let kept = format!("{word}\\");
        let (said, plain) = (ended(&with), ended(&without));
        assert_eq!(
            standing_text(&drawn(&said)),
            standing_text(&drawn(&plain)).replacen(word, &kept, 1),
            "{with:?} drew the block differently"
        );
        assert_eq!(slots(&standing(said)), slots(&standing(plain)), "{with:?}");
    }
    // A row of `-` or `=` under a line says what it was a title of, and the
    // backslash stays in the title.
    for under in ["---", "===", "-", "="] {
        let said = shown(&format!("first\\\n{under}\n"));
        assert!(said.starts_with("first\\"), "{under:?} drew {said:?}");
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
        // The line under it opens a block of its own.
        "first\\\n# heading\n",
        "- item\\\n- next\n",
        "1. one\\\n2. two\n",
        "first\\\n```\ncode\n```\n",
        "first\\\n> quote\n",
        "first\\\n***\n",
        "first\\\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        "first\\\n$$\nx\n$$\n",
        "first\\\n<details>\n<summary>s</summary>\nb\n</details>\n",
    ] {
        let said = shown(answer);
        assert!(
            said.contains('\\'),
            "{answer:?} was drawn without its backslash as {said:?}"
        );
    }
}

#[test]
fn a_backslash_before_a_line_the_answer_ends_in_the_middle_of_is_drawn_as_the_line_is() {
    // The break and what stood after it were held for the line to say what it
    // opens, and the answer's end is the line saying it opens nothing.
    assert_eq!(shown("first\\\n#"), "first\n#");
    assert!(shown("first\\\n-").starts_with("first\\"), "an underline");
    // A number and its stop are the item's, and the end is the blank after.
    assert_eq!(shown("first\\\n1."), "first\\\n1.");
    assert_eq!(shown("first\\\n1.5"), "first\n1.5");
    // A block of bars is a table with its delimiter row, whole.
    assert_eq!(
        shown("first\\\n| a | b |\n|---|---|"),
        "first\n| a | b |\n|---|---|"
    );
    assert_eq!(shown("first\\\n<det"), "first\n<det");
    // A row of dashes or bars under a bare header is a delimiter row only where
    // the reader opens a table, and at the end of the answer it opens none.
    for row in ["---|---", "|---|---|", "-:|-:"] {
        assert_eq!(
            shown(&format!("a | b\\\n{row}")),
            format!("a | b\n{row}"),
            "{row:?}"
        );
    }
    assert_eq!(shown("first\\\n  "), "first\\\n  ");
}

/// What a block is shown as, where it is told again from what it was written
/// with.
fn told(block: &Block) -> String {
    let mut out = String::new();
    tell(
        block,
        Room {
            columns: ROOM,
            glyphs: Glyphs::Unicode,
        },
        &Links::default(),
        &Drawers::default(),
        &mut |_, text, _| {
            out.push_str(text);
        },
    );
    out
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
    // And it is drawn, when the block is shown as written: the break under it
    // is no more of a paragraph, and the backslash is not a line break.
    let shown: Vec<String> = blocks.iter().map(told).collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown.iter().all(|text| text.contains("x\\\ny")),
        "the backslash was lost from {shown:?}"
    );
}

#[test]
fn an_entity_inside_display_math_is_kept_as_written() {
    for source in [
        "$$\na &lt; b &amp; c\n$$\n",
        "\\[\n&#169; &#x41;\n\\]\n",
        "$$\n&amp\n$$\n",
    ] {
        let (blocks, _) = blocks(source);
        let shown: Vec<String> = blocks.iter().map(told).collect();
        assert_eq!(shown.len(), 1, "{source:?} {shown:?}");
        for entity in ["&lt;", "&amp;", "&#169;", "&#x41;", "&amp"] {
            assert_eq!(
                source.contains(entity),
                shown.iter().any(|text| text.contains(entity)),
                "{source:?} was shown as {shown:?}"
            );
        }
    }
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
        "first\\\n# heading\n",
        "first\\\n#hashtag\n",
        "- item\\\n- next\n",
        "1. one\\\n2. two\n",
        "first\\\n12) maybe\n",
        "first\\\n```\ncode\n```\n",
        "first\\\n~~~\ncode\n~~~\n",
        "first\\\n> quote\n",
        "first\\\n***\n",
        "first\\\n*****************************\n",
        "first\\\n***bold***\n",
        "first\\\n---\n",
        "first\\\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        "first\\\n$$\nx\n$$\n",
        "first\\\n\\[\nx\n\\]\n",
        "first\\\n\\begin{matrix}\nx\n\\end{matrix}\n",
        "first\\\n<details>\n<summary>s</summary>\nb\n</details>\n",
        "first\\\n   <summary>s</summary>\n",
        "first\\\n#",
        "first\\\n-",
    ] {
        drawn_the_same_cut_anywhere(answer);
        assert_eq!(
            drawn_a_character_at_a_time(answer),
            shown(answer),
            "{answer:?}"
        );
    }
}

/// A line and what is written under it, and whether the reader opens a block
/// of its own there, which is where a backslash at the end of the first line
/// stays. A line it goes on with, and a line that only starts like a block, is
/// more of the paragraph.
///
/// What the reader does is read off the answer without the backslash: the
/// bullet, the number, the heading, the rule, the quote, the fence, the table,
/// the display math and the `<details>` are drawn or handed on as what they
/// are, or the line above becomes a title; and the same answer with the
/// backslash must draw the same, the backslash kept where the reader opened
/// something and gone where it did not.
const AGREEMENT: [(&str, &str, bool); 63] = [
    // A numbered item: a stop and a blank or the end of the line after it.
    ("first", "1. x\n", true),
    ("first", "1) x\n", true),
    ("first", "1.\tx\n", true),
    ("first", "1.\n", true),
    ("first", "1)\n", true),
    ("first", "123456789. x\n", true),
    ("- a", "1.\n", true),
    ("first", "1.5 times\n", false),
    ("first", "12)maybe\n", false),
    ("first", "1.x\n", false),
    ("first", "1234567890. x\n", false),
    // A bullet and a heading, at the margin or after spaces, and not after a
    // tab.
    ("first", "- x\n", true),
    ("first", "* x\n", true),
    ("first", "+ x\n", true),
    ("first", "   - x\n", true),
    ("first", "    - x\n", true),
    ("first", "    1. x\n", true),
    ("first", "     ---\n", true),
    ("first", "\t- x\n", false),
    ("first", "# h\n", true),
    ("first", "   # h\n", true),
    ("first", "\t# h\n", false),
    ("first", "#h\n", false),
    ("first", "-x\n", false),
    // An underline says what the first line is the title of, which a list
    // item is not.
    ("first", "-\n", true),
    ("first", "=\n", true),
    ("first", "---\n", true),
    ("- a", "-\n", false),
    ("- a", "- b\n", true),
    // A row of bars is the start of a table only with its delimiter row.
    ("first", "|\n", false),
    ("first", "| not a table\n", false),
    ("first", "|a|b|\n", false),
    ("first", "| a | b |\n|---|---|\n| 1 | 2 |\n", true),
    ("a | b", "| c | d\n", false),
    ("a | b", "|---|---|\n", true),
    ("a | b", "---|---\n", true),
    ("a | b", "---|-x\n", false),
    ("first", "-|-\n", false),
    ("first", "-:\n", false),
    ("first", "--:\n", false),
    ("a | b", "-:|-:\n", true),
    ("a | b", "- x\n", true),
    ("a | b", "1.\n", true),
    ("> q", "| a | b |\n|---|---|\n", true),
    ("- a", "| a | b |\n|---|---|\n", true),
    ("- a", "  | a | b |\n  |---|---|\n", false),
    // A quote, a fence, a rule, display math and the tags of a block.
    ("first", "> q\n", true),
    ("first", ">q\n", false),
    ("first", "```\ncode\n```\n", true),
    ("first", "~~~\ncode\n~~~\n", true),
    ("first", "***\n", true),
    ("first", "___\n", true),
    ("- a", "---\n", true),
    ("first", "***  \n", false),
    ("first", "---  \n", true),
    ("first", "* * *\n", true),
    ("first", "$$\nx\n$$\n", true),
    ("first", "$5 and $6\n", false),
    (
        "first",
        "<details>\n<summary>s</summary>\nb\n</details>\n",
        true,
    ),
    ("first", "<details open>\n", true),
    ("first", "<b>not a block</b>\n", false),
    // Ordinary prose.
    ("first", "second line\n", false),
    ("first", "**bold** word\n", false),
];

/// Whether the reader opened something of its own at `line`, which is written
/// under `head` and has no backslash in front of it: a block handed on, a run
/// drawn in a slot other than the plain one, or a line drawn as other than it
/// was written, as a swallowed tag or a blank line is, beyond what `head` is
/// by itself.
fn opened_something(head: &str, line: &str) -> bool {
    let (alone, under) = (format!("{head}\n"), format!("{head}\n{line}"));
    let marked = |answer: &str| {
        standing(ended(answer))
            .iter()
            .filter(|(slot, _)| *slot != Slot::Plain)
            .count()
    };
    blocks(&under).0.len() > blocks(&alone).0.len()
        || marked(&under) > marked(&alone)
        || drawn(&standing(ended(&under))).strip_prefix(&drawn(&standing(ended(&alone))))
            != Some(line)
}

/// What `head`, a backslash, a break and `line` draw, held to what the reader
/// does with the same lines without the backslash. `label` is what
/// [`AGREEMENT`] says for it, and is believed only where the reader bears it
/// out: a line the reader opens nothing at, however it is written, is more of
/// the paragraph. `Some` says what is wrong.
fn disagrees(head: &str, line: &str, label: bool) -> Option<String> {
    let with = format!("{head}\\\n{line}");
    let without = format!("{head}\n{line}");
    let opens = label && opened_something(head, line);
    // A line that is listed as one the reader opens a block at must be one.
    // The answer ended in the middle of it is another line, which the end of
    // the answer may leave without a block, and is held to the reader as well.
    if label && line.ends_with('\n') && !opens {
        return Some(format!(
            "{without:?} is listed as a block, the reader opened none"
        ));
    }
    let plain = drawn(&standing(ended(&without)));
    let said = drawn(&standing(ended(&with)));
    // Where the reader opened a block the backslash stays among what is drawn,
    // in the line it ended or in the title it is part of, and nothing else
    // changes but the width of a rule under that title; where it did not,
    // nothing of it is drawn.
    let bare = |text: &str| -> String {
        text.chars()
            .filter(|c| !matches!(c, '\\' | '\u{2500}' | '\u{2501}' | '\u{253c}'))
            .collect()
    };
    let agrees = if opens {
        said.contains('\\') && bare(&said) == bare(&plain)
    } else {
        said == plain
    };
    if !agrees {
        return Some(format!("{with:?} opens a block: {opens}, drew {said:?}"));
    }
    if slots(&standing(ended(&with))) != slots(&standing(ended(&without))) {
        return Some(format!("{with:?} drew other slots than {without:?}"));
    }
    None
}

#[test]
fn a_backslash_is_kept_exactly_where_the_reader_opens_a_block() {
    let mut misses = Vec::new();
    for (head, line, opens) in AGREEMENT {
        // The line whole, and the answer ended in the middle of it, which is
        // the break that ends it.
        misses.extend(disagrees(head, line, opens));
        let open = line.trim_end_matches('\n');
        if open != line && !open.is_empty() {
            misses.extend(disagrees(head, open, opens));
        }

        // Cut anywhere, between the backslash and its break and after it among
        // them, and a character at a time, it is drawn as whole.
        let with = format!("{head}\\\n{line}");
        drawn_the_same_cut_anywhere(&with);
        assert_eq!(
            drawn_a_character_at_a_time(&with),
            drawn(&standing(ended(&with))),
            "{with:?}"
        );
        let trimmed = with.trim_end_matches('\n');
        if trimmed.len() > head.len() + 2 {
            drawn_the_same_cut_anywhere(trimmed);
        }
    }
    assert!(misses.is_empty(), "{misses:#?}");
}

#[test]
fn a_backslash_hold_ends_where_the_reader_ends_what_it_reads() {
    // A line of bars is read for a table no longer than a table is, a header
    // no longer than a header is, and no other line is held past its start.
    for (answer, held) in [
        (format!("first\\\n|{}", "x".repeat(20_000)), false),
        (format!("a | b\\\n{}", "-|".repeat(100)), false),
        (format!("first\\\n{}", "x".repeat(200)), false),
        (format!("first\\\n{}", "- ".repeat(5)), false),
        ("first\\\n|a|b|".to_owned(), true),
        ("a | b\\\n--".to_owned(), true),
        ("first\\\n<details open".to_owned(), true),
        (format!("first\\\n<details {}", "x".repeat(80)), false),
    ] {
        let mut markdown = Markdown::default();
        read(&mut markdown, &answer);
        assert_eq!(markdown.slashed.is_some(), held, "{:?}", &answer[..20]);
    }
    let long = format!("first\\\n|{}\nsecond", "x".repeat(9_000));
    let plain = format!("first\n|{}\nsecond", "x".repeat(9_000));
    assert_eq!(shown(&long), plain);
    assert_eq!(drawn_a_character_at_a_time(&long), plain);
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
        ("[a &amp; b](https://x.test)\n", "a & b (x.test)\n"),
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
            "https://x.test\n[x] and x (x.test)\n",
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
                // The quiet address after a link's words is the one other run.
                .all(|(slot, _)| matches!(slot, Slot::Plain | Slot::Link | Slot::Quiet)),
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
        "$$\na &lt; b &amp; c &copy; &#169;\n$$\n",
        "\\[\n&#x41;\n\\]\n",
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

/// One answer read against a repository, cut into these deltas, with where
/// each run points.
fn counted_in(deltas: &[&str]) -> Vec<(Slot, String, Option<String>)> {
    let mut markdown = Markdown::default().counting(Some(forge()));
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    for delta in deltas {
        markdown.read_runs(delta, ROOM, &mut into);
    }
    markdown.finish_runs(ROOM, &mut into);
    said
}

/// What `said` points at: the words of each run that carries an address, and
/// the address, with the runs of one address put together.
fn pointed_at(said: &[(Slot, String, Option<String>)]) -> Vec<(String, String)> {
    let mut points: Vec<(String, String)> = Vec::new();
    for (_, text, link) in said {
        let Some(link) = link else { continue };
        match points.last_mut() {
            Some((words, last)) if last == link => words.push_str(text),
            _ => points.push((text.clone(), link.clone())),
        }
    }
    points
}

#[test]
fn a_number_sign_after_an_ampersand_that_is_no_entity_still_names_an_issue() {
    let issue = "https://github.com/augments-labs/crucible-code/issues/12";
    for (answer, points) in [
        ("see &#12 now\n", vec![("#12", issue)]),
        ("see &#12\n", vec![("#12", issue)]),
        ("&#12", vec![("#12", issue)]),
        ("see &#12, now\n", vec![("#12", issue)]),
        ("a &#12a b\n", vec![("#12", issue)]),
        ("a &#x1F b\n", vec![]),
    ] {
        let whole = counted_in(&[answer]);
        assert_eq!(
            wrote(&whole),
            answer,
            "{answer:?} was not drawn as written: {whole:?}"
        );
        let points: Vec<(String, String)> = points
            .into_iter()
            .map(|(words, address)| (words.to_owned(), address.to_owned()))
            .collect();
        assert_eq!(pointed_at(&whole), points, "{answer:?}: {whole:?}");

        for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
            let (first, second) = answer.split_at(cut);
            let said = counted_in(&[first, second]);
            assert_eq!(wrote(&said), wrote(&whole), "{answer:?} cut at {cut}");
            assert_eq!(pointed_at(&said), points, "{answer:?} cut at {cut}");
        }
    }
}

#[test]
fn an_entity_inside_a_sub_or_a_sup_run_is_drawn_as_the_character_it_names() {
    for (answer, drawn_text) in [
        ("x<sup>&#50;</sup>\n", "x²\n"),
        ("x<sub>&#x32;</sub>\n", "x₂\n"),
        ("a<sup>&#49;&#48;</sup>b\n", "a¹⁰b\n"),
        ("a <sup>&lt;b&gt;</sup>\n", "a <b>\n"),
        ("a <sub>&#27;[31m</sub>\n", "a &#27;[31m\n"),
        ("a <sub>&amp</sub>\n", "a &amp\n"),
    ] {
        assert_eq!(shown(answer), drawn_text, "{answer:?}");
        drawn_the_same_cut_anywhere(answer);
    }
}
