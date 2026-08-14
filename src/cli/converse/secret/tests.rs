//! What the keys do to a line nobody can see.

use super::*;

/// The line after every one of `keys` has been pressed against it, and what the
/// last of them did.
fn typed(keys: &[Pressed]) -> (String, Typing) {
    let mut held = String::new();
    let mut last = Typing::Still;

    for key in keys {
        last = typing(*key, &mut held);
    }

    (held, last)
}

/// One character, spelled the way a keyboard sends it.
fn character(typed: char) -> Pressed {
    Pressed::Key(Key::Char(typed))
}

#[test]
fn what_is_typed_is_held_and_the_dots_follow_it() {
    let (held, last) = typed(&[character('s'), character('k'), character('-')]);

    assert_eq!(held, "sk-");
    assert_eq!(last, Typing::Redraw);
}

#[test]
fn backspace_rubs_out_from_the_end_and_costs_no_frame_at_the_start() {
    // The one edit that needs no sight of the line. Against an empty one there
    // is nothing to rub out, and a frame drawn for it would be a frame per
    // press for somebody holding the key down.
    let (held, last) = typed(&[character('a'), character('b'), Pressed::Key(Key::Backspace)]);

    assert_eq!(held, "a");
    assert_eq!(last, Typing::Redraw);

    assert_eq!(typed(&[Pressed::Key(Key::Backspace)]).1, Typing::Still);
}

#[test]
fn the_keys_that_would_move_a_cursor_move_nothing() {
    // There is no cursor to move: the line cannot be read, so an insertion
    // point in the middle of it is one somebody would have to guess at. A key
    // that does nothing also draws nothing.
    for moving in [Key::Left, Key::Right, Key::WordLeft, Key::Home, Key::End] {
        let (held, last) = typed(&[character('a'), Pressed::Key(moving), character('b')]);

        assert_eq!(held, "ab", "{moving:?}");
        assert_eq!(last, Typing::Redraw, "{moving:?}");
    }
}

#[test]
fn return_finishes_the_line_and_the_keys_that_end_a_session_leave_it() {
    assert_eq!(typed(&[Pressed::Key(Key::Enter)]).1, Typing::Done);

    for leaving in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
    ] {
        assert_eq!(
            typed(&[character('a'), leaving]).1,
            Typing::Left,
            "{leaving:?}"
        );
    }
}

#[test]
fn a_window_that_changed_size_is_answered_by_drawing_again() {
    let (held, last) = typed(&[character('a'), Pressed::Resized]);

    assert_eq!(held, "a", "a resize is not an edit");
    assert_eq!(last, Typing::Redraw);
}

#[test]
fn a_key_pasted_with_whatever_the_clipboard_had_round_it_is_trimmed() {
    // A key arrives by paste far more often than it is typed, and a paste
    // carries the newline the copy took with it. Sent as it stands, every
    // vendor refuses it with a sentence about the key being wrong.
    assert_eq!(taken("  sk-a-key\n").as_deref(), Some("sk-a-key"));

    // Nothing left after that is the same answer as leaving, because it is the
    // same thing: a key was asked for and none was given. Writing the empty
    // string down would be a provider that looks logged in and is refused by
    // the vendor every turn.
    for blank in ["", "   ", "\n"] {
        assert_eq!(taken(blank), None, "{blank:?}");
    }
}
