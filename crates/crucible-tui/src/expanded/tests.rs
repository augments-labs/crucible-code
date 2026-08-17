use super::*;

/// What the view says, row by row.
fn art(expanded: &Expanded<'_>, columns: usize, room: usize) -> Vec<String> {
    expanded
        .within(columns, room, Glyphs::Unicode)
        .iter()
        .map(Row::text)
        .collect()
}

/// A result of `lines` numbered lines, so a window's edges are readable.
fn counted(lines: usize) -> String {
    (1..=lines)
        .map(|at| format!("line {at}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_result_stands_under_the_line_of_the_call_it_answers() {
    let text = "one\ntwo\nthree";
    let shown = [Shown {
        called: "Bash(cargo test)",
        text,
    }];
    let rows = art(
        &Expanded {
            shown: &shown,
            from: 0,
        },
        40,
        20,
    );

    assert_eq!(rows.first().map(String::as_str), Some(&"─".repeat(40)[..]));
    assert_eq!(
        rows.get(2).map(|row| row.trim_end()),
        Some("Bash(cargo test)")
    );
    assert_eq!(rows.get(4).map(|row| row.trim_end()), Some("one"));
    assert_eq!(rows.get(6).map(|row| row.trim_end()), Some("three"));

    // The blanks that part the view's parts: under the rule, under the call's
    // line, and above the footer.
    let parting = |at: &usize| rows.get(*at).is_some_and(|row| row.trim().is_empty());
    assert!([1, 3, 7].iter().all(parting), "{rows:?}");
}

#[test]
fn the_view_is_only_as_tall_as_what_it_holds() {
    // Padding out to the window would put the footer at the foot of the screen
    // with a block of nothing above it. Most results are a few lines, and the
    // view is meant to be read and closed rather than lived in.
    let shown = [Shown {
        called: "Read(one)",
        text: "only this",
    }];
    let rows = art(
        &Expanded {
            shown: &shown,
            from: 0,
        },
        40,
        40,
    );

    assert_eq!(rows.len(), 7, "{rows:?}");
}

#[test]
fn results_are_parted_from_each_other_and_not_from_the_rule() {
    // A blank leading the list would part it from a heading that is not there.
    let shown = [
        Shown {
            called: "Read(one)",
            text: "first",
        },
        Shown {
            called: "Read(two)",
            text: "second",
        },
    ];
    let rows = art(
        &Expanded {
            shown: &shown,
            from: 0,
        },
        40,
        20,
    );

    assert_eq!(rows.get(2).map(|row| row.trim_end()), Some("Read(one)"));
    assert_eq!(rows.get(4).map(|row| row.trim_end()), Some("first"));
    assert!(
        rows.get(5).is_some_and(|row| row.trim().is_empty()),
        "{rows:?}"
    );
    assert_eq!(rows.get(6).map(|row| row.trim_end()), Some("Read(two)"));
}

#[test]
fn the_arrows_are_named_only_where_there_is_something_left_to_see() {
    // A footer that offers them either way is one nobody can believe the rest
    // of the time.
    let short = [Shown {
        called: "Read(one)",
        text: "one\ntwo",
    }];
    let rows = art(
        &Expanded {
            shown: &short,
            from: 0,
        },
        40,
        20,
    );
    assert_eq!(rows.last().map(String::as_str), Some("esc to close"));

    let long = [Shown {
        called: "Read(one)",
        text: &counted(40),
    }];
    let rows = art(
        &Expanded {
            shown: &long,
            from: 0,
        },
        40,
        20,
    );
    assert_eq!(
        rows.last().map(String::as_str),
        Some("esc to close · ↑↓ to see more")
    );
}

#[test]
fn the_window_opens_where_it_was_asked_to() {
    let shown = [Shown {
        called: "Read(one)",
        text: &counted(40),
    }];

    // Ten rows of window: the rule, a blank, six rows of view, a blank and the
    // footer. Two of those six are the call's line and the blank under it, so
    // the first frame reaches "line 4".
    let rows = art(
        &Expanded {
            shown: &shown,
            from: 0,
        },
        40,
        10,
    );
    assert_eq!(rows.get(2).map(|row| row.trim_end()), Some("Read(one)"));
    assert_eq!(rows.get(7).map(|row| row.trim_end()), Some("line 4"));

    // Two rows down, the call's line has gone past the top and two more lines
    // have arrived at the bottom.
    let rows = art(
        &Expanded {
            shown: &shown,
            from: 2,
        },
        40,
        10,
    );
    assert_eq!(rows.get(2).map(|row| row.trim_end()), Some("line 1"));
    assert_eq!(rows.get(7).map(|row| row.trim_end()), Some("line 6"));
}

#[test]
fn the_window_never_opens_past_the_last_row_it_could_show() {
    // Otherwise a key held down walks the view off its own bottom and leaves
    // the reader looking at a rule, a blank and a footer.
    let shown = [Shown {
        called: "Read(one)",
        text: &counted(40),
    }];
    let expanded = Expanded {
        shown: &shown,
        from: usize::MAX,
    };

    let rows = art(&expanded, 40, 10);
    assert_eq!(rows.get(7).map(|row| row.trim_end()), Some("line 40"));
    assert_eq!(rows.len(), 10, "{rows:?}");
}

#[test]
fn how_far_down_it_may_go_is_answered_at_the_size_it_was_asked_about() {
    // The keyboard cannot know it: how many rows the results came to depends on
    // this width, and the frame that discovers it is the frame that says.
    let shown = [Shown {
        called: "Read(one)",
        text: &counted(40),
    }];
    let expanded = Expanded {
        shown: &shown,
        from: 0,
    };

    // Forty-two rows laid out — the call's line, a blank, and forty lines —
    // against the six a ten-row window holds.
    assert_eq!(expanded.end(40, 10), 36);

    // And nothing to scroll where the whole of it fits.
    assert_eq!(expanded.end(40, 50), 0);
}

#[test]
fn a_window_with_no_room_for_the_view_is_given_nothing() {
    // Rather than as much as fits. A live region drawn short is one the next
    // frame rewinds over rows the terminal has already taken.
    let shown = [Shown {
        called: "Read(one)",
        text: "one",
    }];
    let expanded = Expanded {
        shown: &shown,
        from: 0,
    };

    for room in 0..=CHROME {
        assert!(
            expanded.within(40, room, Glyphs::Unicode).is_empty(),
            "{room} rows"
        );
    }
}

#[test]
fn no_row_is_ever_wider_than_the_window() {
    // A row past the last column is one the terminal wraps itself, which leaves
    // the cursor a row below where the next frame expects it. The lines here are
    // longer than every width tried, including the one narrower than the footer.
    let shown = [Shown {
        called: "Bash(a command far longer than any of these windows)",
        text: "an output line that is also far longer than any of these windows",
    }];
    let expanded = Expanded {
        shown: &shown,
        from: 0,
    };

    for columns in 1..=60 {
        for row in expanded.within(columns, 12, Glyphs::Unicode) {
            assert!(
                wide(&row.text()) <= columns,
                "{columns} columns: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn a_result_that_said_nothing_is_still_shown_under_its_call() {
    // A tool that answered with an empty line still answered. The view says so
    // by standing the call's line over nothing, which is the truth about it —
    // where saying nothing at all would read as the key having missed.
    let shown = [Shown {
        called: "Bash(true)",
        text: "",
    }];
    let rows = art(
        &Expanded {
            shown: &shown,
            from: 0,
        },
        40,
        20,
    );

    assert_eq!(rows.get(2).map(|row| row.trim_end()), Some("Bash(true)"));
    assert_eq!(rows.len(), 6, "{rows:?}");
}
