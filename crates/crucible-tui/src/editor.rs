//! One line, and where the cursor is in it.
//!
//! The terminal does this itself in its usual mode, and does it well. What it
//! cannot do is tell the process anything until the line is finished — so a
//! program that draws a box around what is being typed, or that reacts to a key
//! rather than to a line, has to take the job over. [`crate::Raw`] is what takes
//! it; this is what does it afterwards.
//!
//! Deliberately the smallest thing that is still an editor: characters,
//! backspace, the arrows, the two ends, and the three keys that end a line.
//! History, word motion, bracketed paste and a second row are each separable,
//! and none of them is what makes a bordered prompt possible.
//!
//! Nothing here reads a key or draws a row. It is a string and an offset, so a
//! test of what a keystroke does is a test of what a keystroke does, and the
//! component that owns the box decides what any of it looks like.

use crate::width;

/// A key, as an editor cares about it.
///
/// A closed set: the reader turns whatever a terminal sent into one of these,
/// and everything below is a `match` that a new key has to be added to before
/// it will compile. What a terminal actually sends — the escape sequences, the
/// modifiers, the same key spelled four ways by four emulators — is the
/// reader's problem and stops here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A character was typed.
    Char(char),
    /// Rub out what is behind the cursor.
    Backspace,
    /// Move one character back.
    Left,
    /// Move one character on.
    Right,
    /// Move to the start of the line.
    Home,
    /// Move to the end of it.
    End,
    /// Submit what is there.
    Enter,
    /// Ctrl-C. In raw mode the terminal sends the key rather than a signal, so
    /// what it means is decided here.
    Interrupt,
    /// Ctrl-D, which a terminal means as the end of input.
    Eof,
}

/// What a key did.
///
/// The caller redraws on [`Typed::Changed`] and on nothing else, so a key that
/// moved nothing costs no frame — an arrow held down against the end of a line
/// is the case that would otherwise redraw at the speed of the key repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    /// The line or the cursor moved. Draw it again.
    Changed,
    /// Nothing moved. There is nothing to draw.
    Ignored,
    /// The line is finished and is waiting in the editor to be taken.
    Submitted,
    /// The session is over.
    Ended,
}

/// The line being typed, and where the cursor sits in it.
#[derive(Debug, Default, Clone)]
pub struct Editor {
    /// What has been typed.
    said: String,
    /// Where the cursor is, as a byte offset into it.
    ///
    /// Bytes rather than characters because every edit is an insert or a remove
    /// at this point, and both are spelled in bytes. It is only ever moved by a
    /// whole character's width, so it is always on a boundary — which is the
    /// invariant every slice below rests on.
    at: usize,
}

impl Editor {
    /// An empty line.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What has been typed so far.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.said
    }

    /// Whether anything has been.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.said.is_empty()
    }

    /// How many display columns the cursor sits after.
    ///
    /// Columns rather than characters, because that is what the terminal moves
    /// in: a CJK glyph takes two of them and a combining mark takes none, so a
    /// cursor placed by counting characters lands somewhere the user did not
    /// type.
    #[must_use]
    pub fn column(&self) -> usize {
        width::columns(self.before())
    }

    /// Takes the line, leaving an empty one behind.
    ///
    /// What [`Typed::Submitted`] is followed by. The editor is reusable
    /// afterwards rather than replaced, so the prompt holds one for the whole
    /// session and the allocation the last line grew to is the one the next
    /// line starts in.
    pub fn take(&mut self) -> String {
        self.at = 0;
        std::mem::take(&mut self.said)
    }

    /// Empties the line without taking it anywhere.
    pub fn clear(&mut self) {
        self.said.clear();
        self.at = 0;
    }

    /// Applies a key, and says what it did.
    pub fn press(&mut self, key: Key) -> Typed {
        match key {
            Key::Char(typed) => self.insert(typed),
            Key::Backspace => self.rub(),
            Key::Left => self.left(),
            Key::Right => self.right(),
            Key::Home => self.jump(0),
            Key::End => self.jump(self.said.len()),
            Key::Enter => self.submit(),
            Key::Interrupt => self.interrupt(),
            Key::Eof => self.eof(),
        }
    }

    /// Everything the cursor has passed.
    ///
    /// The offset is always on a character boundary, so the empty string is
    /// unreachable rather than a fallback: it is what a bug here would read as,
    /// and a cursor drawn at the start of the line is the mildest thing that
    /// could go wrong with one.
    fn before(&self) -> &str {
        self.said.get(..self.at).unwrap_or_default()
    }

    /// The character behind the cursor, in bytes.
    fn back(&self) -> Option<usize> {
        self.before().chars().next_back().map(char::len_utf8)
    }

    /// The one in front of it.
    fn ahead(&self) -> Option<usize> {
        self.said
            .get(self.at..)
            .and_then(|rest| rest.chars().next())
            .map(char::len_utf8)
    }

    /// Puts a character where the cursor is.
    ///
    /// A control character is dropped rather than stored. Every key that means
    /// something arrives as its own [`Key`], so what is left is a byte that
    /// would draw as nothing, count as no columns, and move a cursor the
    /// renderer had already placed — most often out of a paste, which this
    /// release reads as typing.
    fn insert(&mut self, typed: char) -> Typed {
        if typed.is_control() {
            return Typed::Ignored;
        }

        self.said.insert(self.at, typed);
        self.at += typed.len_utf8();
        Typed::Changed
    }

    /// Removes the character behind the cursor.
    fn rub(&mut self) -> Typed {
        let Some(back) = self.back() else {
            return Typed::Ignored;
        };

        self.at -= back;
        self.said.remove(self.at);
        Typed::Changed
    }

    fn left(&mut self) -> Typed {
        match self.back() {
            Some(back) => {
                self.at -= back;
                Typed::Changed
            }
            None => Typed::Ignored,
        }
    }

    fn right(&mut self) -> Typed {
        match self.ahead() {
            Some(ahead) => {
                self.at += ahead;
                Typed::Changed
            }
            None => Typed::Ignored,
        }
    }

    /// Moves the cursor to an end.
    fn jump(&mut self, to: usize) -> Typed {
        if self.at == to {
            return Typed::Ignored;
        }

        self.at = to;
        Typed::Changed
    }

    /// Finishes the line, unless there is no line to finish.
    ///
    /// Return on an empty prompt is somebody looking at the screen, not
    /// somebody asking for nothing. Submitting it would cost a turn and answer
    /// a question nobody asked.
    fn submit(&self) -> Typed {
        if self.said.is_empty() {
            Typed::Ignored
        } else {
            Typed::Submitted
        }
    }

    /// Abandons the line, or the session if there is no line to abandon.
    ///
    /// The two halves of what a terminal does with Ctrl-C, kept: a line being
    /// typed is thrown away, and pressing it again — now against an empty line
    /// — leaves. Nothing is submitted either way, so a command half-typed
    /// cannot be run by the key that was meant to call it off.
    fn interrupt(&mut self) -> Typed {
        if self.said.is_empty() {
            return Typed::Ended;
        }

        self.clear();
        Typed::Changed
    }

    /// Ends the session, but only from an empty line.
    ///
    /// A terminal reads Ctrl-D as the end of input, and on a line with
    /// something in it that is ambiguous enough to be dangerous — it is one key
    /// away from the ones that edit, and what it would end is a prompt somebody
    /// is still writing.
    fn eof(&self) -> Typed {
        if self.said.is_empty() {
            Typed::Ended
        } else {
            Typed::Ignored
        }
    }
}

#[cfg(test)]
mod tests;
