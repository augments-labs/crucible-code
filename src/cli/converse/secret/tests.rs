//! What the keys do to a line nobody can see.

use crucible_tui::Glyphs;

use super::*;

/// The line after every one of `keys` has been pressed against it, and what the
/// last of them did.
fn typed(keys: &[Pressed]) -> (String, Moved) {
    let mut held = String::new();
    let mut last = Moved::Still;

    for key in keys {
        last = typing(key.clone(), &mut held);
    }

    (held, last)
}

/// One character, spelled the way a keyboard sends it.
fn character(typed: char) -> Pressed {
    Pressed::Key(Key::Char(typed))
}

#[test]
fn what_is_typed_is_held_and_a_mark_follows_each_character() {
    let (held, last) = typed(&[character('s'), character('k'), character('-')]);

    assert_eq!(held, "sk-");
    assert_eq!(last, Moved::Redraw);
}

#[test]
fn backspace_rubs_out_from_the_end_and_costs_no_frame_at_the_start() {
    // The one edit that needs no sight of the line. Against an empty one there
    // is nothing to rub out, and a frame drawn for it would be a frame per
    // press for somebody holding the key down.
    let (held, last) = typed(&[character('a'), character('b'), Pressed::Key(Key::Backspace)]);

    assert_eq!(held, "a");
    assert_eq!(last, Moved::Redraw);

    assert_eq!(typed(&[Pressed::Key(Key::Backspace)]).1, Moved::Still);
}

#[test]
fn the_keys_that_would_move_a_cursor_move_nothing() {
    // There is no cursor to move: the line cannot be read, so an insertion
    // point in the middle of it is one somebody would have to guess at. A key
    // that does nothing also draws nothing.
    for moving in [Key::Left, Key::Right, Key::WordLeft, Key::Home, Key::End] {
        let (held, last) = typed(&[character('a'), Pressed::Key(moving), character('b')]);

        assert_eq!(held, "ab", "{moving:?}");
        assert_eq!(last, Moved::Redraw, "{moving:?}");
    }
}

#[test]
fn return_finishes_a_held_line_and_the_keys_that_end_a_session_leave_it() {
    assert_eq!(
        typed(&[character('a'), Pressed::Key(Key::Enter)]).1,
        Moved::Took
    );

    for leaving in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
    ] {
        assert_eq!(
            typed(&[character('a'), leaving.clone()]).1,
            Moved::Left,
            "{leaving:?}"
        );
    }
}

#[test]
fn a_window_that_changed_size_is_answered_by_drawing_again() {
    let (held, last) = typed(&[character('a'), Pressed::Resized]);

    assert_eq!(held, "a", "a resize is not an edit");
    assert_eq!(last, Moved::Redraw);
}

#[test]
fn a_character_past_the_box_ceiling_is_refused_and_nothing_is_said() {
    // The bound exists so a pasted file cannot become secret-shaped process
    // memory: what does not fit is not retained, and the box says nothing
    // about it. No row of prose beneath the frame, no row anywhere: the rows
    // after the refusal are the rows before it.
    let mut held = "x".repeat(MAX_BYTES);
    let before = standing("Anthropic", &held, 80, 24, Glyphs::Unicode);

    assert_eq!(
        typing(character('x'), &mut held),
        Moved::Still,
        "one past the ceiling"
    );
    assert_eq!(held.len(), MAX_BYTES, "the refusal retains nothing more");
    assert_eq!(
        standing("Anthropic", &held, 80, 24, Glyphs::Unicode),
        before
    );
}

#[test]
fn what_stands_in_for_a_character_comes_out_of_the_glyph_set() {
    // One mark per character is the whole of what this box draws about the
    // line, so a terminal whose font has no dot draws a row of hollow squares
    // over a key being pasted — on the one row where there is nothing else to
    // check what was typed against.
    for (glyphs, mark) in [(Glyphs::Unicode, "•"), (Glyphs::Ascii, "*")] {
        let (rows, _) = standing("Anthropic", "abc", 80, 24, glyphs);

        assert!(
            rows.iter().any(|row| row.text().contains(&mark.repeat(3))),
            "{glyphs:?}"
        );
    }
}

#[test]
fn no_row_of_the_standing_screen_carries_the_key() {
    // The count is all the panel is handed, so the characters have nowhere
    // to reach a row from. Checked anyway, on the one screen where a slip
    // would put a key in a terminal's scrollback.
    let key = "sk-ant-secret-1234";
    let (rows, _) = standing("Anthropic", key, 80, 24, Glyphs::Unicode);

    assert!(rows.iter().all(|row| !row.text().contains("secret")));
    assert!(rows.iter().all(|row| !row.text().contains("sk-ant")));
    assert!(!format!("{rows:?}").contains(key));
}

#[test]
fn unicode_hidden_marks_leave_the_caret_after_every_mark() {
    // The mark is one column wide in either set and more than one byte in
    // one of them; the caret is counted in characters, not bytes.
    let (rows, caret) = standing("Anthropic", "ab", 80, 24, Glyphs::Unicode);

    assert_eq!(caret, Some(Caret { row: 9, column: 6 }));
    assert!(rows.get(9).is_some_and(|row| row.text().contains("••")));
}

#[test]
fn a_key_with_surrounding_whitespace_is_trimmed() {
    // A copied key often carries spaces around it. Sent as it stands, every
    // provider refuses it with a sentence about the key being wrong.
    assert_eq!(taken("  sk-a-key\n").as_deref(), Some("sk-a-key"));

    // Nothing left after that is the same answer as leaving, because it is the
    // same thing: a key was asked for and none was given. Writing the empty
    // string down would be a provider that looks logged in and is refused by
    // the vendor every turn.
    for blank in ["", "   ", "\n"] {
        assert_eq!(taken(blank), None, "{blank:?}");
    }
}

/// One paste, the way a bracketed terminal sends it.
fn pasted(text: &str) -> Pressed {
    Pressed::Pasted(text.into())
}

#[test]
fn a_paste_is_held_whole_and_draws_one_mark_per_character() {
    // A key reaches this box by paste more often than by typing, and comes
    // with what a clipboard carries: the spaces and the newline around it, and
    // now and then a control character. What is held is the key alone, so what
    // Enter hands over is what the provider expects.
    let key = format!("sk-ant-{}", "k".repeat(55));
    let (held, last) = typed(&[pasted(&format!("  {key}\n"))]);

    assert_eq!(held, key);
    assert_eq!(last, Moved::Redraw);
    assert_eq!(taken(&held).as_deref(), Some(key.as_str()));
    let (rows, _) = standing("Anthropic", &held, 80, 24, Glyphs::Unicode);
    assert!(rows.iter().any(|row| row.text().contains(&"•".repeat(62))));

    let (held, _) = typed(&[pasted("ab\u{7}c\r")]);
    assert_eq!(held, "abc", "control characters are dropped");
}

#[test]
fn return_on_an_empty_box_does_nothing_and_the_box_goes_on_standing() {
    // Nothing to save is nothing to do: the box stays exactly as it was, and
    // the next key is read against it.
    assert_eq!(typed(&[Pressed::Key(Key::Enter)]).1, Moved::Still);
    assert_eq!(
        typed(&[character(' '), Pressed::Key(Key::Enter)]).1,
        Moved::Still,
        "blank is empty"
    );

    let (held, last) = typed(&[
        Pressed::Key(Key::Enter),
        character('a'),
        Pressed::Key(Key::Enter),
    ]);
    assert_eq!(held, "a");
    assert_eq!(last, Moved::Took);
}

#[test]
fn a_paste_past_the_box_ceiling_is_refused_whole_and_nothing_is_said() {
    // What does not fit is not retained, none of it: a paste is one unit, and
    // half a key is worth less than no key. The box says nothing about it —
    // no supported credential comes near the bound, so what was pasted was
    // not a key, and the marks not growing is the whole of the answer.
    let before = "x".repeat(MAX_BYTES - 1);
    let mut held = before.clone();

    let rows = standing("Anthropic", &held, 80, 24, Glyphs::Unicode);

    assert_eq!(typing(pasted("yy"), &mut held), Moved::Still);
    assert_eq!(held, before, "the refusal retains nothing");
    assert_eq!(standing("Anthropic", &held, 80, 24, Glyphs::Unicode), rows);
}
