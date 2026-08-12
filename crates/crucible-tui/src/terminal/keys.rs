//! What the terminal has to say, turned into what this process acts on.
//!
//! In raw mode a key arrives as bytes, and which bytes depends on the terminal:
//! the same arrow is three sequences across four emulators, and a modifier is a
//! parameter inside one of them. Recognising all of that is what `crossterm` is
//! here for, and this is the file where its answer stops being its own type.
//!
//! Above this line there is [`Key`], which is closed and small. A key nothing
//! here maps is [`Pressed::Ignored`] rather than a variant, so an emulator
//! sending something exotic costs a frame that is not drawn instead of a
//! character nobody typed.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::TerminalError;
use crate::editor::Key;

/// What arrived while the prompt was waiting.
///
/// Four of these are keys the editor has no business seeing: they act on what
/// is drawn around the line rather than on the line. A line is a line whatever
/// mode the session is in and whatever is listed above it, and the editor stays
/// what its own module says it is — a string and an offset — rather than
/// growing a case for every key that acts on something beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    /// A key the editor knows what to do with.
    Key(Key),
    /// Shift+Tab: step the mode on to the next one.
    Cycle,
    /// Escape, pressed on its own rather than opening a sequence.
    Escape,
    /// The up arrow: back one row through whatever is listed above the box.
    Up,
    /// The down arrow: on one row through it.
    Down,
    /// The window changed size, so whatever is live was laid out for a width
    /// the terminal no longer has.
    Resized,
    /// Something arrived that means nothing here.
    Ignored,
}

/// Waits for the next thing the terminal has to say.
///
/// Blocking, and deliberately so: the thread that draws the prompt has nothing
/// else to do until a key arrives, and a poll loop would burn a core to find
/// that out repeatedly. A resize arrives on the same path as a key, which is
/// what makes the window changing something the prompt notices as it happens
/// rather than at the next thing the reader types.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be read from.
pub fn pressed() -> Result<Pressed, TerminalError> {
    Ok(meaning(&event::read()?))
}

/// What one event from the terminal means.
///
/// Separate from the read so that every mapping below is a test rather than a
/// keyboard.
fn meaning(event: &Event) -> Pressed {
    match event {
        Event::Resize(..) => Pressed::Resized,
        Event::Key(key) => key_pressed(*key),
        _ => Pressed::Ignored,
    }
}

/// The same for a key, once it is one.
///
/// Presses only. Windows reports the release of every key as well, and acting
/// on both would type each character twice.
fn key_pressed(key: KeyEvent) -> Pressed {
    if key.kind != KeyEventKind::Press {
        return Pressed::Ignored;
    }

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        // The two the terminal used to answer itself. In raw mode they are keys
        // like any other, and what they mean is the editor's to decide.
        KeyCode::Char('c') if control => Pressed::Key(Key::Interrupt),
        KeyCode::Char('d') if control => Pressed::Key(Key::Eof),

        // A word either way, spelled the three ways the terminals here spell
        // it: control and an arrow on Linux and Windows, alt and an arrow on
        // macOS, and the pair readline has answered to for as long as there
        // have been shells. All three are the terminal's own conventions, so
        // whichever one is already in the fingers is the one that works.
        KeyCode::Left if control || alt => Pressed::Key(Key::WordLeft),
        KeyCode::Right if control || alt => Pressed::Key(Key::WordRight),
        KeyCode::Char('b') if alt => Pressed::Key(Key::WordLeft),
        KeyCode::Char('f') if alt => Pressed::Key(Key::WordRight),

        // Anything else held with either modifier is a binding this release has
        // not given a meaning to. Typed as a bare character it would be the
        // letter without the modifier, which is not what was pressed.
        KeyCode::Char(_) if control || alt => Pressed::Ignored,

        // Shift+Tab, which every terminal here sends as one sequence of its
        // own rather than as tab with a modifier — so this is the code to
        // match, and the modifier a given emulator does or does not set
        // alongside it is not a second case.
        KeyCode::BackTab => Pressed::Cycle,
        KeyCode::Esc => Pressed::Escape,

        // The line is one row, so up and down are never about it. What they
        // move through is whatever the line has opened above the box.
        KeyCode::Up => Pressed::Up,
        KeyCode::Down => Pressed::Down,

        KeyCode::Char(typed) => Pressed::Key(Key::Char(typed)),
        KeyCode::Backspace => Pressed::Key(Key::Backspace),
        KeyCode::Left => Pressed::Key(Key::Left),
        KeyCode::Right => Pressed::Key(Key::Right),
        KeyCode::Home => Pressed::Key(Key::Home),
        KeyCode::End => Pressed::Key(Key::End),
        KeyCode::Enter => Pressed::Key(Key::Enter),
        _ => Pressed::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The event a terminal sends for a key held with no modifier.
    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// The same, held with control.
    fn control(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
    }

    /// The same, held with alt.
    fn alt(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::ALT))
    }

    #[test]
    fn a_character_is_the_character_that_was_typed() {
        assert_eq!(
            meaning(&press(KeyCode::Char('a'))),
            Pressed::Key(Key::Char('a'))
        );
        assert_eq!(
            meaning(&press(KeyCode::Char('日'))),
            Pressed::Key(Key::Char('日'))
        );
    }

    #[test]
    fn the_keys_that_move_and_the_keys_that_edit_are_their_own() {
        for (code, key) in [
            (KeyCode::Backspace, Key::Backspace),
            (KeyCode::Left, Key::Left),
            (KeyCode::Right, Key::Right),
            (KeyCode::Home, Key::Home),
            (KeyCode::End, Key::End),
            (KeyCode::Enter, Key::Enter),
        ] {
            assert_eq!(meaning(&press(code)), Pressed::Key(key), "{code:?}");
        }
    }

    #[test]
    fn the_two_the_terminal_used_to_answer_itself_arrive_as_keys() {
        // Raw mode is what stops these being a signal and an end of file, so
        // this file is where they stop being either.
        assert_eq!(
            meaning(&control(KeyCode::Char('c'))),
            Pressed::Key(Key::Interrupt)
        );
        assert_eq!(
            meaning(&control(KeyCode::Char('d'))),
            Pressed::Key(Key::Eof)
        );
    }

    #[test]
    fn the_keys_that_act_on_the_mode_are_not_keys_the_editor_sees() {
        // A line is a line whatever mode the session is in, so neither of
        // these reaches the thing holding one.
        assert_eq!(meaning(&press(KeyCode::BackTab)), Pressed::Cycle);
        assert_eq!(meaning(&press(KeyCode::Esc)), Pressed::Escape);

        // Some emulators set the modifier as well as sending the code, and
        // one that does is not sending a different key.
        let shifted = Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(meaning(&shifted), Pressed::Cycle);
    }

    #[test]
    fn the_arrows_that_move_down_a_list_are_not_keys_the_editor_sees() {
        // A line is one row. Up and down could only mean the line if there
        // were a second one, and what is above the box is not the line.
        assert_eq!(meaning(&press(KeyCode::Up)), Pressed::Up);
        assert_eq!(meaning(&press(KeyCode::Down)), Pressed::Down);
    }

    #[test]
    fn tab_on_its_own_is_not_the_key_that_steps_the_mode() {
        // The two are one key apart and only one of them was bound. Reading a
        // near miss as the binding is how somebody ends up in a mode they did
        // not ask for.
        assert_eq!(meaning(&press(KeyCode::Tab)), Pressed::Ignored);
    }

    #[test]
    fn every_way_a_terminal_here_spells_a_word_reaches_the_editor_as_one() {
        // Three spellings and one meaning. A reader who learned the binding on
        // one of these platforms is not asked to learn it again on the next.
        for held in [control(KeyCode::Left), alt(KeyCode::Left)] {
            assert_eq!(meaning(&held), Pressed::Key(Key::WordLeft), "{held:?}");
        }

        for held in [control(KeyCode::Right), alt(KeyCode::Right)] {
            assert_eq!(meaning(&held), Pressed::Key(Key::WordRight), "{held:?}");
        }

        assert_eq!(
            meaning(&alt(KeyCode::Char('b'))),
            Pressed::Key(Key::WordLeft)
        );
        assert_eq!(
            meaning(&alt(KeyCode::Char('f'))),
            Pressed::Key(Key::WordRight)
        );
    }

    #[test]
    fn a_binding_this_release_has_no_meaning_for_types_nothing() {
        // Ctrl-A is the start of a line in one program and select-all in the
        // next. Typing a bare `a` for it would be the worst of the three.
        assert_eq!(meaning(&control(KeyCode::Char('a'))), Pressed::Ignored);

        // Alt is the modifier a reader is most likely to be holding for
        // something this program has never heard of — a window manager's, an
        // emulator's — and the letter under it is not what they meant to type.
        assert_eq!(meaning(&alt(KeyCode::Char('a'))), Pressed::Ignored);
    }

    #[test]
    fn a_key_being_let_go_of_is_not_a_key_being_pressed() {
        // Windows reports both, and a character typed twice per press is the
        // bug that would be found on Windows and nowhere else.
        let released = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));

        assert_eq!(meaning(&released), Pressed::Ignored);
    }

    #[test]
    fn a_window_that_changed_size_is_not_a_key() {
        assert_eq!(meaning(&Event::Resize(100, 40)), Pressed::Resized);
    }

    #[test]
    fn everything_else_the_terminal_can_send_means_nothing_here() {
        assert_eq!(meaning(&Event::FocusGained), Pressed::Ignored);
        assert_eq!(meaning(&press(KeyCode::F(5))), Pressed::Ignored);
    }
}
