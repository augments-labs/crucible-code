//! The look a change is drawn in, read back from the rows: the box, the
//! numbers, the raised words, the fold and the gaps, and the pictures checked
//! in beside the drawer.

use std::fmt::Write as _;

use crucible_tui::drawing::{Draws, Room};
use crucible_tui::testing::dump;
use crucible_tui::{Glyphs, Row, Slot};

use crate::{Change, Changes, FOLD, File, Line, NotADiff, Side, read};

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
fn a_change_opens_with_its_path_and_counts_and_closes_its_box() {
    let change = read(TWO_FILES).expect("the mock is a diff");
    let rows = drawn(&change, 64, UNICODE);
    let pictured = dump(&rows, 64);
    let mut lines = pictured.lines();
    assert_eq!(
        lines.next().map(str::trim_end),
        Some("<Quiet>╭─</> <Path>src/parse.rs</> <AddedNumber>+3</> <RemovedNumber>−2</>")
    );
    assert!(
        pictured.contains("<Quiet>├─</> <Path>README.md</> <AddedNumber>+1</>"),
        "{pictured}"
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
fn a_count_of_nothing_is_left_off_the_header() {
    let change = read("--- a/x\n+++ b/x\n@@ -1,2 +1,1 @@\n a\n-b\n").expect("a diff");
    let rows = drawn(&change, 64, UNICODE);
    assert_eq!(texts(&rows).first().map(String::as_str), Some("╭─ x −1"));
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
        pictured.contains("<Quiet>│</> <AddedNumber>  12 +</> <Added>        tree.mark(at);"),
        "{pictured}"
    );
    // A changed line carries its ground to the last column.
    for row in drawn(&change, 64, UNICODE) {
        if row
            .kinds()
            .any(|slot| slot == Slot::Added || slot == Slot::Removed)
        {
            assert_eq!(row.columns(), 64, "{:?}", row.text());
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
            "<RemovedNumber>  11 -</> <Removed>    for </><RemovedWord>line</><Removed> in input.lines() {"
        ),
        "{pictured}"
    );
    assert!(
        pictured.contains(
            "<AddedNumber>  11 +</> <Added>    for </><AddedWord>(at, line)</><Added> in input.lines()</><AddedWord>.enumerate()</><Added> {"
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
        format!("│  ⋯  {} lines folded  ⋯", total - 28)
    );
    // Nothing yet shows the folded lines, so the fold names no key and holds
    // nothing that lights under the pointer as an offer would.
    assert!(
        fold.spans().all(|(slot, _)| slot != Slot::Cut),
        "{:?}",
        fold.spans().collect::<Vec<_>>()
    );
    // Every line it folded is still in the change.
    assert_eq!(lines_of(&change), total);
    let ascii = drawn(&change, 80, ASCII);
    assert_eq!(
        ascii
            .get(25)
            .map(|row| row.text().trim_end().to_owned())
            .unwrap_or_default(),
        format!("|  ...  {} lines folded  ...", total - 28)
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
fn lines_the_tool_never_kept_are_a_count_inside_the_box() {
    let change = Change::new(vec![
        File::numbered(Some("x"), [(1, Side::Added, "a")])
            .counted(100, 100)
            .dropping(136),
    ]);
    let rows = texts(&drawn(&change, 64, UNICODE));
    assert_eq!(rows.first().map(String::as_str), Some("╭─ x +100 −100"));
    assert_eq!(
        rows.get(rows.len() - 2).map(String::as_str),
        Some("│  ⋯  136 lines not shown  ⋯")
    );
}

#[test]
fn a_change_with_no_path_opens_on_its_counts_alone() {
    let change = Change::new(vec![File::numbered(None, [(1, Side::Added, "a")])]);
    assert_eq!(
        texts(&drawn(&change, 64, UNICODE))
            .first()
            .map(String::as_str),
        Some("╭─ +1")
    );
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
    assert!(ascii.starts_with("+- src/parse.rs +3 -2"), "{ascii}");
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

// The pictures, at 64 columns, in both glyph sets.

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

#[test]
fn the_caps_the_user_docs_state_are_the_ones_a_change_is_drawn_to() {
    // The caps a tool's edit reaches; a diff the model writes in a reply is
    // not drawn in a box yet, so the caps only it reaches are not stated.
    use crate::{PAIRS, SYNTAX_BYTES, SYNTAX_LINES, TOKENS};
    let documented = include_str!("../../../docs/tools/files.md");
    let documented = documented.split_whitespace().collect::<Vec<_>>().join(" ");
    for stated in [
        format!("reads at most {SYNTAX_LINES} of its lines, and {SYNTAX_BYTES} bytes of them"),
        format!("in at most {PAIRS} replaced lines, each of at most {TOKENS} words"),
    ] {
        assert!(
            documented.contains(&stated),
            "docs/tools/files.md does not say {stated:?}"
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
        Some(format!("│  ⋯  {hidden} lines folded  ⋯").as_str()),
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
