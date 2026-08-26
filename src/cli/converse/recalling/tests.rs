//! Walking back through what was asked, and what ends the walk.

use std::fs;

use super::*;

use crate::cli::sample::Sample;

/// A box with `said` typed into it, cursor at the end.
///
/// An empty one is the box a session opens with, so putting nothing in it is
/// the ordinary case here rather than the refusal it reads as.
fn typed(said: &str) -> Editor {
    let mut editor = Editor::new().multiline();
    assert_ne!(editor.put(said), crucible_tui::Typed::Refused);
    editor
}

/// A walk over `held`, oldest first, with nowhere to write.
fn over(held: &[&str]) -> Recalling {
    Recalling::holding(held.iter().map(|said| (*said).to_owned()).collect())
}

#[test]
fn back_reaches_the_newest_prompt_first_and_older_ones_after() {
    let mut recalling = over(&["first", "second", "third"]);
    let mut editor = typed("");

    assert!(recalling.back(&mut editor));
    assert_eq!(editor.text(), "third");

    assert!(recalling.back(&mut editor));
    assert_eq!(editor.text(), "second");
}

#[test]
fn the_walk_stops_at_the_oldest_rather_than_wrapping() {
    // Wrapping would put the newest prompt back under a key somebody is
    // holding down to reach the oldest, and they would send it thinking they
    // had arrived somewhere.
    let mut recalling = over(&["first", "second"]);
    let mut editor = typed("");

    assert!(recalling.back(&mut editor));
    assert!(recalling.back(&mut editor));
    assert!(!recalling.back(&mut editor));
    assert_eq!(editor.text(), "first");
}

#[test]
fn on_walks_forward_and_gives_the_line_back_at_the_end_of_it() {
    // The half-written line is the reason the walk is reversible at all: a
    // reader who reached back to check something has not finished writing.
    let mut recalling = over(&["first", "second"]);
    let mut editor = typed("half a thought");

    assert!(recalling.back(&mut editor));
    assert!(recalling.back(&mut editor));
    assert_eq!(editor.text(), "first");

    assert!(recalling.on(&mut editor));
    assert_eq!(editor.text(), "second");

    assert!(recalling.on(&mut editor));
    assert_eq!(editor.text(), "half a thought");
}

#[test]
fn forward_from_a_line_nobody_reached_back_from_moves_nothing() {
    // Down at rest belongs to whatever is standing over the box, so the walk
    // has to say it did nothing rather than empty the line.
    let mut recalling = over(&["first"]);
    let mut editor = typed("still being written");

    assert!(!recalling.on(&mut editor));
    assert_eq!(editor.text(), "still being written");
}

#[test]
fn back_through_nothing_moves_nothing() {
    let mut recalling = over(&[]);
    let mut editor = typed("the only line there is");

    assert!(!recalling.back(&mut editor));
    assert_eq!(editor.text(), "the only line there is");
    assert_eq!(recalling.place(), Recalled::default());
}

#[test]
fn the_place_counts_the_presses_back_against_a_window_that_does_not_move() {
    // Which is the number the top border says. It rises with the key, so what
    // it reports is how far back the reader has come; the second is the window
    // it is counted against, and three prompts held is still `1/100` because
    // what a reader is told is how far they may go and not how much they have
    // typed.
    let mut recalling = over(&["first", "second", "third"]);
    let mut editor = typed("");

    assert_eq!(recalling.place(), Recalled::default());

    recalling.back(&mut editor);
    assert_eq!(recalling.place(), Recalled::new(1, PROMPTS));

    recalling.back(&mut editor);
    assert_eq!(recalling.place(), Recalled::new(2, PROMPTS));

    recalling.back(&mut editor);
    assert_eq!(recalling.place(), Recalled::new(3, PROMPTS));
}

#[test]
fn the_walk_ends_where_the_line_is_edited_and_the_place_goes_with_it() {
    // Any edit at all, which is what the reader is told by the number
    // vanishing: from that key on the line is theirs, and the arrows are back
    // to whatever else they mean.
    let mut recalling = over(&["first", "second"]);
    let mut editor = typed("");

    recalling.back(&mut editor);
    assert_ne!(recalling.place(), Recalled::default());

    recalling.left();
    assert_eq!(
        editor.text(),
        "second",
        "the line stays; only the walk ends"
    );
    assert_eq!(recalling.place(), Recalled::default());
}

#[test]
fn a_walk_that_ended_starts_again_from_the_newest() {
    let mut recalling = over(&["first", "second"]);
    let mut editor = typed("");

    recalling.back(&mut editor);
    recalling.back(&mut editor);
    recalling.left();

    assert!(recalling.back(&mut editor));
    assert_eq!(editor.text(), "second");
    assert_eq!(recalling.place(), Recalled::new(1, PROMPTS));
}

#[test]
fn a_line_that_was_sent_is_the_next_walk_s_newest() {
    let mut recalling = over(&["first"]);
    let mut editor = typed("");

    recalling.keep("second");

    assert!(recalling.back(&mut editor));
    assert_eq!(editor.text(), "second");
    assert_eq!(recalling.place(), Recalled::new(1, PROMPTS));
}

#[test]
fn keeping_a_line_ends_whatever_walk_was_open() {
    // Enter is a selection, and the top border says nothing about a box that
    // has just been emptied by one.
    let mut recalling = over(&["first", "second"]);
    let mut editor = typed("");

    recalling.back(&mut editor);
    recalling.keep(&editor.take());

    assert_eq!(recalling.place(), Recalled::default());
}

#[test]
fn a_line_of_nothing_is_not_kept() {
    let mut recalling = over(&["first"]);
    let mut editor = typed("");

    recalling.keep("   \n ");

    assert!(recalling.back(&mut editor));
    assert_eq!(editor.text(), "first");
    assert_eq!(recalling.place(), Recalled::new(1, PROMPTS));
}

#[test]
fn what_is_kept_in_memory_is_bounded_by_what_a_walk_may_reach() {
    // The second number is the assertion: it is how many prompts are there to
    // reach, and a session that sent more than the window keeps the window.
    let mut recalling = over(&[]);

    for nth in 0..PROMPTS + 5 {
        recalling.keep(&format!("prompt {nth}"));
    }

    let mut editor = typed("");
    recalling.back(&mut editor);
    assert_eq!(recalling.place(), Recalled::new(1, PROMPTS));
}

#[test]
fn a_line_sent_here_is_offered_back_to_the_next_session_in_this_directory() {
    // The whole reason the walk reaches past the session it is in. Written on
    // the way out of the box rather than at parting, because a session that
    // crashed still asked what it asked.
    let sample = Sample::new("recalling-persists");
    fs::create_dir_all(sample.logs()).expect("a temporary directory");

    let mut first = Recalling::new(sample.logs(), sample.workspace());
    first.keep("rename the tail's bound");

    let mut next = Recalling::new(sample.logs(), sample.workspace());
    let mut editor = typed("");

    assert!(next.back(&mut editor));
    assert_eq!(editor.text(), "rename the tail's bound");
}

#[test]
fn a_directory_that_cannot_be_written_to_costs_the_history_and_not_the_prompt() {
    // The file is decoration on a key nobody has to press. A session that
    // refused a prompt because it could not write one down would be trading
    // the thing somebody asked for against the convenience of asking it again.
    let sample = Sample::new("recalling-unwritable");
    let mut recalling = Recalling::new(sample.logs(), sample.workspace());

    recalling.keep("said all the same");

    let mut editor = typed("");
    assert!(recalling.back(&mut editor));
    assert_eq!(editor.text(), "said all the same");
}

#[test]
fn a_key_that_only_moved_the_cursor_leaves_the_walk_standing() {
    // The border says where the line came from, not where in it somebody is
    // looking. Home and the arrows along a line change neither.
    let mut recalling = over(&["first", "second"]);
    let mut editor = typed("");

    recalling.back(&mut editor);
    editor.press(crucible_tui::Key::Home);
    recalling.standing(&editor);

    assert_eq!(recalling.place(), Recalled::new(1, PROMPTS));
}

#[test]
fn a_key_that_changed_the_line_ends_the_walk() {
    // Backspace among them, which is the one somebody presses when they have
    // reached the prompt they wanted and are about to make it theirs.
    let mut recalling = over(&["first", "second"]);
    let mut editor = typed("");

    recalling.back(&mut editor);
    editor.press(crucible_tui::Key::Backspace);
    recalling.standing(&editor);

    assert_eq!(recalling.place(), Recalled::default());
    assert_eq!(editor.text(), "secon", "the line stays as it was edited");
}

#[test]
fn a_walk_nobody_opened_is_not_ended_by_typing() {
    let mut recalling = over(&["first"]);
    let editor = typed("a line of somebody's own");

    recalling.standing(&editor);

    assert_eq!(recalling.place(), Recalled::default());
}
