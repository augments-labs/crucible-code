use super::*;
use crate::dump::dump;

/// Five sessions of this repository, marked on the last.
const FIVE: [Kept<'static>; 5] = [
    Kept {
        title: "Prompt history with arrow navigation",
        when: "now",
        branch: "main",
    },
    Kept {
        title: "/plugin",
        when: "7 hours ago",
        branch: "fix/background-command-offer",
    },
    Kept {
        title: "/plugin",
        when: "8 hours ago",
        branch: "fix/minor-session-display",
    },
    Kept {
        title: "/clear",
        when: "13 hours ago",
        branch: "main",
    },
    Kept {
        title: "Release 0.23.0 smoke gates",
        when: "17 hours ago",
        branch: "main",
    },
];

const KEYS: (&str, &str) = (
    "↑↓ to walk · ctrl+r to rename · type to search · esc to cancel",
    "↑↓ · ctrl+r · esc",
);

/// The tail the preview pane is handed, already drawn.
fn tail() -> Vec<Row> {
    [
        "› scripts/check.sh passes — publish the release",
        "",
        "● Release is live with all artifacts. Running the",
        "  after-tag smoke against the published tarball.",
        "",
        "● Bash(scripts/smoke.sh v0.23.0 2>&1 | tail -12)",
        "  └ all smoke gates passed",
        "",
        "● crucible 0.23.0 is published. All seven",
        "  platforms built, publish attested and",
        "  checksummed. ↓ wheel to see more",
    ]
    .iter()
    .map(|line| Row::new().then(Slot::Plain, *line))
    .collect()
}

fn picker<'a>(sessions: &'a [Kept<'a>], preview: &'a [Row]) -> Picker<'a> {
    Picker {
        heading: "Resume a session · 5 of 5 · ~/Projects/Github/augments-labs/crucible-code",
        query: "",
        typed: 0,
        hint: "a session, or a branch",
        sessions,
        marked: 4,
        renaming: None,
        preview,
        preview_meta: "17 hours ago · 7 messages · main",
        takes: "Enter to resume · Esc to cancel",
        nothing: "no earlier session for this workspace",
        noview: "nothing to show",
        keys: KEYS,
        pointer: None,
    }
}

/// One row of what a picker drew, as text.
fn said(rows: &[Row], at: usize) -> String {
    rows.get(at).expect("a row the picker drew").text()
}

/// A snapshot drawing without the row-end cells `dump` pads to the width.
fn picture(rows: &[Row], columns: usize) -> String {
    dump(rows, columns)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_picker_fills_its_room_rather_than_stopping_at_the_last_session() {
    // Five sessions in room for far more still answer with every row, because
    // the band the picker stands in has no share and takes what it asks for.
    let preview = tail();
    let rows = picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode);

    assert_eq!(rows.len(), 30);
}

#[test]
fn each_session_is_two_rows_with_the_title_over_its_age_and_branch() {
    // The pair is the point of variant A: age and branch stay readable on
    // every row, not only under the mark.
    let preview = tail();
    let rows = picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode);

    let title = said(&rows, LISTED + 3);
    let meta = said(&rows, LISTED + 4);
    assert!(title.contains("/plugin"), "{title:?}");
    assert!(meta.contains("7 hours ago · fix/background"), "{meta:?}");
}

#[test]
fn a_blank_row_parts_one_session_from_the_next() {
    // Two rows of words against two more rows of words read as one block of
    // text the reader has to count through. The row between them is what makes
    // a session a thing on the list rather than a line in it.
    let preview = tail();
    let rows = picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode);

    // The list pane only: the preview beside it goes on saying its own thing
    // through every row of the split.
    let parting = said(&rows, LISTED + 2);
    let listed = parting.split('│').nth(1).expect("the list pane's own row");
    assert!(
        listed.trim().is_empty(),
        "words in the row that parts two sessions: {listed:?}"
    );
    assert!(
        said(&rows, LISTED + 1).contains("now · main"),
        "{:?}",
        said(&rows, LISTED + 1)
    );
    assert!(
        said(&rows, LISTED + 3).contains("/plugin"),
        "{:?}",
        said(&rows, LISTED + 3)
    );
}

#[test]
fn the_marked_session_carries_the_caret_and_the_others_align_past_it() {
    let preview = tail();
    let rows = picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode);

    // The crate's one caret mark, not the reference art's: a mark spelled
    // outside `Glyphs` would survive into the ascii set it has no place in.
    let marked = said(&rows, LISTED + 12);
    assert!(
        marked.contains("› Release 0.23.0 smoke gates"),
        "{marked:?}"
    );
    let unmarked = said(&rows, LISTED);
    assert!(
        unmarked.contains("   Prompt history"),
        "no leading air to align with the mark: {unmarked:?}"
    );
}

#[test]
fn the_preview_ends_in_a_rule_then_the_metadata_then_what_the_keys_take() {
    // The anchored foot is what variant A won on: the rule, then age, count
    // and branch, then Enter and Esc in words, always in that order and always
    // at the bottom of the pane — never a header over the preview.
    let preview = tail();
    let rows = picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode);

    let takes = said(&rows, 30 - 4);
    let meta = said(&rows, 30 - 5);
    let rule = said(&rows, 30 - 6);
    assert!(
        takes.contains("Enter to resume · Esc to cancel"),
        "{takes:?}"
    );
    assert!(
        meta.contains("17 hours ago · 7 messages · main"),
        "{meta:?}"
    );
    assert!(rule.contains("──"), "{rule:?}");
}

#[test]
fn the_keys_are_the_last_row_and_the_heading_sits_under_the_search_line() {
    let preview = tail();
    let rows = picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode);

    assert!(
        said(&rows, 30 - 1).contains("↑↓ to walk · ctrl+r to rename"),
        "{:?}",
        said(&rows, 30 - 1)
    );
    assert!(
        said(&rows, 3).contains("Resume a session · 5 of 5"),
        "{:?}",
        said(&rows, 3)
    );
}

#[test]
fn each_pane_stands_in_its_own_rounded_frame() {
    // The list and the preview each open with corners and close with corners:
    // one frame apiece, over the first row of the panes and under the last.
    let preview = tail();
    let rows = picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode);

    let over = said(&rows, LISTED - 1);
    let under = said(&rows, 30 - 3);
    assert_eq!(over.matches('╭').count(), 2, "{over:?}");
    assert_eq!(over.matches('╮').count(), 2, "{over:?}");
    assert_eq!(under.matches('╰').count(), 2, "{under:?}");
    assert_eq!(under.matches('╯').count(), 2, "{under:?}");
}

#[test]
fn the_preview_shows_the_end_of_what_it_was_handed() {
    // What a reader opens a session to learn is how it finished. Handed more
    // rows than the pane has room for, the tail is what survives.
    let preview: Vec<Row> = (0..40)
        .map(|line| Row::new().then(Slot::Plain, format!("line {line}")))
        .collect();
    let rows = picker(&FIVE, &preview).within(100, 20, Glyphs::Unicode);

    let drawn = picture(&rows, 100);
    assert!(drawn.contains("line 39"), "the tail is missing:\n{drawn}");
    assert!(!drawn.contains("line 0\n"), "drawn from the top:\n{drawn}");
}

#[test]
fn below_the_fold_the_preview_folds_away_and_the_list_takes_every_column() {
    // Two panes under seventy columns leave neither side room for a sentence.
    // What must not survive the fold is any piece of the preview: its rows,
    // its rule, its foot.
    let preview = tail();
    let picker = picker(&FIVE, &preview);

    let folded = picker.within(Picker::FOLDS_AT - 1, 30, Glyphs::Unicode);
    let drawn = picture(&folded, Picker::FOLDS_AT - 1);
    assert!(!drawn.contains("Enter to resume"), "{drawn}");
    assert!(!drawn.contains("publish the release"), "{drawn}");
    assert_eq!(
        said(&folded, LISTED).matches('│').count(),
        2,
        "more edges than the list's own frame: {:?}",
        said(&folded, LISTED)
    );

    let apart = picker.within(Picker::FOLDS_AT, 30, Glyphs::Unicode);
    assert_eq!(
        said(&apart, LISTED).matches('│').count(),
        4,
        "two panes are two frames: {:?}",
        said(&apart, LISTED)
    );
}

#[test]
fn nothing_is_drawn_where_the_window_or_the_room_is_short_of_one_entry() {
    // A picker with nothing to pick is not a picker, and a search line that
    // cannot show the query is the one row here that has to. Written out
    // rather than read from the constants, which would make this a test of
    // nothing.
    let preview = tail();
    let picker = picker(&FIVE, &preview);

    for columns in 0..24 {
        assert!(
            picker.within(columns, 30, Glyphs::Unicode).is_empty(),
            "drew something into {columns} columns"
        );
    }
    for room in 0..11 {
        assert!(
            picker.within(100, room, Glyphs::Unicode).is_empty(),
            "drew something into a room of {room}"
        );
    }
    assert_eq!(picker.within(100, 11, Glyphs::Unicode).len(), 11);
}

#[test]
fn the_marked_session_is_on_screen_on_every_rung_of_the_scroll() {
    // A list scrolled to its top would leave Enter about to resume something
    // the reader cannot see.
    let crowd: Vec<Kept<'static>> = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth",
    ]
    .iter()
    .map(|title| Kept {
        title,
        when: "now",
        branch: "main",
    })
    .collect();
    let preview = tail();

    for (mark, wanted) in ["first", "fourth", "eighth"].iter().enumerate() {
        let mut picker = picker(&crowd, &preview);
        picker.marked = match mark {
            0 => 0,
            1 => 3,
            _ => 7,
        };

        let rows = picker.within(100, 13, Glyphs::Unicode);
        assert!(
            rows.iter().any(|row| row.text().contains(wanted)),
            "{wanted} is off screen with the mark on it"
        );
    }
}

#[test]
fn a_workspace_with_no_sessions_says_so_across_the_whole_width() {
    // Nothing was ever recorded here, so there is no split to draw —
    // no divider, no preview, no foot — only the quiet sentence and the keys.
    let picker = Picker {
        sessions: &[],
        preview: &[],
        preview_meta: "",
        ..picker(&FIVE, &[])
    };

    let rows = picker.within(100, 20, Glyphs::Unicode);
    let drawn = picture(&rows, 100);
    assert!(
        said(&rows, LISTED).contains("no earlier session for this workspace"),
        "{:?}",
        said(&rows, LISTED)
    );
    assert_eq!(
        said(&rows, LISTED).matches('│').count(),
        2,
        "more edges than the list's own frame: {:?}",
        said(&rows, LISTED)
    );
    assert!(!drawn.contains("Enter to resume"), "{drawn}");
}

#[test]
fn a_query_that_left_nothing_keeps_the_split_and_says_so_on_both_sides() {
    // The reason the list emptied is the query in the line above it,
    // so the split stays — the list says what matched nothing, and the
    // preview says it has nothing to show in place of a tail and its foot.
    let picker = Picker {
        query: "deploy",
        typed: 6,
        sessions: &[],
        preview: &[],
        preview_meta: "",
        nothing: "no session holds \"deploy\"",
        ..picker(&FIVE, &[])
    };

    let rows = picker.within(100, 20, Glyphs::Unicode);
    let drawn = picture(&rows, 100);
    assert!(
        said(&rows, LISTED).contains("no session holds \"deploy\""),
        "{:?}",
        said(&rows, LISTED)
    );
    assert_eq!(
        said(&rows, LISTED).matches('│').count(),
        4,
        "two panes are two frames: {:?}",
        said(&rows, LISTED)
    );
    assert!(drawn.contains("nothing to show"), "{drawn}");
    assert!(!drawn.contains("Enter to resume"), "{drawn}");
}

/// One row of what a picker drew, whole.
fn row(rows: &[Row], at: usize) -> &Row {
    rows.get(at).expect("a row the picker drew")
}

#[test]
fn what_is_resting_under_the_pointer_is_what_the_picker_answers_for() {
    // At a hundred columns the list keeps forty, so the divider stands at
    // forty and the preview opens past it.
    let preview = tail();
    let mut picker = picker(&FIVE, &preview);

    for (pointer, hit) in [
        ((SEARCHING, 12), Hit::Search),
        ((SEARCHING - 1, 0), Hit::Search),
        ((SEARCHING + 2, 5), Hit::Nothing),
        ((LISTED, 5), Hit::Session(0)),
        ((LISTED + 1, 5), Hit::Session(0)),
        ((LISTED + 2, 5), Hit::Session(0)),
        ((LISTED + 12, 3), Hit::Session(4)),
        ((LISTED + 13, 3), Hit::Session(4)),
        ((LISTED + 15, 3), Hit::Nothing),
        ((LISTED, 60), Hit::Preview),
        ((29, 5), Hit::Nothing),
    ] {
        picker.pointer = Some(pointer);
        assert_eq!(picker.resting(100, 30), hit, "{pointer:?}");
    }

    picker.pointer = None;
    assert_eq!(picker.resting(100, 30), Hit::Nothing);
}

#[test]
fn a_scrolled_list_answers_with_the_session_the_reader_can_see() {
    // Eight sessions, the mark on the last, room for three pairs: the first
    // row on screen is the sixth session, and a pointer on it is owed that
    // one — not the first of the slice.
    let crowd: Vec<Kept<'static>> = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth",
    ]
    .iter()
    .map(|title| Kept {
        title,
        when: "now",
        branch: "main",
    })
    .collect();
    let preview = tail();
    let mut picker = picker(&crowd, &preview);
    picker.marked = 7;
    picker.pointer = Some((LISTED, 5));

    assert_eq!(picker.resting(100, 18), Hit::Session(5));
}

#[test]
fn the_pair_under_the_pointer_lights_together() {
    let preview = tail();
    let mut picker = picker(&FIVE, &preview);
    picker.pointer = Some((LISTED, 5));

    let rows = picker.within(100, 30, Glyphs::Unicode);
    assert!(row(&rows, LISTED).kinds().any(|slot| slot == Slot::Pointed));
    assert!(
        row(&rows, LISTED + 1)
            .kinds()
            .any(|slot| slot == Slot::Pointed),
        "the pair is one thing to the reader, and half a band says it is two"
    );
    assert!(
        row(&rows, LISTED + 2)
            .kinds()
            .all(|slot| slot != Slot::Pointed)
    );
}

#[test]
fn the_search_frame_brightens_only_under_the_pointer() {
    let preview = tail();
    let mut picker = picker(&FIVE, &preview);

    picker.pointer = Some((LISTED, 5));
    let rows = picker.within(100, 30, Glyphs::Unicode);
    assert!(row(&rows, 0).kinds().all(|slot| slot != Slot::Accent));

    picker.pointer = Some((SEARCHING, 12));
    let rows = picker.within(100, 30, Glyphs::Unicode);
    assert!(row(&rows, 0).kinds().any(|slot| slot == Slot::Accent));
    assert!(row(&rows, LISTED).kinds().all(|slot| slot != Slot::Pointed));
}

#[test]
fn the_caret_stands_in_the_search_line_past_what_was_typed() {
    let preview = tail();
    let mut picker = picker(&FIVE, &preview);

    let caret = picker.caret(100, 30, Glyphs::Unicode);
    assert_eq!((caret.row, caret.column), (SEARCHING, TYPED_AT));

    picker.query = "fix";
    picker.typed = 3;
    let caret = picker.caret(100, 30, Glyphs::Unicode);
    assert_eq!((caret.row, caret.column), (SEARCHING, TYPED_AT + 3));
}

#[test]
fn the_caret_lands_inside_the_search_line_at_every_width_the_picker_is_drawn_at() {
    let preview = tail();
    let mut picker = picker(&FIVE, &preview);
    picker.query = "a query much longer than a narrow window has columns for";
    picker.typed = picker.query.chars().count();

    for columns in Picker::NARROWEST..=200 {
        let rows = picker.within(columns, 30, Glyphs::Unicode);
        let caret = picker.caret(columns, 30, Glyphs::Unicode);
        assert!(caret.row < rows.len(), "{columns}: past the last row");
        assert!(
            (1..columns - 1).contains(&caret.column),
            "{columns}: caret at column {}",
            caret.column
        );
    }
}

#[test]
fn a_rename_stands_where_the_title_was_and_holds_the_caret() {
    let preview = tail();
    let mut picker = picker(&FIVE, &preview);
    picker.renaming = Some("Release gates, renamed");
    picker.typed = 7;

    let rows = picker.within(100, 30, Glyphs::Unicode);
    assert!(
        said(&rows, LISTED + 12).contains("Release gates, renamed"),
        "{:?}",
        said(&rows, LISTED + 12)
    );
    assert!(!picture(&rows, 100).contains("Release 0.23.0 smoke gates"));

    let caret = picker.caret(100, 30, Glyphs::Unicode);
    assert_eq!((caret.row, caret.column), (LISTED + 12, LEADING + 1 + 7));
}

#[test]
fn the_whole_picker() {
    let preview = tail();
    insta::assert_snapshot!(picture(
        &picker(&FIVE, &preview).within(100, 30, Glyphs::Unicode),
        100
    ));
}

#[test]
fn the_whole_picker_folded() {
    let preview = tail();
    insta::assert_snapshot!(picture(
        &picker(&FIVE, &preview).within(58, 24, Glyphs::Unicode),
        58
    ));
}

#[test]
fn the_whole_picker_in_ascii() {
    let preview = tail();
    insta::assert_snapshot!(picture(
        &picker(&FIVE, &preview).within(100, 30, Glyphs::Ascii),
        100
    ));
}

#[test]
fn the_whole_picker_after_a_query_that_matched_nothing() {
    let mut picker = picker(&[], &[]);
    picker.query = "deploy";
    picker.typed = 6;
    picker.preview_meta = "";
    picker.nothing = "no session holds \"deploy\"";

    insta::assert_snapshot!(picture(&picker.within(100, 20, Glyphs::Unicode), 100));
}
