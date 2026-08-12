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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    /// A key the editor knows what to do with.
    Key(Key),
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

    match key.code {
        // The two the terminal used to answer itself. In raw mode they are keys
        // like any other, and what they mean is the editor's to decide.
        KeyCode::Char('c') if control => Pressed::Key(Key::Interrupt),
        KeyCode::Char('d') if control => Pressed::Key(Key::Eof),

        // Anything else held with control is a binding this release has not
        // given a meaning to. Typed as a bare character it would be the letter
        // without the modifier, which is not what was pressed.
        KeyCode::Char(_) if control => Pressed::Ignored,

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
    fn a_binding_this_release_has_no_meaning_for_types_nothing() {
        // Ctrl-A is the start of a line in one program and select-all in the
        // next. Typing a bare `a` for it would be the worst of the three.
        assert_eq!(meaning(&control(KeyCode::Char('a'))), Pressed::Ignored);
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
