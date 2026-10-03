//! Native mode, asserted against a terminal that keeps a scrollback.
//!
//! [`Picture`](crate::Picture) is a screen this process owns: a row is named
//! and nothing scrolls. Native mode writes into a buffer that does, so these
//! tests replay its bytes into [`Emulator`], which moves the cursor relatively,
//! wraps, scrolls rows off the top into a scrollback and erases to the end of
//! the screen — the handful of things a native frame says. What a reader would
//! find on scrolling back is then a list of strings to assert on.

use std::cell::RefCell;
use std::rc::Rc;

use super::super::*;
use crate::color::{Palette, Slot};
use crate::row::Row;
use crate::terminal::{Size, Terminal, TerminalError};

/// The main buffer of a terminal: a screen, and what scrolled off its top.
#[derive(Debug)]
struct Emulator {
    columns: usize,
    rows: usize,
    screen: Vec<Vec<char>>,
    scrollback: Vec<String>,
    /// Row and column of the cursor.
    at: (usize, usize),
    /// Whether the last character filled the row, so the next one wraps.
    wrapping: bool,
}

impl Emulator {
    fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns,
            rows,
            screen: vec![Vec::new(); rows],
            scrollback: Vec::new(),
            at: (0, 0),
            wrapping: false,
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
        self.scrollback
            .push(top.iter().collect::<String>().trim_end().to_owned());
        self.screen.push(Vec::new());
    }

    fn put(&mut self, character: char) {
        if self.wrapping {
            self.at.1 = 0;
            self.down();
            self.wrapping = false;
        }
        let (row, column) = self.at;
        if let Some(line) = self.screen.get_mut(row) {
            if line.len() <= column {
                line.resize(column + 1, ' ');
            }
            if let Some(cell) = line.get_mut(column) {
                *cell = character;
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
            'J' => {
                let (row, column) = self.at;
                if let Some(line) = self.screen.get_mut(row) {
                    line.truncate(column);
                }
                for below in self.screen.iter_mut().skip(row + 1) {
                    below.clear();
                }
            }
            'K' => {
                let (row, column) = self.at;
                if let Some(line) = self.screen.get_mut(row) {
                    line.truncate(column);
                }
            }
            _ => return,
        }
        self.wrapping = false;
    }

    /// The window made wider, as a reader dragging its corner would.
    fn widen(&mut self, columns: usize) {
        assert!(columns >= self.columns, "this terminal does not narrow");
        self.columns = columns;
    }

    /// The screen, one string a row, padding taken off.
    fn screen(&self) -> Vec<String> {
        self.screen
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_owned())
            .collect()
    }

    /// Everything a reader could scroll to: the scrollback, then the screen.
    fn all(&self) -> Vec<String> {
        let mut all = self.scrollback.clone();
        all.extend(self.screen());
        all
    }
}

/// What the terminal was sent and what it shows, shared with the renderer so a
/// test can read both after the renderer has gone.
#[derive(Debug)]
struct Seen {
    written: String,
    emulator: Emulator,
    is_terminal: bool,
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
        })))
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

    fn widen(&self, columns: usize) {
        self.0.borrow_mut().emulator.widen(columns);
    }

    /// How many rows anywhere in the buffer say `text`.
    fn rows_saying(&self, text: &str) -> usize {
        self.all().iter().filter(|row| row.contains(text)).count()
    }
}

impl Terminal for Window {
    fn size(&self) -> Result<Size, TerminalError> {
        let seen = self.0.borrow();
        Ok(Size {
            columns: seen.emulator.columns,
            rows: seen.emulator.rows,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        let mut seen = self.0.borrow_mut();
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
fn native_writes_no_alternate_screen_mouse_capture_or_screen_row_address() {
    let window = Window::new(40, 10);
    let mut render = native(&window);

    render
        .opens(Box::new(|_| vec![row("crucible"), row("the opening")]))
        .unwrap();
    stands(&mut render);
    render.commit("> hello").unwrap();
    render.stream("an answer on its way").unwrap();
    render
        .under(&[row("* thinking")], None, Palette::plain())
        .unwrap();
    render.seal().unwrap();
    render.settle().unwrap();
    window.widen(50);
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

    window.widen(60);
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
