//! One line, and where the cursor is in it.
//!
//! The terminal does this itself in its usual mode, and does it well. What it
//! cannot do is tell the process anything until the line is finished — so a
//! program that draws a box around what is being typed, or that reacts to a key
//! rather than to a line, has to take the job over. [`crate::Raw`] is what takes
//! it; this is what does it afterwards.
//!
//! Deliberately the smallest thing that is still an editor: characters,
//! sanitized bulk text, backspace, the arrows, a word either way, the two ends,
//! and the three keys that end a line. History is separable, and it is not what
//! makes a bordered prompt possible.
//!
//! The text is many lines rather than one: a newline is a character like any
//! other, pasted or typed, and the prompt grows a row for it. That is what a
//! paste of several lines and the key that inserts one both come to — the
//! string carries them, and the component that owns the box lays the lines out.
//! The one-line prompt this used to be could not safely draw after a newline,
//! which is why there was none; the box below lays out by line, so now there
//! is.
//!
//! Nothing here reads a key or draws a row. It is a string and an offset, so a
//! test of what a keystroke does is a test of what a keystroke does, and the
//! component that owns the box decides what any of it looks like.
//! The string retains at most one MiB. An edit that would cross that boundary is
//! refused whole, leaving the caller to say so in the box.

use crate::width;

/// What a pasted tab arrives as.
///
/// A tab cannot be kept as itself: drawn, the terminal moves the cursor to a
/// stop of its own choosing and every row this process counted after it is
/// wrong. Dropped, which is what happened before, a snippet written with tabs
/// arrives with its indentation gone -- out of the box, and out of the prompt
/// that is sent. Four columns is what one level of it reads as in a box.
const TAB: &str = "    ";

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
    /// Move to the line above, at the column the cursor last stood at.
    Up,
    /// Move to the line below, the same way.
    Down,
    /// A newline, inserted where the cursor is. Distinct from the key that
    /// submits, which is what a bare Return stays.
    Newline,
    /// Move back over the word behind the cursor, to where it starts.
    WordLeft,
    /// Move on over the word ahead of it, to where it ends.
    WordRight,
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
    /// The edit would exceed the line's retained-memory ceiling.
    Refused,
    /// The line is finished and is waiting in the editor to be taken.
    Submitted,
    /// Ctrl-C arrived with no line to abandon, so what is left to abandon is
    /// the session. Whether that is what it does is the caller's: this says
    /// what the key found, and the caller is the one holding a clock.
    Interrupted,
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
    /// Whether a newline is a character rather than the end of the line.
    ///
    /// Off by default: a permission answer, a secret and a name are one line,
    /// and a newline pasted into one is noise rather than structure. The prompt
    /// turns it on, which is the only place a second row has somewhere to be
    /// drawn.
    multiline: bool,
    /// The column the cursor was on when it last moved sideways, kept across a
    /// vertical move so that up then down returns to the same column rather
    /// than drifting left over a run of short lines. `None` while the cursor
    /// has only ever moved sideways.
    wanted: Option<usize>,
}

impl Editor {
    /// The most prompt text this process retains, in UTF-8 bytes.
    pub const MAX_BYTES: usize = 1024 * 1024;

    /// An empty line.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets a newline be a character, so the text can be many lines.
    ///
    /// The prompt is the one caller: it is the only editor with rows to give a
    /// second line. Every other stays one line, and a newline pasted into it is
    /// still left out.
    #[must_use]
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
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

    /// How many display columns the cursor sits after, on its line.
    ///
    /// Columns rather than characters, because that is what the terminal moves
    /// in: a CJK glyph takes two of them and a combining mark takes none, so a
    /// cursor placed by counting characters lands somewhere the user did not
    /// type. On a multi-line editor this is the column within the cursor's own
    /// line, not into the whole text — which is the number the box lays a line
    /// out against.
    #[must_use]
    pub fn column(&self) -> usize {
        width::columns(self.line_before())
    }

    /// Which line the cursor is on, counting from none.
    ///
    /// Zero on a one-line editor, so the box that never asked for a second line
    /// reads the same number it always did.
    #[must_use]
    pub fn line(&self) -> usize {
        self.before().matches('\n').count()
    }

    /// Puts the cursor `column` display columns into the line.
    ///
    /// What a click on a one-line box comes to. Past the end of the line is the
    /// end of the line, which is where the eye reads a line as ending — and the
    /// offset can only land on a character boundary, so a click on the far half
    /// of a wide glyph puts the cursor in front of it rather than inside it.
    ///
    /// [`Typed::Ignored`] where the cursor was already there, so a click that
    /// moved nothing costs no frame.
    pub fn place(&mut self, column: usize) -> Typed {
        let at = width::cut(&self.said, column).unwrap_or(self.said.len());

        if at == self.at {
            return Typed::Ignored;
        }

        self.at = at;
        self.wanted = None;
        Typed::Changed
    }

    /// Puts the cursor on `line`, `column` display columns into it.
    ///
    /// What a click on a multi-line box comes to: the row is known, so the
    /// column is wanted within that line rather than into the whole text. A
    /// line the text does not have is the end of it; a column past a line's end
    /// is that line's end. The offset can only land on a character boundary, so
    /// a click on the far half of a wide glyph puts the cursor in front of it.
    ///
    /// [`Typed::Ignored`] where the cursor was already there.
    pub fn place_at(&mut self, line: usize, column: usize) -> Typed {
        let mut start = 0;
        let mut at_line = 0;

        for (offset, character) in self.said.char_indices() {
            if at_line == line {
                break;
            }
            if character == '\n' {
                at_line += 1;
                start = offset + 1;
            }
        }

        // Past the last line is the end of the text, which is where the eye
        // reads the last line as ending.
        let rest = self.said.get(start..).unwrap_or_default();
        let line_text = rest.split('\n').next().unwrap_or_default();
        let at = start + width::cut(line_text, column).unwrap_or(line_text.len());

        if at == self.at {
            return Typed::Ignored;
        }

        self.at = at;
        self.wanted = None;
        Typed::Changed
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

    /// Whether `key` would move the cursor, without moving it.
    ///
    /// Asked by the prompt to tell a vertical move within the text from one
    /// meant for a list standing beside it: a one-line line has no row above or
    /// below, and the key belongs to the list there. Only the vertical keys are
    /// answered; the rest are the line's wherever it is.
    #[must_use]
    pub fn moves(&self, key: Key) -> bool {
        match key {
            Key::Up => self.line_start() > 0,
            Key::Down => self.line_end() < self.said.len(),
            _ => false,
        }
    }

    /// Applies a key, and says what it did.
    pub fn press(&mut self, key: Key) -> Typed {
        match key {
            Key::Char(typed) => self.insert(typed),
            Key::Backspace => self.rub(),
            Key::Left => self.left(),
            Key::Right => self.right(),
            Key::Up => self.up(),
            Key::Down => self.down(),
            Key::WordLeft => self.jump(self.word_back()),
            Key::WordRight => self.jump(self.word_ahead()),
            Key::Home => self.jump(self.line_start()),
            Key::End => self.jump(self.line_end()),
            Key::Newline => self.newline(),
            Key::Enter => self.submit(),
            Key::Interrupt => self.interrupt(),
            Key::Eof => self.eof(),
        }
    }

    /// Inserts sanitized bulk text at the cursor.
    ///
    /// The suffix is moved once by `insert_str`, rather than once per pasted
    /// character. That makes a bulk insertion into the middle linear in the
    /// line plus the inserted text. Control characters are left out for the
    /// reason `insert` leaves them out — drawn, they would move a cursor the
    /// renderer had already placed — with two exceptions: a newline, on an
    /// editor that is many lines, is kept, and a tab arrives as [`TAB`].
    /// Dropping the first turned a paste of several lines into one long line
    /// with the breaks gone; dropping the second took the indentation off
    /// every snippet written with tabs.
    pub fn paste(&mut self, pasted: &str) -> Typed {
        let multiline = self.multiline;
        let keeps = |character: char| !character.is_control() || (multiline && character == '\n');

        let Some(first_control) = pasted.find(|character: char| !keeps(character)) else {
            return self.insert_text(pasted);
        };

        let width = |character: char| match character {
            '\t' => TAB.len(),
            character if keeps(character) => character.len_utf8(),
            _ => 0,
        };

        let remaining = Self::MAX_BYTES.saturating_sub(self.said.len());
        let mut kept = 0;
        for character in pasted.chars() {
            kept += width(character);
            if kept > remaining {
                return Typed::Refused;
            }
        }

        let mut plain = String::with_capacity(kept);
        plain.push_str(pasted.get(..first_control).unwrap_or_default());
        for character in pasted.get(first_control..).unwrap_or_default().chars() {
            match character {
                '\t' => plain.push_str(TAB),
                character if keeps(character) => plain.push(character),
                _ => {}
            }
        }
        self.insert_text(&plain)
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

    /// The part of the cursor's own line that it has passed.
    fn line_before(&self) -> &str {
        self.before().split('\n').next_back().unwrap_or_default()
    }

    /// Where the cursor's line begins, as a byte offset into the text.
    fn line_start(&self) -> usize {
        self.before().rfind('\n').map_or(0, |newline| newline + 1)
    }

    /// Where the cursor's line ends.
    fn line_end(&self) -> usize {
        self.said
            .get(self.at..)
            .and_then(|rest| rest.find('\n').map(|within| self.at + within))
            .unwrap_or(self.said.len())
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
        // A newline is a character where the editor is many lines, and a control
        // everywhere: everywhere else it is a byte that would draw as nothing
        // and move a cursor the renderer had already placed.
        if typed.is_control() && !(self.multiline && typed == '\n') {
            return Typed::Ignored;
        }
        if typed.len_utf8() > Self::MAX_BYTES.saturating_sub(self.said.len()) {
            return Typed::Refused;
        }

        self.said.insert(self.at, typed);
        self.at += typed.len_utf8();
        self.wanted = None;
        Typed::Changed
    }

    /// Puts a newline where the cursor is, splitting the line there.
    ///
    /// One line's editor turns it down, which is what the key that produces it
    /// reads as *nothing happened* — the answer to a newline where a newline
    /// cannot be drawn.
    fn newline(&mut self) -> Typed {
        if !self.multiline {
            return Typed::Ignored;
        }

        self.insert('\n')
    }

    /// Moves the cursor to the line above, at the column it last stood at.
    ///
    /// The column is the one the cursor reached sideways, remembered across the
    /// move, so a run of short lines does not walk the cursor to the left edge.
    /// A line shorter than that column takes its end, which is where the next
    /// character typed on it would go. On the first line there is nowhere to go.
    fn up(&mut self) -> Typed {
        let start = self.line_start();
        if start == 0 {
            return Typed::Ignored;
        }

        // The line above ends one byte before the newline that began this one.
        let above_end = start - 1;
        let above_start = self
            .said
            .get(..above_end)
            .and_then(|before| before.rfind('\n').map(|newline| newline + 1))
            .unwrap_or(0);

        self.vertical(above_start, above_end)
    }

    /// The same, downwards.
    fn down(&mut self) -> Typed {
        let end = self.line_end();
        if end == self.said.len() {
            return Typed::Ignored;
        }

        let below_start = end + 1;
        let below_end = self
            .said
            .get(below_start..)
            .and_then(|rest| rest.find('\n').map(|within| below_start + within))
            .unwrap_or(self.said.len());

        self.vertical(below_start, below_end)
    }

    /// The shared half of [`Editor::up`] and [`Editor::down`]: put the cursor on
    /// the line bounded by `start..end`, at the column it is wanted at.
    fn vertical(&mut self, start: usize, end: usize) -> Typed {
        // Read before `wanted` is taken mutably: the column is a borrow of the
        // text, and the two cannot be held at once.
        let column = self.column();
        let wanted = *self.wanted.get_or_insert(column);
        let line = self.said.get(start..end).unwrap_or_default();
        let within = width::cut(line, wanted).unwrap_or(line.len());
        let to = start + within;

        if to == self.at {
            return Typed::Ignored;
        }

        self.at = to;
        Typed::Changed
    }

    /// Puts already-sanitized text at the cursor with one suffix move.
    fn insert_text(&mut self, text: &str) -> Typed {
        if text.is_empty() {
            return Typed::Ignored;
        }
        if text.len() > Self::MAX_BYTES.saturating_sub(self.said.len()) {
            return Typed::Refused;
        }

        self.said.insert_str(self.at, text);
        self.at += text.len();
        self.wanted = None;
        Typed::Changed
    }

    /// Removes the character behind the cursor.
    fn rub(&mut self) -> Typed {
        let Some(back) = self.back() else {
            return Typed::Ignored;
        };

        self.at -= back;
        self.said.remove(self.at);
        self.wanted = None;
        Typed::Changed
    }

    fn left(&mut self) -> Typed {
        match self.back() {
            Some(back) => {
                self.at -= back;
                self.wanted = None;
                Typed::Changed
            }
            None => Typed::Ignored,
        }
    }

    fn right(&mut self) -> Typed {
        match self.ahead() {
            Some(ahead) => {
                self.at += ahead;
                self.wanted = None;
                Typed::Changed
            }
            None => Typed::Ignored,
        }
    }

    /// Where a word back from the cursor is.
    ///
    /// A word is a run of anything that is not a space, so a path is one word
    /// and so is `parser's`. That is the rule a shell uses, and it is the one
    /// that suits what gets typed here: the reason to cross a line in one press
    /// is usually a path or an identifier near the far end of it, and a rule
    /// that stopped inside either would need the press again to finish the job.
    ///
    /// Any space immediately behind the cursor is crossed first. Without that,
    /// a cursor sitting after a space would land on the end of the word it is
    /// already past rather than on the start of it, and pressing the key twice
    /// would be how you get one word back.
    fn word_back(&self) -> usize {
        self.before()
            .trim_end()
            .char_indices()
            .rev()
            .find(|(_, one)| one.is_whitespace())
            .map_or(0, |(at, one)| at + one.len_utf8())
    }

    /// Where a word on from it is.
    fn word_ahead(&self) -> usize {
        let after = self.said.get(self.at..).unwrap_or_default();
        let word = after.trim_start();
        let spaces = after.len() - word.len();

        word.find(char::is_whitespace)
            .map_or(self.said.len(), |at| self.at + spaces + at)
    }

    /// Moves the cursor somewhere the line already has a boundary.
    ///
    /// A move that lands where it started is nothing happening, which is what
    /// keeps the key that reaches an end it is already at from redrawing.
    fn jump(&mut self, to: usize) -> Typed {
        if self.at == to {
            return Typed::Ignored;
        }

        self.at = to;
        self.wanted = None;
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

    /// Abandons the line, or says there was none to abandon.
    ///
    /// The two halves of what a terminal does with Ctrl-C, kept: a line being
    /// typed is thrown away, and pressing it against an empty one is aimed at
    /// the session instead. Nothing is submitted either way, so a command
    /// half-typed cannot be run by the key that was meant to call it off.
    ///
    /// What the second half *does* is not settled here. Ending a session is one
    /// keystroke away from clearing a line, and the difference between the two
    /// is whether anything had been typed — which is not a difference somebody
    /// reaching for the key can see. So this reports the press and the caller
    /// decides, having the clock that tells one press from two.
    fn interrupt(&mut self) -> Typed {
        if self.said.is_empty() {
            return Typed::Interrupted;
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
