use crucible_tui::{Editor, Hit, Key, Pressed};

use super::super::region::Moved;
use super::{Standing, matches, sifting};

/// A picker standing over one found session, nothing typed and nothing open.
fn standing() -> Standing {
    Standing {
        query: Editor::new(),
        renaming: None,
        refused: false,
        saving: None,
        found: vec![0],
        marked: 0,
        behind: 0,
        over: 0,
        pointer: None,
        lit: None,
    }
}

#[test]
fn an_empty_query_answers_every_session() {
    // The picker opens with nothing typed and the whole list showing.
    assert!(matches("fix the caret drift", None, ""));
    assert!(matches("", None, ""));
}

#[test]
fn a_query_reaches_a_title_without_matching_its_case() {
    // Titles come from the first prompt, typed however it was typed; the
    // reader searching for one should not have to remember its shape.
    assert!(matches("Fix the caret drift", None, "caret"));
    assert!(matches("fix the caret drift", None, "CARET"));
    assert!(!matches("fix the caret drift", None, "wheel"));
}

#[test]
fn a_query_reaches_a_branch_the_same_way_it_reaches_a_title() {
    // The branch is on the row beside the title, and nothing on screen says
    // which of the two a reader is looking at when they start typing.
    assert!(matches("fix the caret drift", Some("feature/caret"), "feature"));
    assert!(matches("fix the caret drift", Some("Feature/Caret"), "feature"));
    assert!(!matches("fix the caret drift", None, "feature"));
}

#[test]
fn escape_closes_the_rename_before_it_touches_the_query() {
    // Each press undoes the innermost thing the reader built. The query under
    // the rename is theirs and stays.
    let mut standing = standing();
    standing.query.put("caret");
    standing.renaming = Some(Editor::new());

    assert_eq!(sifting(Pressed::Escape, &mut standing, None), Moved::Redraw);
    assert!(standing.renaming.is_none());
    assert_eq!(standing.query.text(), "caret");
}

#[test]
fn escape_clears_the_query_before_it_leaves() {
    // Clearing widens the list back out, so the mark walks back to the top
    // rather than standing on whatever row number the narrowing had lit.
    let mut standing = standing();
    standing.query.put("caret");
    standing.marked = 2;
    standing.behind = 3;

    assert_eq!(sifting(Pressed::Escape, &mut standing, None), Moved::Redraw);
    assert!(standing.query.is_empty());
    assert_eq!(standing.marked, 0);
    assert_eq!(standing.behind, 0);
}

#[test]
fn escape_leaves_only_once_nothing_is_left_to_undo() {
    assert_eq!(sifting(Pressed::Escape, &mut standing(), None), Moved::Left);
}

#[test]
fn an_interrupt_leaves_through_everything_at_once() {
    // Ctrl+C is not a step back, it is the reader closing the whole thing.
    let mut standing = standing();
    standing.query.put("caret");
    standing.renaming = Some(Editor::new());

    assert_eq!(sifting(Pressed::Key(Key::Interrupt), &mut standing, None), Moved::Left);
}

#[test]
fn the_rename_key_opens_over_the_title_the_row_already_has() {
    // Most renames are edits, not replacements; starting from an empty line
    // would make the reader retype the part they wanted to keep.
    let mut standing = standing();

    assert_eq!(
        sifting(Pressed::Rename, &mut standing, Some("fix the caret drift")),
        Moved::Redraw
    );
    let renaming = standing.renaming.expect("a rename should be open");
    assert_eq!(renaming.text(), "fix the caret drift");
}

#[test]
fn the_rename_key_does_nothing_where_nothing_is_marked() {
    // The narrowing left no rows, so there is no title for the key to open.
    let mut standing = standing();
    standing.found.clear();

    assert_eq!(sifting(Pressed::Rename, &mut standing, None), Moved::Still);
    assert!(standing.renaming.is_none());
}

#[test]
fn enter_stages_the_accepted_title_and_closes_the_rename() {
    // Staged rather than written: the session index the title goes into is
    // the caller's to reach, and the caller drains this on the next frame.
    let mut standing = standing();
    let mut renaming = Editor::new();
    renaming.put("caret drift, round two");
    standing.renaming = Some(renaming);

    assert_eq!(
        sifting(Pressed::Key(Key::Enter), &mut standing, None),
        Moved::Redraw
    );
    assert!(standing.renaming.is_none());
    assert_eq!(standing.saving.as_deref(), Some("caret drift, round two"));
}

#[test]
fn enter_refuses_a_title_with_nothing_in_it() {
    // A session with no title falls back to its first prompt, so an empty
    // rename would not stick anyway; refusing it in place says so.
    let mut standing = standing();
    standing.renaming = Some(Editor::new());

    assert_eq!(
        sifting(Pressed::Key(Key::Enter), &mut standing, None),
        Moved::Redraw
    );
    assert!(standing.renaming.is_some());
    assert!(standing.refused);
    assert!(standing.saving.is_none());
}

#[test]
fn typing_lands_in_the_rename_while_one_is_open() {
    let mut standing = standing();
    standing.query.put("caret");
    standing.renaming = Some(Editor::new());

    assert_eq!(
        sifting(Pressed::Key(Key::Char('x')), &mut standing, None),
        Moved::Redraw
    );
    let renaming = standing.renaming.expect("the rename should stay open");
    assert_eq!(renaming.text(), "x");
    assert_eq!(standing.query.text(), "caret");
}

#[test]
fn enter_refuses_where_the_query_left_nothing() {
    // A picker that closed on a query matching nothing would be the list
    // disagreeing with the search line the reader is looking at.
    let mut standing = standing();
    standing.found.clear();

    assert_eq!(
        sifting(Pressed::Key(Key::Enter), &mut standing, None),
        Moved::Still
    );
}

#[test]
fn enter_takes_the_marked_session() {
    assert_eq!(
        sifting(Pressed::Key(Key::Enter), &mut standing(), None),
        Moved::Took
    );
}

#[test]
fn the_arrows_walk_the_list_and_stop_at_its_ends() {
    let mut standing = standing();
    standing.found = vec![0, 1, 2];

    assert_eq!(sifting(Pressed::Down, &mut standing, None), Moved::Redraw);
    assert_eq!(standing.marked, 1);
    assert_eq!(sifting(Pressed::Up, &mut standing, None), Moved::Redraw);
    assert_eq!(standing.marked, 0);
    assert_eq!(sifting(Pressed::Up, &mut standing, None), Moved::Still);
    assert_eq!(standing.marked, 0);

    standing.marked = 2;
    assert_eq!(sifting(Pressed::Down, &mut standing, None), Moved::Still);
    assert_eq!(standing.marked, 2);
}

#[test]
fn moving_the_mark_reopens_the_preview_at_its_tail() {
    // The window the reader had scrolled belonged to the session they were
    // reading; the next one opens where a session is picked up — the end.
    let mut standing = standing();
    standing.found = vec![0, 1];
    standing.behind = 4;

    assert_eq!(sifting(Pressed::Down, &mut standing, None), Moved::Redraw);
    assert_eq!(standing.behind, 0);
}

#[test]
fn the_wheel_over_the_list_steps_the_mark() {
    let mut standing = standing();
    standing.found = vec![0, 1];
    standing.lit = Some(Hit::Session(0));

    assert_eq!(
        sifting(Pressed::Scrolled { back: false }, &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.marked, 1);
    assert_eq!(
        sifting(Pressed::Scrolled { back: true }, &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.marked, 0);
}

#[test]
fn the_wheel_over_the_preview_walks_the_tail_no_further_than_it_goes() {
    let mut standing = standing();
    standing.lit = Some(Hit::Preview);
    standing.over = 2;

    assert_eq!(
        sifting(Pressed::Scrolled { back: true }, &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(
        sifting(Pressed::Scrolled { back: true }, &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.behind, 2);
    assert_eq!(
        sifting(Pressed::Scrolled { back: true }, &mut standing, None),
        Moved::Still
    );

    assert_eq!(
        sifting(Pressed::Scrolled { back: false }, &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.behind, 1);
}

#[test]
fn the_wheel_over_nothing_of_the_pickers_is_not_answered() {
    // Still hands the wheel back to the loop, whose transcript it scrolls.
    let mut standing = standing();

    assert_eq!(
        sifting(Pressed::Scrolled { back: true }, &mut standing, None),
        Moved::Still
    );

    standing.lit = Some(Hit::Nothing);
    assert_eq!(
        sifting(Pressed::Scrolled { back: true }, &mut standing, None),
        Moved::Still
    );
}

#[test]
fn a_pointer_report_redraws_only_where_it_moved() {
    let mut standing = standing();

    assert_eq!(
        sifting(Pressed::Hovered { row: 3, column: 7 }, &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.pointer, Some((3, 7)));
    assert_eq!(
        sifting(Pressed::Hovered { row: 3, column: 7 }, &mut standing, None),
        Moved::Still
    );

    // The row a terminal reports when the pointer left the window entirely.
    assert_eq!(
        sifting(
            Pressed::Hovered {
                row: usize::MAX,
                column: 0
            },
            &mut standing,
            None
        ),
        Moved::Redraw
    );
    assert_eq!(standing.pointer, None);
}

#[test]
fn a_click_lights_a_row_before_a_second_takes_it() {
    let mut standing = standing();
    standing.found = vec![0, 1];

    // The first click landed where nothing was lit; it lights and stops.
    assert_eq!(
        sifting(Pressed::Clicked { row: 3, column: 7 }, &mut standing, None),
        Moved::Redraw
    );

    // The frame lit the second row under the pointer; a click marks it.
    standing.lit = Some(Hit::Session(1));
    assert_eq!(
        sifting(Pressed::Clicked { row: 3, column: 7 }, &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.marked, 1);

    // And a click on the row the mark already stands on takes it.
    assert_eq!(
        sifting(Pressed::Clicked { row: 3, column: 7 }, &mut standing, None),
        Moved::Took
    );
}

#[test]
fn typing_narrows_and_walks_the_mark_back_to_the_top() {
    let mut standing = standing();
    standing.found = vec![0, 1, 2];
    standing.marked = 2;
    standing.behind = 4;

    assert_eq!(
        sifting(Pressed::Key(Key::Char('c')), &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.query.text(), "c");
    assert_eq!(standing.marked, 0);
    assert_eq!(standing.behind, 0);
}

#[test]
fn moving_the_cursor_leaves_the_mark_where_it_stood() {
    // The query is the same query, so the list under it is the same list.
    let mut standing = standing();
    standing.query.put("caret");
    standing.marked = 1;

    assert_eq!(
        sifting(Pressed::Key(Key::Left), &mut standing, None),
        Moved::Redraw
    );
    assert_eq!(standing.marked, 1);
}
