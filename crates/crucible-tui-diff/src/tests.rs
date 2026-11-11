//! The look a change is drawn in, read back from the rows: the box, the
//! numbers, the raised words, the fold and the gaps, and the pictures checked
//! in beside the drawer.

use std::fmt::Write as _;
use std::sync::Arc;

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Drawers, Draws, EXPAND, Room};
use crucible_tui::testing::{ANSWERS, dump, read as answered, resized};
use crucible_tui::{Glyphs, Row, Slot};

use crate::{
    Change, Changes, FOLD, File, Line, MOST_BYTES, MOST_FILES, MOST_LINES, NotADiff, Side, read,
    unoffered,
};

const UNICODE: Glyphs = Glyphs::Unicode;
const ASCII: Glyphs = Glyphs::Ascii;

fn room(columns: usize, glyphs: Glyphs) -> Room {
    Room { columns, glyphs }
}

fn drawn(change: &Change, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    Changes.draw(change, &room(columns, glyphs)).rows
}

fn lines_of(change: &Change) -> usize {
    change.files().first().map_or(0, |file| file.lines().len())
}

fn texts(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect()
}

/// The approved mock's two files, as a unified diff.
const TWO_FILES: &str = "diff --git a/src/parse.rs b/src/parse.rs
--- a/src/parse.rs
+++ b/src/parse.rs
@@ -10,3 +10,4 @@
     let mut tree = Tree::new();
-    for line in input.lines() {
+    for (at, line) in input.lines().enumerate() {
+        tree.mark(at);
         tree.push(line);
@@ -40,1 +40,1 @@ fn parse
-    tree
+    tree.finish()
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1,1 +1,2 @@
 # parse
+A small parser.
";

/// A file a tool changed, numbered the way a tool numbers it: a removed line
/// by where it was, every other line by where it is now.
fn edited(path: Option<&str>, lines: usize) -> Change {
    let mut numbered = Vec::new();
    for at in 1..=lines {
        let text = format!("line {at} of the file");
        if at % 5 == 0 {
            numbered.push((at, Side::Removed, text.clone()));
            numbered.push((at, Side::Added, format!("line {at} of the changed file")));
        } else {
            numbered.push((at, Side::Kept, text));
        }
    }
    Change::new(vec![File::numbered(
        path,
        numbered
            .iter()
            .map(|(at, side, text)| (*at, *side, text.as_str())),
    )])
}

#[test]
fn a_change_opens_with_its_path_and_its_summary_and_closes_its_box() {
    // The words are the reader's own ink, the dot is quiet, and each count is
    // in its side's count slot, which has no ground.
    let change = read(TWO_FILES).expect("the mock is a diff");
    let rows = drawn(&change, 64, UNICODE);
    let pictured = dump(&rows, 64);
    let mut lines = pictured.lines();
    assert_eq!(
        lines.next().map(str::trim_end),
        Some(
            "<Quiet>╭─</> <Path>src/parse.rs</><Quiet> · </>Added <AddedCount>3</> lines, removed <RemovedCount>2</> lines"
        )
    );
    // A later file opens the same way, and one line is a `line`.
    assert_eq!(
        pictured.lines().nth(9).map(str::trim_end),
        Some("<Quiet>├─</> <Path>README.md</><Quiet> · </>Added <AddedCount>1</> line")
    );
    assert_eq!(lines.last().map(str::trim_end), Some("<Quiet>╰─</>"));
    for row in rows.iter().skip(1).take(rows.len() - 2) {
        assert!(
            row.text().starts_with("│ ") || row.text().starts_with("├─"),
            "{:?}",
            row.text()
        );
    }
}

#[test]
fn a_side_with_no_lines_is_left_out_of_the_summary() {
    let opener = |lines: &[(usize, Side, &str)]| {
        let change = Change::new(vec![File::numbered(Some("x"), lines.iter().copied())]);
        texts(&drawn(&change, 64, UNICODE))
            .first()
            .cloned()
            .unwrap_or_default()
    };
    // The side left alone opens the summary, so it takes the capital.
    assert_eq!(
        opener(&[(1, Side::Kept, "a"), (2, Side::Removed, "b")]),
        "╭─ x · Removed 1 line"
    );
    assert_eq!(
        opener(&[(1, Side::Added, "a"), (2, Side::Added, "b")]),
        "╭─ x · Added 2 lines"
    );
    // One line on each side is a `line` on each.
    assert_eq!(
        opener(&[(1, Side::Removed, "a"), (1, Side::Added, "b")]),
        "╭─ x · Added 1 line, removed 1 line"
    );
    // Nothing moved: no summary, and no dot left standing for one.
    assert_eq!(opener(&[(1, Side::Kept, "a")]), "╭─ x");
}

#[test]
fn every_line_wears_its_number_its_sign_and_its_ground() {
    let change = read(TWO_FILES).expect("a diff");
    let pictured = dump(&drawn(&change, 64, UNICODE), 64);
    assert!(
        pictured.contains("<Quiet>│</> <Quiet>  10  </> "),
        "{pictured}"
    );
    assert!(
        pictured.contains("<Quiet>│</> <AddedNumber>  12 + </><Added>        tree.mark(at);"),
        "{pictured}"
    );
    // A changed line carries its ground to one column short of the room.
    for row in drawn(&change, 64, UNICODE) {
        if changed(&row) {
            assert_eq!(row.columns(), 63, "{:?}", row.text());
        }
    }
}

/// Whether `row` is a line that moved.
fn changed(row: &Row) -> bool {
    row.kinds()
        .any(|slot| slot == Slot::Added || slot == Slot::Removed)
}

#[test]
fn the_ground_of_a_changed_row_is_unbroken_from_its_number_to_its_edge() {
    // After the box's bar and the cell beside it, every cell of a changed row
    // is in a slot that carries its side's ground: a cell in any other slot
    // is a hole in the band, as the one between the sign and the line was.
    let change = read(TWO_FILES).expect("a diff");
    let mut seen = 0;
    for row in drawn(&change, 64, UNICODE) {
        if !changed(&row) {
            continue;
        }
        seen += 1;
        let inside: Vec<(Slot, &str)> = row.spans().skip(2).collect();
        let added = [Slot::AddedNumber, Slot::Added, Slot::AddedWord];
        let removed = [Slot::RemovedNumber, Slot::Removed, Slot::RemovedWord];
        assert!(
            inside.iter().all(|(slot, _)| added.contains(slot))
                || inside.iter().all(|(slot, _)| removed.contains(slot)),
            "{inside:?}"
        );
        assert_eq!(
            row.spans().take(2).collect::<Vec<_>>(),
            [(Slot::Quiet, "│"), (Slot::Plain, " ")]
        );
    }
    assert_eq!(seen, 6);
}

/// One file with a short and a long line on each side and a long line that
/// did not move.
fn short_and_long() -> Change {
    let long = "a line that runs on ".repeat(12);
    Change::new(vec![File::numbered(
        Some("x"),
        [
            (1, Side::Kept, long.as_str()),
            (2, Side::Removed, "short"),
            (3, Side::Removed, long.as_str()),
            (2, Side::Added, "brief"),
            (3, Side::Added, long.as_str()),
        ],
    )])
}

#[test]
fn a_changed_row_ends_one_column_short_of_the_room_whatever_its_length() {
    let change = short_and_long();
    for (glyphs, more) in [(UNICODE, "…"), (ASCII, "...")] {
        for columns in [24, 40, 64, 120] {
            let rows = drawn(&change, columns, glyphs);
            let moved: Vec<&Row> = rows.iter().filter(|row| changed(row)).collect();
            assert_eq!(moved.len(), 4, "{}", dump(&rows, columns));
            for row in &moved {
                assert_eq!(row.columns(), columns - 1, "{:?}", row.text());
            }
            // The long line of each side is cut with the mark on its ground,
            // in the last cell the ground takes.
            for (at, ground) in [(1, Slot::Removed), (3, Slot::Added)] {
                let row = moved.get(at).expect("a long row");
                assert_eq!(row.spans().last(), Some((ground, more)), "{:?}", row.text());
            }
            // A short one is not marked.
            for at in [0, 2] {
                let row = moved.get(at).expect("a short row");
                assert!(!row.text().contains(more), "{:?}", row.text());
            }
        }
    }
}

#[test]
fn a_line_cut_where_a_wide_character_does_not_fit_still_ends_in_the_mark_at_the_edge() {
    // Each character is two columns, so at one of two neighbouring widths
    // the cut leaves a column over: the ground takes it, before the mark.
    let wide = "漢字".repeat(60);
    let change = Change::new(vec![File::numbered(
        Some("x"),
        [
            (1, Side::Kept, wide.as_str()),
            (2, Side::Removed, wide.as_str()),
            (2, Side::Added, wide.as_str()),
        ],
    )]);
    for columns in [40, 41] {
        let rows = drawn(&change, columns, UNICODE);
        for (at, slot) in [(1, Slot::Quiet), (2, Slot::Removed), (3, Slot::Added)] {
            let row = rows.get(at).expect("a line");
            assert_eq!(row.columns(), columns - 1, "{:?}", row.text());
            assert_eq!(row.spans().last(), Some((slot, "…")), "{:?}", row.text());
        }
    }
}

#[test]
fn a_line_that_did_not_move_is_cut_at_the_column_the_grounds_end_at() {
    let change = short_and_long();
    for (glyphs, more) in [(UNICODE, "…"), (ASCII, "...")] {
        for columns in [24, 40, 64, 120] {
            let rows = drawn(&change, columns, glyphs);
            let kept = rows.get(1).expect("the kept row");
            assert_eq!(kept.columns(), columns - 1, "{:?}", kept.text());
            assert_eq!(kept.spans().last(), Some((Slot::Quiet, more)));
        }
    }
}

#[test]
fn a_line_that_did_not_move_is_cut_where_the_grounds_end_in_a_room_too_narrow_for_its_number() {
    // Where the number and the mark do not both fit, the row is cut as a
    // whole, and a line that did not move is cut at the same column as the
    // lines that did: none of them takes the room's last column.
    let change = short_and_long();
    for (glyphs, more) in [(UNICODE, "…"), (ASCII, "...")] {
        for columns in 0..=13_usize {
            let rows = drawn(&change, columns, glyphs);
            let kept = rows.get(1).expect("the kept row");
            let removed = rows.get(3).expect("the long removed row");
            let added = rows.get(5).expect("the long added row");
            let edge = columns.saturating_sub(1);

            assert_eq!(kept.columns(), edge, "{columns}: {:?}", kept.text());
            assert_eq!(removed.columns(), edge, "{columns}: {:?}", removed.text());
            assert_eq!(added.columns(), edge, "{columns}: {:?}", added.text());
            assert_eq!(
                kept.text().ends_with(more),
                removed.text().ends_with(more),
                "{columns}: {:?} against {:?}",
                kept.text(),
                removed.text()
            );
        }
    }
}

#[test]
fn a_replaced_word_is_raised_on_both_lines_of_its_pair() {
    // The removed line's word, and what replaced it on the added line.
    let change = read(TWO_FILES).expect("a diff");
    let pictured = dump(&drawn(&change, 64, UNICODE), 64);
    assert!(
        pictured.contains(
            "<RemovedNumber>  11 - </><Removed>    for </><RemovedWord>line</><Removed> in input.lines() {"
        ),
        "{pictured}"
    );
    assert!(
        pictured.contains(
            "<AddedNumber>  11 + </><Added>    for </><AddedWord>(at, line)</><Added> in input.lines()</><AddedWord>.enumerate()</><Added> {"
        ),
        "{pictured}"
    );
    // An added line with nothing to pair with is raised nowhere.
    assert!(
        !pictured.contains("<AddedWord>        tree.mark"),
        "{pictured}"
    );
    // Only the part of a word that changed: `tree` stays, `.finish()` is new.
    assert!(
        pictured.contains("<Added>    tree</><AddedWord>.finish()</>"),
        "{pictured}"
    );
}

#[test]
fn two_lines_with_nothing_in_common_are_not_raised_word_by_word() {
    let change =
        read("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-alpha beta\n+gamma delta\n").expect("a diff");
    let rows = drawn(&change, 64, UNICODE);
    assert!(
        rows.iter().all(|row| row
            .kinds()
            .all(|slot| slot != Slot::AddedWord && slot != Slot::RemovedWord)),
        "{}",
        dump(&rows, 64)
    );
}

#[test]
fn the_lines_between_two_hunks_are_one_quiet_row_with_their_count() {
    // What the diff never held is counted, with the section it is in.
    let change = read(TWO_FILES).expect("a diff");
    let pictured = dump(&drawn(&change, 64, UNICODE), 64);
    assert!(
        pictured.contains("<Quiet>│</><Quiet>  ⋯  26 lines between · fn parse  ⋯</>"),
        "{pictured}"
    );
    let ascii = dump(&drawn(&change, 64, ASCII), 64);
    assert!(
        ascii.contains("<Quiet>|</><Quiet>  ...  26 lines between · fn parse  ...</>"),
        "{ascii}"
    );
}

#[test]
fn a_tool_s_lines_that_jump_are_parted_by_a_gap_row() {
    let file = File::numbered(
        Some("x"),
        [
            (3, Side::Kept, "a"),
            (4, Side::Added, "b"),
            (20, Side::Removed, "c"),
            (20, Side::Kept, "d"),
        ],
    );
    assert_eq!(
        file.lines()
            .iter()
            .filter(|line| matches!(line, Line::Gap { .. }))
            .count(),
        1
    );
    let rows = texts(&drawn(&Change::new(vec![file]), 64, UNICODE));
    assert!(
        rows.contains(&"│  ⋯  15 lines between  ⋯".to_owned()),
        "{rows:#?}"
    );
}

#[test]
fn a_file_longer_than_the_fold_shows_its_head_and_tail_and_counts_the_rest() {
    // The first twenty-four rows and the last four, one quiet row between.
    let change = edited(Some("long.txt"), 40);
    let total = lines_of(&change);
    assert!(total > FOLD);
    let rows = drawn(&change, 80, UNICODE);
    // The header, twenty-four, the fold, four, and the close.
    assert_eq!(rows.len(), 1 + 24 + 1 + 4 + 1, "{:#?}", texts(&rows));
    let fold = rows.get(25).expect("a fold row");
    assert_eq!(
        fold.text().trim_end(),
        format!("│  ⋯  {} lines folded · ctrl+o to expand  ⋯", total - 28)
    );
    // The key is the one every cut row names, in the slot that lights under
    // the pointer as an offer does; the rest of the row is quiet.
    assert_eq!(
        fold.spans().collect::<Vec<_>>(),
        [
            (Slot::Quiet, "│"),
            (Slot::Quiet, "  ⋯  20 lines folded · "),
            (Slot::Cut, EXPAND),
            (Slot::Quiet, "  ⋯"),
        ]
    );
    // Every line it folded is still in the change.
    assert_eq!(lines_of(&change), total);
    let ascii = drawn(&change, 80, ASCII);
    assert_eq!(
        ascii
            .get(25)
            .map(|row| row.text().trim_end().to_owned())
            .unwrap_or_default(),
        format!(
            "|  ...  {} lines folded · ctrl+o to expand  ...",
            total - 28
        )
    );
}

#[test]
fn a_file_of_exactly_the_fold_is_drawn_whole() {
    let mut lines = Vec::new();
    for at in 1..=FOLD {
        lines.push((at, Side::Kept, "same"));
    }
    let change = Change::new(vec![File::numbered(Some("x"), lines)]);
    assert_eq!(lines_of(&change), FOLD);
    assert_eq!(drawn(&change, 80, UNICODE).len(), FOLD + 2);
}

#[test]
fn a_file_one_row_past_the_fold_is_folded_and_offers_the_rows_it_hides() {
    let lines: Vec<(usize, Side, &str)> =
        (1..=FOLD + 1).map(|at| (at, Side::Kept, "same")).collect();
    let change = Change::new(vec![File::numbered(Some("x"), lines)]);
    assert_eq!(lines_of(&change), FOLD + 1);
    let rows = texts(&drawn(&change, 80, UNICODE));
    // The header, the head, the fold, the tail, and the close.
    assert_eq!(rows.len(), 1 + 24 + 1 + 4 + 1, "{rows:#?}");
    assert_eq!(
        rows.get(25).map(String::as_str),
        Some("│  ⋯  5 lines folded · ctrl+o to expand  ⋯")
    );
    assert_eq!(rows.get(24).map(String::as_str), Some("│   24   same"));
    assert_eq!(rows.get(26).map(String::as_str), Some("│   30   same"));
}

#[test]
fn a_file_says_it_folds_exactly_when_its_box_counts_rows_it_hides() {
    // Asked of the file and not read off its rows, because a narrow room cuts
    // the fold's row short of the word and of the key, and the file is folded
    // all the same.
    for rows in [1, FOLD - 1, FOLD, FOLD + 1, FOLD + 2, 2 * FOLD] {
        let lines: Vec<(usize, Side, &str)> =
            (1..=rows).map(|at| (at, Side::Kept, "same")).collect();
        let file = File::numbered(Some("x"), lines);
        let change = Change::new(vec![file.clone()]);
        let counts = texts(&drawn(&change, 80, UNICODE))
            .iter()
            .any(|row| row.contains("lines folded") || row.contains("line folded"));
        assert_eq!(file.folds(), counts, "{rows} rows");
        assert_eq!(file.folds(), rows > FOLD, "{rows} rows");
    }

    // A row that stands for the lines between two hunks is a row of the box:
    // one short of the fold in lines, and past it with the gap.
    let mut lines: Vec<(usize, Side, &str)> =
        (1..FOLD).map(|at| (at, Side::Kept, "same")).collect();
    lines.push((900, Side::Kept, "far"));
    lines.push((901, Side::Kept, "far"));
    let gapped = File::numbered(Some("x"), lines);
    assert_eq!(gapped.lines().len(), FOLD + 2);
    assert!(gapped.folds());
    let narrow = texts(&drawn(&Change::new(vec![gapped]), 12, UNICODE));
    assert!(
        narrow.iter().all(|row| !row.contains("folded")),
        "{narrow:#?}"
    );
}

#[test]
fn a_fold_that_stops_offering_keeps_its_count_and_loses_its_key() {
    let change = edited(Some("long.txt"), 40);
    for (glyphs, counted) in [
        (UNICODE, "│  ⋯  20 lines folded  ⋯"),
        (ASCII, "|  ...  20 lines folded  ..."),
    ] {
        let offering = drawn(&change, 80, glyphs);
        let mut rows = offering.clone();
        for row in &mut rows {
            unoffered(row);
        }
        for (at, (row, offered)) in rows.iter().zip(&offering).enumerate() {
            if at == 25 {
                assert_eq!(row.text().trim_end(), counted);
                assert!(
                    row.kinds().all(|slot| slot == Slot::Quiet),
                    "{:?}",
                    row.spans().collect::<Vec<_>>()
                );
            } else {
                // No other row of the box said anything of a key.
                assert_eq!(row, offered, "row {at}");
            }
        }
    }

    // A fold cut inside its key by a narrow room loses what was left of it.
    let mut narrow = drawn(&change, 34, UNICODE);
    let fold = narrow.get_mut(25).expect("a fold row");
    assert_eq!(fold.text(), "│  ⋯  20 lines folded · ctrl+o to…");
    unoffered(fold);
    assert_eq!(fold.text(), "│  ⋯  20 lines folded…");
    assert!(fold.kinds().all(|slot| slot == Slot::Quiet));

    // A row that offers something else in the same words is not a fold's.
    let other = Row::new()
        .then(Slot::Quiet, "3 lines cut · ")
        .then(Slot::Cut, EXPAND);
    let mut left = other.clone();
    unoffered(&mut left);
    assert_eq!(left, other);
}

#[test]
fn lines_the_tool_never_kept_are_a_count_inside_the_box() {
    let change = Change::new(vec![
        File::numbered(Some("x"), [(1, Side::Added, "a")])
            .counted(100, 100)
            .dropping(136),
    ]);
    let rows = texts(&drawn(&change, 64, UNICODE));
    assert_eq!(
        rows.first().map(String::as_str),
        Some("╭─ x · Added 100 lines, removed 100 lines")
    );
    assert_eq!(
        rows.get(rows.len() - 2).map(String::as_str),
        Some("│  ⋯  136 lines not shown  ⋯")
    );
}

#[test]
fn a_change_with_no_path_opens_on_its_summary_alone() {
    // No path, so no dot to part it from the summary either.
    let change = Change::new(vec![File::numbered(None, [(1, Side::Added, "a")])]);
    let rows = drawn(&change, 64, UNICODE);
    assert_eq!(
        dump(&rows, 64).lines().next().map(str::trim_end),
        Some("<Quiet>╭─</> Added <AddedCount>1</> line")
    );
    let change = Change::new(vec![File::numbered(None, [(1, Side::Removed, "a")])]);
    assert_eq!(
        texts(&drawn(&change, 64, UNICODE))
            .first()
            .map(String::as_str),
        Some("╭─ Removed 1 line")
    );
    // A path that names nothing is no path.
    let change = Change::new(vec![File::numbered(Some(""), [(1, Side::Added, "a")])]);
    assert_eq!(
        texts(&drawn(&change, 64, UNICODE))
            .first()
            .map(String::as_str),
        Some("╭─ Added 1 line")
    );
}

#[test]
fn a_header_too_wide_shortens_its_path_and_then_leaves_its_summary_off_whole() {
    let path = "a/very/long/path/that/does/not/fit/in/a/narrow/room/parse.rs";
    let summary = " · Added 1 line, removed 1 line";
    let change = Change::new(vec![File::numbered(
        Some(path),
        [(1, Side::Removed, "a"), (1, Side::Added, "b")],
    )]);
    // Room for both: nothing is given up.
    let wide = texts(&drawn(&change, 120, UNICODE));
    assert_eq!(
        wide.first().map(String::as_str),
        Some(format!("╭─ {path}{summary}").as_str())
    );
    // Room for the summary and the end of the path: the path's start goes.
    let narrow = texts(&drawn(&change, 64, UNICODE));
    let opener = narrow.first().expect("an opener");
    assert!(opener.starts_with("╭─ …"), "{opener}");
    assert!(opener.ends_with(&format!("parse.rs{summary}")), "{opener}");
    // At every width the summary is whole or absent, its dot with it, and
    // the row fits.
    for glyphs in [UNICODE, ASCII] {
        let dot = format!(" {} ", glyphs.dot());
        for columns in 0..=120 {
            let rows = drawn(&change, columns, glyphs);
            let row = rows.first().expect("an opener");
            let opener = row.text();
            assert!(row.columns() <= columns, "{columns}: {opener:?}");
            let whole = opener.ends_with(&format!("{dot}Added 1 line, removed 1 line"));
            let absent = !opener.contains("Added") && !opener.contains(&dot);
            assert!(whole || absent, "{columns}: {opener:?}");
        }
    }
}

#[test]
fn a_known_extension_colours_the_lines_that_did_not_move() {
    let change =
        read("--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n fn kept() {}\n-let a = 1;\n+let a = 2;\n")
            .expect("a diff");
    let rows = drawn(&change, 64, UNICODE);
    let kept = rows.get(1).expect("a kept row");
    assert!(
        kept.kinds().any(|slot| slot == Slot::Keyword),
        "{}",
        dump(&rows, 64)
    );
    // A changed line wears its change: the ground carries it whole.
    assert!(
        rows.get(2)
            .expect("a removed row")
            .kinds()
            .all(|slot| slot != Slot::Keyword)
    );
}

fn keyworded(row: Option<&Row>) -> bool {
    row.is_some_and(|row| row.kinds().any(|slot| slot == Slot::Keyword))
}

#[test]
fn colour_stops_at_the_first_line_past_what_a_change_reads_and_the_rest_draw_plain() {
    // A short line read, then one longer than what is left, then a short one
    // again: from the line that passes, nothing is coloured, so no line wears
    // a colour read from a state that skipped the line before it.
    let long = "let a = 1; ".repeat(crate::SYNTAX_BYTES / 11 + 1);
    let change = read(&format!(
        "--- a/x.rs\n+++ b/x.rs\n@@ -1,3 +1,3 @@\n fn kept() {{}}\n {long}\n fn after() {{}}\n"
    ))
    .expect("a diff");
    let rows = drawn(&change, 64, UNICODE);
    let pictured = dump(&rows, 64);
    assert!(keyworded(rows.get(1)), "{pictured}");
    assert!(!keyworded(rows.get(2)), "{pictured}");
    assert!(!keyworded(rows.get(3)), "{pictured}");
}

#[test]
fn colour_stops_after_the_lines_a_change_reads() {
    let mut source = format!(
        "--- a/x.rs\n+++ b/x.rs\n@@ -1,{0} +1,{0} @@\n",
        crate::SYNTAX_LINES + 1
    );
    for _ in 0..=crate::SYNTAX_LINES {
        source.push_str(" fn a() {}\n");
    }
    let change = read(&source).expect("a diff");
    let rows = drawn(&change, 64, UNICODE);
    let pictured = dump(&rows, 64);
    assert!(keyworded(rows.get(crate::SYNTAX_LINES)), "{pictured}");
    assert!(!keyworded(rows.get(crate::SYNTAX_LINES + 1)), "{pictured}");
}

#[test]
fn lines_after_the_last_coloured_one_are_not_charged_to_the_colour_a_change_reads() {
    // A file of added lines only wears no colour, so reading it would spend
    // what the next file's lines that did not move are coloured with.
    let added = "let a = 1; ".repeat(crate::SYNTAX_BYTES / 11 / 8 + 1);
    let mut source = format!("--- a/x.rs\n+++ b/x.rs\n@@ -0,0 +1,{FOLD} @@\n");
    for _ in 0..FOLD {
        let _ = writeln!(source, "+{added}");
    }
    source.push_str("--- a/y.rs\n+++ b/y.rs\n@@ -1,1 +1,1 @@\n fn kept() {}\n");
    let change = read(&source).expect("a diff");
    let rows = drawn(&change, 64, UNICODE);
    let kept = rows.iter().rev().nth(1);
    assert!(keyworded(kept), "{}", dump(&rows, 64));
}

#[test]
fn pairs_past_what_a_change_compares_are_drawn_on_their_ground_alone() {
    // Each file holds as many pairs as it shows without folding, and one
    // file more holds the pair past the cap.
    let per_file = FOLD / 2;
    let files = crate::PAIRS / per_file + 1;
    let mut source = String::new();
    for file in 0..files {
        let _ = write!(
            source,
            "--- a/{file}.txt\n+++ b/{file}.txt\n@@ -1,{per_file} +1,{per_file} @@\n"
        );
        for side in ['-', '+'] {
            for at in 0..per_file {
                let _ = writeln!(source, "{side}word {at} of {side}{file}");
            }
        }
    }
    let change = read(&source).expect("a diff");
    let rows = drawn(&change, 64, UNICODE);
    let raised = rows
        .iter()
        .filter(|row| row.kinds().any(|slot| slot == Slot::AddedWord))
        .count();
    assert_eq!(raised, crate::PAIRS, "{}", dump(&rows, 64));
}

#[test]
fn every_row_fits_the_room_and_the_numbers_line_up() {
    let change = read(TWO_FILES).expect("a diff");
    for glyphs in [UNICODE, ASCII] {
        for columns in 0..=80 {
            for row in drawn(&change, columns, glyphs) {
                assert!(row.columns() <= columns, "{columns}: {:?}", row.text());
            }
        }
    }
}

#[test]
fn a_glyph_set_changed_lays_the_same_change_out_in_the_new_one() {
    let change = read(TWO_FILES).expect("a diff");
    let unicode = texts(&drawn(&change, 64, UNICODE)).join("\n");
    let ascii = texts(&drawn(&change, 64, ASCII)).join("\n");
    assert!(unicode.contains('╭') && unicode.contains('│') && unicode.contains('⋯'));
    for mark in ['╭', '├', '╰', '│', '⋯', '−'] {
        assert!(!ascii.contains(mark), "{mark} left in ascii:\n{ascii}");
    }
    assert_eq!(
        ascii.lines().next(),
        Some("+- src/parse.rs - Added 3 lines, removed 2 lines"),
        "{ascii}"
    );
}

#[test]
fn a_change_is_drawn_the_same_however_the_window_reached_its_width() {
    // Laid out once and again on resize, from wider and from narrower.
    let change = read(TWO_FILES).expect("a diff");
    for columns in 8..=160 {
        let fresh = drawn(&change, columns, UNICODE);
        for from in [160, 8] {
            let _ = drawn(&change, from, UNICODE);
            assert_eq!(
                drawn(&change, columns, UNICODE),
                fresh,
                "{columns} from {from}"
            );
        }
    }
}

#[test]
fn the_drawer_takes_diff_and_patch_fences() {
    assert_eq!(Changes.names(), &["diff", "patch"]);
}

#[test]
fn a_fence_that_is_not_a_diff_is_shown_as_written_without_a_refusal() {
    // Not drawn here, which is not a reason anything went wrong.
    for source in [
        "",
        "just prose\nwith lines",
        "@@ not a hunk",
        "+ a list\n- of things",
    ] {
        assert_eq!(
            read(source).map(|_| ()),
            Err(NotADiff::Unread),
            "{source:?}"
        );
        assert_eq!(
            Changes
                .block(source, &room(64, UNICODE))
                .map(|laid| laid.rows.len()),
            Err(crucible_tui::chrome::Undrawn::NotHere)
        );
    }
}

#[test]
fn a_diff_past_its_caps_is_refused_before_it_is_read() {
    let long = format!(
        "--- a/x\n+++ b/x\n@@ -1 +1 @@\n{}",
        "+x\n".repeat(crate::MOST_LINES)
    );
    assert_eq!(read(&long).map(|_| ()), Err(NotADiff::PastCap));
    assert_eq!(
        Changes
            .block(&long, &room(64, UNICODE))
            .map(|laid| laid.rows.len()),
        Err(crucible_tui::chrome::Undrawn::PastCap)
    );
    let wide = format!(
        "--- a/x\n+++ b/x\n@@ -1 +1 @@\n+{}\n",
        "x".repeat(crate::MOST_BYTES)
    );
    assert_eq!(read(&wide).map(|_| ()), Err(NotADiff::PastCap));
}

#[test]
fn a_laid_change_says_its_natural_size() {
    let change = read(TWO_FILES).expect("a diff");
    let laid = Changes.draw(&change, &room(20, UNICODE));
    let wide = Changes.draw(&change, &room(400, UNICODE));
    assert_eq!(laid.natural.1, laid.rows.len());
    assert_eq!(laid.natural, wide.natural);
    assert!(laid.natural.0 > 20);
    assert!(laid.narrow.is_none());
}

// The pictures, at 64 columns and at 120, in both glyph sets.

#[test]
fn picture_of_a_tool_edit_with_a_fold() {
    let change = edited(Some("src/parse.rs"), 40);
    insta::assert_snapshot!(
        "tool_edit_fold_unicode",
        dump(&drawn(&change, 64, UNICODE), 64)
    );
    insta::assert_snapshot!("tool_edit_fold_ascii", dump(&drawn(&change, 64, ASCII), 64));
}

#[test]
fn picture_of_a_word_level_change() {
    let change = read(
        "--- a/src/parse.rs\n+++ b/src/parse.rs\n@@ -7,3 +7,3 @@\n let width = 8;\n-let height = columns / 2;\n+let height = rows / 2;\n let depth = 1;\n",
    )
    .expect("a diff");
    insta::assert_snapshot!("word_level_unicode", dump(&drawn(&change, 64, UNICODE), 64));
    insta::assert_snapshot!("word_level_ascii", dump(&drawn(&change, 64, ASCII), 64));
}

#[test]
fn picture_of_a_two_file_change() {
    let change = read(TWO_FILES).expect("a diff");
    insta::assert_snapshot!("two_files_unicode", dump(&drawn(&change, 64, UNICODE), 64));
    insta::assert_snapshot!("two_files_ascii", dump(&drawn(&change, 64, ASCII), 64));
}

/// Two files: one whose two changed lines are each cut at 64 columns and the
/// removed one at 120 too, with one line on each side; and one with nothing
/// removed.
const CUT_AND_COUNTED: &str = "--- a/src/parse.rs
+++ b/src/parse.rs
@@ -3,3 +3,3 @@
 let width = 8;
-let message = \"a line that runs well past the right edge of a room of one hundred and twenty columns, so that its ground ends in the mark\";
+let message = \"a line that runs past the right edge of a room of sixty-four columns\";
 let depth = 1;
--- a/README.md
+++ b/README.md
@@ -1,1 +1,2 @@
 # parse
+A small parser.
";

#[test]
fn picture_of_a_cut_line_and_a_summary_of_one_line_a_side_and_of_one_side() {
    let change = read(CUT_AND_COUNTED).expect("a diff");
    insta::assert_snapshot!(
        "cut_and_counted_64_unicode",
        dump(&drawn(&change, 64, UNICODE), 64)
    );
    insta::assert_snapshot!(
        "cut_and_counted_64_ascii",
        dump(&drawn(&change, 64, ASCII), 64)
    );
    insta::assert_snapshot!(
        "cut_and_counted_120_unicode",
        dump(&drawn(&change, 120, UNICODE), 120)
    );
    insta::assert_snapshot!(
        "cut_and_counted_120_ascii",
        dump(&drawn(&change, 120, ASCII), 120)
    );
}

#[test]
fn the_caps_the_user_docs_state_are_the_ones_a_change_is_drawn_to() {
    // The caps a tool's edit reaches are stated beside the tools, and the
    // ones only a diff written in a reply reaches beside the transcript.
    use crate::{FOLD, PAIRS, SYNTAX_BYTES, SYNTAX_LINES, TOKENS};
    let spaced = |page: &str| page.split_whitespace().collect::<Vec<_>>().join(" ");
    let files = spaced(include_str!("../../../docs/tools/files.md"));
    let transcript = spaced(include_str!("../../../docs/sessions/transcript.md"));
    for (page, documented, stated) in [
        (
            "docs/tools/files.md",
            &files,
            format!("reads at most {SYNTAX_LINES} of its lines, and {SYNTAX_BYTES} bytes of them"),
        ),
        (
            "docs/tools/files.md",
            &files,
            format!("in at most {PAIRS} replaced lines, each of at most {TOKENS} words"),
        ),
        (
            "docs/tools/files.md",
            &files,
            format!("names more than {MOST_FILES} files is shown as the model wrote it"),
        ),
        (
            "docs/sessions/transcript.md",
            &transcript,
            format!("names more than {MOST_FILES} files or runs to {MOST_LINES} lines or more"),
        ),
        (
            "docs/sessions/transcript.md",
            &transcript,
            format!("A file of more than {FOLD} rows is folded"),
        ),
    ] {
        assert!(
            documented.contains(&stated),
            "{page} does not say {stated:?}"
        );
    }
}

#[test]
fn a_fold_counts_the_lines_it_hides_and_not_a_gap_among_them_as_one() {
    // Thirty lines, a jump of 469, then forty-one more: one gap row sits
    // among the rows the fold hides, and it is not a line of the file.
    let mut lines: Vec<(usize, Side, String)> = (1..=30)
        .map(|at| (at, Side::Kept, format!("line {at}")))
        .collect();
    lines.extend((500..=540).map(|at| (at, Side::Kept, format!("line {at}"))));
    let file = File::numbered(
        Some("x"),
        lines
            .iter()
            .map(|(at, side, text)| (*at, *side, text.as_str())),
    );
    let rows = file.lines().len();
    assert_eq!(rows, 30 + 1 + 41);
    let text = texts(&drawn(&Change::new(vec![file]), 80, UNICODE));
    let hidden = rows - 24 - 4 - 1;
    assert_eq!(
        text.get(25).map(String::as_str),
        Some(format!("│  ⋯  {hidden} lines folded · ctrl+o to expand  ⋯").as_str()),
        "{text:#?}"
    );
}

#[test]
fn files_past_what_a_change_keeps_are_counted_inside_the_box() {
    let file = File::numbered(Some("x"), [(1, Side::Added, "a")]);
    let change = Change::new(vec![file; crate::MOST_FILES + 3]);
    assert_eq!(change.files().len(), crate::MOST_FILES);
    let rows = texts(&drawn(&change, 64, UNICODE));
    assert_eq!(
        rows.get(rows.len() - 2).map(String::as_str),
        Some("│  ⋯  3 files not shown  ⋯"),
        "{rows:#?}"
    );
}

#[test]
fn a_diff_of_more_files_than_a_change_keeps_is_refused_however_its_files_open() {
    // `count` files, each written by `one`.
    let files = |count: usize, one: fn(&mut String, usize) -> std::fmt::Result| {
        let mut diff = String::new();
        for at in 0..count {
            one(&mut diff, at).expect("a string to take the file");
        }
        diff
    };
    // Each opened by its `+++` alone, the first on the diff's first line.
    let headed =
        |diff: &mut String, at: usize| write!(diff, "+++ b/f{at}.txt\n@@ -0,0 +1,1 @@\n+a\n");
    // Each opened by `diff --git` and holding a binary note, with no `+++`.
    let binary = |diff: &mut String, at: usize| {
        write!(
            diff,
            "diff --git a/f{at}.png b/f{at}.png\nindex 0000000..1111111 100644\n\
             Binary files a/f{at}.png and b/f{at}.png differ\n"
        )
    };
    let past = crate::MOST_FILES + 1;
    assert_eq!(read(&files(past, headed)), Err(NotADiff::PastCap));
    assert_eq!(read(&files(past, binary)), Err(NotADiff::PastCap));
    // As many as it keeps are read.
    assert_eq!(
        read(&files(crate::MOST_FILES, headed)).map(|change| change.files().len()),
        Ok(crate::MOST_FILES)
    );
}

#[test]
fn a_file_keeps_its_path_and_its_lines_out_of_debug() {
    // Built as the tool path builds one, from a credential-shaped path.
    let file = File::numbered(
        Some("secrets/sk-ant-not-a-real-key.env"),
        [
            (1, Side::Kept, "TOKEN_KEPT=kept-line-text"),
            (5, Side::Removed, "TOKEN=old-line-text"),
            (5, Side::Added, "TOKEN=new-line-text"),
        ],
    );
    // And as a diff builds one, renamed and with a named section in a gap.
    let renamed = read(
        "diff --git a/secrets/sk-ant-old.env b/secrets/sk-ant-new.env\n\
         rename from secrets/sk-ant-old.env\nrename to secrets/sk-ant-new.env\n\
         --- a/secrets/sk-ant-old.env\n+++ b/secrets/sk-ant-new.env\n\
         @@ -1,1 +1,1 @@\n-gone-line-text\n+came-line-text\n\
         @@ -40,1 +40,1 @@ fn sk_ant_section\n-gone-again\n+came-again\n",
    )
    .expect("a diff to read");
    let gap = renamed.files().first().map(|file| file.lines().to_vec());
    let said = [
        format!("{file:?}"),
        format!("{:?}", Change::new(vec![file.clone()])),
        format!("{renamed:?}"),
        format!("{:?}", file.lines()),
        format!("{gap:?}"),
    ];
    for said in said {
        for kept_out in ["sk-ant", "sk_ant", "line-text", "again"] {
            assert!(!said.contains(kept_out), "{kept_out:?} in {said}");
        }
    }
    // What is there to be counted still is.
    let said = format!("{file:?}");
    assert!(said.contains("path: Some(\"[redacted]\")"), "{said}");
    assert!(said.contains("from: None"), "{said}");
    let said = format!("{:?}", file.lines().first());
    assert!(
        said.contains("Kept") && said.contains("number: 1"),
        "{said}"
    );
    let said = format!("{gap:?}");
    assert!(said.contains("section: Some(\"[redacted]\")"), "{said}");
    let said = format!("{renamed:?}");
    assert!(said.contains("from: Some(\"[redacted]\")"), "{said}");
}

#[test]
fn a_git_header_names_its_file_only_where_its_two_halves_agree() {
    let path = |diff: &str| {
        read(diff)
            .expect("a diff to read")
            .files()
            .first()
            .map(|file| file.path().map(str::to_owned))
    };
    // A binary file whose path holds ` b/`, with no `+++` to name it.
    assert_eq!(
        path(
            "diff --git a/x b/y.png b/x b/y.png\nindex 0000000..1111111 100644\n\
             Binary files a/x b/y.png and b/x b/y.png differ\n"
        ),
        Some(Some("x b/y.png".to_owned()))
    );
    // A binary patch, likewise.
    assert_eq!(
        path("diff --git a/run b/me.bin b/run b/me.bin\nGIT binary patch\nliteral 0\n"),
        Some(Some("run b/me.bin".to_owned()))
    );
    // A change to its mode alone is no diff to draw, so it names nothing.
    assert_eq!(
        read("diff --git a/run b/me.sh b/run b/me.sh\nold mode 100644\nnew mode 100755\n"),
        Err(NotADiff::Unread)
    );
    // Halves that differ, with nothing else to name it: no path, not a guess.
    assert_eq!(
        path(
            "diff --git a/x b/y b/z\nindex 0000000..1111111 100644\n\
             Binary files differ\n"
        ),
        Some(None)
    );
    // Where `+++` names it, that name is the one drawn.
    assert_eq!(
        path(
            "diff --git a/x b/y b/x b/y\n--- a/x b/y\n+++ b/x b/y\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n"
        ),
        Some(Some("x b/y".to_owned()))
    );
}

// A change written out as plain text, for what shows a folded change whole.

#[test]
fn a_folded_change_is_written_out_with_every_line_it_keeps() {
    let change = edited(Some("long.txt"), 40);
    assert!(lines_of(&change) > FOLD);
    let written = Changes.written(&change, UNICODE);
    let lines: Vec<&str> = written.lines().collect();
    // Worded as the opener is, and then a line for each line of the file.
    assert_eq!(
        lines.first().copied(),
        Some("long.txt · Added 8 lines, removed 8 lines")
    );
    assert_eq!(lines.len(), 1 + lines_of(&change), "{written}");
    // The head, the lines the box folds, and the tail, each with its number
    // as wide as the widest and its sign.
    for kept in [
        "  1   line 1 of the file",
        "  5 - line 5 of the file",
        "  5 + line 5 of the changed file",
        " 27   line 27 of the file",
        " 30 - line 30 of the file",
        " 30 + line 30 of the changed file",
        " 36   line 36 of the file",
        " 40 + line 40 of the changed file",
    ] {
        assert!(lines.contains(&kept), "{kept:?} is not in:\n{written}");
    }
    // It is the change and not the box: no frame, no fold and no key.
    for drawn_only in ["│", "╭", "╰", "folded", EXPAND] {
        assert!(
            !written.contains(drawn_only),
            "{drawn_only:?} in:\n{written}"
        );
    }
    assert_eq!(
        Changes.written(&change, ASCII).lines().next(),
        Some("long.txt - Added 8 lines, removed 8 lines")
    );
}

#[test]
fn a_change_written_out_says_row_for_row_what_its_box_says_inside_its_frame() {
    // A tool's file with a jump in it, a tab, a line ending in spaces, and
    // lines the tool never kept.
    let jumped = Change::new(vec![
        File::numbered(
            Some("src/x.rs"),
            [
                (3, Side::Kept, "a"),
                (4, Side::Added, "\tb"),
                (1200, Side::Removed, "c"),
                (1200, Side::Kept, "d  "),
            ],
        )
        .counted(100, 100)
        .dropping(136),
    ]);
    let many = Change::new(vec![
        File::numbered(Some("x"), [(1, Side::Added, "a")]);
        MOST_FILES + 3
    ]);
    let unnamed = Change::new(vec![File::numbered(None, [(1, Side::Removed, "a")])]);
    let renamed = read(
        "diff --git a/old.rs b/new.rs\nrename from old.rs\nrename to new.rs\n\
         --- a/old.rs\n+++ b/new.rs\n@@ -1,1 +1,1 @@\n-gone\n+came\n\
         @@ -40,1 +40,1 @@ fn section\n-gone again\n+came again\n\
         diff --git a/f.png b/f.png\nindex 0000000..1111111 100644\n\
         Binary files a/f.png and b/f.png differ\n",
    )
    .expect("a diff");
    let mock = read(TWO_FILES).expect("a diff");

    for (name, change) in [
        ("jumped", &jumped),
        ("many", &many),
        ("unnamed", &unnamed),
        ("renamed", &renamed),
        ("mock", &mock),
    ] {
        for glyphs in [UNICODE, ASCII] {
            // Wide enough that the box cuts nothing, and short enough that it
            // folds nothing.
            let rows = drawn(change, 400, glyphs);
            assert!(rows.len() > 2, "{name}");
            let inside: Vec<String> = rows
                .iter()
                .take(rows.len() - 1)
                .map(|row| {
                    let text: String = row.text().chars().skip(3).collect();
                    text.trim_end().to_owned()
                })
                .collect();
            let written = Changes.written(change, glyphs);
            assert_eq!(
                written.lines().collect::<Vec<_>>(),
                inside,
                "{name} in {glyphs:?}"
            );
        }
    }
    // What stands for lines and files is worded as the box words it.
    let written = Changes.written(&jumped, UNICODE);
    assert!(
        written.contains("\n⋯  1195 lines between  ⋯\n"),
        "{written}"
    );
    assert!(
        written.ends_with("⋯  136 lines not shown  ⋯\n"),
        "{written}"
    );
    assert!(written.contains("\n   4 +     b\n"), "{written}");
    assert!(
        Changes
            .written(&many, ASCII)
            .ends_with("...  3 files not shown  ...\n")
    );
    assert!(
        Changes
            .written(&renamed, UNICODE)
            .contains("⋯  binary, not drawn  ⋯")
    );
}

#[test]
fn a_change_written_out_holds_no_control_character_but_its_line_ends() {
    let change = Change::new(vec![File::numbered(
        Some("a\u{1b}[31m.rs"),
        [
            (1, Side::Removed, "an escape \u{1b}[2J and a bell \u{7}"),
            (
                1,
                Side::Added,
                "a return \r and a second set \u{9b}2J\u{85}",
            ),
        ],
    )]);
    let written = Changes.written(&change, UNICODE);
    assert_eq!(written.lines().count(), 3, "{written:?}");
    assert!(written.contains("an escape"), "{written:?}");
    assert!(
        !written
            .chars()
            .any(|character| character.is_control() && character != '\n'),
        "{written:?}"
    );
}

// A fence of an answer, read by the reader the transcript reads with and
// drawn by this crate's drawer.

/// The registry a composition hands the reader, holding this crate's drawer.
fn changes() -> Drawers {
    Drawers::default().with(Arc::new(Changes))
}

/// `body` as an answer that is one closed fence named `name`.
fn fenced(name: &str, body: &str) -> String {
    format!("```{name}\n{body}```\n")
}

/// The first row of `rows`, as text.
fn opening(rows: &[Row]) -> String {
    rows.first()
        .map(|row| row.text().trim_end().to_owned())
        .unwrap_or_default()
}

/// What the reader says on the frame of a fence its drawer cannot hold.
const PAST: &str = "┌─ diff · longer than this can hold · shown as written";

#[test]
fn picture_of_a_two_file_fence_the_reader_read_and_the_drawer_drew() {
    let answer = fenced("diff", TWO_FILES);
    let change = read(TWO_FILES).expect("a diff");
    for (glyphs, name, second) in [
        (
            UNICODE,
            "fence_two_files_unicode",
            "├─ README.md · Added 1 line",
        ),
        (
            ASCII,
            "fence_two_files_ascii",
            "+- README.md - Added 1 line",
        ),
    ] {
        let rows = answered(&[&answer], &changes(), room(64, glyphs));
        // The fence is the box the change draws, with no frame of the
        // reader's around it and no fence line left in it.
        assert_eq!(
            dump(&rows, 64),
            dump(&drawn(&change, 64, glyphs), 64),
            "{glyphs:?}"
        );
        assert_eq!(
            rows.get(9).map(|row| row.text().trim_end().to_owned()),
            Some(second.to_owned())
        );
        insta::assert_snapshot!(name, dump(&rows, 64));
    }
}

#[test]
fn a_diff_fence_and_a_patch_fence_are_drawn_as_the_change_and_any_other_as_written() {
    let change = read(TWO_FILES).expect("a diff");
    let boxed = dump(&drawn(&change, 64, UNICODE), 64);
    for name in ["diff", "patch", "DIFF"] {
        let rows = answered(&[&fenced(name, TWO_FILES)], &changes(), room(64, UNICODE));
        assert_eq!(dump(&rows, 64), boxed, "{name}");
    }
    // A fence under another name is not this drawer's, whatever it holds.
    let rows = answered(&[&fenced("text", TWO_FILES)], &changes(), room(64, UNICODE));
    assert_eq!(opening(&rows), "┌─ text");
}

#[test]
fn a_fence_this_drawer_does_not_draw_is_byte_for_byte_the_fence_no_drawer_was_asked_about() {
    let not_a_diff = fenced("diff", "this is not a unified diff\nit has no hunks\n");
    let unclosed = format!("```diff\n{TWO_FILES}");
    for (what, answer) in [("not a diff", &not_a_diff), ("unclosed", &unclosed)] {
        for glyphs in [UNICODE, ASCII] {
            let as_written = answered(&[answer], &Drawers::default(), room(64, glyphs));
            let rows = answered(&[answer], &changes(), room(64, glyphs));
            assert_eq!(rows, as_written, "{what} in {glyphs:?}");
            assert_eq!(dump(&rows, 64), dump(&as_written, 64));
        }
        // Framed as code under its name, with no refusal on the frame.
        let rows = answered(&[answer], &changes(), room(64, UNICODE));
        assert_eq!(opening(&rows), "┌─ diff", "{what}");
    }
    // And with no drawer registered, a diff is the code it was written as.
    let rows = answered(
        &[&fenced("diff", TWO_FILES)],
        &Drawers::default(),
        room(64, UNICODE),
    );
    assert_eq!(opening(&rows), "┌─ diff");
    assert_eq!(
        rows.get(2).map(Row::text).as_deref().map(str::trim_end),
        Some("│ --- a/src/parse.rs")
    );
}

/// A diff of `files` files, each opened by its `+++` alone.
fn of_files(files: usize) -> String {
    let mut diff = String::new();
    for at in 0..files {
        let _ = write!(diff, "+++ b/f{at}.txt\n@@ -0,0 +1,1 @@\n+a\n");
    }
    diff
}

/// A diff of one file that is `lines` lines long, counting its three header
/// lines, each ended.
fn of_lines(lines: usize) -> String {
    let added = lines - 3;
    format!(
        "--- a/x\n+++ b/x\n@@ -0,0 +1,{added} @@\n{}",
        "+x\n".repeat(added)
    )
}

#[test]
fn a_fence_is_drawn_up_to_the_files_a_change_keeps_and_shown_as_written_one_past() {
    for (files, drawn_here) in [
        (MOST_FILES - 1, true),
        (MOST_FILES, true),
        (MOST_FILES + 1, false),
    ] {
        let rows = answered(
            &[&fenced("diff", &of_files(files))],
            &changes(),
            room(64, UNICODE),
        );
        if drawn_here {
            assert_eq!(opening(&rows), "╭─ f0.txt · Added 1 line", "{files} files");
            // A header and a line for each file, and the close.
            assert_eq!(rows.len(), files * 2 + 1, "{files} files");
        } else {
            // As written, in the frame that says why, every line of it.
            assert_eq!(opening(&rows), PAST, "{files} files");
            assert_eq!(rows.len(), files * 3 + 2, "{files} files");
        }
    }
}

#[test]
fn a_diff_is_read_up_to_each_of_its_caps_and_refused_one_past_it() {
    let block = |source: &str| {
        Changes
            .block(source, &room(64, UNICODE))
            .map(|laid| laid.rows.len() > 2)
    };
    // Lines, the last of them not ended: as many as a file keeps, and one more.
    for (lines, read_here) in [
        (MOST_LINES - 1, true),
        (MOST_LINES, true),
        (MOST_LINES + 1, false),
    ] {
        let source = of_lines(lines);
        let source = source.trim_end_matches('\n');
        assert_eq!(source.split('\n').count(), lines);
        let expected = if read_here {
            Ok(true)
        } else {
            Err(Undrawn::PastCap)
        };
        assert_eq!(block(source), expected, "{lines} lines");
    }
    // Bytes: one long line makes up the count.
    for (bytes, read_here) in [
        (MOST_BYTES - 1, true),
        (MOST_BYTES, true),
        (MOST_BYTES + 1, false),
    ] {
        let head = "--- a/x\n+++ b/x\n@@ -0,0 +1 @@\n+";
        let source = format!("{head}{}\n", "x".repeat(bytes - head.len() - 1));
        assert_eq!(source.len(), bytes);
        let expected = if read_here {
            Ok(true)
        } else {
            Err(Undrawn::PastCap)
        };
        assert_eq!(block(&source), expected, "{bytes} bytes");
    }
    // Files.
    for (files, read_here) in [
        (MOST_FILES - 1, true),
        (MOST_FILES, true),
        (MOST_FILES + 1, false),
    ] {
        let expected = if read_here {
            Ok(true)
        } else {
            Err(Undrawn::PastCap)
        };
        assert_eq!(block(&of_files(files)), expected, "{files} files");
    }
}

#[test]
fn a_fence_of_fewer_lines_than_a_file_keeps_is_drawn_and_one_of_as_many_is_shown_as_written() {
    let under = answered(
        &[&fenced("diff", &of_lines(MOST_LINES - 1))],
        &changes(),
        room(64, UNICODE),
    );
    assert_eq!(
        opening(&under),
        format!("╭─ x · Added {} lines", MOST_LINES - 4)
    );
    // Every line of a fence is ended, so the one that reaches the count is
    // already past it; each is shown as written, every line between its edges.
    let at = answered(
        &[&fenced("diff", &of_lines(MOST_LINES))],
        &changes(),
        room(64, UNICODE),
    );
    assert_eq!(opening(&at), PAST);
    assert_eq!(at.len(), MOST_LINES + 2);
    let past = answered(
        &[&fenced("diff", &of_lines(MOST_LINES + 1))],
        &changes(),
        room(64, UNICODE),
    );
    assert_eq!(opening(&past), PAST);
    assert_eq!(past.len(), MOST_LINES + 1 + 2);
}

/// The lines of the first fence of `answer`, between its two fence lines.
fn first_change(answer: &str) -> &str {
    let (_, after) = answer.split_once("```diff\n").expect("a diff fence");
    let end = after.find("```").expect("a closing fence");
    after.get(..end).expect("the lines of the fence")
}

#[test]
fn every_diff_fence_of_the_corpus_is_drawn_alike_however_its_window_reached_a_width() {
    let answers: Vec<(&str, &str)> = ANSWERS
        .iter()
        .copied()
        .filter(|(_, answer)| answer.contains("```diff\n") || answer.contains("```patch\n"))
        .collect();
    assert!(!answers.is_empty());
    for (name, answer) in answers {
        for (glyphs, corner) in [(UNICODE, "╭─ "), (ASCII, "+- ")] {
            // The sweep is of drawn changes: the box is there at a common width.
            let common = texts(&answered(&[answer], &changes(), room(80, glyphs)));
            assert!(
                common.iter().any(|row| row.starts_with(corner)),
                "{name}: {common:#?}"
            );
            for columns in 8..=160 {
                let fresh = answered(&[answer], &changes(), room(columns, glyphs));
                for row in &fresh {
                    assert!(
                        row.columns() <= columns,
                        "{name} at {columns} in {glyphs:?}: {:?}",
                        row.text()
                    );
                }
                let fresh = dump(&fresh, columns);
                // The box is the one the drawer lays out in that room, with
                // no row of it folded again by what keeps it.
                let laid = Changes
                    .block(first_change(answer), &room(columns, glyphs))
                    .expect("a diff");
                assert!(
                    fresh.contains(&dump(&laid.rows, columns)),
                    "{name} at {columns} in {glyphs:?}: {fresh}"
                );
                for from in [160, 8] {
                    let moved = resized(&[answer], &changes(), room(from, glyphs), columns);
                    assert_eq!(
                        dump(&moved, columns),
                        fresh,
                        "{name} at {columns} from {from} in {glyphs:?}"
                    );
                }
            }
        }
    }
}

/// A small answer with a fence of two files between two lines of prose.
const SMALL_ANSWER: &str = "Before.\n\n```diff\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-x\n+y\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1,2 @@\n z\n+w\n```\n\nAfter.\n";

#[test]
fn a_diff_fence_cut_anywhere_as_it_arrives_is_drawn_as_the_fence_whole() {
    // Every cut of a small fence of two files.
    let whole = dump(
        &answered(&[SMALL_ANSWER], &changes(), room(40, UNICODE)),
        40,
    );
    assert!(
        whole.contains("╭─</> <Path>a.rs") && whole.contains("├─</> <Path>b.rs"),
        "{whole}"
    );
    let mut cuts = 0;
    for (at, _) in SMALL_ANSWER.char_indices().skip(1) {
        let (before, after) = SMALL_ANSWER.split_at(at);
        let rows = answered(&[before, after], &changes(), room(40, UNICODE));
        assert_eq!(dump(&rows, 40), whole, "cut at {at}: {before:?}");
        cuts += 1;
    }
    assert!(cuts > 100, "{cuts}");

    // The mock's fence, cut inside each kind of line a diff holds: a file
    // header, a hunk header, a changed line, and the fence that closes it.
    let answer = format!("A change:\n\n{}\nDone.\n", fenced("diff", TWO_FILES));
    let whole = dump(&answered(&[&answer], &changes(), room(64, UNICODE)), 64);
    assert!(whole.contains("├─</> <Path>README.md"), "{whole}");
    let closing = answer.rfind("```").expect("a closing fence") + 2;
    let inside = |line: &str| answer.find(line).expect("a line of the fence") + line.len();
    for (what, at) in [
        ("a file header", inside("+++ b/src/par")),
        ("a git header", inside("diff --git a/README.md b/READ")),
        ("a hunk header", inside("@@ -40,1 +4")),
        ("a changed line", inside("+    for (at, li")),
        ("the closing fence", closing),
    ] {
        let (before, after) = answer.split_at(at);
        let rows = answered(&[before, after], &changes(), room(64, UNICODE));
        assert_eq!(dump(&rows, 64), whole, "cut inside {what}: {before:?}");
    }
}
