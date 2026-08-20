//! What the terminal has to say, turned into what this process acts on.
//!
//! In raw mode a key arrives as bytes, and which bytes depends on the terminal:
//! the same arrow is three sequences across four emulators, and a modifier is a
//! parameter inside one of them. Recognising all of that is what `crossterm` is
//! here for, and this is the file where its answer stops being its own type.
//!
//! Above this line there is [`Key`], which is closed and small. A key nothing
//! here maps is [`Pressed::Ignored`] rather than a variant, so an emulator
//! sending something exotic costs no frame. Paste reporting stays disabled:
//! crossterm otherwise retains the entire block before this boundary can cap it.
//! Immediately ready character events are instead gathered by [`characters`]
//! into one bounded run. The first structural event ends the run and is handed
//! back, so batching changes neither its order nor its meaning.
//!
//! Crossterm still assumes a syntactically unfinished terminal control sequence
//! is finite. That input comes only from the local terminal under the user's
//! control; provider and model bytes are rendered as text and never reach this
//! parser. Replacing the terminal reader is what would change that trust
//! boundary, rather than presenting this local assumption as a remote bound.

use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

use super::TerminalError;
use crate::editor::Key;

/// Most character events one call gathers before returning to its caller.
///
/// A prompt cannot retain more than this many one-byte characters. Keeping the
/// same ceiling on already refused input also prevents a terminal that stays
/// readable from monopolising the drawing thread in one nominal batch.
const READY_CHARACTERS: usize = 1024 * 1024;

/// What arrived while the prompt was waiting.
///
/// Six of these are keys the editor has no business seeing: they act on what
/// is drawn around the line rather than on the line. A line is a line whatever
/// mode the session is in and whatever is listed above it, and the editor stays
/// what its own module says it is — a string and an offset — rather than
/// growing a case for every key that acts on something beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pressed {
    /// A key the editor knows what to do with.
    Key(Key),
    /// A bracketed paste: a block of text the terminal marked as pasted rather
    /// than typed.
    ///
    /// Its own variant rather than a run of characters because its newlines are
    /// structure, not submissions — typed, each one would be a Return. The text
    /// is boxed so the variant costs the enum no more than a pointer: a paste
    /// is bounded by the editor it lands in, not by what this type can hold.
    Pasted(Box<str>),
    /// Shift+Tab: step the mode on to the next one.
    Cycle,
    /// Ctrl+E: show what is on screen in full, or hide it again.
    ///
    /// A toggle rather than an action, so the same key is the way back — which
    /// is what lets whatever it opens be as tall as it needs to be.
    Explain,
    /// Ctrl+O: show in full whatever the transcript had to cut down to a row,
    /// or hide it again.
    ///
    /// A toggle for the same reason [`Pressed::Explain`] is, and answering the
    /// same want one step further out: that key is about the thing standing on
    /// screen now, and this one is about what has already been written down.
    Expand,
    /// Ctrl+B: move a command across the line between what a turn is waiting on
    /// and what is merely running.
    ///
    /// A toggle for the reason the three above are, and the same key is the way
    /// back — but only one of its two directions changes anything about the turn.
    /// Pressed while a call is out it answers that call now, so the turn goes on
    /// instead of waiting; pressed with nothing out it decides which running
    /// command is being watched, and no key can un-answer a call.
    Background,

    /// Ctrl+T: show the whole plan the agent is working to, or bound it again.
    ///
    /// Named for what it acts on rather than for what it does, which is what
    /// keeps it apart from [`Pressed::Expand`]: both keys say *expand* on the
    /// screen, and the difference between them is that one is about the plan
    /// standing above the box and the other about a result down the transcript.
    Plan,
    /// Ctrl+Q: show every prompt waiting behind the turn, and take one back.
    ///
    /// The panel above the box names as many as fit and counts the rest; this
    /// is the list the count is about, and the only place a queued line can be
    /// dropped before its turn sends it.
    Queue,
    /// Escape, pressed on its own rather than opening a sequence.
    Escape,
    /// The up arrow: back one row through whatever is listed above the box.
    Up,
    /// The down arrow: on one row through it.
    Down,
    /// A button went down somewhere on the screen, counted from its top left.
    ///
    /// Absolute, because that is what the terminal reports and this file is
    /// where its answer stops being its own type. Working out which row of
    /// which component that is belongs to whoever drew them.
    Clicked {
        /// How many rows down the screen.
        row: usize,
        /// How many columns across it.
        column: usize,
    },
    /// The window changed size, so whatever is live was laid out for a width
    /// the terminal no longer has.
    Resized,
    /// Something arrived that means nothing here.
    Ignored,
}

/// One immediately ready run of ordinary characters.
///
/// Text never grows past the room its caller said remained. Characters beyond
/// that boundary are consumed but marked refused, which lets the editor retain
/// its prefix and draw one visible refusal instead of allocating or redrawing
/// for every excess character. The first non-character is kept in `following`
/// for the caller to process next.
#[derive(Debug, PartialEq, Eq)]
pub struct Characters {
    text: String,
    following: Option<Pressed>,
    refused: bool,
    room: usize,
}

impl Characters {
    /// Starts a bounded run with the character already read.
    fn new(first: char, room: usize) -> Self {
        let mut run = Self {
            text: String::with_capacity(room.min(4096)),
            following: None,
            refused: false,
            room,
        };
        let _ = run.push(Pressed::Key(Key::Char(first)));
        run
    }

    /// Takes the gathered text, refusal mark, and structural boundary.
    #[must_use]
    pub fn into_parts(self) -> (String, bool, Option<Pressed>) {
        (self.text, self.refused, self.following)
    }

    /// Adds one event, returning whether it was another character.
    fn push(&mut self, arrived: Pressed) -> bool {
        let Pressed::Key(Key::Char(character)) = arrived else {
            self.following = Some(arrived);
            return false;
        };

        if character.is_control() {
            return true;
        }
        if character.len_utf8() > self.room.saturating_sub(self.text.len()) {
            self.refused = true;
            return true;
        }

        let needed = self.text.len() + character.len_utf8();
        if needed > self.text.capacity() {
            let grown = self
                .text
                .capacity()
                .max(64)
                .saturating_mul(2)
                .min(self.room)
                .max(needed);
            self.text.reserve_exact(grown - self.text.len());
        }
        self.text.push(character);
        true
    }
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
    Ok(meaning(event::read()?))
}

/// Gathers the immediately ready characters following `first`.
///
/// The first structural event is read but returned with the run, so Enter,
/// navigation, resize and every modifier remain boundaries. Input that has not
/// arrived is never waited for: ordinary typing therefore still draws as each
/// keystroke arrives, while a terminal paste already queued in the event reader
/// becomes one insertion and one redraw.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be polled or read.
pub fn characters(first: char, room: usize) -> Result<Characters, TerminalError> {
    let mut run = Characters::new(first, room);
    let mut gathered = 1;

    while gathered < READY_CHARACTERS && run.following.is_none() && waiting(Duration::ZERO)? {
        if !run.push(pressed()?) {
            break;
        }
        gathered += 1;
    }

    Ok(run)
}

/// Whether the terminal has something to say, waiting up to `patience` for it.
///
/// What [`pressed`] is for a caller that has something else to watch. A turn is
/// running on another thread and reporting through a channel, and the reader
/// here has to serve both — so it waits a little on one, looks at the other,
/// and goes round. Zero asks whether anything is already in hand.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be read from.
pub fn waiting(patience: Duration) -> Result<bool, TerminalError> {
    Ok(event::poll(patience)?)
}

/// What one event from the terminal means.
///
/// Separate from the read so that every mapping below is a test rather than a
/// keyboard.
fn meaning(event: Event) -> Pressed {
    match event {
        Event::Resize(..) => Pressed::Resized,
        Event::Key(key) => key_pressed(key),
        Event::Mouse(mouse) => clicked(mouse),
        // A paste arrives whole, terminator and all. Its newlines are kept here
        // rather than turned into submissions, which is what reading it as a run
        // of key events did. The editor bounds what it retains of the text.
        Event::Paste(text) => Pressed::Pasted(text.into()),
        _ => Pressed::Ignored,
    }
}

/// The same for the mouse, once it is the mouse.
///
/// A button going down and nothing else. Reporting is only ever on while a
/// prompt is up, and what a prompt has for the pointer to do is put the cursor
/// somewhere — a release, a drag and a wheel have no answer here, and acting on
/// the release as well would answer one click twice.
///
/// Either button, because either is the one somebody reaches for: the left is
/// what every editor places a caret with, and the right is what a terminal user
/// who has never had a caret to place reaches for first. Neither has a second
/// meaning in this box for the other to collide with. The middle is left alone —
/// it is paste on this platform, and answering it would take that away and put
/// a cursor move in its place.
fn clicked(mouse: MouseEvent) -> Pressed {
    if !matches!(
        mouse.kind,
        MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
    ) {
        return Pressed::Ignored;
    }

    Pressed::Clicked {
        row: mouse.row as usize,
        column: mouse.column as usize,
    }
}

/// Where the terminal says its cursor is, counted from the top left of the
/// screen.
///
/// A question rather than a read, and the one place this crate asks the
/// terminal anything on the way *in*: the answer comes back as a sequence in
/// the same stream keys arrive on. It is asked once per click and never per
/// frame — a click is somebody's hand, and a round trip on the render path
/// would be a round trip per keystroke.
///
/// It is what makes a click mean anything. crucible draws inline, so it does
/// not know which row of the screen it is drawing on; the cursor is parked
/// where the caret is at the moment a click arrives, so this and the caret
/// together say where the component starts.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal would not answer.
pub fn caret() -> Result<(usize, usize), TerminalError> {
    let (column, row) = crossterm::cursor::position()?;

    Ok((row as usize, column as usize))
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

        // Named above the arm that ignores every other modified letter, which
        // is where it would otherwise land. Ctrl+E is readline's end-of-line
        // and this is not a line: nothing here reads a modified letter as
        // editing, so the binding is free and the mnemonic is the one worth
        // having.
        KeyCode::Char('e') if control => Pressed::Explain,

        // Above the same arm and free for the same reason. Ctrl+O is readline's
        // operate-and-get-next, which needs a history this prompt does not keep
        // — so the letter is available and it is the letter the want is spelled
        // with.
        KeyCode::Char('o') if control => Pressed::Expand,

        // And above it again. Ctrl+T is readline's transpose-chars, which is an
        // edit to a line — this prompt has no such key and the letter is the
        // one the panel it opens is spelled with.
        KeyCode::Char('t') if control => Pressed::Plan,

        // And once more. Ctrl+B is readline's backward-char, which is what the
        // left arrow does here and what nothing else needs a letter for — so the
        // letter is free and it is the letter the two things it moves between are
        // both spelled with.
        KeyCode::Char('b') if control => Pressed::Background,

        // And last of these. Ctrl+Q is readline's quoted-insert, which is how a
        // control character reaches a line — this editor takes one as paste and
        // has no use for the key, so the letter is free and it is the one the
        // panel of waiting prompts is spelled with.
        KeyCode::Char('q') if control => Pressed::Queue,

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

        // A newline, spelled the two ways a terminal here spells it. The prompt
        // is many lines, so one of these inserts a break and the bare key is
        // left meaning *finished*: a Return that both broke the line and sent
        // it is a prompt nobody could write a second line into deliberately.
        // Which of the two a terminal delivers is its own — some send neither —
        // so the editor answers both, and where neither arrives a newline is
        // still a paste away.
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => Pressed::Key(Key::Newline),
        KeyCode::Enter if alt => Pressed::Key(Key::Newline),
        KeyCode::Enter => Pressed::Key(Key::Enter),

        // Up and down walk whatever a panel or a list is showing, so they are
        // not the line's keys here. The one place they are is the prompt when
        // its text is many rows: it routes them to the editor itself, which is
        // the caller that knows whether there is a row to move to.
        KeyCode::Up => Pressed::Up,
        KeyCode::Down => Pressed::Down,

        KeyCode::Char(typed) => Pressed::Key(Key::Char(typed)),
        KeyCode::Backspace => Pressed::Key(Key::Backspace),
        KeyCode::Left => Pressed::Key(Key::Left),
        KeyCode::Right => Pressed::Key(Key::Right),
        KeyCode::Home => Pressed::Key(Key::Home),
        KeyCode::End => Pressed::Key(Key::End),
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
            meaning(press(KeyCode::Char('a'))),
            Pressed::Key(Key::Char('a'))
        );
        assert_eq!(
            meaning(press(KeyCode::Char('日'))),
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
            assert_eq!(meaning(press(code)), Pressed::Key(key), "{code:?}");
        }
    }

    #[test]
    fn the_two_the_terminal_used_to_answer_itself_arrive_as_keys() {
        // Raw mode is what stops these being a signal and an end of file, so
        // this file is where they stop being either.
        assert_eq!(
            meaning(control(KeyCode::Char('c'))),
            Pressed::Key(Key::Interrupt)
        );
        assert_eq!(meaning(control(KeyCode::Char('d'))), Pressed::Key(Key::Eof));
    }

    #[test]
    fn the_keys_that_act_on_the_mode_are_not_keys_the_editor_sees() {
        // A line is a line whatever mode the session is in, so neither of
        // these reaches the thing holding one.
        assert_eq!(meaning(press(KeyCode::BackTab)), Pressed::Cycle);
        assert_eq!(meaning(press(KeyCode::Esc)), Pressed::Escape);

        // Some emulators set the modifier as well as sending the code, and
        // one that does is not sending a different key.
        let shifted = Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(meaning(shifted), Pressed::Cycle);
    }

    #[test]
    fn the_key_that_asks_for_the_whole_of_something_is_read_before_the_arm_that_drops_it() {
        // It is a modified letter, and every other modified letter here is a
        // binding this release has not given a meaning to. Order is the whole
        // of what keeps this one from joining them, so order is what this pins.
        assert_eq!(meaning(control(KeyCode::Char('e'))), Pressed::Explain);
        assert_eq!(meaning(control(KeyCode::Char('o'))), Pressed::Expand);
        assert_eq!(meaning(control(KeyCode::Char('t'))), Pressed::Plan);
        assert_eq!(
            meaning(control(KeyCode::Char('b'))),
            Pressed::Background,
            "the key that moves a command across the line was read as an edit"
        );

        // Its neighbours in that arm, unbound and staying so. Typed as bare
        // characters they would be the letters without the modifier, which is
        // not what was pressed.
        for letter in ['w', 'r', 'x'] {
            assert_eq!(
                meaning(control(KeyCode::Char(letter))),
                Pressed::Ignored,
                "ctrl+{letter}"
            );
        }

        // And without the modifier it is the letter, which is what somebody
        // typing `echo` into the box is sending.
        assert_eq!(
            meaning(press(KeyCode::Char('e'))),
            Pressed::Key(Key::Char('e'))
        );
    }

    #[test]
    fn the_arrows_that_move_down_a_list_are_not_keys_the_editor_sees() {
        // A panel or a list is what they walk, so they are not the line's keys
        // here. The one place they reach the line is the prompt when its text
        // is many rows, and that routing is the prompt's rather than the
        // reader's — it is the one holding both.
        assert_eq!(meaning(press(KeyCode::Up)), Pressed::Up);
        assert_eq!(meaning(press(KeyCode::Down)), Pressed::Down);
    }

    #[test]
    fn a_newline_is_spelled_the_two_ways_a_terminal_spells_it() {
        // Shift+Enter and Alt+Enter both insert a break; the bare key is left
        // meaning *finished*. A terminal that sends neither still has the paste.
        let shifted = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(meaning(shifted), Pressed::Key(Key::Newline));
        assert_eq!(meaning(alt(KeyCode::Enter)), Pressed::Key(Key::Newline));
        assert_eq!(meaning(press(KeyCode::Enter)), Pressed::Key(Key::Enter));
    }

    #[test]
    fn tab_on_its_own_is_not_the_key_that_steps_the_mode() {
        // The two are one key apart and only one of them was bound. Reading a
        // near miss as the binding is how somebody ends up in a mode they did
        // not ask for.
        assert_eq!(meaning(press(KeyCode::Tab)), Pressed::Ignored);
    }

    #[test]
    fn every_way_a_terminal_here_spells_a_word_reaches_the_editor_as_one() {
        // Three spellings and one meaning. A reader who learned the binding on
        // one of these platforms is not asked to learn it again on the next.
        for held in [control(KeyCode::Left), alt(KeyCode::Left)] {
            let spelling = format!("{held:?}");
            assert_eq!(meaning(held), Pressed::Key(Key::WordLeft), "{spelling}");
        }

        for held in [control(KeyCode::Right), alt(KeyCode::Right)] {
            let spelling = format!("{held:?}");
            assert_eq!(meaning(held), Pressed::Key(Key::WordRight), "{spelling}");
        }

        assert_eq!(
            meaning(alt(KeyCode::Char('b'))),
            Pressed::Key(Key::WordLeft)
        );
        assert_eq!(
            meaning(alt(KeyCode::Char('f'))),
            Pressed::Key(Key::WordRight)
        );
    }

    #[test]
    fn a_binding_this_release_has_no_meaning_for_types_nothing() {
        // Ctrl-A is the start of a line in one program and select-all in the
        // next. Typing a bare `a` for it would be the worst of the three.
        assert_eq!(meaning(control(KeyCode::Char('a'))), Pressed::Ignored);

        // Alt is the modifier a reader is most likely to be holding for
        // something this program has never heard of — a window manager's, an
        // emulator's — and the letter under it is not what they meant to type.
        assert_eq!(meaning(alt(KeyCode::Char('a'))), Pressed::Ignored);
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

        assert_eq!(meaning(released), Pressed::Ignored);
    }

    #[test]
    fn a_window_that_changed_size_is_not_a_key() {
        assert_eq!(meaning(Event::Resize(100, 40)), Pressed::Resized);
    }

    #[test]
    fn everything_else_the_terminal_can_send_means_nothing_here() {
        assert_eq!(meaning(Event::FocusGained), Pressed::Ignored);
        assert_eq!(meaning(press(KeyCode::F(5))), Pressed::Ignored);
    }

    #[test]
    fn a_bracketed_paste_arrives_as_text_rather_than_as_keystrokes() {
        // The variant the whole feature exists for: a paste's newlines are
        // structure, and read as keystrokes each one would be a Return.
        assert_eq!(
            meaning(Event::Paste("two\nlines".to_owned())),
            Pressed::Pasted("two\nlines".into())
        );
    }

    #[test]
    fn a_large_character_run_is_bounded_and_stops_at_enter() {
        const ROOM: usize = 900 * 1024;
        let mut run = Characters::new('a', ROOM);

        for _ in 1..ROOM + 4096 {
            assert!(run.push(Pressed::Key(Key::Char('a'))));
        }
        assert!(!run.push(Pressed::Key(Key::Enter)));

        let (text, refused, following) = run.into_parts();
        assert_eq!(text.len(), ROOM);
        assert!(text.capacity() <= ROOM);
        assert!(refused);
        assert_eq!(following, Some(Pressed::Key(Key::Enter)));
    }

    #[test]
    fn navigation_is_a_boundary_and_not_part_of_a_character_run() {
        let mut run = Characters::new('a', 16);

        assert!(run.push(Pressed::Key(Key::Char('b'))));
        assert!(!run.push(Pressed::Key(Key::Left)));

        assert_eq!(
            run.into_parts(),
            ("ab".to_owned(), false, Some(Pressed::Key(Key::Left)))
        );
    }
}
