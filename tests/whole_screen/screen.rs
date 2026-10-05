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
//! no row is ever wider than the terminal, no cell outside the window is ever
//! addressed, and a frame that asked the screen to be held asks for it to be
//! shown again. All three are cheap enough to hold continuously, and none is
//! visible to a component test, which sees rows and never a screen.
//!
//! The second of those is what replaced a rewind that could reach above the top
//! of the screen. Every position crucible writes at is now named outright, so
//! the way that guarantee fails is an address off the window rather than a
//! count of rows that was one too many — and `scrolled` staying at zero for the
//! whole of a session is the other half of it, since a process that owns its
//! screen has no reason to push a row off the top of one.
//!
//! A native launch owns no screen. It draws a live region at the foot of the
//! terminal's own buffer, moving relatively and letting finished rows scroll
//! into the scrollback, so the screen opened for one applies the three
//! sequences that mode is made of — erase below, cursor up, cursor to column —
//! the way a terminal does, keeps the rows pushed off the top as a scrollback
//! the case can read, and rewraps everything it holds when the window changes
//! width, which is what the renderer's own count of how far back its region is
//! assumes of the terminal. Each is still refused on a fullscreen launch, where
//! a frame that moved relatively is one the renderer never composed. Entering
//! the alternate screen is remembered in both, because a native case proves it
//! ran in native mode by that and not by its rows.
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

/// What else ends one: the string terminator, `ESC \\`.
///
/// Both spellings, because a real terminal takes both and crucible writes both
/// — the tab title ends with a bell and a hyperlink ends with this, which is
/// the spelling the hyperlink's own definition is written in.
const TERMINATOR: &str = "\x1b\\";

/// The first half of a row ending, and a whole instruction on its own.
const RETURN: u8 = b'\r';

/// The bytes that can end a control sequence and say what it was.
const ENDS: std::ops::RangeInclusive<char> = '\u{40}'..='\u{7e}';

/// Which of crucible's two screens a launch asked for, and so which sequences
/// the renderer has promised to confine itself to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The alternate screen, every row of it addressed by name.
    Fullscreen,
    /// The terminal's own buffer, drawn relatively at its foot.
    Native,
}

/// A screen, and everything crucible did to it.
#[derive(Debug)]
pub(crate) struct Screen {
    /// Which screen this is.
    mode: Mode,
    /// How wide the terminal is.
    columns: usize,
    /// How tall it is.
    rows: usize,
    /// What is on it, one row per line of the window, each as wide as what was
    /// written on it rather than as wide as the window.
    grid: Vec<Vec<char>>,
    /// Whether each row of the window is the fold of a wider line, running on
    /// into the row under it — which only a resize of a native screen makes,
    /// and which the next resize needs so as to join the pieces again.
    ran_on: Vec<bool>,
    /// Which row the cursor is on, counted from the top of the window.
    row: usize,
    /// How many columns across it is.
    column: usize,
    /// How many rows have been pushed off the top of a fullscreen window, which
    /// is the one thing that happened that the picture below cannot show.
    ///
    /// It is expected to stay at nought: a process that owns its screen writes
    /// at the position it means and has no reason to make the window move under
    /// what it drew.
    scrolled: usize,
    /// The rows pushed off the top of a native window, oldest first: what a
    /// reader would find on scrolling back.
    scrollback: Vec<Vec<char>>,
    /// Whether each row of the scrollback ran on into the one under it.
    ran_on_back: Vec<bool>,
    /// Whether the session entered the alternate screen.
    alternate: bool,
    /// What crucible did that it does not promise to do, in the order it was
    /// first done, each said once.
    refused: Vec<String>,
    /// The command strings that arrived, in the order they did.
    ///
    /// Kept rather than dropped because some of them are the whole point of a
    /// frame and none of them lands in a column: a hyperlink is an address
    /// wrapped around words that are drawn either way, so a picture is exactly
    /// the same whether one was written or not.
    commanded: Vec<String>,
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

/// The version this build draws on its opening screen.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The character that stands in for each of its characters.
///
/// One per character, so the row keeps its width and the frame around it keeps
/// its corners: the picture is a grid already drawn, and a stand-in of another
/// length would move every bar to its right rather than only the version. The
/// mask therefore follows the version's own length, which leaves one case that
/// still moves a snapshot — a release whose version is spelled in more
/// characters than the last. That one is worth reading, because it moves what
/// the opening screen is laid out around.
const MASK: char = '#';

impl Screen {
    /// An empty screen of that size, for a fullscreen launch.
    pub(crate) fn new(columns: usize, rows: usize) -> Self {
        Self::opened(Mode::Fullscreen, columns, rows)
    }

    /// An empty screen of that size, for a native launch.
    pub(crate) fn native(columns: usize, rows: usize) -> Self {
        Self::opened(Mode::Native, columns, rows)
    }

    fn opened(mode: Mode, columns: usize, rows: usize) -> Self {
        Self {
            mode,
            columns,
            rows,
            grid: vec![Vec::new(); rows],
            ran_on: vec![false; rows],
            row: 0,
            column: 0,
            scrolled: 0,
            scrollback: Vec::new(),
            ran_on_back: Vec::new(),
            alternate: false,
            refused: Vec::new(),
            commanded: Vec::new(),
            holding: false,
            pending: Vec::new(),
        }
    }

    /// The rows pushed off the top of the window, oldest first, drawn the way
    /// [`picture`](Self::picture) draws its rows so the two read on from each
    /// other: everything a reader could scroll back to, then the window.
    ///
    /// Empty on a fullscreen screen, where a row pushed off the top is counted
    /// in the picture's header instead, since nothing there should be making
    /// one.
    pub(crate) fn scrollback(&self) -> String {
        self.framed(&self.scrollback)
    }

    /// Whether the session entered the alternate screen.
    ///
    /// A fullscreen launch does this first of all and a native one never does,
    /// so it is the one byte that says which screen a session was drawn on —
    /// rows look the same in both until something scrolls.
    pub(crate) fn entered_alternate(&self) -> bool {
        self.alternate
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
    /// On a fullscreen screen, rows are kept and clipped rather than reflowed,
    /// which is what xterm does and what leaves the picture rectangular; a row
    /// drawn legally at the old width is not charged for the new one, because
    /// the check that matters happens where the row is written. A window that
    /// lost rows loses them off the top, so what was at the foot of it stays at
    /// the foot.
    ///
    /// None of that is scrolling. A window that got shorter has fewer rows to
    /// show, which is the reader's doing and says nothing about what crucible
    /// wrote — and the count exists to catch a frame reaching past the bottom
    /// of a screen this process owns.
    ///
    /// A native screen rewraps instead, as most terminals now do and as the
    /// native renderer assumes when it works out how far back the top of its
    /// region is: see [`Self::rewrap`].
    pub(crate) fn resize(&mut self, columns: usize, rows: usize) {
        match self.mode {
            Mode::Fullscreen => self.clip(columns, rows),
            Mode::Native => self.rewrap(columns, rows),
        }
    }

    /// Clips every row to the new width and the window to the new height.
    fn clip(&mut self, columns: usize, rows: usize) {
        for row in &mut self.grid {
            row.truncate(columns);
        }

        while self.grid.len() > rows {
            self.grid.remove(0);
            self.row = self.row.saturating_sub(1);
        }
        self.grid.resize(rows, Vec::new());
        self.ran_on = vec![false; rows];

        self.columns = columns;
        self.rows = rows;
        self.column = self.column.min(columns);
        self.row = self.row.min(rows.saturating_sub(1));
    }

    /// Folds everything the terminal holds to the new width, the way a reader
    /// dragging the corner of a rewrapping terminal would see it.
    ///
    /// Rows that ran on into each other are one line again, and each line is
    /// folded at the new width. The cursor keeps its place in the line it was
    /// on; nothing empty below it is kept; and the window is the foot of what
    /// is left, the rest above it being scrollback.
    fn rewrap(&mut self, columns: usize, rows: usize) {
        let was = self.columns;
        let cursor = self.scrollback.len() + self.row;
        let back = self.scrollback.drain(..).zip(self.ran_on_back.drain(..));
        let held: Vec<(Vec<char>, bool)> = back
            .chain(self.grid.drain(..).zip(self.ran_on.drain(..)))
            .collect();

        // Rows back into lines, with where in its line the cursor was.
        let mut lines: Vec<Vec<char>> = Vec::new();
        let mut line = Vec::new();
        let mut caret = (0, 0);
        for (index, (mut row, ran_on)) in held.into_iter().enumerate() {
            if index == cursor {
                caret = (lines.len(), line.len() + self.column);
            }
            if ran_on {
                row.resize(was, ' ');
                line.extend(row);
            } else {
                line.extend(row);
                lines.push(std::mem::take(&mut line));
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        while lines.len() > caret.0 + 1 && lines.last().is_some_and(Vec::is_empty) {
            lines.pop();
        }

        // And lines into rows at the new width.
        let width = columns.max(1);
        let mut folded: Vec<(Vec<char>, bool)> = Vec::new();
        let mut at = (0, 0);
        for (index, line) in lines.into_iter().enumerate() {
            let first = folded.len();
            let pieces: Vec<Vec<char>> = if line.is_empty() {
                vec![Vec::new()]
            } else {
                line.chunks(width).map(<[char]>::to_vec).collect()
            };
            let count = pieces.len();
            for (piece, row) in pieces.into_iter().enumerate() {
                folded.push((row, piece + 1 < count));
            }
            if index == caret.0 {
                at = (first + caret.1 / width, caret.1 % width);
                while folded.len() <= at.0 {
                    folded.push((Vec::new(), false));
                }
            }
        }

        let top = folded.len().saturating_sub(rows);
        let window = folded.split_off(top);
        for (row, ran_on) in folded {
            self.scrollback.push(row);
            self.ran_on_back.push(ran_on);
        }
        for (row, ran_on) in window {
            self.grid.push(row);
            self.ran_on.push(ran_on);
        }
        self.grid.resize(rows, Vec::new());
        self.ran_on.resize(rows, false);
        self.row = at.0.saturating_sub(top);
        self.column = at.1;
        self.columns = columns;
        self.rows = rows;
    }

    /// What crucible did that it does not promise to do.
    pub(crate) fn refusals(&self) -> &[String] {
        &self.refused
    }

    /// The command strings this screen was sent, payloads only.
    pub(crate) fn commands(&self) -> &[String] {
        &self.commanded
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

    /// Whether `wanted` is on a frame that has finished being written.
    ///
    /// A read of the terminal can end anywhere, inside a frame as easily as
    /// between two, and a real terminal goes on showing the frame before until
    /// the held one is closed. Text from a frame still being held is what this
    /// picture has and that terminal does not show yet.
    pub(crate) fn shows(&self, wanted: &str) -> bool {
        !self.holding && self.picture().contains(wanted)
    }

    /// The screen, as a picture with the size and the cursor above it.
    ///
    /// Every row is padded to the full width and closed with a bar, so a
    /// trailing space is a character in the diff rather than something an
    /// editor is free to strip. The cursor is on the header line because where
    /// it parks is half of what the arithmetic under test decides, and a
    /// picture that only showed the text would assert the other half.
    ///
    /// This build's own version is masked out of it. The opening screen draws
    /// one, and a snapshot holding it is a snapshot every release has to
    /// re-accept — which is how a picture stops being read and becomes a file
    /// that gets a yes. [`MASK`] is what stands there instead, one per character.
    pub(crate) fn picture(&self) -> String {
        let header = match self.mode {
            Mode::Fullscreen => format!(
                "{}x{} cursor {},{} scrolled {}",
                self.columns, self.rows, self.row, self.column, self.scrolled
            ),
            Mode::Native => format!(
                "{}x{} cursor {},{} scrollback {}",
                self.columns,
                self.rows,
                self.row,
                self.column,
                self.scrollback.len()
            ),
        };

        let rows = self.framed(&self.grid);
        if rows.is_empty() {
            header
        } else {
            format!("{header}\n{rows}")
        }
    }

    /// `rows` as the lines of a picture: each padded to the full width, closed
    /// with a bar, and with this build's version masked out.
    fn framed(&self, rows: &[Vec<char>]) -> String {
        let mut lines = Vec::with_capacity(rows.len());

        for row in rows {
            let mut line: String = row.iter().collect();
            for _ in row.len()..self.columns {
                line.push(' ');
            }
            lines.push(format!("|{line}|"));
        }

        let masked = MASK.to_string().repeat(VERSION.len());

        lines.join("\n").replace(VERSION, &masked)
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
    ///
    /// The row pushed off the top is kept on a native screen, where scrolling
    /// is how a finished row reaches the reader's scrollback, and counted on a
    /// fullscreen one, where nothing should be making the window move.
    fn down(&mut self) {
        self.row += 1;

        if self.row >= self.rows {
            let top = self.grid.remove(0);
            let ran_on = self.ran_on.remove(0);
            self.grid.push(Vec::new());
            self.ran_on.push(false);
            self.row = self.rows.saturating_sub(1);

            match self.mode {
                Mode::Fullscreen => self.scrolled += 1,
                Mode::Native => {
                    self.scrollback.push(top);
                    self.ran_on_back.push(ran_on);
                }
            }
        }
    }

    /// Erases from the cursor to the end of the row it is on.
    fn erase_row(&mut self) {
        if let Some(row) = self.grid.get_mut(self.row) {
            row.truncate(self.column);
        }
        if let Some(ran_on) = self.ran_on.get_mut(self.row) {
            *ran_on = false;
        }
    }

    /// Erases from the cursor to the end of the screen: the rest of this row
    /// and every row under it.
    fn erase_below(&mut self) {
        self.erase_row();
        for row in self.grid.iter_mut().skip(self.row + 1) {
            row.clear();
        }
        for ran_on in self.ran_on.iter_mut().skip(self.row + 1) {
            *ran_on = false;
        }
    }

    /// Moves the cursor up `params` rows, one when none is given, and stops at
    /// the top of the window as a terminal does.
    fn up(&mut self, params: &str) {
        let count = params.parse::<usize>().unwrap_or(1).max(1);
        self.row = self.row.saturating_sub(count);
    }

    /// Puts the cursor in column `params`, counted from one, and no further
    /// right than the last column the window has.
    fn across(&mut self, params: &str) {
        let count = params.parse::<usize>().unwrap_or(1).max(1);
        self.column = (count - 1).min(self.columns.saturating_sub(1));
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
    /// The whole set the renderer promises: park at a named cell, erase the
    /// rest of a row, colour, the two that hold a frame until all of it has
    /// arrived, the modes crucible borrows from the terminal — the screen it
    /// draws on among them — and the one question it asks. A native launch
    /// adds the three its frames are made of: erase to the end of the screen,
    /// cursor up and cursor to a column.
    fn act(&mut self, params: &str, ends: char) {
        if params == "?1049" && ends == 'h' {
            self.alternate = true;
        }

        if self.mode == Mode::Native {
            match (params, ends) {
                ("" | "0", 'J') => return self.erase_below(),
                (_, 'A') => return self.up(params),
                (_, 'G') => return self.across(params),
                _ => {}
            }
        }

        match (params, ends) {
            // Colour, the modes crucible borrows from the terminal, and the
            // device-attributes question it asks once at startup. None of them
            // moves the cursor or fills a column: the modes are state handed
            // back by a guard, and the last is a question.
            //
            // The modes are the mouse switched on at the prompt — buttons,
            // motion while one is held and motion while none is, and
            // coordinates that survive a wide window — bracketed
            // paste so a pasted newline is not a submission, and the level of
            // the newer key encoding that says which modifier was held — that
            // last one pushed with `>1u` and popped with `<u`, which is a stack
            // on the terminal rather than a pair of switches.
            //
            // Device attributes is not asked for its own sake. It is what says
            // the answer to the question before it has already arrived, because
            // a terminal replies in the order it was asked — without it, a
            // terminal implementing neither would be waited on for the whole
            // patience rather than answered at once.
            (_, 'm')
            | ("?1000" | "?1002" | "?1003" | "?1006" | "?2004" | "?25" | "?1049", 'h' | 'l')
            | (">1" | "<", 'u')
            | ("", 'c') => {}
            (_, 'H') => self.park(params),
            ("" | "0", 'K') => self.erase_row(),
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

    /// Puts the cursor at a named cell, counted the way the terminal counts
    /// them: row and then column, both from one, and either left out meaning
    /// the first.
    ///
    /// This is the whole of how the renderer moves, which is what makes the
    /// check on it worth carrying. A cell off the window is a frame drawn
    /// somewhere the reader cannot see, and on a real terminal it is clamped
    /// rather than refused — so nothing about the picture would say it had
    /// happened.
    fn park(&mut self, params: &str) {
        let mut at = params.split(';');
        let row = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
        let column = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
        let (row, column): (usize, usize) = (usize::max(row, 1) - 1, usize::max(column, 1) - 1);

        if row >= self.rows {
            self.refuse(format!(
                "parked the cursor on row {row} of a screen {} rows tall",
                self.rows
            ));
        }

        // One past the last column is where a cursor rests at the end of a full
        // row; anything beyond that is a column this window does not have.
        if column > self.columns {
            self.refuse(format!(
                "parked the cursor at column {column} on a screen {} columns wide",
                self.columns
            ));
        }

        self.row = row.min(self.rows.saturating_sub(1));
        self.column = column.min(self.columns);
    }

    /// Reads `ESC ] … BEL`: the tab title, and the one question at startup.
    fn command<'a>(&mut self, after: &'a str) -> &'a str {
        // Whichever ending comes first. A payload cannot contain either, so
        // the first one found is the one that ends this string rather than a
        // later one belonging to something else.
        let bell = after.find(char::from(BELL)).map(|at| (at, 1));
        let terminated = after.find(TERMINATOR).map(|at| (at, TERMINATOR.len()));
        let Some((at, ends)) = bell.into_iter().chain(terminated).min() else {
            self.refuse("wrote a command string with no end to it".to_owned());
            return "";
        };

        let said = after.get(..at).unwrap_or_default();
        self.commanded.push(said.to_owned());

        // `0;` is the tab title, which crucible holds for as long as it runs.
        // `11;?` asks the terminal what colour its own background is, which is
        // what the row a prompt is left on takes its ground from — asked once,
        // before the first frame, and never again. A terminal that does not
        // implement it ignores it, as it ignores any command string it does not
        // know; this window is not a terminal and draws the payload instead,
        // which is why it has to be named here rather than left to be noticed.
        // `8;;` is a hyperlink: the address the run after it points at, and
        // the same with nothing after it to close one. A terminal that knows
        // them makes the words between clickable, and one that does not draws
        // the words and drops these — which is what this does, since a link
        // changes nothing about which column anything lands in.
        if !said.starts_with("0;") && !said.starts_with("8;") && said != "11;?" {
            self.refuse(format!("wrote the command string {said:?}"));
        }

        after.get(at + ends..).unwrap_or_default()
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
        Some(b']') => {
            tail.contains(&BELL) || tail.windows(2).any(|pair| pair == TERMINATOR.as_bytes())
        }
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
        "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h",
        "\x1b[?2026h\x1b[?25l",
        "\x1b[1;1H\x1b[m\x1b[Kcrucible v0.0.9",
        "\x1b[2;1H\x1b[m\x1b[K\x1b[36m│ › ───\x1b[0m",
        "\x1b[2;1H\x1b[?25h\x1b[?2026l",
        "\x1b[?2026h\x1b[?25l",
        "\x1b[6;1H\x1b[m\x1b[K╭──╮",
        "\x1b[7;1H\x1b[m\x1b[K│  │",
        "\x1b[8;1H\x1b[m\x1b[K╰──╯",
        "\x1b[7;3H\x1b[?25h\x1b[?2026l",
    );

    /// The same bytes one at a time, which is every cut point at once.
    fn byte_at_a_time(written: &str) -> Screen {
        let mut screen = Screen::new(24, 8);

        for byte in written.as_bytes() {
            screen.feed(std::slice::from_ref(byte));
        }
        screen
    }

    /// A picture holds no version, so cutting a release costs no snapshot.
    ///
    /// The opening screen draws this build's own version, and one case here
    /// leaves it on screen. Without the mask, every release re-accepts that
    /// snapshot — and a snapshot re-accepted on a schedule is one nobody
    /// reads, which is the whole of what it was for.
    #[test]
    fn a_picture_holds_no_version_of_the_build_that_drew_it() {
        let mut screen = Screen::new(24, 8);
        screen.feed(concat!("crucible v", env!("CARGO_PKG_VERSION")).as_bytes());

        let picture = screen.picture();

        assert!(
            !picture.contains(env!("CARGO_PKG_VERSION")),
            "the picture names the version it was drawn by:\n{picture}"
        );
        assert!(picture.contains("crucible v#"), "{picture}");
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
    fn parking_the_cursor_off_the_window_is_reported() {
        // The second. A real terminal clamps an address it does not have,
        // which is what makes this worth watching for: the row is drawn
        // somewhere the reader can see and nothing about the picture says the
        // frame meant it to be anywhere else.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[9;1H");

        assert!(
            screen
                .refusals()
                .iter()
                .any(|said| said.contains("of a screen 4 rows tall")),
            "{:?}",
            screen.refusals()
        );
    }

    #[test]
    fn a_sequence_the_renderer_does_not_promise_is_reported_by_name() {
        // Erasing the whole screen at once is not how a frame gets there: a
        // row is erased by the frame that is about to write it, so a screen
        // cleared out from under one is a sequence nothing here composed.
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
        screen.feed(b"\x1b[?2026h\x1b[1;1H\x1b[Kone");

        assert!(screen.is_holding());
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());

        screen.feed(b"\x1b[?2026l");
        assert!(!screen.is_holding());
    }

    #[test]
    fn text_in_a_frame_still_being_written_is_not_yet_shown() {
        // A read can end inside a frame: the box's top edge has arrived and its
        // bottom edge has not. A real terminal shows the frame before until the
        // closing sequence, so a step waiting for the top edge must not take
        // the screen until the rest of that frame is on it too.
        let mut screen = Screen::new(12, 4);
        screen.feed(b"\x1b[?2026h\x1b[1;1H\x1b[K+- 1 queued");

        assert!(!screen.shows("1 queued"), "{}", screen.picture());

        screen.feed(b"\x1b[2;1H\x1b[K+----------\x1b[?2026l");
        assert!(screen.shows("1 queued"), "{}", screen.picture());
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
        // across the screen — and the picture assembled from them is not the
        // one the reader saw.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"one\ntwo");

        assert_eq!(screen.refusals(), ["ended a row with a bare newline"]);
    }

    /// The rows of `picture`, edges and header off.
    fn rows(picture: &str) -> Vec<String> {
        picture
            .lines()
            .skip(1)
            .map(|line| line.trim_matches('|').to_owned())
            .collect()
    }

    #[test]
    fn erase_below_on_a_native_screen_clears_from_the_cursor_to_the_foot() {
        // The sequence every native frame opens with: back to the top of the
        // region, then everything from there to the foot of the screen goes,
        // the rest of the row the cursor is on included. What stands above the
        // cursor is the reader's and is not touched.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"one\r\ntwo\r\nthree\r\nfour");
        screen.feed(b"\r\x1b[2At\x1b[J");

        assert_eq!(
            rows(&screen.picture()),
            ["one     ", "t       ", "        ", "        "]
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn cursor_up_on_a_native_screen_climbs_and_stops_at_the_top() {
        // A terminal clamps a climb past its top row, and the native renderer
        // leans on that: how far back the region's top is after a resize is
        // worked out rather than read, so a count that lands high is one the
        // terminal is trusted to stop.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"one\r\ntwo");
        screen.feed(b"\x1b[5AX");

        assert_eq!(
            rows(&screen.picture()),
            ["oneX    ", "two     ", "        ", "        "]
        );
        assert!(
            screen.picture().starts_with("8x4 cursor 0,4"),
            "{}",
            screen.picture()
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn cursor_to_column_on_a_native_screen_lands_there() {
        // Counted from one, the way the terminal counts, and clamped to the
        // last column the window has.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"one two\x1b[3Gx\x1b[99G!");

        assert_eq!(
            rows(&screen.picture()).first().map(String::as_str),
            Some("onx two!")
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn the_three_native_sequences_are_still_refused_on_a_fullscreen_screen() {
        // The full screen names every row it writes, so a frame that moved
        // relatively or erased wholesale is one it never composed — and a
        // screen that applied either would be agreeing with the claim it is
        // here to check.
        let mut screen = Screen::new(8, 4);
        screen.feed(b"\x1b[J\x1b[1A\x1b[1G");

        assert_eq!(
            screen.refusals(),
            ["wrote ESC[J", "wrote ESC[1A", "wrote ESC[1G"]
        );
    }

    #[test]
    fn a_row_scrolled_off_a_native_screen_is_in_its_scrollback() {
        // In the terminal's own buffer a row pushed off the top is kept, and
        // what a reader could scroll back to is half of what a native case
        // asserts on: a row written once is a row found once in the scrollback
        // and the window together.
        let mut screen = Screen::native(8, 2);
        screen.feed(b"one\r\ntwo\r\nthree");

        assert_eq!(screen.scrollback(), "|one     |");
        assert_eq!(rows(&screen.picture()), ["two     ", "three   "]);
        assert!(
            screen.picture().starts_with("8x2 cursor 1,5 scrollback 1"),
            "{}",
            screen.picture()
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_native_screen_narrowed_rewraps_what_it_holds_and_widened_joins_it_again() {
        // A terminal that rewraps folds each line at the new width and joins
        // the pieces again when the window widens, which is what the native
        // renderer counts on when it works out where the region's top went.
        let mut screen = Screen::native(8, 4);
        screen.feed(b"abcdefgh\r\nij\r\n");

        screen.resize(4, 4);
        assert_eq!(rows(&screen.picture()), ["abcd", "efgh", "ij  ", "    "]);
        assert!(
            screen.picture().starts_with("4x4 cursor 3,0 scrollback 0"),
            "{}",
            screen.picture()
        );

        screen.resize(8, 4);
        assert_eq!(
            rows(&screen.picture()),
            ["abcdefgh", "ij      ", "        ", "        "]
        );
        assert!(
            screen.picture().starts_with("8x4 cursor 2,0 scrollback 0"),
            "{}",
            screen.picture()
        );
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }

    #[test]
    fn a_native_screen_narrowed_past_its_height_scrolls_the_top_rows_back() {
        // Rows that no longer fit go into the scrollback, and the cursor keeps
        // its place in the line it was on.
        let mut screen = Screen::native(8, 3);
        screen.feed(b"abcdefgh\r\nij\r\nk");

        screen.resize(4, 3);

        assert_eq!(screen.scrollback(), "|abcd|");
        assert_eq!(rows(&screen.picture()), ["efgh", "ij  ", "k   "]);
        assert!(
            screen.picture().starts_with("4x3 cursor 2,1 scrollback 1"),
            "{}",
            screen.picture()
        );
    }

    #[test]
    fn entering_the_alternate_screen_is_remembered() {
        // The one thing that says a session took the full screen rather than
        // the reader's buffer, kept so a native case can ask.
        let mut screen = Screen::native(8, 4);
        assert!(!screen.entered_alternate());

        screen.feed(b"\x1b[?1049h");

        assert!(screen.entered_alternate());
        assert!(screen.refusals().is_empty(), "{:?}", screen.refusals());
    }
}
