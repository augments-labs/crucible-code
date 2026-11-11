//! Native mode, asserted against a terminal that keeps a scrollback.
//!
//! [`Picture`](crate::Picture) is a screen this process owns: a row is named
//! and nothing scrolls. Native mode writes into a buffer that does, so these
//! tests replay its bytes into [`Emulator`], which moves the cursor relatively,
//! wraps, scrolls rows off the top into a scrollback, erases to the end of the
//! screen, and clears the screen and the scrollback for a resize to be given
//! everything again — the handful of things a native frame says — and rewraps what it
//! holds when the window changes width, as most terminals now do. Each cell
//! keeps the colour it was written in. What a reader would find on scrolling
//! back is then a list of strings to assert on.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;

use super::super::*;
use super::SETTLE;
use crate::color::{Palette, Slot};
use crate::row::Row;
use crate::terminal::{Size, Terminal, TerminalError};

/// A character on screen, and the colour it was written in: an index into
/// [`Emulator::pens`].
type Cell = (char, usize);

/// A cell nothing was written into.
const EMPTY: Cell = (' ', 0);

/// The main buffer of a terminal: a screen, and what scrolled off its top.
#[derive(Debug)]
struct Emulator {
    columns: usize,
    rows: usize,
    screen: Vec<Vec<Cell>>,
    /// Whether each row of the screen ran on into the next by wrapping.
    ran_on: Vec<bool>,
    scrollback: Vec<Vec<Cell>>,
    /// The same, for each row of the scrollback.
    ran_on_back: Vec<bool>,
    /// Row and column of the cursor.
    at: (usize, usize),
    /// Whether the last character filled the row, so the next one wraps.
    wrapping: bool,
    /// Every colour written so far, as the parameters that set it since the
    /// last reset; the first is no colour at all.
    pens: Vec<String>,
    /// The colour the next character is written in.
    pen: usize,
}

impl Emulator {
    fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns,
            rows,
            screen: vec![Vec::new(); rows],
            ran_on: vec![false; rows],
            scrollback: Vec::new(),
            ran_on_back: Vec::new(),
            at: (0, 0),
            wrapping: false,
            pens: vec![String::new()],
            pen: 0,
        }
    }

    /// Replays `bytes` onto the buffer.
    fn feed(&mut self, bytes: &str) {
        let mut left = bytes.chars().peekable();
        while let Some(character) = left.next() {
            match character {
                '\r' => {
                    self.at.1 = 0;
                    self.wrapping = false;
                }
                '\n' => {
                    self.down();
                    self.wrapping = false;
                }
                '\x1b' => match left.next() {
                    Some('[') => {
                        let mut parameters = String::new();
                        let ending = loop {
                            match left.next() {
                                Some(byte @ '@'..='~') => break byte,
                                Some(byte) => parameters.push(byte),
                                None => return,
                            }
                        };
                        self.control(ending, &parameters);
                    }
                    Some(']') => {
                        while let Some(byte) = left.next() {
                            if byte == '\x07' {
                                break;
                            }
                            if byte == '\x1b' && left.peek() == Some(&'\\') {
                                left.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                printed => self.put(printed),
            }
        }
    }

    /// One row down, scrolling the top row into the scrollback at the foot.
    fn down(&mut self) {
        if self.at.0 + 1 < self.rows {
            self.at.0 += 1;
            return;
        }
        let top = self.screen.remove(0);
        self.scrollback.push(top);
        self.screen.push(Vec::new());
        self.ran_on_back.push(self.ran_on.remove(0));
        self.ran_on.push(false);
    }

    fn put(&mut self, character: char) {
        if self.wrapping {
            if let Some(ran_on) = self.ran_on.get_mut(self.at.0) {
                *ran_on = true;
            }
            self.at.1 = 0;
            self.down();
            self.wrapping = false;
        }
        let (row, column) = self.at;
        if let Some(line) = self.screen.get_mut(row) {
            if line.len() <= column {
                line.resize(column + 1, EMPTY);
            }
            if let Some(cell) = line.get_mut(column) {
                *cell = (character, self.pen);
            }
        }
        if column + 1 >= self.columns {
            self.wrapping = true;
        } else {
            self.at.1 += 1;
        }
    }

    fn control(&mut self, ending: char, parameters: &str) {
        // Private modes say nothing about where a character goes.
        if parameters.starts_with('?') {
            return;
        }
        if ending == 'm' {
            self.colours(parameters);
            return;
        }
        let count = parameters.parse::<usize>().unwrap_or(1).max(1);
        match ending {
            'A' => self.at.0 = self.at.0.saturating_sub(count),
            'B' => self.at.0 = (self.at.0 + count).min(self.rows - 1),
            'C' => self.at.1 = (self.at.1 + count).min(self.columns - 1),
            'D' => self.at.1 = self.at.1.saturating_sub(count),
            'G' => self.at.1 = (count - 1).min(self.columns - 1),
            'H' => {
                let mut at = parameters.split(';');
                let row = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
                let column = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
                self.at = (
                    usize::max(row, 1).min(self.rows) - 1,
                    usize::max(column, 1).min(self.columns) - 1,
                );
            }
            'J' if parameters == "2" => {
                for line in &mut self.screen {
                    line.clear();
                }
                for ran_on in &mut self.ran_on {
                    *ran_on = false;
                }
            }
            'J' if parameters == "3" => {
                self.scrollback.clear();
                self.ran_on_back.clear();
            }
            'J' => {
                let (row, column) = self.at;
                if let Some(line) = self.screen.get_mut(row) {
                    line.truncate(column);
                }
                for below in self.screen.iter_mut().skip(row + 1) {
                    below.clear();
                }
                for ran_on in self.ran_on.iter_mut().skip(row) {
                    *ran_on = false;
                }
            }
            'K' => {
                let (row, column) = self.at;
                if let Some(line) = self.screen.get_mut(row) {
                    line.truncate(column);
                }
                if let Some(ran_on) = self.ran_on.get_mut(row) {
                    *ran_on = false;
                }
            }
            _ => return,
        }
        self.wrapping = false;
    }

    /// Takes up the colour `parameters` set, on top of the one in force
    /// unless they open with a reset.
    fn colours(&mut self, parameters: &str) {
        let (reset, added) = match parameters.split_once(';') {
            Some(("0" | "", after)) => (true, after),
            _ if matches!(parameters, "" | "0") => (true, ""),
            _ => (false, parameters),
        };
        let worn = if reset {
            ""
        } else {
            self.pens.get(self.pen).map_or("", String::as_str)
        };
        let pen = match (worn, added) {
            (only, "") | ("", only) => only.to_owned(),
            (under, over) => format!("{under};{over}"),
        };
        self.pen = self
            .pens
            .iter()
            .position(|known| *known == pen)
            .unwrap_or_else(|| {
                self.pens.push(pen);
                self.pens.len() - 1
            });
    }

    /// The window made `columns` wide and `rows` tall in one step, as a
    /// reader dragging its corner would, with everything it holds rewrapped
    /// to the new width.
    ///
    /// Rows that ran on into each other by wrapping are one line again, and
    /// each line is folded at the new width. The cursor keeps its place in the
    /// line it was on; nothing below it that is empty is kept; and the screen
    /// is the foot of what is left at the new height, the rest above it being
    /// scrollback.
    fn resize(&mut self, columns: usize, rows: usize) {
        let was = self.columns;
        self.rows = rows;
        let cursor = self.scrollback.len() + self.at.0;
        let back = self.scrollback.drain(..).zip(self.ran_on_back.drain(..));
        let rows: Vec<(Vec<Cell>, bool)> = back
            .chain(self.screen.drain(..).zip(self.ran_on.drain(..)))
            .collect();

        // Rows back into lines, with where in its line the cursor was.
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        let mut line = Vec::new();
        let mut caret = (0, 0);
        for (index, (mut row, ran_on)) in rows.into_iter().enumerate() {
            if index == cursor {
                caret = (lines.len(), line.len() + self.at.1);
            }
            if ran_on {
                row.resize(was, EMPTY);
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
        let mut folded: Vec<(Vec<Cell>, bool)> = Vec::new();
        let mut at = (0, 0);
        for (index, line) in lines.into_iter().enumerate() {
            let first = folded.len();
            let pieces: Vec<Vec<Cell>> = if line.is_empty() {
                vec![Vec::new()]
            } else {
                line.chunks(columns).map(<[Cell]>::to_vec).collect()
            };
            let count = pieces.len();
            for (piece, row) in pieces.into_iter().enumerate() {
                folded.push((row, piece + 1 < count));
            }
            if index == caret.0 {
                at = (first + caret.1 / columns, caret.1 % columns);
                while folded.len() <= at.0 {
                    folded.push((Vec::new(), false));
                }
            }
        }

        let top = folded.len().saturating_sub(self.rows);
        let screen = folded.split_off(top);
        for (row, ran_on) in folded {
            self.scrollback.push(row);
            self.ran_on_back.push(ran_on);
        }
        for (row, ran_on) in screen {
            self.screen.push(row);
            self.ran_on.push(ran_on);
        }
        self.screen.resize(self.rows, Vec::new());
        self.ran_on.resize(self.rows, false);
        self.at = (at.0.saturating_sub(top), at.1);
        self.columns = columns;
        self.wrapping = false;
    }

    /// The screen, one string a row, padding taken off.
    fn screen(&self) -> Vec<String> {
        self.screen.iter().map(|row| text(row)).collect()
    }

    /// The scrollback, the same way.
    fn scrollback(&self) -> Vec<String> {
        self.scrollback.iter().map(|row| text(row)).collect()
    }

    /// Everything a reader could scroll to: the scrollback, then the screen.
    fn all(&self) -> Vec<String> {
        let mut all = self.scrollback();
        all.extend(self.screen());
        all
    }

    /// The first `rows` rows a reader could scroll to, scrollback first, each
    /// with its colours named: a run in a colour is the colour's parameters in
    /// angle brackets, then its text.
    fn coloured(&self, rows: usize) -> Vec<String> {
        self.scrollback
            .iter()
            .chain(&self.screen)
            .take(rows)
            .map(|row| {
                let kept = row.len() - row.iter().rev().take_while(|cell| **cell == EMPTY).count();
                let mut said = String::new();
                let mut pen = 0;
                for (character, worn) in row.iter().take(kept) {
                    if *worn != pen {
                        pen = *worn;
                        let _ = write!(said, "<{}>", self.pens.get(pen).map_or("", String::as_str));
                    }
                    said.push(*character);
                }
                said
            })
            .collect()
    }
}

/// A row of cells as its characters, padding taken off.
fn text(row: &[Cell]) -> String {
    row.iter()
        .map(|(character, _)| character)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// What the terminal was sent and what it shows, shared with the renderer so a
/// test can read both after the renderer has gone.
#[derive(Debug)]
struct Seen {
    written: String,
    emulator: Emulator,
    is_terminal: bool,
    /// How many of the next size queries fail.
    unsizable: usize,
    /// How many of the next writes are refused.
    unwritable: usize,
    /// The size the terminal reports, where that is not the size it draws at.
    reports: Option<Size>,
    /// How many times the terminal was asked its size.
    asked: usize,
}

/// A terminal with a scrollback, for a renderer to own and a test to read.
#[derive(Debug, Clone)]
struct Window(Rc<RefCell<Seen>>);

impl Window {
    fn new(columns: usize, rows: usize) -> Self {
        Self(Rc::new(RefCell::new(Seen {
            written: String::new(),
            emulator: Emulator::new(columns, rows),
            is_terminal: true,
            unsizable: 0,
            unwritable: 0,
            reports: None,
            asked: 0,
        })))
    }

    /// Makes the next `times` size queries fail, as a terminal that would
    /// not say does.
    fn loses_size(&self, times: usize) {
        self.0.borrow_mut().unsizable = times;
    }

    /// Makes the next `times` writes fail, writing nothing, as a terminal
    /// that cannot be written to for a while does.
    fn refuses_writes(&self, times: usize) {
        self.0.borrow_mut().unwritable = times;
    }

    /// Makes every size query answer `size`, whatever the window draws at: a
    /// pty nobody told its size reports none, while the display behind it has
    /// rows all the same.
    fn reports(&self, size: Size) {
        self.0.borrow_mut().reports = Some(size);
    }

    /// How many times the terminal was asked its size.
    fn asked(&self) -> usize {
        self.0.borrow().asked
    }

    fn redirected(columns: usize, rows: usize) -> Self {
        let window = Self::new(columns, rows);
        window.0.borrow_mut().is_terminal = false;
        window
    }

    fn written(&self) -> String {
        self.0.borrow().written.clone()
    }

    /// Forgets what was written, keeping what it drew.
    fn take(&self) -> String {
        std::mem::take(&mut self.0.borrow_mut().written)
    }

    fn all(&self) -> Vec<String> {
        self.0.borrow().emulator.all()
    }

    fn screen(&self) -> Vec<String> {
        self.0.borrow().emulator.screen()
    }

    fn caret(&self) -> (usize, usize) {
        self.0.borrow().emulator.at
    }

    fn resize(&self, columns: usize, rows: usize) {
        self.0.borrow_mut().emulator.resize(columns, rows);
    }

    fn scrollback(&self) -> Vec<String> {
        self.0.borrow().emulator.scrollback()
    }

    /// How many rows anywhere in the buffer say `text`.
    fn rows_saying(&self, text: &str) -> usize {
        self.all().iter().filter(|row| row.contains(text)).count()
    }
}

impl Terminal for Window {
    fn size(&self) -> Result<Size, TerminalError> {
        let mut seen = self.0.borrow_mut();
        seen.asked += 1;
        if seen.unsizable > 0 {
            seen.unsizable = seen.unsizable.saturating_sub(1);
            return Err(TerminalError::Io(std::io::Error::other(
                "the window would not say",
            )));
        }
        Ok(seen.reports.unwrap_or(Size {
            columns: seen.emulator.columns,
            rows: seen.emulator.rows,
        }))
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        let mut seen = self.0.borrow_mut();
        if seen.unwritable > 0 {
            seen.unwritable = seen.unwritable.saturating_sub(1);
            return Err(TerminalError::Io(std::io::ErrorKind::BrokenPipe.into()));
        }
        seen.written.push_str(text);
        if seen.is_terminal {
            seen.emulator.feed(text);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        Ok(())
    }

    fn is_terminal(&self) -> bool {
        self.0.borrow().is_terminal
    }
}

/// A native renderer on `window`.
fn native(window: &Window) -> Renderer<Window> {
    Renderer::drawing(window.clone(), ScreenMode::Native)
}

fn row(text: &str) -> Row {
    Row::new().then(Slot::Plain, text)
}

/// A box of three rows with the caret after its mark, as the prompt stands.
fn boxed() -> (Vec<Row>, Caret) {
    (
        vec![row("+--box--+"), row("| > typed"), row("+-------+")],
        Caret { row: 1, column: 4 },
    )
}

fn stands(render: &mut Renderer<Window>) {
    let (rows, caret) = boxed();
    render.live(&rows, caret, Palette::plain()).unwrap();
}

/// Whether `written` names a screen row by number anywhere in it.
fn addresses_a_row(written: &str) -> bool {
    written.split("\x1b[").skip(1).any(|piece| {
        piece
            .split_once(['H', 'f'])
            .is_some_and(|(at, _)| at.chars().all(|byte| byte.is_ascii_digit() || byte == ';'))
    })
}

#[test]
fn a_native_frame_never_names_a_screen_row() {
    // The alternate screen and the pointer are taken by guards the session
    // holds, not by the renderer, so the sequences checked first below are
    // ones no renderer writes in either mode: they are a fence, not the proof.
    // That native mode takes neither is `holds`, in the session's own tests.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    render
        .opens(Box::new(|_, _| vec![row("crucible"), row("the opening")]))
        .unwrap();
    stands(&mut render);
    render.commit("> hello").unwrap();
    render.stream("an answer on its way").unwrap();
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    render.seal().unwrap();
    render.settle().unwrap();
    window.resize(50, 10);
    render.resized().unwrap();
    stands(&mut render);
    drop(render);

    let written = window.written();
    for held in [
        "\x1b[?1049h",
        "\x1b[?1000h",
        "\x1b[?1002h",
        "\x1b[?1003h",
        "\x1b[?1006h",
    ] {
        assert!(
            !written.contains(held),
            "native wrote {held:?}: {written:?}"
        );
    }
    assert!(
        !addresses_a_row(&written),
        "a native frame named a screen row, which in the reader's own buffer \
         lands on whatever their shell left there: {written:?}"
    );
}

#[test]
fn a_finished_native_row_is_written_exactly_once_across_later_frames() {
    let window = Window::new(40, 6);
    let mut render = native(&window);

    stands(&mut render);
    // Until it is sealed the row stands in the live region, which is redrawn
    // like the rest of it; the seal is where it goes out, once, for good.
    render.commit("a finished row").unwrap();
    window.take();
    render.seal().unwrap();
    assert_eq!(
        window.take().matches("a finished row").count(),
        1,
        "the seal did not send the row out exactly once"
    );
    for at in 0..8 {
        render.stream(&format!("piece {at} ")).unwrap();
        render
            .under(&[row(&format!("tick {at}"))], None, Palette::plain())
            .unwrap();
        stands(&mut render);
        render.seal().unwrap();
    }
    render.settle().unwrap();
    for at in 0..8 {
        render.commit(&format!("later {at}")).unwrap();
        render.seal().unwrap();
    }

    assert_eq!(
        window.written().matches("a finished row").count(),
        0,
        "a later frame wrote the sealed row again"
    );
    assert_eq!(window.rows_saying("a finished row"), 1);
    for at in 0..8 {
        assert_eq!(window.rows_saying(&format!("later {at}")), 1, "later {at}");
    }
}

#[test]
fn a_native_region_that_grew_in_a_turn_keeps_its_height_so_the_box_stays_at_the_foot() {
    // A turn grows the region: the line asked and the answer stand in it with
    // the spinner under them, and the growth pushes the rows above into the
    // scrollback. When the turn ends the answer is sealed out and the spinner
    // goes, and what the region still has to show is shorter than the rows it
    // stood in. The rows it pushed out cannot come back, so the region keeps
    // its height, padded with blank rows at its top, and the box stands at
    // the foot where the turn left it rather than two rows above blank ones.
    let window = Window::new(40, 8);
    let mut render = native(&window);
    for at in 0..5 {
        render.commit(&format!("said {at}")).unwrap();
    }
    stands(&mut render);
    render.seal().unwrap();

    render.commit("> asked").unwrap();
    render.stream("an answer").unwrap();
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    stands(&mut render);
    assert_eq!(
        window.scrollback(),
        ["said 0", "said 1", "said 2"],
        "the turn did not grow the region: {:#?}",
        window.all()
    );
    render.settle().unwrap();
    render.under(&[], None, Palette::plain()).unwrap();
    stands(&mut render);
    render.seal().unwrap();

    assert_eq!(
        window.screen(),
        [
            "said 3",
            "said 4",
            "> asked",
            "an answer",
            "",
            "+--box--+",
            "| > typed",
            "+-------+",
        ],
        "{:#?}",
        window.all()
    );
    assert_eq!(window.caret(), (6, 4));
    assert_eq!(window.scrollback(), ["said 0", "said 1", "said 2"]);
}

#[test]
fn a_native_panel_opened_and_closed_leaves_no_rows_behind() {
    let window = Window::new(40, 8);
    let mut render = native(&window);

    stands(&mut render);
    for at in 0..12 {
        render.commit(&format!("said {at:02}")).unwrap();
    }
    render.seal().unwrap();

    let panel: Vec<Row> = (0..5).map(|at| row(&format!("panel row {at}"))).collect();
    render
        .live(&[], Caret::default(), Palette::plain())
        .unwrap();
    render.under(&panel, None, Palette::plain()).unwrap();
    render.seal().unwrap();
    render.under(&[], None, Palette::plain()).unwrap();
    stands(&mut render);

    assert_eq!(window.rows_saying("panel row"), 0, "{:#?}", window.all());
    for at in 0..12 {
        assert_eq!(
            window.rows_saying(&format!("said {at:02}")),
            1,
            "said {at:02}: {:#?}",
            window.all()
        );
    }
    let screen = window.screen();
    let foot: Vec<&str> = screen
        .iter()
        .map(String::as_str)
        .filter(|row| !row.is_empty())
        .rev()
        .take(3)
        .collect();
    assert_eq!(foot, ["+-------+", "| > typed", "+--box--+"]);
}

#[test]
fn a_native_resize_redraws_only_the_live_region() {
    let window = Window::new(40, 10);
    let mut render = native(&window);

    stands(&mut render);
    render.commit("in the scrollback").unwrap();
    render.commit("and so is this").unwrap();
    render.seal().unwrap();
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    window.take();

    window.resize(60, 10);
    render.resized().unwrap();
    stands(&mut render);
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();

    let after = window.take();
    assert!(
        !after.contains("in the scrollback") && !after.contains("and so is this"),
        "a resize wrote finished rows again: {after:?}"
    );
    assert_eq!(window.rows_saying("in the scrollback"), 1);
    assert_eq!(window.rows_saying("and so is this"), 1);
    assert_eq!(window.rows_saying("* thinking"), 1, "{:#?}", window.all());
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
}

/// Draws `said` as finished rows over the box and a running turn, as a native
/// session stands before its window changes size.
fn finished(render: &mut Renderer<Window>, said: &[String]) {
    stands(render);
    for line in said {
        render.commit(line).unwrap();
    }
    render.seal().unwrap();
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
}

/// Resizes `window` to `columns` by `rows`, tells `render`, and lets the window
/// settle at that size.
fn settles_at(window: &Window, render: &mut Renderer<Window>, columns: usize, rows: usize) {
    window.resize(columns, rows);
    render.resized().unwrap();
    render.settled(Instant::now() + SETTLE).unwrap();
}

#[test]
fn a_settled_native_resize_writes_every_kept_row_once_at_the_new_width() {
    // What the terminal kept was cut at forty columns, and no two terminals
    // put that at twenty the same way. Once the window has kept its new size,
    // the screen and the scrollback are cleared and every finished row is
    // written again, folded at twenty, and the region under it once.
    let window = Window::new(40, 6);
    let mut render = native(&window);
    let said: Vec<String> = (0..8)
        .map(|at| format!("said {at:02} in words that fold at twenty"))
        .collect();
    finished(&mut render, &said);
    window.take();

    window.resize(20, 6);
    render.resized().unwrap();
    render.settled(Instant::now()).unwrap();
    let before = window.take();
    assert!(
        !before.contains("\x1b[3J"),
        "a window still being dragged was given everything again: {before:?}"
    );

    render.settled(Instant::now() + SETTLE).unwrap();
    let after = window.take();
    assert!(after.contains("\x1b[2J\x1b[3J"), "{after:?}");
    for at in 0..8 {
        assert_eq!(
            window.rows_saying(&format!("said {at:02} in")),
            1,
            "said {at:02}: {:#?}",
            window.all()
        );
    }
    assert!(
        window.all().iter().any(|row| row == "that fold at twenty"),
        "a finished row was not folded at the new width: {:#?}",
        window.all()
    );
    assert_eq!(window.rows_saying("* thinking"), 1, "{:#?}", window.all());
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());

    // Given once: the next wait clears nothing and writes nothing of it again.
    render.settled(Instant::now() + SETTLE).unwrap();
    let again = window.take();
    assert!(
        !again.contains("\x1b[3J"),
        "the scrollback was cleared a second time: {again:?}"
    );
    assert!(!again.contains("said"), "{again:?}");
}

#[test]
fn a_native_resize_before_the_first_frame_draws_at_the_new_size_and_clears_nothing() {
    // The window changes size between the renderer being made and its first
    // frame. Nothing it wrote was cut at the old width, so there is nothing
    // to give back: the first frame is drawn at the new width, and settling
    // leaves the reader's scrollback, and the shell's lines in it, alone.
    let window = Window::new(40, 10);
    window.clone().write("$ crucible\r\n").unwrap();
    let mut render = native(&window);

    window.resize(30, 10);
    stands(&mut render);
    assert_eq!(render.columns(), 30);
    assert_eq!(
        render.settles_in(Instant::now()),
        None,
        "a window not yet drawn in waits to be given everything again"
    );
    render.settled(Instant::now() + SETTLE).unwrap();

    let written = window.take();
    assert!(
        !written.contains("\x1b[3J"),
        "the scrollback was cleared: {written:?}"
    );
    assert_eq!(window.rows_saying("$ crucible"), 1, "{:#?}", window.all());
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
}

#[test]
fn a_native_replay_after_the_ceiling_dropped_rows_says_the_rest_is_in_the_session_log() {
    let window = Window::new(40, 6);
    let mut render = native(&window);
    stands(&mut render);
    let mut at = 0;
    while !render.record.spilled() {
        render.commit(&format!("said {at:05}")).unwrap();
        at += 1;
        assert!(at < 100_000, "the ceiling never dropped a row");
    }
    render.seal().unwrap();
    stands(&mut render);

    settles_at(&window, &mut render, 50, 6);

    let all = window.all();
    assert_eq!(
        all.iter()
            .filter(|row| row.as_str() == "Earlier output is in the session log.")
            .count(),
        1,
        "{:#?}",
        all.get(..4)
    );
    assert_eq!(window.rows_saying("said 00000"), 0, "{:#?}", all.get(..4));
    assert_eq!(window.rows_saying(&format!("said {:05}", at - 1)), 1);
}

#[test]
fn a_native_replay_of_an_opening_partly_sent_writes_each_of_its_rows_once() {
    // A window shorter than the card sends its top rows out and keeps the rest
    // in the region. What is given back is what went out, and the region
    // draws the rest under it, so no row of the card is written twice.
    let window = Window::new(40, 5);
    let mut render = native(&window);
    render
        .opens(Box::new(|_, _| {
            (1..=6).map(|at| row(&format!("card row {at}"))).collect()
        }))
        .unwrap();
    stands(&mut render);
    assert!(
        render.record.first() > 0 && render.record.first() < 6,
        "the card was not partly sent: {:#?}",
        window.all()
    );

    settles_at(&window, &mut render, 30, 5);

    for at in 1..=6 {
        assert_eq!(
            window.rows_saying(&format!("card row {at}")),
            1,
            "card row {at}: {:#?}",
            window.all()
        );
    }
}

#[test]
fn a_native_replay_of_an_opening_partly_sent_lays_all_of_it_at_the_new_width() {
    // A card with a border, laid to the width it is handed. A window shorter
    // than the card sends its top rows out, and a narrower one then has to be
    // given the whole card again at its own width: rows laid for the old
    // width, cut at the new one, are a border with its right edge gone.
    let card = |columns: usize, _| -> Vec<Row> {
        let inner = columns.saturating_sub(2);
        (1..=6)
            .map(|at| row(&format!("<{:-^inner$}>", format!("card row {at}"))))
            .collect()
    };
    let window = Window::new(40, 5);
    let mut render = native(&window);
    render.opens(Box::new(card)).unwrap();
    stands(&mut render);
    assert!(
        render.record.first() > 0 && render.record.first() < 6,
        "the card was not partly sent: {:#?}",
        window.all()
    );

    settles_at(&window, &mut render, 30, 5);

    let laid = card(render.transcript_columns(), Glyphs::Unicode);
    let all = window.all();
    for (at, laid) in (1..=6).zip(laid.iter().map(Row::text)) {
        let named = format!("card row {at}");
        let shown: Vec<&String> = all.iter().filter(|row| row.contains(&named)).collect();
        assert_eq!(
            shown,
            [&laid],
            "{named} is not the card laid at the new width: {all:#?}"
        );
    }
}

#[test]
fn a_native_replay_lays_an_opening_wholly_sent_afresh_at_the_new_width() {
    // The card went out whole at forty columns and the terminal holds it as
    // it was laid then. Given back once the window settles at thirty, it is
    // laid again for thirty, as the full screen would draw it, rather than
    // written as the rows it was cut into at forty.
    let window = Window::new(40, 10);
    let mut render = native(&window);
    render
        .opens(Box::new(|columns, _| {
            vec![row(&format!("card laid at {columns}")), row("card foot")]
        }))
        .unwrap();
    stands(&mut render);
    render.commit("said under the card").unwrap();
    render.seal().unwrap();
    assert!(
        render.record.first() > 2,
        "the card was not wholly sent: {:#?}",
        window.all()
    );

    settles_at(&window, &mut render, 30, 10);

    assert_eq!(
        window.rows_saying("card laid at 30"),
        1,
        "{:#?}",
        window.all()
    );
    assert_eq!(
        window.rows_saying("card laid at 40"),
        0,
        "{:#?}",
        window.all()
    );
    assert_eq!(window.rows_saying("card foot"), 1, "{:#?}", window.all());
    assert_eq!(
        window.rows_saying("said under the card"),
        1,
        "{:#?}",
        window.all()
    );
}

#[test]
fn a_native_replay_after_a_clear_gives_back_nothing_from_before_it() {
    let window = Window::new(40, 6);
    let mut render = native(&window);
    finished(&mut render, &["said before the clear".to_owned()]);
    render.empties().unwrap();
    finished(&mut render, &["said after the clear".to_owned()]);

    settles_at(&window, &mut render, 30, 6);

    assert_eq!(
        window.rows_saying("before the clear"),
        0,
        "{:#?}",
        window.all()
    );
    assert_eq!(
        window.rows_saying("after the clear"),
        1,
        "{:#?}",
        window.all()
    );
}

#[test]
fn a_native_replay_whose_write_fails_is_written_again_at_the_next_frame() {
    // The frame that clears the screen and gives back what was kept could not
    // be written. Nothing of it reached the terminal, so it is still owed: the
    // next frame clears and writes everything kept, rather than drawing the
    // region alone over whatever the failed write left.
    let window = Window::new(40, 6);
    let mut render = native(&window);
    let said: Vec<String> = (0..8)
        .map(|at| format!("said {at:02} in words that fold at twenty"))
        .collect();
    finished(&mut render, &said);

    window.resize(20, 6);
    render.resized().unwrap();
    window.take();
    window.refuses_writes(1);
    assert!(
        render.settled(Instant::now() + SETTLE).is_err(),
        "the replay was written: {:?}",
        window.take()
    );

    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    let after = window.take();
    assert!(
        after.contains("\x1b[2J\x1b[3J"),
        "the replay was not written again: {after:?}"
    );
    for at in 0..8 {
        assert_eq!(
            window.rows_saying(&format!("said {at:02} in")),
            1,
            "said {at:02}: {:#?}",
            window.all()
        );
    }
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
}

#[test]
fn a_native_narrowing_keeps_the_finished_rows_that_still_fit_on_screen() {
    // Counted as a terminal that rewraps would count it, and replayed into
    // one that does: the region grows a row, the finished row at the top goes
    // into the scrollback, and every other finished row stays on screen once,
    // above a region drawn once at the new width.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    stands(&mut render);
    for at in 0..6 {
        render.commit(&format!("said {at:02}")).unwrap();
    }
    render.seal().unwrap();
    let thinking = [row("* thinking about the narrower window")];
    render.under(&thinking, None, Palette::plain()).unwrap();
    assert_eq!(
        window.screen().iter().filter(|row| !row.is_empty()).count(),
        10,
        "the window was not full before it narrowed: {:#?}",
        window.screen()
    );
    window.take();

    window.resize(20, 10);
    render.resized().unwrap();
    stands(&mut render);
    render.under(&thinking, None, Palette::plain()).unwrap();

    let after = window.take();
    assert!(
        !after.contains("said"),
        "a resize wrote finished rows again: {after:?}"
    );
    assert_eq!(window.scrollback(), ["said 00"], "{:#?}", window.all());
    let screen = window.screen();
    assert_eq!(
        screen.get(..5),
        Some(&["said 01", "said 02", "said 03", "said 04", "said 05"].map(String::from)[..]),
        "a finished row that still fitted was erased or moved: {screen:#?}"
    );
    for at in 0..6 {
        assert_eq!(
            window.rows_saying(&format!("said {at:02}")),
            1,
            "said {at:02}: {:#?}",
            window.all()
        );
    }
    assert_eq!(window.rows_saying("* thinking"), 1, "{:#?}", window.all());
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
}

#[test]
fn a_native_frame_drawn_before_the_resize_is_reported_takes_the_new_width() {
    // The kernel narrows the window before the press reporting it is read,
    // and an answer still arriving draws frames in between. Replayed into a
    // terminal that wraps what is written past its edge: a frame drawn at the
    // old width wraps, the next rewinds over the rows it counted rather than
    // the rows the terminal made of them, and what it did not reach stays
    // above the region as a second copy.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    render
        .stream("alfa bravo charlie delta echo foxtrot golf ")
        .unwrap();
    window.take();

    window.resize(20, 10);
    render.stream("hotel ").unwrap();
    render.stream("india ").unwrap();
    render.resized().unwrap();
    stands(&mut render);

    let after = window.take();
    assert!(
        !after.contains("> asked"),
        "a frame wrote a finished row again: {after:?}"
    );
    for word in ["alfa", "delta", "golf", "hotel", "india"] {
        assert_eq!(window.rows_saying(word), 1, "{word}: {:#?}", window.all());
    }
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
    assert!(
        window.all().iter().all(|row| row.chars().count() <= 20),
        "a row was written wider than the window: {:#?}",
        window.all()
    );
}

#[test]
fn a_native_frame_with_no_resize_since_the_last_asks_the_window_nothing() {
    // Told of a resize by the operating system, a frame has no reason to ask
    // the window its size, and an answer streaming at speed draws many.
    let window = Window::new(40, 10);
    let mut render = native(&window);
    render.watches_size(ResizeFlag::default());

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    let before = window.asked();
    for word in ["alfa ", "bravo ", "charlie ", "delta "] {
        render.stream(word).unwrap();
    }

    assert_eq!(window.asked(), before, "a frame asked the size unprompted");
}

#[test]
fn a_native_frame_after_a_resize_is_told_of_asks_once_and_takes_the_new_width() {
    // The same narrowing as the frame drawn before the resize is reported,
    // told by the flag rather than found by asking: the first frame after it
    // takes the new width, and the frames after that ask nothing again.
    let window = Window::new(40, 10);
    let mut render = native(&window);
    let resizes = ResizeFlag::default();
    render.watches_size(resizes.clone());

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    render
        .stream("alfa bravo charlie delta echo foxtrot golf ")
        .unwrap();
    window.take();

    window.resize(20, 10);
    resizes.raise();
    render.stream("hotel ").unwrap();
    let asked = window.asked();
    render.stream("india ").unwrap();
    stands(&mut render);

    assert_eq!(
        window.asked(),
        asked,
        "a frame asked again after the resize"
    );
    let after = window.take();
    assert!(
        !after.contains("> asked"),
        "a frame wrote a finished row again: {after:?}"
    );
    for word in ["alfa", "delta", "golf", "hotel", "india"] {
        assert_eq!(window.rows_saying(word), 1, "{word}: {:#?}", window.all());
    }
    assert!(
        window.all().iter().all(|row| row.chars().count() <= 20),
        "a row was written wider than the window: {:#?}",
        window.all()
    );
}

#[test]
fn a_native_frame_told_of_a_resize_whose_size_query_fails_leaves_the_next_frame_to_ask() {
    // The flag is lowered by the frame that takes it, and the query that frame
    // makes fails. The resize is still owed: the next frame asks once more and
    // takes the new width, rather than drawing at the old one until another
    // signal or a press. One that fails again is not asked about frame after
    // frame, and a signal that arrives while one is owed is owed its own.
    let window = Window::new(40, 10);
    let mut render = native(&window);
    let resizes = ResizeFlag::default();
    render.watches_size(resizes.clone());

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    render.stream("alfa ").unwrap();

    window.resize(20, 10);
    resizes.raise();
    window.loses_size(1);
    render.stream("bravo ").unwrap();
    assert_eq!(render.columns(), 40, "the failed query was taken as a size");
    render.stream("charlie ").unwrap();
    assert_eq!(
        render.columns(),
        20,
        "the frame after the failed query kept the old width"
    );

    window.resize(30, 10);
    resizes.raise();
    window.loses_size(2);
    let asked = window.asked();
    render.stream("delta ").unwrap();
    render.stream("echo ").unwrap();
    assert_eq!(window.asked(), asked + 2, "{:#?}", window.all());
    render.stream("foxtrot ").unwrap();
    render.stream("golf ").unwrap();
    assert_eq!(
        window.asked(),
        asked + 2,
        "a query that failed twice was asked again unprompted"
    );
    assert_eq!(render.columns(), 20);

    // A signal that arrives while a resize is still owed is owed a query of
    // its own: when the frame that asks for both fails, the next one asks.
    window.resize(25, 10);
    resizes.raise();
    window.loses_size(1);
    render.stream("hotel ").unwrap();
    window.resize(35, 10);
    resizes.raise();
    window.loses_size(1);
    render.stream("india ").unwrap();
    assert_eq!(render.columns(), 20, "a failed query was taken as a size");
    render.stream("juliett ").unwrap();
    assert_eq!(
        render.columns(),
        35,
        "a resize told of while another was owed was dropped when its query failed"
    );
}

#[test]
fn a_native_window_narrowed_twice_and_widened_keeps_every_finished_row_once() {
    // Three resizes in one answer, each taken while it is still arriving: the
    // window narrowed, narrowed again, and widened back to where it began.
    // Each is a step a reader can take, and a rewind counted wrong at any of
    // them leaves a row twice or takes one away, which only the whole
    // sequence shows.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    for (words, columns) in [
        ("alfa bravo charlie delta ", 30),
        ("echo foxtrot golf hotel ", 20),
        ("india juliett kilo lima ", 40),
    ] {
        render.stream(words).unwrap();
        window.resize(columns, 10);
        render.resized().unwrap();
        stands(&mut render);
    }

    for word in ["> asked", "alfa", "echo", "india", "lima"] {
        assert_eq!(window.rows_saying(word), 1, "{word}: {:#?}", window.all());
    }
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
}

#[test]
fn a_native_window_shortened_and_narrowed_in_one_step_keeps_every_finished_row_once() {
    // A corner dragged on the diagonal: the window loses rows and columns in
    // the same resize, with the region still short enough to fit what is
    // left of it.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    stands(&mut render);
    for at in 0..6 {
        render.commit(&format!("said {at:02}")).unwrap();
    }
    render.seal().unwrap();
    window.take();

    window.resize(30, 7);
    render.resized().unwrap();
    stands(&mut render);

    for at in 0..6 {
        assert_eq!(
            window.rows_saying(&format!("said {at:02}")),
            1,
            "said {at:02}: {:#?}",
            window.all()
        );
    }
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
    assert_eq!(window.screen().len(), 7);
}

#[test]
fn a_native_frame_whose_size_query_fails_is_drawn_at_the_size_already_known() {
    // A size query that fails says nothing about the window: it is neither a
    // resize to the fallback size nor a reason to hold the frame back. The
    // frame goes out for the window as it was last known, whether the query
    // fails once, between the frame asking and the resize asking again, or
    // for good.
    let window = Window::new(100, 10);
    let mut render = native(&window);

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    render.stream("alfa bravo charlie delta ").unwrap();
    window.take();

    window.loses_size(1);
    render.stream("echo ").unwrap();
    assert!(
        window.take().contains("echo"),
        "the frame was held back: {:#?}",
        window.all()
    );

    window.loses_size(usize::MAX);
    render
        .stream("foxtrot golf hotel india juliett kilo lima mike november oscar")
        .unwrap();
    stands(&mut render);

    let said: Vec<String> = window
        .all()
        .into_iter()
        .filter(|row| row.contains("alfa") || row.contains("oscar"))
        .collect();
    assert_eq!(
        said.len(),
        1,
        "the row was folded for a window of eighty: {:#?}",
        window.all()
    );
    assert!(
        said.iter().all(|row| row.chars().count() > 80),
        "the row was folded for a window of eighty: {:#?}",
        window.all()
    );
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
}

#[test]
fn a_native_resize_inside_a_frame_keeps_the_box_and_the_turn_on_screen() {
    // A frame finds the window narrowed while an answer streams. What stands
    // at the foot was laid out for the old width, and the caller lays it out
    // again only when the press reporting the resize is read; until then the
    // frame draws it cut at the new edge rather than drawing without it.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    render.stream("alfa bravo ").unwrap();

    window.resize(30, 10);
    render.stream("charlie ").unwrap();

    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());
    assert_eq!(window.rows_saying("| > typed"), 1, "{:#?}", window.all());
    assert_eq!(window.rows_saying("* thinking"), 1, "{:#?}", window.all());
    assert_eq!(window.rows_saying("charlie"), 1, "{:#?}", window.all());
}

#[test]
fn a_standing_row_handed_in_before_a_resize_is_written_no_wider_than_the_window() {
    // Laid out at forty columns and still standing when the window narrows to
    // twenty: the frame writes it cut at twenty, so the terminal has nothing
    // to wrap onto a row the region did not count.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    render
        .live(
            &[row("a box row that is thirty-four wide")],
            Caret::default(),
            Palette::plain(),
        )
        .unwrap();
    render.commit("> asked").unwrap();
    render.seal().unwrap();

    window.resize(20, 10);
    render.stream("alfa ").unwrap();

    assert_eq!(
        window.rows_saying("a box row that is th"),
        1,
        "{:#?}",
        window.all()
    );
    assert!(
        window.all().iter().all(|row| row.chars().count() <= 20),
        "a row was written wider than the window: {:#?}",
        window.all()
    );
}

#[test]
fn a_resize_press_whose_size_query_fails_keeps_the_size_already_known() {
    // The press says the window changed and the query that should say how
    // fails. That is no news of a size, as it is no news on the frame's path:
    // the session is not folded again for a window of eighty, and the region
    // is not rewound as though it were eighty wide. Every query fails while
    // it is asked, so a second one cannot answer for the first.
    let window = Window::new(100, 10);
    let mut render = native(&window);

    stands(&mut render);
    render.commit("> asked").unwrap();
    render.seal().unwrap();
    render
        .stream(
            "alfa bravo charlie delta echo foxtrot golf hotel india juliett kilo lima mike oscar",
        )
        .unwrap();

    window.loses_size(usize::MAX);
    render.resized().unwrap();
    stands(&mut render);

    assert_eq!(render.columns(), 100);
    let said: Vec<String> = window
        .all()
        .into_iter()
        .filter(|row| row.contains("alfa") || row.contains("oscar"))
        .collect();
    assert_eq!(
        said.len(),
        1,
        "the row was folded for a window of eighty: {:#?}",
        window.all()
    );
    assert_eq!(window.rows_saying("+--box--+"), 1, "{:#?}", window.all());

    window.loses_size(0);
    let mut full = Renderer::drawing(window.clone(), ScreenMode::Fullscreen);
    window.loses_size(usize::MAX);
    full.resized().unwrap();
    assert_eq!(full.columns(), 100);
}

/// A palette that writes every hue it has, so a row's colours are on the
/// record as well as its words.
fn colourful() -> Palette {
    Palette::resolve(true, crate::color::Theme::Dark, None, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

/// Three turns, as a session puts them to its renderer: the line asked, a
/// blank before the answer, the answer streamed in pieces with the session
/// waiting for a key between them, the turn settled, and the box back under
/// it while the session waits for the next line.
fn three_turns(render: &mut Renderer<Window>) {
    let turns: [(&str, &[&str]); 3] = [
        (
            "where is the renderer?",
            &[
                "The renderer is in **crucible-tui**; the session drives it ",
                "from `converse`.",
            ],
        ),
        (
            "what does native mode keep?",
            &[
                "A finished line goes out once.\n\n",
                "Only the part still changing is drawn again.",
            ],
        ),
        ("and scrolling?", &["That is your terminal's own."]),
    ];
    render.wears(colourful());
    render
        .opens(Box::new(|_, _| vec![row("crucible"), row("the opening")]))
        .unwrap();
    stands(render);
    render.seal().unwrap();
    for (asked, pieces) in turns {
        render.follows().unwrap();
        render.settle().unwrap();
        render.apart().unwrap();
        render.landmark();
        render
            .responsive(
                asked.len(),
                Box::new(move |_| {
                    vec![
                        Row::new()
                            .then(Slot::Accent, "› ")
                            .then(Slot::Strong, asked),
                    ]
                }),
            )
            .unwrap();
        render.apart().unwrap();
        for piece in pieces {
            render.stream(piece).unwrap();
            render
                .under(&[row("* thinking")], None, Palette::plain())
                .unwrap();
            render.seal().unwrap();
        }
        render.settle().unwrap();
        render.under(&[], None, Palette::plain()).unwrap();
        stands(render);
        render.seal().unwrap();
    }
}

#[test]
fn what_a_native_session_leaves_above_its_region_is_the_full_screen_transcript() {
    // The two modes are one record drawn two ways: whatever the full screen
    // would show as the transcript, row for row and colour for colour, is what
    // native mode has handed the terminal by the time the session waits for a
    // key -- blanks between blocks included.
    let fullscreen = Window::new(40, 10);
    let mut whole = Renderer::drawing(fullscreen.clone(), ScreenMode::Fullscreen);
    three_turns(&mut whole);
    let mut painted = String::new();
    let mut rows = 0;
    for line in 0..whole.record.lines() {
        for row in whole.record.folded(line) {
            row.clipped(40).paint_into(&colourful(), &mut painted);
            painted.push_str("\r\n");
            rows += 1;
        }
    }
    let mut transcript = Emulator::new(40, rows + 1);
    transcript.feed(&painted);
    let transcript = transcript.coloured(rows);

    let window = Window::new(40, 10);
    let mut render = native(&window);
    three_turns(&mut render);
    let parked = render.native.as_ref().map_or(0, |native| native.parked);
    let seen = window.0.borrow();
    let above = seen.emulator.scrollback.len() + seen.emulator.at.0 - parked;
    let held = seen.emulator.coloured(above);

    assert_eq!(held, transcript);
}

/// What a native session leaves on the reader's screen once its renderer is
/// gone: everything said, nothing that stood, and the cursor on a row of its
/// own.
fn left_clean(window: &Window) {
    assert_eq!(
        window.rows_saying("said and finished"),
        1,
        "{:#?}",
        window.all()
    );
    assert_eq!(
        window.rows_saying("not yet sealed"),
        1,
        "{:#?}",
        window.all()
    );
    assert_eq!(window.rows_saying("box"), 0, "{:#?}", window.all());
    assert_eq!(window.rows_saying("* thinking"), 0, "{:#?}", window.all());
    let (row, column) = window.caret();
    assert_eq!(column, 0, "the shell would start mid-row");
    assert!(
        window.screen().iter().skip(row).all(String::is_empty),
        "the shell would write over what is under the cursor: {:#?}",
        window.screen()
    );
    let written = window.written();
    assert!(
        written.rfind("\x1b[?25h") >= written.rfind("\x1b[?25l"),
        "the cursor was left hidden"
    );
}

fn session(window: &Window) -> Renderer<Window> {
    let mut render = native(window);
    render.commit("said and finished").unwrap();
    render.seal().unwrap();
    stands(&mut render);
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    render.commit("not yet sealed").unwrap();
    render
}

#[test]
fn a_native_renderer_closes_its_live_region_when_dropped() {
    let window = Window::new(40, 10);
    let render = session(&window);

    drop(render);

    left_clean(&window);
}

#[test]
fn a_native_renderer_closes_its_live_region_when_a_panic_unwinds_past_it() {
    let window = Window::new(40, 10);
    let held = window.clone();

    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _render = session(&held);
        panic!("a turn failed with the live region on screen");
    }));

    assert!(unwound.is_err(), "nothing unwound");
    left_clean(&window);
}

#[test]
fn a_native_region_closed_before_the_renderer_goes_is_left_alone_after() {
    // A session closes its region while the guards it took are still held,
    // so whatever is written on the way out lands on a clean row below it
    // rather than inside a region the drop would then rewind into.
    let window = Window::new(40, 10);
    let mut render = session(&window);

    render.closes().unwrap();
    left_clean(&window);
    let closed = window.take();
    render.commit("too late to draw").unwrap();
    drop(render);

    assert!(!closed.is_empty(), "closing wrote nothing");
    assert_eq!(window.written(), "", "the renderer drew after closing");
}

#[test]
fn a_native_region_turns_autowrap_off_once_and_back_on_when_it_closes() {
    // Off in the first frame and in no later one, and on again as the last
    // thing the region writes, so the shell gets its wrapping lines back.
    let window = Window::new(40, 10);
    let mut render = native(&window);

    stands(&mut render);
    let first = window.take();
    render.commit("> hello").unwrap();
    render.seal().unwrap();
    stands(&mut render);
    let later = window.take();
    drop(render);
    let closing = window.take();

    assert_eq!(first.matches("\x1b[?7l").count(), 1, "{first:?}");
    assert!(!later.contains("\x1b[?7l"), "a later frame: {later:?}");
    assert!(!first.contains("\x1b[?7h") && !later.contains("\x1b[?7h"));
    assert!(closing.ends_with("\x1b[?7h"), "closing: {closing:?}");
}

#[test]
fn a_redirected_native_renderer_writes_plain_text_and_nothing_on_drop() {
    let window = Window::redirected(40, 10);
    let mut render = native(&window);

    render.commit("one line").unwrap();
    render.stream("streamed").unwrap();
    render.settle().unwrap();
    render.present(&[row("a row")]).unwrap();
    stands(&mut render);
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    render.seal().unwrap();
    let before = window.written();
    drop(render);

    assert_eq!(before, "one line\nstreamed\na row\n");
    assert_eq!(window.written(), before, "dropping wrote to a pipe");
}

#[test]
fn native_mouse_presses_do_nothing_and_offer_no_rail_or_scrolling() {
    let window = Window::new(40, 10);
    let mut render = native(&window);
    for at in 0..30 {
        render.commit(&format!("said {at:02}")).unwrap();
    }
    stands(&mut render);
    window.take();

    for press in [
        Pressed::Clicked { row: 1, column: 1 },
        Pressed::Dragged { row: 2, column: 4 },
        Pressed::Released { row: 2, column: 4 },
        Pressed::Hovered { row: 3, column: 3 },
        Pressed::Scrolled { back: true },
    ] {
        assert_eq!(render.took(press.clone()).unwrap(), None, "{press:?}");
    }
    assert!(!render.scrolled(-3).unwrap(), "the transcript scrolled");
    assert!(!render.notched(true).unwrap(), "the wheel scrolled");
    assert_eq!(render.aimed(1), None, "a row of the window was aimed at");

    render.rails(true);
    assert_eq!(
        render.transcript_columns(),
        render.columns(),
        "a column was kept for a rail native mode does not draw"
    );
    assert_eq!(window.take(), "", "a press nobody answers drew something");
}

#[test]
fn a_native_renderer_says_it_is_native_and_a_new_one_is_fullscreen() {
    assert_eq!(native(&Window::new(40, 10)).screen(), ScreenMode::Native);
    assert_eq!(
        Renderer::new(Window::new(40, 10)).screen(),
        ScreenMode::Fullscreen
    );
}

#[test]
fn a_native_panel_gets_half_the_window_rounded_down_and_a_fullscreen_one_the_whole() {
    // What stands in place of the box in native mode is held to half the
    // window, so that what it stands over is still on screen rather than
    // pushed into a scrollback that closing the panel cannot take back. The
    // full screen owns every row and gives the whole window, as before.
    assert_eq!(native(&Window::new(80, 24)).room(), 12);
    assert_eq!(native(&Window::new(80, 23)).room(), 11);
    assert_eq!(native(&Window::new(80, 1)).room(), 0);
    assert_eq!(Renderer::new(Window::new(80, 24)).room(), 24);
}

#[test]
fn a_native_divider_has_one_blank_row_above_it_and_none_below_however_the_transcript_ended() {
    // Emptying a native transcript takes nothing back from the terminal, so
    // what the divider is parted from is whatever went out last: a row of
    // words, or a row of nothing that a block before it already left. Either
    // way one blank row stands above the divider, and the block that follows
    // it asks for none, because the divider is what parts them.
    for (case, ending) in [
        ("words", &["the last answer"][..]),
        ("blank", &["the last answer", ""][..]),
    ] {
        let window = Window::new(40, 10);
        let mut render = native(&window);
        for line in ending {
            render.commit(line).unwrap();
        }
        stands(&mut render);
        render.seal().unwrap();

        render.empties().unwrap();
        render.divides("new session").unwrap();
        render.apart().unwrap();
        render.commit("what follows").unwrap();
        stands(&mut render);
        render.seal().unwrap();

        let all = window.all();
        let at = all
            .iter()
            .position(|row| row.starts_with("── new session ─"))
            .unwrap_or_else(|| panic!("{case}: no divider in {all:#?}"));
        let divider = format!("── new session {}", "─".repeat(40 - 15));
        let around: Vec<&str> = all
            .iter()
            .skip(at.saturating_sub(2))
            .take(4)
            .map(String::as_str)
            .collect();
        assert_eq!(
            around,
            ["the last answer", "", divider.as_str(), "what follows"],
            "{case}: {all:#?}"
        );
    }
}

#[test]
fn a_blank_line_committed_under_a_native_divider_is_not_written() {
    // A command's answer ends with a blank line that parts it from what is
    // said next. When the answer is a divider, the divider already does that,
    // and a blank row under it would part the new session from its own start.
    let window = Window::new(40, 10);
    let mut render = native(&window);
    render.commit("the last answer").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    render.empties().unwrap();
    render.divides("new session").unwrap();
    render.commit("").unwrap();
    stands(&mut render);
    render.seal().unwrap();
    render.apart().unwrap();
    render.commit("what follows").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let all = window.all();
    let divider = format!("── new session {}", "─".repeat(40 - 15));
    let under: Vec<&str> = all
        .iter()
        .skip_while(|row| **row != divider)
        .take(2)
        .map(String::as_str)
        .collect();
    assert_eq!(under, [divider.as_str(), "what follows"], "{all:#?}");
}

#[test]
fn a_native_divider_let_go_in_the_frame_that_draws_it_still_parts_what_follows() {
    // Emptying the transcript takes the box down, so when the divider is drawn
    // the transcript band is the whole window, and the one window with no row
    // for it is one that reports no rows at all: a pty nobody told its size
    // says so, while the display behind it has rows. There every line goes
    // out to the scrollback in the frame that first draws it, the divider
    // included, and the mark that parts what follows from it has to be on it
    // by then, or the block after it is given the blank row the divider was
    // meant to stand in for.
    let window = Window::new(40, 10);
    window.reports(Size {
        columns: 40,
        rows: 0,
    });
    let mut render = native(&window);
    render.commit("> /clear").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    render.empties().unwrap();
    render.divides("new session").unwrap();
    render.apart().unwrap();
    render.commit("what follows").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let all = window.all();
    let divider = format!("── new session {}", "─".repeat(40 - 15));
    let at = all
        .iter()
        .position(|row| row.as_str() == "> /clear")
        .unwrap_or_else(|| panic!("nothing asked: {all:#?}"));
    let after: Vec<&str> = all.iter().skip(at).take(4).map(String::as_str).collect();
    assert_eq!(
        after,
        ["> /clear", "", divider.as_str(), "what follows"],
        "{all:#?}"
    );
}

#[test]
fn a_redirected_native_divider_is_written_plain_once_and_parts_what_follows() {
    // Where output is redirected nothing is framed, so the divider is written
    // through as plain text the moment it is laid, as every other row is, and
    // the mark that spares the block after it a blank row holds there too.
    let window = Window::redirected(40, 10);
    let mut render = native(&window);
    render.commit("> /clear").unwrap();
    render.empties().unwrap();
    render.divides("new session").unwrap();
    render.apart().unwrap();
    render.commit("what follows").unwrap();

    let divider = format!("── new session {}", "─".repeat(40 - 15));
    assert_eq!(
        window.written(),
        format!("> /clear\n\n{divider}\nwhat follows\n")
    );
}

/// Every row of `rows` from the one that reads `asked` through the first that
/// is blank, without that blank.
fn under<'a>(rows: &'a [String], asked: &str) -> Vec<&'a str> {
    rows.iter()
        .skip_while(|row| *row != asked)
        .skip(1)
        .take_while(|row| !row.is_empty())
        .map(String::as_str)
        .collect()
}

#[test]
fn a_native_reply_that_leaves_the_region_before_its_command_ends_goes_out_marked() {
    // A command whose reply is taller than the region: most of it goes out to
    // the scrollback while the command is still writing, and a key wait in the
    // middle seals the rest written so far. Each row is written once, so each
    // has to go out already carrying the mark or the indent under it.
    let window = Window::new(40, 6);
    let mut render = native(&window);
    render.commit("> /usage").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let start = render.lines();
    render.hangs(Glyphs::Unicode);
    for at in 0..10 {
        render.commit(&format!("row {at}")).unwrap();
        stands(&mut render);
    }
    render.seal().unwrap();
    for at in 10..12 {
        render.commit(&format!("row {at}")).unwrap();
    }
    render.subordinate(start, Glyphs::Unicode).unwrap();
    render.commit("").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let all = window.all();
    let mut wanted = vec!["⎿ row 0".to_owned()];
    wanted.extend((1..12).map(|at| format!("  row {at}")));
    assert_eq!(under(&all, "> /usage"), wanted, "{all:#?}");
    for at in 0..12 {
        assert_eq!(
            all.iter()
                .filter(|row| row.ends_with(&format!("row {at}")))
                .count(),
            1,
            "row {at} was not written exactly once: {all:#?}"
        );
    }
}

#[test]
fn a_native_reply_written_after_the_last_key_wait_is_marked_before_it_goes_out() {
    // How `/compact` answers the line that asked: its reply is drawn once the
    // work is over, and the mark is hung before the session next waits for a
    // key, so nothing of it has gone out unmarked.
    let window = Window::new(40, 10);
    let mut render = native(&window);
    render.commit("> /compact").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let start = render.lines();
    render.commit("nothing to compact").unwrap();
    render.subordinate(start, Glyphs::Unicode).unwrap();
    render.commit("").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let all = window.all();
    assert_eq!(
        under(&all, "> /compact"),
        ["⎿ nothing to compact"],
        "{all:#?}"
    );
    assert_eq!(window.rows_saying("nothing to compact"), 1, "{all:#?}");
}

#[test]
fn a_native_mark_waiting_when_the_transcript_empties_is_dropped() {
    // What the full screen does with a reply that emptied the transcript is
    // hang nothing at all, because what the mark would hang from has gone.
    // Here what was written before the emptying stays in the scrollback, and
    // neither it nor the divider nor anything under the divider takes a mark.
    let window = Window::new(40, 10);
    let mut render = native(&window);
    render.commit("> /clear").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let start = render.lines();
    render.hangs(Glyphs::Unicode);
    render.commit("! the session file is gone").unwrap();
    render.empties().unwrap();
    render.divides("new session").unwrap();
    render.subordinate(start, Glyphs::Unicode).unwrap();
    render.commit("").unwrap();
    render.apart().unwrap();
    render.commit("what follows").unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let all = window.all();
    let divider = format!("── new session {}", "─".repeat(40 - 15));
    let at = all
        .iter()
        .position(|row| row.as_str() == "> /clear")
        .unwrap_or_else(|| panic!("nothing asked: {all:#?}"));
    let after: Vec<&str> = all.iter().skip(at).take(5).map(String::as_str).collect();
    assert_eq!(
        after,
        [
            "> /clear",
            "! the session file is gone",
            "",
            divider.as_str(),
            "what follows"
        ],
        "{all:#?}"
    );
}

/// What a native session on `window` keeps of `answer` once it has settled
/// at `columns` by `rows`: every row but the box's and the blank ones.
fn answered_natively(
    window: &Window,
    answer: &str,
    resized: Option<(usize, usize)>,
) -> Vec<String> {
    answered_natively_in(window, Glyphs::Unicode, answer, resized)
}

/// [`answered_natively`], drawing with `glyphs`.
fn answered_natively_in(
    window: &Window,
    glyphs: Glyphs,
    answer: &str,
    resized: Option<(usize, usize)>,
) -> Vec<String> {
    answered_natively_in_pieces(window, glyphs, &[answer], resized)
}

/// [`answered_natively_in`], the answer arriving as `pieces`, one delta each.
fn answered_natively_in_pieces(
    window: &Window,
    glyphs: Glyphs,
    pieces: &[&str],
    resized: Option<(usize, usize)>,
) -> Vec<String> {
    let mut render = native(window);
    render.draws(glyphs);
    render.wears(colourful());
    stands(&mut render);
    for piece in pieces {
        render.stream(piece).unwrap();
    }
    render.settle().unwrap();
    stands(&mut render);
    render.seal().unwrap();
    if let Some((columns, rows)) = resized {
        settles_at(window, &mut render, columns, rows);
    }
    let (boxed, _) = boxed();
    let boxed: Vec<String> = boxed.iter().map(Row::text).collect();
    window
        .all()
        .into_iter()
        .filter(|row| !row.trim().is_empty() && !boxed.contains(row))
        .collect()
}

#[test]
fn a_native_table_narrowed_is_the_table_a_window_that_narrow_draws() {
    let table = "| name | what it does | since |\n|---|---|---|\n\
         | alpha | reads the markdown out of a stream | 0.1 |\n\
         | beta | lays a table against the window | 0.2 |\n";
    let fresh = answered_natively(&Window::new(30, 20), table, None);
    let narrowed = answered_natively(&Window::new(60, 20), table, Some((30, 20)));

    assert_eq!(narrowed, fresh);
}

#[test]
fn a_native_table_written_without_outer_bars_is_drawn_as_a_table() {
    let rows = answered_natively(&Window::new(40, 8), "a | b\n--|--\n1 | 2\n", None);

    assert!(!rows.iter().any(|row| row.contains("--|--")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains('│')), "{rows:#?}");
}

#[test]
fn a_native_frame_drawn_while_a_header_is_in_doubt_leaves_what_drawing_the_whole_answer_leaves() {
    // A frame between two deltas, wherever the answer is cut: inside the
    // header, between it and the delimiter row, or inside that row. What
    // reached the scrollback is what one delta gives, so no header was sent
    // out as prose that the table then repeated.
    let answer = "before\n\nname | what it does\n--|--\nalpha | first\n\nafter\n";
    let fresh = answered_natively(&Window::new(40, 10), answer, None);
    assert!(fresh.iter().any(|row| row.contains('│')), "{fresh:#?}");
    assert!(
        !fresh
            .iter()
            .any(|row| row.contains("name | what") || row.contains("--|--")),
        "{fresh:#?}"
    );

    for cut in 0..=answer.len() {
        let (first, second) = answer.split_at(cut);
        let window = Window::new(40, 10);
        let mut render = native(&window);
        render.wears(colourful());
        stands(&mut render);
        render.stream(first).unwrap();
        stands(&mut render);
        render.stream(second).unwrap();
        render.settle().unwrap();
        stands(&mut render);
        render.seal().unwrap();

        let (boxed, _) = boxed();
        let boxed: Vec<String> = boxed.iter().map(Row::text).collect();
        let rows: Vec<String> = window
            .all()
            .into_iter()
            .filter(|row| !row.trim().is_empty() && !boxed.contains(row))
            .collect();
        assert_eq!(rows, fresh, "cut at {cut}");
    }
}

/// A call that writes under the line an answer left open.
type Writes = fn(&mut Renderer<Window>);

/// What a native screen is left holding when `call` writes under an answer
/// cut at `cut`, then the rest of the answer arrives.
fn written_under_the_cut(cut: &str, call: Writes) -> Vec<String> {
    let window = Window::new(40, 12);
    let mut render = native(&window);
    render.wears(colourful());
    stands(&mut render);
    render.stream(&format!("before\n\n{cut}")).unwrap();
    call(&mut render);
    render.stream("\n--|--\nalpha | 0.1\n\nafter\n").unwrap();
    render.settle().unwrap();
    stands(&mut render);
    render.seal().unwrap();
    let (boxed, _) = boxed();
    let boxed: Vec<String> = boxed.iter().map(Row::text).collect();
    window
        .all()
        .into_iter()
        .filter(|row| !row.trim().is_empty() && !boxed.contains(row))
        .collect()
}

#[test]
fn a_native_call_that_writes_under_a_header_in_doubt_ends_it_as_the_prose_it_was() {
    // Presenting rows, committing a line and writing a prompt each end the
    // line an answer left open, and a header that is still only a guess is
    // the prose it looked like when they do: said once in the scrollback,
    // with the same rows for the same words that could not have been a header.
    let calls: [(&str, Writes); 3] = [
        ("present", |render| {
            render.present(&[row("component")]).unwrap();
        }),
        ("commit", |render| render.commit("note").unwrap()),
        ("prompt", |render| {
            render.prompt(Slot::Plain, "note").unwrap();
        }),
    ];
    for (name, call) in calls {
        for cut in [
            "name | what it does\n",
            "name | what",
            "name | what it does\n--",
        ] {
            let doubtful = written_under_the_cut(cut, call);
            let plain: Vec<String> = written_under_the_cut(&cut.replace('|', "-"), call)
                .iter()
                .map(|row| row.replace(" - ", " | "))
                .collect();

            assert_eq!(
                doubtful
                    .iter()
                    .filter(|row| row.contains("name | what"))
                    .count(),
                1,
                "{name} {cut:?}: {doubtful:#?}"
            );
            assert_eq!(doubtful, plain, "{name} {cut:?}");
        }
    }
}

#[test]
fn a_native_responsive_block_laid_under_a_header_in_doubt_ends_it_as_the_prose_it_was() {
    // On a native screen a block that keeps what lays it ends the line an
    // answer left open, and a header still in doubt goes to the scrollback as
    // the prose it looked like, said once, never as a table made of it later.
    let call: Writes = |render| {
        render
            .responsive(9, Box::new(|_| vec![row("component")]))
            .unwrap();
    };
    for cut in [
        "name | what it does\n",
        "name | what",
        "name | what it does\n--",
    ] {
        let doubtful = written_under_the_cut(cut, call);
        let plain: Vec<String> = written_under_the_cut(&cut.replace('|', "-"), call)
            .iter()
            .map(|row| row.replace(" - ", " | "))
            .collect();

        assert_eq!(
            doubtful
                .iter()
                .filter(|row| row.contains("name | what"))
                .count(),
            1,
            "{cut:?}: {doubtful:#?}"
        );
        assert!(
            !doubtful.iter().any(|row| row.contains('│')),
            "{cut:?}: {doubtful:#?}"
        );
        assert_eq!(doubtful, plain, "{cut:?}");
    }
}

#[test]
fn a_native_block_charged_as_laid_under_a_header_in_doubt_ends_it_as_the_prose_it_was() {
    // As the responsive block, for the one charged as its rows: the header
    // still in doubt reaches the scrollback once, as the prose it looked like.
    let call: Writes = |render| {
        render
            .responsive_as_laid(Box::new(|_| vec![row("component")]))
            .unwrap();
    };
    for cut in [
        "name | what it does\n",
        "name | what",
        "name | what it does\n--",
    ] {
        let doubtful = written_under_the_cut(cut, call);
        let plain: Vec<String> = written_under_the_cut(&cut.replace('|', "-"), call)
            .iter()
            .map(|row| row.replace(" - ", " | "))
            .collect();

        assert_eq!(
            doubtful
                .iter()
                .filter(|row| row.contains("name | what"))
                .count(),
            1,
            "{cut:?}: {doubtful:#?}"
        );
        assert!(
            !doubtful.iter().any(|row| row.contains('│')),
            "{cut:?}: {doubtful:#?}"
        );
        assert_eq!(doubtful, plain, "{cut:?}");
    }
}

#[test]
fn a_native_opening_laid_under_a_header_in_doubt_ends_it_as_the_prose_it_was() {
    // The opening ends the line an answer left open the same way on a native
    // screen: the header still in doubt reaches the scrollback once, as prose.
    let call: Writes = |render| {
        render
            .opens(Box::new(|_, _| vec![row("component")]))
            .unwrap();
    };
    for cut in [
        "name | what it does\n",
        "name | what",
        "name | what it does\n--",
    ] {
        let doubtful = written_under_the_cut(cut, call);
        let plain: Vec<String> = written_under_the_cut(&cut.replace('|', "-"), call)
            .iter()
            .map(|row| row.replace(" - ", " | "))
            .collect();

        assert_eq!(
            doubtful
                .iter()
                .filter(|row| row.contains("name | what"))
                .count(),
            1,
            "{cut:?}: {doubtful:#?}"
        );
        assert!(
            !doubtful.iter().any(|row| row.contains('│')),
            "{cut:?}: {doubtful:#?}"
        );
        assert_eq!(doubtful, plain, "{cut:?}");
    }
}

#[test]
fn a_native_divider_laid_under_a_header_in_doubt_ends_it_as_the_prose_it_was() {
    // A divider parts what is above it in the scrollback from what follows,
    // so a header still in doubt over it goes there once, as the prose it
    // looked like, and the delimiter row that comes later makes no table of it.
    let call: Writes = |render| render.divides("new session").unwrap();
    for cut in [
        "name | what it does\n",
        "name | what",
        "name | what it does\n--",
    ] {
        let doubtful = written_under_the_cut(cut, call);
        let plain: Vec<String> = written_under_the_cut(&cut.replace('|', "-"), call)
            .iter()
            .map(|row| row.replace(" - ", " | "))
            .collect();

        assert_eq!(
            doubtful
                .iter()
                .filter(|row| row.contains("name | what"))
                .count(),
            1,
            "{cut:?}: {doubtful:#?}"
        );
        assert!(
            !doubtful.iter().any(|row| row.contains('│')),
            "{cut:?}: {doubtful:#?}"
        );
        assert_eq!(doubtful, plain, "{cut:?}");
    }
}

#[test]
fn a_native_transcript_emptied_under_a_header_in_doubt_ends_it_as_the_prose_it_was() {
    // Emptying a native transcript writes what it held out to the scrollback,
    // so a header still in doubt goes there once, as the prose it looked like,
    // and the delimiter row that comes later makes no table of it.
    let call: Writes = |render| render.empties().unwrap();
    for cut in [
        "name | what it does\n",
        "name | what",
        "name | what it does\n--",
    ] {
        let doubtful = written_under_the_cut(cut, call);
        let plain: Vec<String> = written_under_the_cut(&cut.replace('|', "-"), call)
            .iter()
            .map(|row| row.replace(" - ", " | "))
            .collect();

        assert_eq!(
            doubtful
                .iter()
                .filter(|row| row.contains("name | what"))
                .count(),
            1,
            "{cut:?}: {doubtful:#?}"
        );
        assert!(
            !doubtful.iter().any(|row| row.contains('│')),
            "{cut:?}: {doubtful:#?}"
        );
        assert_eq!(doubtful, plain, "{cut:?}");
    }
}

#[test]
fn a_native_answer_hung_while_a_header_is_in_doubt_ends_it_as_the_prose_it_was() {
    // Hanging what was written under a result mark ends the line an answer
    // left open, so on a native screen a header still in doubt reaches the
    // scrollback once, marked, as the prose it looked like.
    let call: Writes = |render| {
        let last = render.lines().saturating_sub(1);
        render.subordinate(last, Glyphs::Unicode).unwrap();
    };
    for cut in [
        "name | what it does\n",
        "name | what",
        "name | what it does\n--",
    ] {
        let doubtful = written_under_the_cut(cut, call);
        let plain: Vec<String> = written_under_the_cut(&cut.replace('|', "-"), call)
            .iter()
            .map(|row| row.replace(" - ", " | "))
            .collect();

        assert_eq!(
            doubtful
                .iter()
                .filter(|row| row.contains("name | what"))
                .count(),
            1,
            "{cut:?}: {doubtful:#?}"
        );
        assert!(
            !doubtful.iter().any(|row| row.contains('│')),
            "{cut:?}: {doubtful:#?}"
        );
        assert_eq!(doubtful, plain, "{cut:?}");
    }
}

#[test]
fn a_native_line_still_arriving_is_on_screen_whatever_it_could_turn_out_to_be() {
    let window = Window::new(40, 8);
    let mut render = native(&window);
    render.wears(colourful());
    stands(&mut render);
    render.stream("It is started. That is all.").unwrap();
    render.seal().unwrap();

    let rows = window.all();
    assert!(
        rows.iter().any(|row| row.contains("That is all.")),
        "{rows:#?}"
    );
}

#[test]
fn a_native_header_stalled_part_way_never_reaches_the_scrollback_as_prose() {
    // A window this short pushes every finished line out into the terminal's
    // own buffer, which cannot take one back: so a header that turned out to
    // be a table's must never have been finished while it was in doubt.
    let window = Window::new(40, 6);
    let mut render = native(&window);
    render.wears(colourful());
    stands(&mut render);
    let piece = |render: &mut Renderer<Window>, text: &str| {
        render.stream(text).unwrap();
        render
            .under(&[row("* thinking")], None, Palette::plain())
            .unwrap();
        render.seal().unwrap();
        window.all()
    };
    let has = |rows: &[String], needle: &str| rows.iter().any(|row| row.contains(needle));

    let rows = piece(&mut render, "one\ntwo\nthree\nfour\n\nname | wha");
    assert!(has(&rows, "name | wha"), "{rows:#?}");

    let rows = piece(&mut render, "t it does\n");
    assert!(has(&rows, "name | what it does"), "{rows:#?}");

    let rows = piece(&mut render, "--|--\n");
    assert!(!has(&rows, "name | what"), "{rows:#?}");
    assert!(!has(&rows, "--|--"), "{rows:#?}");

    let rows = piece(&mut render, "alpha | first\nbeta | second\n\n");
    assert!(has(&rows, "alpha"), "{rows:#?}");

    render.settle().unwrap();
    render.under(&[], None, Palette::plain()).unwrap();
    stands(&mut render);
    render.seal().unwrap();
    let rows = window.all();
    assert!(!has(&rows, "name | what"), "{rows:#?}");
    assert!(!has(&rows, "--|--"), "{rows:#?}");
    assert!(
        rows.iter()
            .any(|row| row.contains("name") && row.contains('│')),
        "{rows:#?}"
    );
}

#[test]
fn a_native_table_written_without_outer_bars_narrowed_is_the_table_a_window_that_narrow_draws() {
    let table = "name | what it does | since\n--|--|--\n\
         alpha | reads the markdown out of a stream | 0.1\n\
         beta | lays a table against the window | 0.2\n";
    let fresh = answered_natively(&Window::new(30, 20), table, None);
    assert!(
        fresh.iter().filter(|row| row.contains('│')).count() >= 3,
        "{fresh:#?}"
    );
    let narrowed = answered_natively(&Window::new(60, 20), table, Some((30, 20)));

    assert_eq!(narrowed, fresh);
}

#[test]
fn a_native_table_written_without_outer_bars_draws_its_structure_in_the_glyphs_worn() {
    let rows = answered_natively_in(
        &Window::new(40, 8),
        Glyphs::Ascii,
        "a | b\n--|--\n1 | 2\n",
        None,
    );

    let rows: Vec<&str> = rows.iter().map(|row| row.trim_end()).collect();
    assert_eq!(rows, ["a | b", "--+--", "1 | 2"], "{rows:#?}");
}

#[test]
fn a_native_answer_cut_anywhere_draws_the_table_written_without_outer_bars_the_whole_answer_draws()
{
    let answer = "before\n\na | b\n--|--\n1 | 2\n\nafter\n";
    let window = Window::new(40, 12);
    let whole = answered_natively(&window, answer, None);
    assert!(whole.iter().any(|row| row.contains('│')), "{whole:#?}");

    for cut in 1..answer.len() {
        let rows = answered_natively_in_pieces(
            &Window::new(40, 12),
            Glyphs::Unicode,
            &[&answer[..cut], &answer[cut..]],
            None,
        );
        assert_eq!(rows, whole, "cut at {cut}");
    }
}

#[test]
fn a_native_rule_narrowed_is_still_one_row() {
    let rows = answered_natively(
        &Window::new(40, 10),
        "above\n\n---\n\nbelow\n",
        Some((25, 10)),
    );

    let ruled: Vec<&String> = rows.iter().filter(|row| row.contains('─')).collect();
    assert_eq!(
        ruled
            .iter()
            .map(|row| crate::width::columns(row))
            .collect::<Vec<_>>(),
        [25],
        "{rows:#?}"
    );
}

/// What a native session on a window `columns` wide keeps of `answer`, which
/// arrives as `pieces`, once it has settled.
fn kept_natively(columns: usize, glyphs: Glyphs, pieces: &[&str]) -> Vec<String> {
    answered_natively_in_pieces(&Window::new(columns, 30), glyphs, pieces, None)
        .into_iter()
        .map(|row| row.trim_end().to_owned())
        .collect()
}

#[test]
fn a_native_setext_underline_is_read_rather_than_drawn() {
    let rows = answered_natively(
        &Window::new(40, 12),
        "Title\n=====\n\nSub\n---\n\nprose\n",
        None,
    );

    assert!(!rows.iter().any(|row| row.contains("=====")), "{rows:#?}");
    assert!(!rows.iter().any(|row| row.contains("---")), "{rows:#?}");
    let is_rule = |row: &str| {
        let row = row.trim();
        !row.is_empty() && row.chars().all(|c| matches!(c, '─' | '━'))
    };
    let above = std::iter::once("").chain(rows.iter().map(String::as_str));
    for (over, row) in above.zip(rows.iter()) {
        if is_rule(row) {
            assert!(
                row.trim().chars().count() <= over.trim().chars().count(),
                "a rule under {over:?} is wider than its words: {rows:#?}"
            );
        }
    }
    assert!(rows.iter().any(|row| row.contains("Title")));
    assert!(rows.iter().any(|row| row.contains("Sub")));
}

#[test]
fn a_native_setext_heading_wears_a_rule_as_wide_as_its_words_in_both_sets() {
    let answer = "Title\n=====\n\nSub\n---\n\nprose\n";
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &[answer]),
        ["Title", "━━━━━", "Sub", "───", "prose"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Ascii, &[answer]),
        ["Title", "=====", "Sub", "---", "prose"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Ascii, &["Title\n==========\n"]),
        ["Title", "====="]
    );
}

#[test]
fn a_native_title_and_its_underline_in_two_deltas_are_one_heading() {
    for pieces in [
        ["Title\n", "=====\n"].as_slice(),
        ["Ti", "tle", "\n", "=", "====", "\n"].as_slice(),
        ["Title\n=====", "\n"].as_slice(),
        ["Title\n", "=====\n\nafter\n"].as_slice(),
    ] {
        let rows = kept_natively(40, Glyphs::Unicode, pieces);
        assert_eq!(
            rows.iter().filter(|row| row.contains("Title")).count(),
            1,
            "{pieces:?}: {rows:#?}"
        );
        assert_eq!(
            rows.iter().take(2).map(String::as_str).collect::<Vec<_>>(),
            ["Title", "━━━━━"],
            "{pieces:?}: {rows:#?}"
        );
    }
}

#[test]
fn a_native_dashed_row_under_something_that_is_not_a_paragraph_stays_a_full_width_rule() {
    let rule = "─".repeat(40);
    for answer in [
        "---\n",
        "above\n\n---\n",
        "- item\n---\n",
        "> quote\n---\n",
        "# Foo\n---\n",
        "```\ncode\n```\n---\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n---\n",
    ] {
        let rows = kept_natively(40, Glyphs::Unicode, &[answer]);
        assert_eq!(rows.last(), Some(&rule), "{answer:?}: {rows:#?}");
    }
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["one cell\n---\n"]),
        ["one cell", "────────"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["a | b\n--|--\n1 | 2\n"]).len(),
        3
    );
}

#[test]
fn a_native_heading_that_wraps_wears_a_rule_as_wide_as_its_last_row() {
    let rows = kept_natively(16, Glyphs::Unicode, &["A title long enough to wrap\n=\n"]);
    assert!(rows.len() > 2, "{rows:#?}");
    let [.., words, rule] = rows.as_slice() else {
        panic!("{rows:#?}");
    };
    assert_eq!(*rule, "━".repeat(crate::width::columns(words)), "{rows:#?}");
}

#[test]
fn a_native_heading_is_laid_out_again_at_a_new_width_and_in_a_new_glyph_set() {
    let answer = "A title long enough to wrap\n=====\n\nAnd a second one to wrap\n---\n";
    let narrowed = answered_natively(&Window::new(40, 30), answer, Some((16, 30)));
    let narrowed: Vec<String> = narrowed
        .iter()
        .map(|row| row.trim_end().to_owned())
        .collect();
    assert!(
        narrowed.iter().any(|row| row.contains('━')),
        "{narrowed:#?}"
    );
    assert_eq!(narrowed, kept_natively(16, Glyphs::Unicode, &[answer]));

    // The set changes after the answer is kept; the next replay is in it.
    let window = Window::new(40, 30);
    let mut render = native(&window);
    render.wears(colourful());
    stands(&mut render);
    render.stream(answer).unwrap();
    render.settle().unwrap();
    stands(&mut render);
    render.seal().unwrap();
    render.draws(Glyphs::Ascii);
    settles_at(&window, &mut render, 16, 30);
    let (boxed, _) = boxed();
    let boxed: Vec<String> = boxed.iter().map(Row::text).collect();
    let replayed: Vec<String> = window
        .all()
        .into_iter()
        .filter(|row| !row.trim().is_empty() && !boxed.contains(row))
        .map(|row| row.trim_end().to_owned())
        .collect();
    assert!(
        replayed.iter().any(|row| row.starts_with('=')),
        "{replayed:#?}"
    );
    assert_eq!(replayed, kept_natively(16, Glyphs::Ascii, &[answer]));
}

#[test]
fn a_native_underline_at_the_end_of_the_message_or_with_trailing_blanks_counts() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["Title\n====="]),
        ["Title", "━━━━━"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["Sub\n---"]),
        ["Sub", "───"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["Title\n===\t\n"]),
        ["Title", "━━━━━"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["Sub\n---\t\n"]),
        ["Sub", "───"]
    );
}

#[test]
fn a_native_underline_and_a_title_of_the_most_the_reader_holds_are_headings_and_one_more_is_not() {
    assert_eq!(
        kept_natively(
            200,
            Glyphs::Unicode,
            &[&format!("Sub\n{}\n", "-".repeat(128))]
        ),
        ["Sub", "───"]
    );
    assert_eq!(
        kept_natively(
            200,
            Glyphs::Unicode,
            &[&format!("Title\n{}\n", "=".repeat(128))]
        ),
        ["Title", "━━━━━"]
    );
    assert_eq!(
        kept_natively(
            200,
            Glyphs::Unicode,
            &[&format!("Sub\n{}\n", "-".repeat(129))]
        ),
        ["Sub".to_owned(), "─".repeat(200)]
    );
    assert_eq!(
        kept_natively(
            200,
            Glyphs::Unicode,
            &[&format!("Title\n{}\n", "=".repeat(129))]
        ),
        ["Title".to_owned(), "=".repeat(129)]
    );
    for (underline, heavy) in [("---", false), ("===", true)] {
        let long = "t".repeat(128);
        let rule = if heavy { "━" } else { "─" }.repeat(128);
        assert_eq!(
            kept_natively(200, Glyphs::Unicode, &[&format!("{long}\n{underline}\n")]),
            [long.clone(), rule]
        );

        let longer = "t".repeat(129);
        let expected = if heavy {
            underline.to_owned()
        } else {
            "─".repeat(200)
        };
        assert_eq!(
            kept_natively(200, Glyphs::Unicode, &[&format!("{longer}\n{underline}\n")]),
            [longer, expected]
        );
    }
}

#[test]
fn a_native_indented_atx_heading_wears_a_rule_as_wide_as_its_words_and_not_its_indent() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["  # Title\nafter\n"]),
        ["  Title", "━━━━━", "after"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["   ## Sub\nafter\n"]),
        ["   Sub", "───", "after"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Ascii, &["  # Title"]),
        ["  Title", "====="]
    );
}

#[test]
fn a_native_title_of_markers_or_opening_with_a_dollar_or_a_backslash_is_a_heading() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["**\n---\n"]),
        ["**", "──"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["--\n---\n"]),
        ["--", "──"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["$5\n---\n"]),
        ["$5", "──"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["\\*x\n===\n"]),
        ["*x", "━━"]
    );
}

#[test]
fn a_native_answer_cut_anywhere_draws_the_setext_heading_the_whole_answer_draws() {
    for answer in [
        "intro\n\nTitle\n=====\n\nSub\n---\nafter\n",
        "intro line\nTitle with **bold**\n=\nafter\n",
        "$5\n---\nx\n\\*x\n===\nend\n",
    ] {
        let whole = kept_natively(40, Glyphs::Unicode, &[answer]);
        assert!(
            whole
                .iter()
                .any(|row| row.contains('━') || row.contains('─')),
            "{answer:?} drew no rule: {whole:#?}"
        );

        for cut in 1..answer.len() {
            let rows = kept_natively(40, Glyphs::Unicode, &[&answer[..cut], &answer[cut..]]);
            assert_eq!(rows, whole, "cut at {cut} of {answer:?}");
        }
    }
}

#[test]
fn a_native_answer_cut_anywhere_draws_the_indented_block_the_whole_answer_draws() {
    let answer = "para\n\n    let total = first_value + second_value;\n\n    done();\n---\n\nmore\n\n    code\n===\n";
    let whole = kept_natively(40, Glyphs::Unicode, &[answer]);
    for row in ["┌─", "│ done();", "└─", "│ code", "==="] {
        assert!(
            whole.iter().any(|kept| kept.trim_start() == row),
            "{row:?} is not kept: {whole:#?}"
        );
    }
    assert!(
        whole.iter().any(|row| row.trim_start().starts_with('┆')),
        "no wrapped code row: {whole:#?}"
    );
    assert!(!whole.iter().any(|row| row.contains('━')), "{whole:#?}");

    for cut in 1..answer.len() {
        let rows = kept_natively(40, Glyphs::Unicode, &[&answer[..cut], &answer[cut..]]);
        assert_eq!(rows, whole, "cut at {cut} of {answer:?}");
    }
}

#[test]
fn a_native_title_that_proved_prose_reaches_the_scrollback_once_as_the_prose_it_was() {
    let rows = kept_natively(40, Glyphs::Unicode, &["Title\n", "not an underline\n"]);
    assert_eq!(rows, ["Title", "not an underline"]);
}

/// An item long enough to wrap at thirty columns onto three rows or more.
const LONG_ITEM: &str = "long enough that it has to wrap onto another row, and then one more";

#[test]
fn a_native_wrapped_item_hangs_under_its_words() {
    for (marker, hang) in [("- a bullet", 2), ("1. a number", 3)] {
        let answer = format!("{marker} {LONG_ITEM}\n");
        for resized in [None, Some((30, 12))] {
            let columns = if resized.is_some() { 60 } else { 30 };
            let rows = answered_natively(&Window::new(columns, 12), &answer, resized);

            assert!(rows.len() > 2, "{rows:#?}");
            for row in rows.iter().skip(1) {
                let indent = row.len() - row.trim_start().len();
                assert_eq!(indent, hang, "{row:?} does not hang under the words");
            }
        }
    }
}

#[test]
fn a_native_wrapped_quote_keeps_its_bar_on_every_row() {
    let answer = format!("> a quote {LONG_ITEM}\n");
    for resized in [None, Some((30, 12))] {
        let columns = if resized.is_some() { 60 } else { 30 };
        let rows = answered_natively(&Window::new(columns, 12), &answer, resized);

        assert!(rows.len() > 2, "{rows:#?}");
        for row in &rows {
            assert!(row.starts_with("│ "), "{row:?} lost the quote bar");
        }
    }
}

#[test]
fn a_native_wrapped_alert_keeps_its_bar_on_every_row() {
    let answer = format!("> [!NOTE]\n> a body {LONG_ITEM}\n");
    for resized in [None, Some((30, 12))] {
        let columns = if resized.is_some() { 60 } else { 30 };
        let rows = answered_natively(&Window::new(columns, 12), &answer, resized);

        assert!(rows.len() > 3, "{rows:#?}");
        assert_eq!(rows.first().map(String::as_str), Some("▎ ◆ Note"));
        for row in &rows {
            assert!(row.starts_with("▎ "), "{row:?} lost the alert bar");
        }
    }
}

#[test]
fn a_native_answer_cut_anywhere_draws_the_alert_the_whole_answer_draws() {
    let answer = "> [!WARNING]\n> a body long enough that it has to wrap onto another row\n\n> [!tip]\n> more\n";
    let whole = kept_natively(40, Glyphs::Unicode, &[answer]);
    for row in ["▎ ▲ Warning", "▎ ✓ Tip", "▎ more"] {
        assert!(
            whole.iter().any(|kept| kept == row),
            "{row:?} is not kept: {whole:#?}"
        );
    }
    assert!(!whole.iter().any(|row| row.contains("[!")), "{whole:#?}");

    for cut in 1..answer.len() {
        let rows = kept_natively(40, Glyphs::Unicode, &[&answer[..cut], &answer[cut..]]);
        assert_eq!(rows, whole, "cut at {cut} of {answer:?}");
    }
}

#[test]
fn a_native_footnote_is_drawn_as_its_number_in_both_glyph_sets() {
    // The rows kept natively leave out the blank one between the two.
    let answer = "A claim[^1] here.\n\n[^1]: the note\n";
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &[answer]),
        ["A claim¹ here.", "¹ the note"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Ascii, &[answer]),
        ["A claim[1] here.", "[1] the note"]
    );
}

#[test]
fn a_native_key_and_sub_and_superscript_are_drawn_without_their_tags() {
    let answer = "Press <kbd>Ctrl</kbd> for H<sub>2</sub>O and x<sup>2</sup>.\n";
    assert_eq!(
        kept_natively(60, Glyphs::Unicode, &[answer]),
        ["Press Ctrl for H₂O and x²."]
    );
    assert_eq!(
        kept_natively(60, Glyphs::Ascii, &[answer]),
        ["Press [Ctrl] for H₂O and x²."]
    );
}

#[test]
fn a_native_line_break_tag_breaks_the_row_and_a_quote_keeps_its_bar_after_it() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["one<br>two\n"]),
        ["one", "two"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["> one<br/>two\n"]),
        ["│ one", "│ two"]
    );
}

#[test]
fn a_native_details_block_draws_its_summary_behind_a_mark_and_its_body_two_columns_in() {
    let answer = "<details>\n<summary>Why it is</summary>\nthe body\n</details>\nafter\n";
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &[answer]),
        ["▸ Why it is", "  the body", "after"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Ascii, &[answer]),
        ["Details: Why it is", "  the body", "after"]
    );
}

#[test]
fn a_native_answer_cut_anywhere_draws_the_footnotes_and_tags_the_whole_answer_draws() {
    let answer = "A claim[^1] with <kbd>Ctrl</kbd>, H<sub>2</sub>O<br>and x<sup>2</sup>.\n\n\
                  <details><summary>Why</summary>\nthe body\n</details>\n\n[^1]: the note\n";
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let whole = kept_natively(40, glyphs, &[answer]);
        assert!(
            !whole
                .iter()
                .any(|row| row.contains('<') || row.contains("[^")),
            "{glyphs:?} drew markup: {whole:#?}"
        );

        for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
            let rows = kept_natively(40, glyphs, &[&answer[..cut], &answer[cut..]]);
            assert_eq!(rows, whole, "{glyphs:?} cut at {cut} of {answer:?}");
        }
    }
}

#[test]
fn a_native_hostile_reference_angle_or_www_address_writes_no_escape_nor_a_picture_cell() {
    for answer in [
        "[a]: https://x.test/\u{1b}]0;t\u{7}\nsee [a] and [a\u{1b}]0;t\u{7}][a]\n",
        "<https://x.test/\u{1b}]8;;https://evil.invalid\u{1b}\\> and <a\u{1b}]0;t\u{7}@b.test>\n",
        "www.x.test/\u{1b}]0;t\u{7} and www.\u{1b}_Gi=1;AAAA\u{1b}\\x.test and www.y.test\u{8}\u{85}\n",
        "[a\u{1b}]0;t\u{7}]: https://x.test/a\nsee [a\u{1b}]0;t\u{7}] and [w\u{8}][a\u{1b}]0;t\u{7}]\n",
        concat!(
            "[\u{10eeee}\u{305}x]: https://x.test\n",
            "see [words][\u{10eeee}\u{305}x]\n",
        ),
    ] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let window = Window::new(64, 12);
            let mut render = native(&window);
            render.images(crate::terminal::graphics::Capability::Graphics {
                protocol: crate::terminal::graphics::Protocol::Kitty,
                cell: crate::terminal::graphics::Cell {
                    width: 9,
                    height: 18,
                },
            });
            render.draws(glyphs);
            render.wears(colourful().addressing(true));
            stands(&mut render);
            render.stream(answer).unwrap();
            render.settle().unwrap();
            render.seal().unwrap();

            // Each hyperlink sequence is taken out first: it may hold
            // printable ASCII and nothing that ends it early.
            let written = window.written();
            let mut rest = written.as_str();
            let mut kept = String::new();
            while let Some(at) = rest.find("\u{1b}]8;") {
                kept.push_str(&rest[..at]);
                let after = &rest[at + 4..];
                let end = after.find("\u{1b}\\").unwrap_or(after.len());
                assert!(
                    after[..end].bytes().all(|byte| byte.is_ascii_graphic()),
                    "a link sequence carried {:?} for {answer:?}",
                    &after[..end]
                );
                rest = &after[(end + 2).min(after.len())..];
            }
            kept.push_str(rest);

            for hostile in [
                "\u{1b}]", "\u{1b}P", "\u{1b}_", "\u{7}", "\u{8}", "\u{85}", "\u{9c}",
            ] {
                assert!(
                    !kept.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
            assert!(
                !kept.contains('\u{10eeee}'),
                "a picture cell was placed for {answer:?} with {glyphs:?}"
            );
        }
    }
}

#[test]
fn a_native_hostile_forge_address_worded_link_or_image_writes_no_escape() {
    for answer in [
        "https://github.com/augments-labs/crucible-code/pull/1\u{1b}]0;t\u{7} and https://github.com/o/r/commit/abcdef1\u{9b}31m\n",
        "[fix\u{1b}]0;t\u{7}](https://github.com/o/r/pull/12) and [x](https://github.com/o/r/pull/12\u{85})\n",
        "[guide](https://example.com/\u{1b}]8;;https://evil.invalid\u{1b}\\/a/b) and [g](https://example.com/\u{202e}\u{200b}x/y/z)\n",
        "![alt\u{1b}]0;t\u{7}](out/\u{1b}_Gi=1;AAAA\u{1b}\\shot.png) and ![a](https://x.test/\u{202e}gnp.exe\u{9c})\n",
        "[write](mailto:a\u{1b}]0;t\u{7}@b.test) and [w](mailto:a@b.test\u{8}\u{9b}2J)\n",
        "| a |\n|---|\n| [g](https://example.com/\u{1b}]0;t\u{7}/x) https://github.com/o/r/pull/3\u{85} |\n",
    ] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let window = Window::new(64, 12);
            let mut render = native(&window);
            render.draws(glyphs);
            render.wears(colourful().addressing(true));
            render.counts(Some(crate::Forge::new(
                "https://github.com",
                "augments-labs/crucible-code",
                "/issues/",
            )));
            stands(&mut render);
            render.stream(answer).unwrap();
            render.settle().unwrap();
            render.seal().unwrap();

            // Each hyperlink sequence is taken out first: it may hold
            // printable ASCII and nothing that ends it early.
            let written = window.written();
            let mut rest = written.as_str();
            let mut kept = String::new();
            while let Some(at) = rest.find("\u{1b}]8;") {
                kept.push_str(&rest[..at]);
                let after = &rest[at + 4..];
                let end = after.find("\u{1b}\\").unwrap_or(after.len());
                assert!(
                    after[..end].bytes().all(|byte| byte.is_ascii_graphic()),
                    "a link sequence carried {:?} for {answer:?}",
                    &after[..end]
                );
                rest = &after[(end + 2).min(after.len())..];
            }
            kept.push_str(rest);

            for hostile in [
                "\u{1b}]",
                "\u{1b}P",
                "\u{1b}_",
                "\u{1b}[31",
                "\u{1b}[2J",
                "\u{7}",
                "\u{8}",
                "\u{85}",
                "\u{9b}",
                "\u{9c}",
                "\u{202e}",
                "\u{200b}",
            ] {
                assert!(
                    !kept.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
        }
    }
}

#[test]
fn a_native_run_with_no_colour_in_it_keeps_every_link_and_image_as_the_model_wrote_it() {
    let three = "https://github.com/augments-labs/crucible-code/pull/1324 \
                 [the guide](https://example.com/docs/guide/intro) ![shot](out/shot.png)";
    let window = Window::new(200, 12);
    let mut render = native(&window);
    render.counts(Some(crate::Forge::new(
        "https://github.com",
        "augments-labs/crucible-code",
        "/issues/",
    )));
    stands(&mut render);
    render.stream(three).unwrap();
    render.settle().unwrap();
    render.seal().unwrap();

    let rows: Vec<String> = window
        .all()
        .into_iter()
        .map(|row| row.trim_end().to_owned())
        .collect();
    assert!(rows.iter().any(|row| row == three), "{rows:#?}");
    for shown in ["#1324", "(example.com", "[image"] {
        assert!(!rows.iter().any(|row| row.contains(shown)), "{rows:#?}");
    }
}

/// How many columns of blank stand in front of `row`'s first word.
fn indent_of(row: &str) -> usize {
    row.len() - row.trim_start().len()
}

/// The rows `answer` comes to natively, drawn at thirty columns, once as
/// written there and once after a window of sixty narrowed to it.
fn at_thirty_and_narrowed(glyphs: Glyphs, answer: &str) -> [Vec<String>; 2] {
    [None, Some((30, 12))].map(|resized| {
        let columns = if resized.is_some() { 60 } else { 30 };
        answered_natively_in(&Window::new(columns, 12), glyphs, answer, resized)
    })
}

#[test]
fn a_native_wrapped_task_hangs_under_its_words() {
    for (marker, first) in [("- [ ]", "□ a task"), ("- [x]", "✓ a task")] {
        let answer = format!("{marker} a task {LONG_ITEM}\n");
        for rows in at_thirty_and_narrowed(Glyphs::Unicode, &answer) {
            assert!(rows.len() > 2, "{rows:#?}");
            assert!(
                rows.first().is_some_and(|row| row.starts_with(first)),
                "{rows:#?}"
            );
            for row in rows.iter().skip(1) {
                assert_eq!(indent_of(row), 2, "{row:?} does not hang under the words");
            }
        }
    }
}

#[test]
fn a_native_wrapped_nested_item_hangs_under_its_words_not_its_parents() {
    let answer = format!("- an outer item\n  - a nested item {LONG_ITEM}\n");
    for rows in at_thirty_and_narrowed(Glyphs::Unicode, &answer) {
        assert_eq!(rows.first().map(String::as_str), Some("• an outer item"));
        assert!(
            rows.get(1)
                .is_some_and(|row| row.starts_with("  • a nested item")),
            "{rows:#?}"
        );
        for row in rows.iter().skip(2) {
            assert_eq!(indent_of(row), 4, "{row:?} does not hang under the words");
        }
    }
}

#[test]
fn a_native_wrapped_item_hangs_under_its_words_in_ascii_too() {
    let answer = format!("- a bullet item {LONG_ITEM}\n");
    for rows in at_thirty_and_narrowed(Glyphs::Ascii, &answer) {
        assert!(rows.len() > 2, "{rows:#?}");
        assert!(
            rows.first()
                .is_some_and(|row| row.starts_with("- a bullet")),
            "{rows:#?}"
        );
        for row in rows.iter().skip(1) {
            assert_eq!(indent_of(row), 2, "{row:?} does not hang under the words");
        }
    }
}

#[test]
fn a_native_wrapped_quote_keeps_its_bar_in_ascii_too() {
    let answer = format!("> a quote {LONG_ITEM}\n");
    for rows in at_thirty_and_narrowed(Glyphs::Ascii, &answer) {
        assert!(rows.len() > 2, "{rows:#?}");
        for row in &rows {
            assert!(row.starts_with("| "), "{row:?} lost the quote bar");
        }
    }
}

#[test]
fn a_native_held_block_shows_one_status_row_and_none_of_its_source() {
    let window = Window::new(64, 12);
    let mut render = native(&window);
    render.wears(colourful());
    stands(&mut render);
    render.stream("before\n\n$$\nx^2 + y^2\n").unwrap();

    let rows = window.all();
    let status: Vec<&String> = rows
        .iter()
        .filter(|row| row.contains("drawing a"))
        .collect();
    assert_eq!(
        status.iter().map(|row| row.trim_end()).collect::<Vec<_>>(),
        ["◇ drawing a formula · 2 lines so far"],
        "{rows:#?}"
    );
    assert!(!rows.iter().any(|row| row.contains("x^2")), "{rows:#?}");

    render.stream("$$\nafter\n").unwrap();
    render.settle().unwrap();
    stands(&mut render);
    render.seal().unwrap();

    let rows = window.all();
    assert!(
        !rows.iter().any(|row| row.contains("drawing a")),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("x^2 + y^2")),
        "{rows:#?}"
    );
}

// A fenced block in its frame. Copying a wrapped code row is the fullscreen
// selection's to answer; in native mode the terminal's own selection and copy
// take whatever is on screen.

/// A line of code that cannot fit thirty columns beside its gutter.
const CODE_LINE: &str = "let total = first_value + second_value + third_value + fourth_value;";

/// The rows `answer` comes to natively in `glyphs`: once as written in a
/// window `columns` wide, and once as written in a window of eleven hundred
/// columns and then narrowed to `columns`, which must come to the same rows.
fn fenced_natively(glyphs: Glyphs, columns: usize, answer: &str) -> [Vec<String>; 2] {
    [None, Some((columns, 40))].map(|resized| {
        let width = if resized.is_some() { 1100 } else { columns };
        answered_natively_in(&Window::new(width, 40), glyphs, answer, resized)
            .into_iter()
            .map(|row| row.trim_end().to_owned())
            .collect()
    })
}

#[test]
fn a_native_fence_is_drawn_in_a_frame_naming_its_language() {
    for rows in fenced_natively(Glyphs::Unicode, 64, "```rust\nlet x = 1;\n```\n") {
        assert_eq!(rows, ["┌─ rust", "│ let x = 1;", "└─"]);
    }
}

#[test]
fn a_native_fence_is_drawn_in_the_ascii_frame_too() {
    for rows in fenced_natively(Glyphs::Ascii, 64, "```rust\nlet x = 1;\n```\n") {
        assert_eq!(rows, ["+- rust", "| let x = 1;", "+-"]);
    }
}

#[test]
fn a_native_fence_naming_no_language_is_a_frame_with_no_label() {
    for rows in fenced_natively(Glyphs::Unicode, 64, "```\nplain text\n```\n") {
        assert_eq!(rows, ["┌─", "│ plain text", "└─"]);
    }
}

#[test]
fn a_native_empty_fence_is_its_frame_and_no_refusal() {
    for rows in fenced_natively(Glyphs::Unicode, 64, "```rust\n```\n") {
        assert_eq!(rows, ["┌─ rust", "└─"]);
    }
}

#[test]
fn a_native_blank_line_of_code_keeps_its_place_in_the_frame() {
    for rows in fenced_natively(Glyphs::Unicode, 64, "```\none\n\ntwo\n```\n") {
        assert_eq!(rows, ["┌─", "│ one", "│", "│ two", "└─"]);
    }
}

#[test]
fn a_native_fence_label_longer_than_a_label_may_be_is_cut_short() {
    let long = "x".repeat(200);
    for rows in fenced_natively(Glyphs::Unicode, 64, &format!("```{long}\ncode\n```\n")) {
        let top = rows.first().expect("a frame");
        assert!(top.starts_with("┌─ xxx"), "{top:?}");
        assert!(top.ends_with('…'), "{top:?}");
        assert!(crate::width::columns(top) < 64, "{top:?}");
    }
}

#[test]
fn a_native_held_block_that_fits_its_cap_carries_no_refusal() {
    for rows in fenced_natively(
        Glyphs::Unicode,
        64,
        "```mermaid\ngraph TD\n  A --> B\n```\n",
    ) {
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("longer than this can hold")),
            "{rows:#?}"
        );
    }
}

#[test]
fn a_native_wrapped_code_row_marks_itself_with_the_glyph_of_its_set() {
    let answer = format!("```\n{CODE_LINE}\n```\n");
    for rows in fenced_natively(Glyphs::Unicode, 30, &answer) {
        assert!(rows.iter().any(|row| row.starts_with("┆ ")), "{rows:#?}");
    }
    for rows in fenced_natively(Glyphs::Ascii, 30, &answer) {
        assert!(rows.iter().any(|row| row.starts_with(": ")), "{rows:#?}");
        assert!(!rows.iter().any(|row| row.contains(['┌', '│', '└', '┆'])));
    }
}

#[test]
fn a_native_wrapped_code_row_is_marked_as_a_continuation() {
    for (glyphs, side, again) in [(Glyphs::Unicode, "│ ", "┆ "), (Glyphs::Ascii, "| ", ": ")] {
        for rows in fenced_natively(glyphs, 30, &format!("```rust\n{CODE_LINE}\n```\n")) {
            let code: Vec<&str> = rows
                .iter()
                .filter_map(|row| row.strip_prefix(side).or_else(|| row.strip_prefix(again)))
                .collect();
            assert!(code.len() > 2, "{rows:#?}");
            assert!(
                rows.iter()
                    .skip(2)
                    .take(code.len() - 1)
                    .all(|row| row.starts_with(again)),
                "{rows:#?}"
            );
            assert_eq!(code.concat(), CODE_LINE, "{rows:#?}");
            for row in &rows {
                assert!(crate::width::columns(row) <= 30, "{row:?} is too wide");
            }
        }
    }
}

/// A fence named `mermaid` holding more than the reader will hold back.
fn mermaid_past_its_cap() -> String {
    let line = format!("A{} --> B", "a".repeat(990));
    let mut answer = String::from("```mermaid\n");
    for _ in 0..20 {
        answer.push_str(&line);
        answer.push('\n');
    }
    answer.push_str("```\n");
    answer
}

#[test]
fn a_native_held_block_past_its_cap_is_drawn_as_written_with_its_refusal_in_the_label() {
    for (glyphs, top, bottom) in [
        (
            Glyphs::Unicode,
            "┌─ mermaid · longer than this can hold · shown as written",
            "└─",
        ),
        (
            Glyphs::Ascii,
            "+- mermaid - longer than this can hold - shown as written",
            "+-",
        ),
    ] {
        let rows = answered_natively_in(
            &Window::new(1100, 40),
            glyphs,
            &mermaid_past_its_cap(),
            None,
        );
        let rows: Vec<&str> = rows.iter().map(|row| row.trim_end()).collect();
        assert_eq!(rows.first(), Some(&top), "{rows:#?}");
        assert_eq!(rows.last(), Some(&bottom), "{rows:#?}");
        assert_eq!(rows.iter().filter(|row| row.len() > 990).count(), 20);
    }
}

#[test]
fn a_native_hostile_fence_label_reaches_the_terminal_as_nothing_but_its_name() {
    // An escape that opens a device control string or an application command
    // is dropped with its introducer only; what follows it is inert text.
    for (label, name) in [
        ("\u{1b}]0;a title it chose\u{7}rust", "rust"),
        ("\u{1b}]8;;https://example.invalid\u{1b}\\rust", "rust"),
        ("ru\u{7}st\u{8}", "rust"),
        ("ru\u{85}st", "rust"),
        ("\u{1b}P1$r\u{1b}\\rust", "1$rrust"),
        ("\u{1b}_Gi=1;AAAA\u{1b}\\rust", "Gi=1;AAAArust"),
        ("\u{10eeee}\u{305}\u{305}rust", "rust"),
        ("\u{2066}rust\u{2069}", "rust"),
        ("\u{202e}tsur\u{202c}", "tsur"),
    ] {
        let window = Window::new(64, 12);
        let rows = answered_natively(&window, &format!("```{label}\ncode\n```\n"), None);
        assert_eq!(
            rows.iter().map(|row| row.trim_end()).collect::<Vec<_>>(),
            [format!("┌─ {name}"), "│ code".to_owned(), "└─".to_owned()],
            "for {label:?}"
        );

        let written = window.written();
        for hostile in ["\u{1b}]", "\u{1b}P", "\u{1b}_", "\u{7}", "\u{8}", "\u{85}"] {
            assert!(
                !written.contains(hostile),
                "{hostile:?} reached the terminal for {label:?}"
            );
        }
        for hostile in ['\u{2066}', '\u{2069}', '\u{202e}', '\u{202c}', '\u{10eeee}'] {
            assert!(
                !written.contains(hostile),
                "{hostile:?} reached the terminal for {label:?}"
            );
        }
    }
}

#[test]
fn a_native_hostile_tag_key_or_footnote_label_writes_no_escape_nor_a_tag_or_label_a_picture_cell() {
    for answer in [
        "<kbd\u{1b}]0;a title\u{7}>K</kbd> and <details \u{1b}_Gi=1;AAAA\u{1b}\\>\n",
        "<kbd>&amp; \u{1b}]8;;https://example.invalid\u{1b}\\ \u{1b}_Gi=1;AAAA\u{1b}\\</kbd>\n",
        "a[^\u{1b}]0;t\u{7}] and [^\u{1b}_Gi=1;AAAA\u{1b}\\]\n\n[^\u{1b}P1$r\u{1b}\\]: a note\n",
        "H<sub>\u{1b}]0;t\u{7}</sub>O and x<sup>\u{8}2\u{85}</sup>\n",
        concat!(
            "<kbd \u{10eeee}\u{305}>K</kbd> a[^\u{10eeee}\u{305}x] H<sub \u{10eeee}\u{305}>2</sub>",
            " <details \u{10eeee}\u{305}>\n\n[^\u{10eeee}\u{305}x]: n\n",
        ),
    ] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let window = Window::new(64, 12);
            let mut render = native(&window);
            render.images(crate::terminal::graphics::Capability::Graphics {
                protocol: crate::terminal::graphics::Protocol::Kitty,
                cell: crate::terminal::graphics::Cell {
                    width: 9,
                    height: 18,
                },
            });
            render.draws(glyphs);
            render.wears(colourful());
            stands(&mut render);
            render.stream(answer).unwrap();
            render.settle().unwrap();
            render.seal().unwrap();

            let written = window.written();
            for hostile in ["\u{1b}]", "\u{1b}P", "\u{1b}_", "\u{7}", "\u{8}", "\u{85}"] {
                assert!(
                    !written.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
            assert!(
                !written.contains('\u{10eeee}'),
                "a picture cell was placed for {answer:?} with {glyphs:?}"
            );
        }
    }
}

#[test]
fn a_native_entity_inside_a_key_is_drawn_as_written() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["<kbd>&amp; &lt;</kbd>\n"]),
        ["&amp; &lt;"]
    );
}

#[test]
fn a_native_session_lists_the_drawn_blocks_newest_first_each_with_its_written_source() {
    let table = "| name | job |\n|---|---|\n| alpha | first |\n| beta | second |\n";
    let math = "$$\nx^2 + y^2\n$$\n";
    let window = Window::new(60, 20);
    let mut render = native(&window);
    render.wears(colourful());
    stands(&mut render);
    render
        .stream(&format!("before\n\n{table}\nbetween\n\n{math}\nafter\n"))
        .unwrap();
    render.settle().unwrap();
    render.seal().unwrap();

    let listed: Vec<(&str, &str)> = render
        .sources()
        .map(|source| (source.word, source.text))
        .collect();
    assert_eq!(listed, [("math", math), ("table", table)]);

    let bound = render.sourced();
    assert!(bound > 0);
    assert!(render.sources().all(|source| source.line < bound));
}

#[test]
fn a_native_session_of_prose_and_a_rule_lists_no_source_and_bounds_nothing() {
    let window = Window::new(60, 20);
    let mut render = native(&window);
    stands(&mut render);
    render
        .stream("just prose\n\n---\n\nand a rule above it\n")
        .unwrap();
    render.settle().unwrap();
    render.seal().unwrap();

    assert_eq!(render.sources().count(), 0);
    assert_eq!(render.sourced(), 0);
}

#[test]
fn a_native_lone_marker_is_an_underline() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["Title\n=\n"]),
        ["Title", "━━━━━"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["Sub\n-\n"]),
        ["Sub", "───"]
    );
}

#[test]
fn native_atx_and_setext_headings_of_one_level_look_alike_and_levels_four_to_six_look_like_three() {
    let atx = kept_natively(40, Glyphs::Unicode, &["# One\n\n## Two\n\n### Three\n"]);
    let setext = kept_natively(40, Glyphs::Unicode, &["One\n===\n\nTwo\n---\n\nThree\n"]);
    assert_eq!(atx, ["One", "━━━", "Two", "───", "Three"]);
    assert_eq!(
        setext.iter().take(4).collect::<Vec<_>>(),
        atx.iter().take(4).collect::<Vec<_>>()
    );
    assert_eq!(
        kept_natively(
            40,
            Glyphs::Unicode,
            &["#### Four\n\n##### Five\n\n###### Six\n"]
        ),
        ["Four", "Five", "Six"]
    );
}

#[test]
fn a_native_line_of_cells_over_a_delimiter_row_is_still_a_table_and_not_a_heading() {
    let rows = kept_natively(40, Glyphs::Unicode, &["a | b\n--|--\n1 | 2\n"]);
    assert!(rows.iter().any(|row| row.contains('│')), "{rows:#?}");
    assert!(
        !rows
            .iter()
            .any(|row| row.contains('─') && !row.contains('┼')),
        "{rows:#?}"
    );
}

#[test]
fn a_native_dashed_row_under_a_line_that_was_no_table_header_underlines_it() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["one cell\n---\n"]),
        ["one cell", "────────"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["a | b\n---\n1 | 2\n"]),
        ["a | b", "─────", "1 | 2"]
    );
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["one cell\n:-:\n"]),
        ["one cell", ":-:"]
    );
}

#[test]
fn a_native_underline_makes_only_the_last_line_of_a_paragraph_the_heading() {
    assert_eq!(
        kept_natively(40, Glyphs::Unicode, &["first line\nsecond line\n===\n"]),
        ["first line", "second line", "━━━━━━━━━━━"]
    );
}

#[test]
fn a_native_details_body_wider_than_the_window_is_two_columns_in_on_every_row() {
    let wide = format!("<details><summary>Why</summary>\n{LONG_ITEM}\n</details>\nafter\n");
    let broken =
        format!("<details><summary>Why</summary>\nthe body<br>{LONG_ITEM}\n</details>\nafter\n");
    for answer in [wide, broken] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for rows in at_thirty_and_narrowed(glyphs, &answer) {
                let body = rows
                    .get(1..rows.len().saturating_sub(1))
                    .unwrap_or_default();
                assert!(body.len() > 2, "{glyphs:?} drew {rows:#?}");
                for row in body {
                    assert_eq!(indent_of(row), 2, "{glyphs:?} drew {row:?} in {rows:#?}");
                }
                assert_eq!(rows.last().map(String::as_str), Some("after"), "{rows:#?}");
            }
        }
    }
}

#[test]
fn a_native_closing_details_tag_after_a_bare_table_is_no_row_of_it_whole_or_cut() {
    let answer = "<details>\n<summary>S</summary>\n\na | b\n--|--\n1 | 2\n</details>\nafter\n";
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let whole = kept_natively(40, glyphs, &[answer]);
        assert!(
            whole.iter().all(|row| !row.contains("details")),
            "{glyphs:?} drew {whole:#?}"
        );
        assert!(
            whole.iter().any(|row| row.contains('2')),
            "{glyphs:?} drew {whole:#?}"
        );
        assert_eq!(
            whole.last().map(String::as_str),
            Some("after"),
            "{whole:#?}"
        );

        for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
            let rows = kept_natively(40, glyphs, &[&answer[..cut], &answer[cut..]]);
            assert_eq!(rows, whole, "{glyphs:?} cut at {cut} of {answer:?}");
        }
    }
}

#[test]
fn a_native_entity_is_drawn_as_the_character_it_names_and_a_backslash_break_draws_no_backslash() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = kept_natively(30, glyphs, &["a &amp; b &lt;c&gt; &#169; &#x41;\n"]);
        assert!(rows.iter().any(|row| row == "a & b <c> © A"), "{rows:#?}");

        let rows = kept_natively(30, glyphs, &["first line\\\nsecond line\n"]);
        assert!(rows.iter().any(|row| row == "first line"), "{rows:#?}");
        assert!(rows.iter().any(|row| row == "second line"), "{rows:#?}");
        assert!(!rows.iter().any(|row| row.contains('\\')), "{rows:#?}");

        let rows = kept_natively(30, glyphs, &["ends on it\\\n\nnext\n"]);
        assert!(rows.iter().any(|row| row == "ends on it\\"), "{rows:#?}");
    }
}

#[test]
fn a_native_hostile_entity_writes_no_escape_nor_a_picture_cell_and_is_drawn_as_written() {
    for answer in [
        "&#27;[31mred&#27;[0m and &#27;]0;t&#7;\n",
        "&#x1b; and &#x1B;]8;;https://evil.invalid&#x7;\n",
        "&#x202E;gnirts and &#8238;\n",
        "&#7; bell, &#0; nothing and &#x0; nothing\n",
        "a &#x10EEEE; b and &#1109742; c\n",
    ] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let window = Window::new(64, 12);
            let mut render = native(&window);
            render.images(crate::terminal::graphics::Capability::Graphics {
                protocol: crate::terminal::graphics::Protocol::Kitty,
                cell: crate::terminal::graphics::Cell {
                    width: 9,
                    height: 18,
                },
            });
            render.draws(glyphs);
            stands(&mut render);
            render.stream(answer).unwrap();
            render.settle().unwrap();
            render.seal().unwrap();

            let written = window.written();
            for hostile in [
                "\u{1b}]",
                "\u{1b}[31",
                "\u{1b}P",
                "\u{1b}_",
                "\u{7}",
                "\u{8}",
                "\u{85}",
                "\u{9c}",
                "\u{202e}",
                "\u{0}",
            ] {
                assert!(
                    !written.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
            assert!(
                !written.contains('\u{10eeee}'),
                "a picture cell was placed for {answer:?} with {glyphs:?}"
            );
        }
    }
    let rows = kept_natively(
        64,
        Glyphs::Unicode,
        &["&#27;[31mred and &#x1b; and &#x202E;gnirts and &#7; and &#0;\n"],
    )
    .join("\n");
    for words in ["&#27;[31m", "&#x1b;", "&#x202E;", "&#7;", "&#0;"] {
        assert!(rows.contains(words), "{rows:?} lost {words:?}");
    }
}

#[test]
fn a_native_struck_phrase_wears_the_strike_and_a_path_stays_as_written() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = kept_natively(80, glyphs, &["~one~ and ~/Projects and cd ~/a and ~/b\n"]);
        assert!(
            rows.iter()
                .any(|row| row == "one and ~/Projects and cd ~/a and ~/b"),
            "{glyphs:?}: {rows:#?}"
        );

        let rows = kept_natively(80, glyphs, &["between ~/a and ~/b\n"]);
        assert!(
            rows.iter().any(|row| row == "between ~/a and ~/b"),
            "{glyphs:?}: {rows:#?}"
        );

        let window = Window::new(80, 12);
        answered_natively_in(&window, glyphs, "~one~ and ~/Projects\n", None);
        let worn = colourful().open(Slot::Struck);
        let written = window.written();
        assert!(
            written.contains(&format!("{}one", worn.as_str())),
            "{glyphs:?}: {written:?}"
        );
        assert!(
            !written.contains(&format!("{}/Projects", worn.as_str())),
            "{glyphs:?}: {written:?}"
        );
    }
}

#[test]
fn a_native_run_under_three_markers_is_drawn_bold_and_italic_together() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let window = Window::new(80, 12);
        let rows = answered_natively_in(&window, glyphs, "a ***both*** b\n", None);
        assert!(
            rows.iter().any(|row| row.trim_end() == "a both b"),
            "{glyphs:?}: {rows:#?}"
        );
        let written = window.written();
        assert!(
            written.contains("\u{1b}[1;3mboth"),
            "{glyphs:?}: {written:?}"
        );
    }
}

#[test]
fn a_native_held_phrase_is_told_of_in_the_foot_and_joins_the_scrollback_once_settled() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let window = Window::new(64, 12);
        let mut render = native(&window);
        render.draws(glyphs);
        render.wears(colourful());
        stands(&mut render);
        render.stream("look in ~/Projects").unwrap();

        let rows = window.all();
        assert!(
            rows.iter().any(|row| row.trim_end() == "~/Projects"),
            "{glyphs:?}: {rows:#?}"
        );
        assert!(
            !window
                .scrollback()
                .iter()
                .any(|row| row.contains("~/Projects")),
            "{glyphs:?}: the held phrase reached the scrollback"
        );

        render.stream("\n\nnext\n").unwrap();
        render.settle().unwrap();
        stands(&mut render);
        render.seal().unwrap();

        let rows: Vec<String> = window
            .all()
            .into_iter()
            .map(|row| row.trim_end().to_owned())
            .collect();
        assert!(
            !rows.iter().any(|row| row == "~/Projects"),
            "{glyphs:?}: {rows:#?}"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| *row == "look in ~/Projects")
                .count(),
            1,
            "{glyphs:?}: {rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row == "next"),
            "{glyphs:?}: {rows:#?}"
        );
    }
}

// A fence drawn by the drawers the renderer was handed. The drawer and the
// hostile fences are the full screen's, so both modes are held to one input.

/// The rows `answer` comes to natively in `glyphs` in a window `columns`
/// wide, with `drawers` handed to the renderer, and everything written.
fn drawn_natively(
    glyphs: Glyphs,
    columns: usize,
    answer: &str,
    drawers: Option<crate::drawing::Drawers>,
    resized: Option<usize>,
) -> (Vec<String>, String) {
    let window = Window::new(columns, 40);
    let mut render = native(&window);
    render.draws(glyphs);
    render.wears(colourful().addressing(true));
    if let Some(drawers) = drawers {
        render.drawers(drawers);
    }
    stands(&mut render);
    render.stream(answer).unwrap();
    render.settle().unwrap();
    stands(&mut render);
    render.seal().unwrap();
    if let Some(columns) = resized {
        settles_at(&window, &mut render, columns, 40);
    }
    let (boxed, _) = boxed();
    let boxed: Vec<String> = boxed.iter().map(Row::text).collect();
    let rows = window
        .all()
        .into_iter()
        .filter(|row| !row.trim().is_empty() && !boxed.contains(row))
        .map(|row| row.trim_end().to_owned())
        .collect();
    (rows, window.written())
}

/// An answer with a change in a fence between two lines of prose.
const CHANGE_FENCED: &str =
    "Before.\n\n```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n```\n\nAfter.\n";

#[test]
fn a_native_fence_of_an_answer_is_drawn_by_the_drawer_the_renderer_was_handed() {
    use super::super::tests::drawers::echoing;

    for (glyphs, mark) in [(Glyphs::Unicode, "»"), (Glyphs::Ascii, ">")] {
        let (rows, _) = drawn_natively(glyphs, 64, CHANGE_FENCED, Some(echoing()), None);
        assert_eq!(
            rows,
            [
                "Before.".to_owned(),
                format!("{mark} --- a/x.rs"),
                format!("{mark} +++ b/x.rs"),
                format!("{mark} @@ -1 +1 @@"),
                format!("{mark} -old"),
                format!("{mark} +new"),
                "After.".to_owned(),
            ],
            "{glyphs:?}"
        );
    }
}

#[test]
fn a_native_drawn_fences_offer_of_a_key_stands_at_each_width_it_is_laid_out_for() {
    use super::super::tests::drawers::offering;

    // Ctrl+O lists the fence in this screen too, so the key is one that
    // answers, in the window it was drawn in and in a narrower one.
    for narrowed in [None, Some(40)] {
        let (rows, _) = drawn_natively(
            Glyphs::Unicode,
            64,
            CHANGE_FENCED,
            Some(offering()),
            narrowed,
        );
        assert_eq!(
            rows,
            ["Before.", "5 lines held · ctrl+o to expand", "After."],
            "{narrowed:?}"
        );
    }
}

#[test]
fn a_native_renderer_handed_no_drawer_draws_a_fence_exactly_as_it_was_written() {
    let (untold, written) = drawn_natively(Glyphs::Unicode, 64, CHANGE_FENCED, None, None);
    assert_eq!(
        untold,
        [
            "Before.",
            "┌─ diff",
            "│ --- a/x.rs",
            "│ +++ b/x.rs",
            "│ @@ -1 +1 @@",
            "│ -old",
            "│ +new",
            "└─",
            "After.",
        ]
    );

    // Handed a registry with nothing in it, it writes the same bytes.
    let empty = Some(crate::drawing::Drawers::default());
    let (rows, same) = drawn_natively(Glyphs::Unicode, 64, CHANGE_FENCED, empty, None);
    assert_eq!(rows, untold);
    assert_eq!(same, written);
}

#[test]
fn a_native_drawn_fence_narrowed_is_the_fence_a_window_that_narrow_draws() {
    use super::super::tests::drawers::echoing;

    let answer = "```diff\n+a line a good deal longer than the narrow window is wide\n```\n";
    let (fresh, _) = drawn_natively(Glyphs::Unicode, 24, answer, Some(echoing()), None);
    let (narrowed, _) = drawn_natively(Glyphs::Unicode, 70, answer, Some(echoing()), Some(24));

    // The room is the window's less the column the rail is spared.
    assert_eq!(fresh, ["» +a line a good deal lo"]);
    assert_eq!(narrowed, fresh);
}

#[test]
fn a_native_hostile_path_or_line_in_a_drawn_fence_writes_no_escape() {
    use super::super::tests::drawers::{HOSTILE_FENCES, OBEYED, echoing};

    for answer in HOSTILE_FENCES {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let (rows, written) = drawn_natively(glyphs, 64, answer, Some(echoing()), None);

            // The drawer drew it, so what it was handed is what is on screen.
            assert!(
                rows.iter().any(|row| row.contains("@@ -1")),
                "{answer:?} with {glyphs:?}: {rows:#?}"
            );
            assert!(
                !rows
                    .iter()
                    .any(|row| row.contains("diff") || row.contains("patch")),
                "{answer:?} with {glyphs:?}: {rows:#?}"
            );
            for hostile in OBEYED {
                assert!(
                    !written.contains(hostile),
                    "{hostile:?} reached the terminal for {answer:?} with {glyphs:?}"
                );
            }
        }
    }
}
