//! A terminal that understands exactly what crucible promises to write.
//!
//! Not a general emulator, and deliberately not one. The renderer's claim is
//! that it moves the cursor with a small set of sequences and reaches nothing
//! above the region it drew; a screen that quietly did something sensible with
//! one outside that set would be agreeing with the claim it was brought here to
//! check. So anything outside the promised set is recorded by name and fails
//! the case that drew it, which makes this a second assertion — that the
//! renderer emits nothing it did not promise — carried by the same run as the
//! pictures.
//!
//! Three of crucible's own guarantees are checked as the bytes arrive rather
//! than at the end, so the frame that broke one is the frame that reports it:
//! no row is ever wider than the terminal, nothing ever moves the cursor above
//! the top of the screen, and a frame that asked the screen to be held asks for
//! it to be shown again. All three are cheap enough to hold continuously, and
//! none is visible to a component test, which sees rows and never a screen.
//!
//! Holding is only recorded here rather than acted on. What a real terminal
//! does with it is show one picture instead of two, which is invisible to a
//! screen assembled from every byte that arrived — so the picture is the same
//! either way, and what this checks is that the two halves of it are paired.
//!
//! Columns are counted in characters here rather than from a width table.
//! Everything these cases put on screen — ASCII, box drawing, the block glyphs
//! of the wordmark, the arrows — is one column wide, so the two counts agree,
//! and counting characters keeps the checker independent of the crate whose
//! arithmetic is under test. A case that drew a CJK glyph would need the table,
//! and until one does the count is the honest one to make.

/// The byte that opens a sequence.
const ESCAPE: u8 = 0x1b;

/// What ends a command string.
const BELL: u8 = 0x07;

/// The first half of a row ending, and a whole instruction on its own.
const RETURN: u8 = b'\r';

/// The bytes that can end a control sequence and say what it was.
const ENDS: std::ops::RangeInclusive<char> = '\u{40}'..='\u{7e}';

/// A screen, and everything crucible did to it.
#[derive(Debug)]
pub(crate) struct Screen {
    /// How wide the terminal is.
    columns: usize,
    /// How tall it is.
    rows: usize,
    /// What is on it, one row per line of the window, each as wide as what was
    /// written on it rather than as wide as the window.
    grid: Vec<Vec<char>>,
    /// Which row the cursor is on, counted from the top of the window.
    row: usize,
    /// How many columns across it is.
    column: usize,
    /// How many rows have left the top of the window for the terminal's own
    /// scrollback, which is the one thing that happened that the picture below
    /// cannot show.
    scrolled: usize,
    /// What crucible did that it does not promise to do, in the order it was
    /// first done, each said once.
    refused: Vec<String>,
    /// Whether a frame has asked the screen to be held and not yet asked for it
    /// to be shown.
    holding: bool,
    /// Bytes that arrived without the rest of what they belong to.
    ///
    /// A read ends wherever the kernel filled the buffer, which is not where
    /// anything was written. Held rather than drawn: half a sequence drawn as
    /// text is rubbish on the screen and columns the row was never charged for,
    /// and half a character decodes to a replacement one column wider than the
    /// character it stands for. Both invent failures that nothing did.
    pending: Vec<u8>,
}

impl Screen {
    /// An empty screen of that size.
    pub(crate) fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns,
            rows,
            grid: vec![Vec::new(); rows],
            row: 0,
            column: 0,
            scrolled: 0,
            refused: Vec::new(),
            holding: false,
            pending: Vec::new(),
        }
    }

    /// Takes bytes as they came off the terminal.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        self.pending = data.split_off(readable(&data));

        match std::str::from_utf8(&data) {
            Ok(text) => self.draw(text),
            // Not a truncation — `readable` has already held one of those back
            // — so these are bytes that are not text at all.
            Err(_) => self.refuse("wrote bytes that are not text".to_owned()),
        }
    }

    /// Changes the size of the window under what is already drawn.
    ///
    /// Rows are kept and clipped rather than reflowed, which is what xterm does
    /// and what leaves the picture rectangular; a row drawn legally at the old
    /// width is not charged for the new one, because the check that matters
    /// happens where the row is written. A window that lost rows loses them off
    /// the top, so the live region at the bottom — the only part crucible will
    /// draw again — stays where it stands.
    pub(crate) fn resize(&mut self, columns: usize, rows: usize) {
        for row in &mut self.grid {
            row.truncate(columns);
        }

        while self.grid.len() > rows {
            self.grid.remove(0);
            self.row = self.row.saturating_sub(1);
            self.scrolled += 1;
        }
        self.grid.resize(rows, Vec::new());

        self.columns = columns;
        self.rows = rows;
        self.column = self.column.min(columns);
        self.row = self.row.min(rows.saturating_sub(1));
    }

    /// What crucible did that it does not promise to do.
    pub(crate) fn refusals(&self) -> &[String] {
        &self.refused
    }

    /// Whether a frame is still being held.
    ///
    /// True on a quiet screen means a frame asked the terminal to wait for the
    /// rest of it and never said the rest had arrived — which on a real one is
    /// a picture that stops changing until the terminal's own timeout gives up
    /// on the frame.
    pub(crate) fn is_holding(&self) -> bool {
        self.holding
    }

    /// The screen, as a picture with the size and the cursor above it.
    ///
    /// Every row is padded to the full width and closed with a bar, so a
    /// trailing space is a character in the diff rather than something an
    /// editor is free to strip. The cursor is on the header line because where
    /// it parks is half of what the arithmetic under test decides, and a
    /// picture that only showed the text would assert the other half.
    pub(crate) fn picture(&self) -> String {
        let mut lines = vec![format!(
            "{}x{} cursor {},{} scrolled {}",
            self.columns, self.rows, self.row, self.column, self.scrolled
        )];

        for row in &self.grid {
            let mut line: String = row.iter().collect();
            for _ in row.len()..self.columns {
                line.push(' ');
            }
            lines.push(format!("|{line}|"));
        }

        lines.join("\n")
    }

    /// Says once that crucible did something it does not promise to do.
    fn refuse(&mut self, what: String) {
        if !self.refused.contains(&what) {
            self.refused.push(what);
        }
    }

    /// Draws text that arrived whole, sequences and all.
    fn draw(&mut self, text: &str) {
        let mut rest = text;

        while !rest.is_empty() {
            match rest.find(char::from(ESCAPE)) {
                Some(0) => rest = self.sequence(rest),
                Some(at) => {
                    self.plain(rest.get(..at).unwrap_or_default());
                    rest = rest.get(at..).unwrap_or_default();
                }
                None => {
                    self.plain(rest);
                    rest = "";
                }
            }
        }
    }

    /// Draws text with no sequence in it, ending rows where it says to.
    fn plain(&mut self, text: &str) {
        let mut rest = text;

        while let Some(at) = rest.find(['\r', '\n']) {
            self.put(rest.get(..at).unwrap_or_default());
            let tail = rest.get(at..).unwrap_or_default();

            let taken = if tail.starts_with("\r\n") {
                self.column = 0;
                self.down();
                2
            } else if tail.starts_with('\r') {
                self.column = 0;
                1
            } else {
                // A terminal in raw mode does not return the carriage on a bare
                // newline, so a row ended with one stair-steps across the
                // screen. The renderer writes `\r\n` on a terminal for exactly
                // that reason, and this is where it would be caught not doing.
                self.refuse("ended a row with a bare newline".to_owned());
                self.down();
                1
            };

            rest = tail.get(taken..).unwrap_or_default();
        }

        self.put(rest);
    }

    /// Writes characters where the cursor is, padding the row to reach it.
    fn put(&mut self, text: &str) {
        let mut column = self.column;

        if let Some(row) = self.grid.get_mut(self.row) {
            while row.len() < column {
                row.push(' ');
            }
            for character in text.chars() {
                match row.get_mut(column) {
                    Some(cell) => *cell = character,
                    None => row.push(character),
                }
                column += 1;
            }
        }

        if column > self.columns {
            self.refuse(format!(
                "wrote row {} out to column {column} on a screen {} columns wide",
                self.row, self.columns
            ));
        }
        self.column = column;
    }

    /// Steps the cursor down a row, scrolling the window when there is none.
    fn down(&mut self) {
        self.row += 1;

        if self.row >= self.rows {
            self.grid.remove(0);
            self.grid.push(Vec::new());
            self.row = self.rows.saturating_sub(1);
            self.scrolled += 1;
        }
    }

    /// Steps the cursor up, which is the move that has to stay on the screen.
    fn up(&mut self, count: usize) {
        if count > self.row {
            self.refuse(format!(
                "moved {count} rows up from row {}, above the top of the screen",
                self.row
            ));
        }
        self.row = self.row.saturating_sub(count);
    }

    /// Erases from the cursor to the end of the screen.
    fn erase_down(&mut self) {
        if let Some(row) = self.grid.get_mut(self.row) {
            row.truncate(self.column);
        }
        for row in self.grid.iter_mut().skip(self.row + 1) {
            row.clear();
        }
    }

    /// Reads one escape sequence and returns what follows it.
    fn sequence<'a>(&mut self, rest: &'a str) -> &'a str {
        let body = rest.get(1..).unwrap_or_default();

        match body.chars().next() {
            Some('[') => self.control(body.get(1..).unwrap_or_default()),
            Some(']') => self.command(body.get(1..).unwrap_or_default()),
            Some(other) => {
                self.refuse(format!("wrote ESC {other}"));
                body.get(other.len_utf8()..).unwrap_or_default()
            }
            None => "",
        }
    }

    /// Reads `ESC [ … ` up to the byte that says what it was.
    fn control<'a>(&mut self, after: &'a str) -> &'a str {
        let Some(at) = after.find(|character| ENDS.contains(&character)) else {
            self.refuse("wrote a control sequence with no end to it".to_owned());
            return "";
        };

        let ends = after.get(at..).and_then(|rest| rest.chars().next());
        let rest = ends.map_or("", |ends| {
            after.get(at + ends.len_utf8()..).unwrap_or_default()
        });

        if let Some(ends) = ends {
            self.act(after.get(..at).unwrap_or_default(), ends);
        }
        rest
    }

    /// Acts on one control sequence, or refuses it by name.
    ///
    /// The whole set the renderer promises: move up, set the column, erase
    /// down, colour, and the two that hold a frame until all of it has arrived.
    fn act(&mut self, params: &str, ends: char) {
        // Every one of these defaults to one where the parameter is left out.
        let count = params.parse::<usize>().unwrap_or(1);

        match (params, ends) {
            // Colour, and the mouse switched on at the prompt and off again.
            // Both are state borrowed from the terminal rather than a mark on
            // it, so neither moves the cursor or fills a column.
            (_, 'm') | ("?1000" | "?1006", 'h' | 'l') => {}
            (_, 'A') => self.up(count),
            (_, 'G') => self.park(count),
            ("" | "0", 'J') => self.erase_down(),
            ("?2026", 'h') => self.hold(),
            ("?2026", 'l') => self.show(),
            _ => self.refuse(format!("wrote ESC[{params}{ends}")),
        }
    }

    /// Holds the screen for a frame that is being written.
    fn hold(&mut self) {
        if self.holding {
            self.refuse("held a screen that was already being held".to_owned());
        }
        self.holding = true;
    }

    /// Shows what was held.
    fn show(&mut self) {
        if !self.holding {
            self.refuse("showed a screen that was never held".to_owned());
        }
        self.holding = false;
    }

    /// Puts the cursor at a column, counted the way the terminal counts them.
    fn park(&mut self, column: usize) {
        let column = column.saturating_sub(1);

        // One past the last column is where a cursor rests at the end of a full
        // row; anything beyond that is a column this window does not have.
        if column > self.columns {
            self.refuse(format!(
                "parked the cursor at column {column} on a screen {} columns wide",
                self.columns
            ));
        }
        self.column = column.min(self.columns);
    }

    /// Reads `ESC ] … BEL`, which the renderer uses for the tab title alone.
    fn command<'a>(&mut self, after: &'a str) -> &'a str {
        let Some(at) = after.find(char::from(BELL)) else {
            self.refuse("wrote a command string with no end to it".to_owned());
            return "";
        };

        let said = after.get(..at).unwrap_or_default();
        if !said.starts_with("0;") {
            self.refuse(format!("wrote the command string {said:?}"));
        }

        after.get(at + 1..).unwrap_or_default()
    }
}

/// How much of `data` can be read now.
///
/// Everything, unless it ends in the middle of something. Three things can be
/// cut in half by a read, and every one of them makes this screen report a
/// failure nothing committed — so each is held back until the rest arrives.
fn readable(data: &[u8]) -> usize {
    let mut end = data.len();

    // A sequence with no end yet. Drawn as text it is rubbish on the screen and
    // columns the row was never charged for.
    if let Some(at) = data.iter().rposition(|byte| *byte == ESCAPE)
        && !finished(data.get(at..).unwrap_or_default())
    {
        end = at;
    }

    end = match std::str::from_utf8(data.get(..end).unwrap_or_default()) {
        // A character with bytes still on their way. Decoded now it becomes a
        // replacement, which is one column wider than what it stands for.
        Err(problem) if problem.error_len().is_none() => problem.valid_up_to(),
        // Whole, or bytes that are not a character at all — which is not this
        // function's to say, and is kept so that `feed` says it.
        _ => end,
    };

    // A carriage return that may be the first half of a row ending. Read on its
    // own it leaves the newline behind it looking like one written alone, which
    // is a thing this screen refuses — and the refusal would land on whichever
    // frame the kernel happened to cut there.
    if end > 0 && data.get(end - 1) == Some(&RETURN) {
        end -= 1;
    }

    end
}

/// Whether `tail`, which begins with an escape, is a whole sequence.
///
/// `tail` starts at the last escape in what has arrived, so nothing inside it
/// opens a second one — which is what makes looking for a single end enough.
fn finished(tail: &[u8]) -> bool {
    match tail.get(1) {
        None => false,
        Some(b'[') => tail
            .iter()
            .skip(2)
            .any(|byte| ENDS.contains(&char::from(*byte))),
        Some(b']') => tail.contains(&BELL),
        // Every other sequence is two bytes, and both are here.
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::Screen;

    /// One of everything the renderer writes, in the shapes it writes it.
    ///
    /// The box characters are three bytes each and one column each, which is
    /// what crucible draws a frame with — so a read cut inside one is a cut
    /// this screen has to survive rather than a case invented for the test.
    const WRITTEN: &str = concat!(
        "\x1b]0;▽ crucible\x07",
        "\x1b[?1000h\x1b[?1006h",
        "\x1b[?2026h\r\x1b[Jcrucible v0.0.9\r\n",
        "\x1b[36m│ › ───\x1b[0m\r\n\x1b[?2026l",
        "\x1b[?2026h\r\x1b[2A\x1b[J╭──╮\r\n│  │\r\n╰──╯",
        "\x1b[1A\x1b[3G\x1b[?2026l",
    );

    /// The same bytes one at a time, which is every cut point at once.
    fn byte_at_a_time(written: &str) -> Screen {
        let mut screen = Screen::new(24, 8);

        for byte in written.as_bytes() {
            screen.feed(std::slice::from_ref(byte));
        }
        screen
    }

    #[test]
    fn a_stream_cut_anywhere_by_the_read_draws_the_same_screen() {
        // A read ends where the kernel filled the buffer, and under load it
        // ends in more places. Every failure this screen could report that
        // nothing committed comes from a cut: half a sequence, half a
        // character, or a `\r` read without the `\n` written with it.
        let mut whole = Screen::new(24, 8);
        whole.feed(WRITTEN.as_bytes());
        let split = byte_at_a_time(WRITTEN);

        assert_eq!(split.picture(), whole.picture());
        assert!(whole.refusals().is_empty(), "{:?}", whole.refusals());
        assert!(split.refusals().is_empty(), "{:?}", split.refusals());
    }

    #[test]
    fn a_row_wider_than_the_terminal_is_reported() {
        // The first of the two invariants, watched failing. One nobody has
        // seen say no is a check the cases are passing for free.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"a row that is far too long");

        assert!(
            screen
                .refusals()
                .iter()
                .any(|said| said.contains("column 26")),
            "{:?}",
            screen.refusals()
        );
    }

    #[test]
    fn moving_the_cursor_above_the_top_of_the_screen_is_reported() {
        // The second, and the one the shipped bug tripped: a frame that
        // rewinds further than the rows it drew erases somebody's scrollback.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"one\r\n\x1b[3A");

        assert!(
            screen
                .refusals()
                .iter()
                .any(|said| said.contains("above the top")),
            "{:?}",
            screen.refusals()
        );
    }

    #[test]
    fn a_sequence_the_renderer_does_not_promise_is_reported_by_name() {
        // Erasing the whole screen reaches rows that have scrolled off it,
        // which is the one thing the inline design rests on never happening.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[2J");

        assert_eq!(screen.refusals(), ["wrote ESC[2J"]);
    }

    #[test]
    fn a_frame_still_held_when_the_screen_goes_quiet_is_visible_from_outside() {
        // The third invariant. A real terminal holds the picture it has until
        // the closing sequence arrives, so a frame that opened one and never
        // closed it is a screen that has stopped changing — which is invisible
        // to a picture assembled from every byte, and is the point of asking.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[?2026h\r\x1b[Jone");

        assert!(screen.is_holding());
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());

        screen.feed(b"\x1b[?2026l");
        assert!(!screen.is_holding());
    }

    #[test]
    fn showing_a_screen_that_was_never_held_is_reported() {
        // The pairing is what the invariant is made of, so the half nothing
        // opened is refused as loudly as the half nothing closed.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[?2026l");

        assert_eq!(screen.refusals(), ["showed a screen that was never held"]);
    }

    #[test]
    fn a_row_ended_with_a_bare_newline_is_reported() {
        // Raw mode does not return the carriage on one, so rows stair-step
        // across the screen and every frame after rewinds over the wrong ones.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"one\ntwo");

        assert_eq!(screen.refusals(), ["ended a row with a bare newline"]);
    }
}
