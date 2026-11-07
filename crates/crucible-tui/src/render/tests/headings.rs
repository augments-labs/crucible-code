//! A line over a row of `=` or `-` is a heading, and the row is read rather
//! than drawn: what a window shows of one, in both glyph sets, at every width
//! and however the answer arrives.

use super::*;

/// The rows of `answer` at `columns` by sixty, the blank ones and the blanks
/// at the end of the rest left out.
fn drawn_in(columns: usize, glyphs: Glyphs, answer: &str) -> Vec<String> {
    let mut drawn = reading(columns, 60);
    drawn.render.draws(glyphs);
    drawn.answer(answer);
    drawn
        .rows()
        .into_iter()
        .map(|row| row.trim_end().to_owned())
        .filter(|row| !row.is_empty())
        .collect()
}

/// A rule of `width` columns in the set `glyphs` draws with: heavy for a
/// level one heading, light otherwise.
fn ruled(width: usize, heavy: bool, glyphs: Glyphs) -> String {
    let mark = match (heavy, glyphs) {
        (true, Glyphs::Unicode) => "━",
        (true, Glyphs::Ascii) => "=",
        (false, Glyphs::Unicode) => "─",
        (false, Glyphs::Ascii) => "-",
    };
    mark.repeat(width)
}

#[test]
fn setext_underlines_are_read_rather_than_drawn() {
    let mut drawn = reading(40, 12);
    drawn.answer("Title\n=====\n\nSub\n---\n\nprose\n");

    let rows = drawn.rows();
    assert!(!rows.iter().any(|row| row.contains("=====")), "{rows:#?}");
    assert!(!rows.iter().any(|row| row.contains("---")), "{rows:#?}");
    let is_rule = |row: &str| {
        let row = row.trim();
        !row.is_empty() && row.chars().all(|c| matches!(c, '─' | '━'))
    };
    let above = std::iter::once("").chain(rows.iter().map(String::as_str));
    for (over, row) in above.zip(rows.iter()) {
        if is_rule(row) {
            assert!(
                row.trim().chars().count() <= over.trim().chars().count(),
                "a rule under {over:?} is wider than its words: {rows:#?}"
            );
        }
    }
    assert!(rows.iter().any(|row| row.contains("Title")));
    assert!(rows.iter().any(|row| row.contains("Sub")));
}

#[test]
fn setext_headings_wear_a_rule_as_wide_as_their_words_in_both_sets() {
    let answer = "Title\n=====\n\nSub\n---\n\nprose\n";
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        assert_eq!(
            drawn_in(40, glyphs, answer),
            [
                "Title".to_owned(),
                ruled(5, true, glyphs),
                "Sub".to_owned(),
                ruled(3, false, glyphs),
                "prose".to_owned()
            ],
            "{glyphs:?}"
        );
    }
    // The underline's own length decides nothing.
    assert_eq!(
        drawn_in(40, Glyphs::Ascii, "Title\n==========\n"),
        ["Title", "====="]
    );
}

#[test]
fn a_lone_marker_is_an_underline() {
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "Title\n=\n"),
        ["Title", "━━━━━"]
    );
    assert_eq!(drawn_in(40, Glyphs::Unicode, "Sub\n-\n"), ["Sub", "───"]);
}

#[test]
fn atx_and_setext_headings_of_one_level_look_alike_and_levels_four_to_six_look_like_three() {
    let atx = drawn_in(40, Glyphs::Unicode, "# One\n\n## Two\n\n### Three\n");
    let setext = drawn_in(40, Glyphs::Unicode, "One\n===\n\nTwo\n---\n\nThree\n");
    assert_eq!(atx, ["One", "━━━", "Two", "───", "Three"]);
    assert_eq!(
        setext.iter().take(4).collect::<Vec<_>>(),
        atx.iter().take(4).collect::<Vec<_>>()
    );
    assert_eq!(
        drawn_in(
            40,
            Glyphs::Unicode,
            "#### Four\n\n##### Five\n\n###### Six\n"
        ),
        ["Four", "Five", "Six"]
    );
}

#[test]
fn a_dashed_row_under_something_that_is_not_a_paragraph_stays_a_full_width_rule() {
    let rule = "─".repeat(40);
    for answer in [
        "---\n",
        "above\n\n---\n",
        "- item\n---\n",
        "> quote\n---\n",
        "# Foo\n---\n",
        "```\ncode\n```\n---\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n---\n",
    ] {
        let rows = drawn_in(40, Glyphs::Unicode, answer);
        assert_eq!(rows.last(), Some(&rule), "{answer:?}: {rows:#?}");
    }
    // The same rows with `===` are prose, and nothing is made a heading.
    for answer in ["- item\n===\n", "# Foo\n===\n", "```\ncode\n```\n===\n"] {
        let rows = drawn_in(40, Glyphs::Unicode, answer);
        assert!(
            rows.iter().any(|row| row.contains("===")),
            "{answer:?}: {rows:#?}"
        );
        assert_eq!(
            rows.iter().filter(|row| row.contains('━')).count(),
            usize::from(answer.starts_with('#')),
            "{answer:?}: {rows:#?}"
        );
    }
}

#[test]
fn a_line_of_cells_over_a_delimiter_row_is_still_a_table_and_not_a_heading() {
    let rows = drawn_in(40, Glyphs::Unicode, "a | b\n--|--\n1 | 2\n");
    assert!(rows.iter().any(|row| row.contains('│')), "{rows:#?}");
    assert!(
        !rows
            .iter()
            .any(|row| row.contains('─') && !row.contains('┼')),
        "{rows:#?}"
    );
}

#[test]
fn a_dashed_row_under_a_line_that_was_no_table_header_underlines_it() {
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "one cell\n---\n"),
        ["one cell", "────────"]
    );
    let rows = drawn_in(40, Glyphs::Unicode, "a | b\n---\n1 | 2\n");
    assert_eq!(rows, ["a | b", "─────", "1 | 2"]);
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "one cell\n:-:\n"),
        ["one cell", ":-:"]
    );
}

#[test]
fn a_screen_reads_only_the_last_line_of_a_paragraph_as_the_heading() {
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "first line\nsecond line\n===\n"),
        ["first line", "second line", "━━━━━━━━━━━"]
    );
}

#[test]
fn a_screen_wraps_a_heading_with_a_rule_as_wide_as_its_last_row() {
    let rows = drawn_in(16, Glyphs::Unicode, "A title long enough to wrap\n=\n");
    assert!(rows.len() > 2, "{rows:#?}");
    let [rest @ .., words, rule] = rows.as_slice() else {
        panic!("{rows:#?}");
    };
    assert_eq!(*rule, "━".repeat(crate::width::columns(words)), "{rows:#?}");
    assert!(rest.iter().all(|row| !row.contains('━')), "{rows:#?}");
}

#[test]
fn a_heading_is_laid_out_again_at_a_new_width_and_in_a_new_glyph_set() {
    let answer = "A title long enough to wrap\n=====\n\nAnd a second one to wrap\n---\n";
    let mut drawn = reading(40, 20);
    drawn.answer(answer);
    drawn.resize(16, 20);
    let fresh = drawn_in(16, Glyphs::Unicode, answer);
    assert!(fresh.iter().any(|row| row.contains('━')), "{fresh:#?}");
    let narrowed: Vec<String> = drawn
        .rows()
        .into_iter()
        .map(|row| row.trim_end().to_owned())
        .filter(|row| !row.is_empty())
        .collect();
    assert_eq!(narrowed, fresh);

    drawn.render.draws(Glyphs::Ascii);
    drawn.render.repaint().unwrap();
    let ascii: Vec<String> = drawn
        .rows()
        .into_iter()
        .map(|row| row.trim_end().to_owned())
        .filter(|row| !row.is_empty())
        .collect();
    assert!(ascii.iter().any(|row| row.starts_with('=')), "{ascii:#?}");
    assert_eq!(ascii, drawn_in(16, Glyphs::Ascii, answer));
}

#[test]
fn an_underline_complete_at_the_end_of_the_message_counts() {
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "Title\n=====").as_slice(),
        ["Title", "━━━━━"]
    );
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "Sub\n---").as_slice(),
        ["Sub", "───"]
    );
}

#[test]
fn trailing_blanks_after_an_underline_are_part_of_it() {
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "Title\n===\t\n"),
        ["Title", "━━━━━"]
    );
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "Sub\n---\t\n"),
        ["Sub", "───"]
    );
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "Sub\n   --- \n"),
        ["Sub", "───"]
    );
}

#[test]
fn a_screen_reads_an_underline_of_a_hundred_and_twenty_eight_marks_and_not_one_more() {
    let dashes = |count: usize| "-".repeat(count);
    let equals = |count: usize| "=".repeat(count);

    assert_eq!(
        drawn_in(200, Glyphs::Unicode, &format!("Sub\n{}\n", dashes(128))),
        ["Sub", "───"]
    );
    assert_eq!(
        drawn_in(200, Glyphs::Unicode, &format!("Title\n{}\n", equals(128))),
        ["Title", "━━━━━"]
    );
    assert_eq!(
        drawn_in(200, Glyphs::Unicode, &format!("Sub\n{}\n", dashes(129))),
        ["Sub".to_owned(), "─".repeat(200)]
    );
    assert_eq!(
        drawn_in(200, Glyphs::Unicode, &format!("Title\n{}\n", equals(129))),
        ["Title".to_owned(), equals(129)]
    );
}

#[test]
fn a_screen_reads_a_title_of_a_hundred_and_twenty_eight_bytes_and_not_one_more() {
    for (underline, heavy) in [("---", false), ("===", true)] {
        let title = "t".repeat(128);
        let rows = drawn_in(200, Glyphs::Unicode, &format!("{title}\n{underline}\n"));
        assert_eq!(rows, [title.clone(), ruled(128, heavy, Glyphs::Unicode)]);

        let title = "t".repeat(129);
        let rows = drawn_in(200, Glyphs::Unicode, &format!("{title}\n{underline}\n"));
        let expected = if heavy {
            underline.to_owned()
        } else {
            "─".repeat(200)
        };
        assert_eq!(rows, [title, expected]);
    }
}

#[test]
fn a_title_of_nothing_but_markers_is_a_heading_with_a_rule_as_wide_as_the_markers() {
    assert_eq!(drawn_in(40, Glyphs::Unicode, "**\n---\n"), ["**", "──"]);
    assert_eq!(drawn_in(40, Glyphs::Unicode, "--\n---\n"), ["--", "──"]);
}

#[test]
fn an_indented_atx_heading_wears_a_rule_as_wide_as_its_words_and_not_its_indent() {
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "  # Title\nafter\n"),
        ["  Title", "━━━━━", "after"]
    );
    assert_eq!(
        drawn_in(40, Glyphs::Unicode, "   ## Sub\nafter\n"),
        ["   Sub", "───", "after"]
    );
    assert_eq!(
        drawn_in(40, Glyphs::Ascii, "  # Title"),
        ["  Title", "====="]
    );
}

#[test]
fn a_title_that_opens_with_a_dollar_or_a_backslash_is_a_heading() {
    assert_eq!(drawn_in(40, Glyphs::Unicode, "$5\n---\n"), ["$5", "──"]);
    assert_eq!(drawn_in(40, Glyphs::Unicode, "\\*x\n===\n"), ["*x", "━━"]);
}

#[test]
fn a_setext_heading_cut_anywhere_draws_what_the_whole_answer_draws() {
    for answer in [
        "intro\n\nTitle\n=====\n\nSub\n---\nafter\n",
        "intro line\nTitle with **bold**\n=\nafter\n",
        "$5\n---\nx\n\\*x\n===\nend\n",
    ] {
        let mut whole = reading(40, 30);
        whole.answer(answer);
        let expected = whole.rows();
        assert!(
            expected
                .iter()
                .any(|row| row.contains('━') || row.contains('─')),
            "{answer:?} drew no rule: {expected:#?}"
        );
        for cut in 1..answer.len() {
            let mut pieces = reading(40, 30);
            pieces.answer_in_pieces(answer, &[cut]);
            assert_eq!(pieces.rows(), expected, "cut at {cut} of {answer:?}");
        }
    }
}

#[test]
fn a_redirected_run_writes_a_title_once_and_only_when_it_is_settled() {
    let cases: [(&[&[&str]], &str); 2] = [
        (
            &[
                &["**Ti", "tle**\n=", "====\n"],
                &["**Title**", "\n", "=====\n"],
                &["**Title**\n", "=====\n"],
                &["**Title**\n=====\n"],
            ],
            "Title\n━━━━━\n",
        ),
        (
            &[
                &["**Su", "b**\n-", "--\n"],
                &["**Sub**", "\n", "---\n"],
                &["**Sub**\n", "---\n"],
                &["**Sub**\n---\n"],
            ],
            "Sub\n───\n",
        ),
    ];
    for (cuts, expected) in cases {
        for pieces in cuts {
            let mut render = Renderer::new(Recording::redirected(80, 24));
            render.wears(colourful());
            for (at, piece) in pieces.iter().enumerate() {
                render.stream(piece).unwrap();
                if at + 1 < pieces.len() {
                    assert_eq!(render.terminal.written(), "", "{pieces:?} at {at}");
                }
            }
            render.settle().unwrap();
            assert_eq!(render.terminal.written(), expected, "{pieces:?}");
        }
    }
}

#[test]
fn a_redirected_run_ends_a_title_that_proved_prose_as_the_prose_it_was() {
    let mut render = Renderer::new(Recording::redirected(80, 24));
    render.wears(colourful());
    render.stream("Title\n").unwrap();
    assert_eq!(render.terminal.written(), "");
    render.stream("not an underline\n").unwrap();
    assert_eq!(render.terminal.written(), "Title\n");
}

#[test]
fn a_drag_over_a_heading_copies_it_as_it_is_drawn_and_lists_no_source() {
    let mut drawn = reading(40, 12);
    drawn.answer("Title\n=====\n\nprose\n");
    let rows = window(&drawn, 12);
    let words = row_containing(&rows, "Title");
    let rule = row_containing(&rows, "━");
    assert_eq!(rule, words + 1, "{rows:#?}");
    drawn.take();

    let wrote = drag(&mut drawn, (words, 0), (rule, 4));

    assert!(
        wrote.contains(&onto_the_clipboard("Title\n━━━━━")),
        "{rows:#?} gave {wrote:?}"
    );
    assert_eq!(drawn.sources().count(), 0);
}
