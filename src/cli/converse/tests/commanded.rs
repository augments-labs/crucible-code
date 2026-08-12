//! What a command does to the loop it was typed into.
//!
//! Which lines are commands at all is settled in [`super::super::command`],
//! over strings and without a terminal. What is left for here is everything
//! that needs the loop running: that a command costs the provider nothing, that
//! it answers on the screen, that `/mode` moves the mode the next prompt is
//! taken under, and that `/exit` ends the session with the lines after it
//! unread.

use super::*;

/// The whole loop over lines that ask for no turn: what was drawn, and the
/// count that says the provider was never reached.
fn commanding(typed: &str) -> (String, usize) {
    over(Script::new(vec![saying("answered")]), Tools::new(), typed)
}

#[test]
fn a_command_is_answered_here_rather_than_by_the_model() {
    let (written, asked) = commanding("/help\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("/model"), "{written}");
    assert!(written.contains("which model answers"), "{written}");
}

#[test]
fn the_model_a_session_is_asking_is_the_one_it_was_built_with() {
    let (written, _) = commanding("/model\n");

    assert!(written.contains("script"), "{written}");
}

#[test]
fn a_word_shaped_like_a_command_that_names_none_says_so_and_lists_what_there_is() {
    // Said back so it can be seen to be a typo, and the list under it so the
    // next thing typed is the right one. Nothing is a turn: a mistyped command
    // that reached the provider would be a request paid for by a slip.
    let (written, asked) = commanding("/hlep\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("! no such command: /hlep"), "{written}");
    assert!(written.contains("what these are"), "{written}");
}

#[test]
fn a_line_that_opens_with_a_path_is_a_prompt_and_takes_a_turn() {
    let (written, asked) = commanding("/etc/hosts is wrong\n");

    assert_eq!(asked, 1, "{written}");
    assert!(written.contains("answered"), "{written}");
}

#[test]
fn naming_a_mode_puts_the_session_in_it() {
    // Read off the mark in front of the next line, which is where a session
    // with no box to type into says which mode it is in. The mode is the
    // engine's, so this is also what says the switch outlived the command.
    let (written, asked) = commanding("/mode allowEdits\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("allow edits on"), "{written}");
    assert!(written.contains("allowEdits › "), "{written}");
}

#[test]
fn asking_which_mode_is_in_force_changes_none_of_it() {
    let (written, _) = commanding("/mode\n");

    assert!(written.contains("ask mode on"), "{written}");
    assert!(
        written.contains("ask · allowEdits · fullAccess"),
        "{written}"
    );
    assert!(!written.contains("allowEdits › "), "{written}");
}

#[test]
fn a_word_that_names_no_mode_leaves_the_session_where_it_was() {
    let (written, _) = commanding("/mode sideways\n");

    assert!(written.contains("! sideways is not a mode"), "{written}");
    assert!(
        written.contains("ask · allowEdits · fullAccess"),
        "{written}"
    );
    assert!(!written.contains("mode on"), "{written}");
}

#[test]
fn forgetting_says_how_much_it_forgot_and_leaves_the_session_running() {
    // The turn before it is what there is to forget: a prompt and the answer to
    // it. The line after it is answered as normal, which is the difference
    // between forgetting a session and ending one.
    let (written, asked) = commanding("hello\n/clear\nhello again\n");

    assert_eq!(asked, 2, "{written}");
    assert!(written.contains("forgotten: 2 messages"), "{written}");
    assert!(
        written.contains("what is on screen stays where it is"),
        "{written}"
    );
}

#[test]
fn forgetting_before_anything_was_said_says_there_was_nothing_to_forget() {
    let (written, asked) = commanding("/clear\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("nothing had been said"), "{written}");
}

#[test]
fn leaving_ends_the_session_with_what_follows_it_unread() {
    let (written, asked) = commanding("/exit\nand this\n");

    assert_eq!(asked, 0, "{written}");
    assert!(!written.contains("answered"), "{written}");
}
