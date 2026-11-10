//! Whether the terminal can draw pictures, learned without a reply ever
//! reaching the prompt.
//!
//! Three decisions, in this order, each able to end it:
//!
//! 1. **Where nothing may be asked.** Output redirected, input not a terminal,
//!    `NO_COLOR`, or colour turned off: no question is written and no picture
//!    drawn ([`select`]).
//! 2. **What the setting says.** `off` is text. A named protocol is used
//!    without asking, except where the support table below says it draws
//!    wrongly. `auto` lets the environment name a candidate, and names none
//!    inside a multiplexer or on Windows.
//! 3. **What the terminal says.** A candidate is confirmed by an exchange
//!    ([`confirm`]): the cursor-position question alone, then — only if that
//!    was answered — the candidate's own question followed by the cursor
//!    position again. A terminal answers in order, so the candidate's reply
//!    arriving before the second cursor position is a yes, and the cursor
//!    position arriving first is a no that cost no wait.
//!
//! **No reply becomes a key.** The exchange runs before the session reads a
//! key, and reads every byte that arrives while it waits, parsing each by its
//! shape ([`Replies`]): a complete sequence is consumed whatever it says, and a
//! sequence that is still arriving when the wait runs out is read to its end,
//! for at most the reply wait again. A yes that arrives behind its cursor
//! position is read for the patience again and counts as a no. A cursor
//! position that arrives after the exchange gave up is one shape the key reader
//! drops itself. One exchange reads at most 16 KiB, so a terminal that sends
//! without end is left at once. What a user types during the exchange is
//! consumed with the replies, which costs at most the exchange's own few
//! milliseconds of typing.
//!
//! **The table is what was seen.** [`support`] holds the cells of the terminal
//! support table that drew right in every step of their screen mode; every
//! other cell is disabled, because a disabled cell is output that was seen to
//! draw wrongly or was never seen at all. A terminal is identified only by what
//! the table identified it by.

use std::fmt;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::AsFd;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use crate::ScreenMode;

/// How a picture is put on a terminal's screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Kitty's graphics protocol.
    Kitty,
    /// iTerm2's inline images, which `WezTerm` draws too.
    Iterm2,
    /// DEC Sixel.
    Sixel,
}

/// The pixel size of one character cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// Pixels across.
    pub width: u16,
    /// Pixels down.
    pub height: u16,
}

impl Cell {
    /// The cell assumed where a protocol is forced and the window reports no
    /// pixels.
    pub const FALLBACK: Self = Self {
        width: 8,
        height: 16,
    };

    /// The cell a window of `columns` by `rows` cells, `width` by `height`
    /// pixels, has; `None` where any of them is nought, which is how a
    /// terminal that does not fill the pixel fields reports them.
    #[must_use]
    pub fn measured(columns: u16, rows: u16, width: u16, height: u16) -> Option<Self> {
        let width = width.checked_div(columns)?;
        let height = height.checked_div(rows)?;
        (width > 0 && height > 0).then_some(Self { width, height })
    }
}

/// What the session draws pictures with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Capability {
    /// Pictures are described in text.
    #[default]
    Text,
    /// Pictures are drawn, with this protocol, into cells this size.
    Graphics {
        /// The protocol confirmed or forced.
        protocol: Protocol,
        /// The cell a picture is fitted to.
        cell: Cell,
    },
}

/// The system the process runs on, passed as a value so every platform's
/// answer can be tested on any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows, which draws text this release.
    Windows,
    /// FreeBSD, where no terminal was seen to draw a picture.
    FreeBsd,
    /// Any other, where none was seen either.
    Other,
}

impl Platform {
    /// The platform this build is for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "freebsd") {
            Self::FreeBsd
        } else {
            Self::Other
        }
    }
}

/// What `output.images` asks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Wanted {
    /// Ask the terminal where the environment names a candidate.
    #[default]
    Auto,
    /// Use this protocol without asking.
    Forced(Protocol),
    /// Draw text.
    Off,
}

/// Whether tmux passes a program's escape sequences through to the terminal
/// outside it, as its server says for this pane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Passthrough {
    /// `allow-passthrough` is `on` or `all`.
    On,
    /// It is `off`.
    Off,
    /// The server was not asked, or did not say.
    #[default]
    Unknown,
}

/// What the session would draw with, if the terminal agreed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Setting {
    /// `output.images`.
    pub images: Wanted,
    /// The screen mode the session draws in.
    pub screen: ScreenMode,
    /// `false` where `output.color` is `never`.
    pub colour: bool,
    /// Whether input comes from a terminal and output goes to one.
    pub terminal: bool,
    /// What tmux said, where it was asked.
    pub passthrough: Passthrough,
}

/// What [`select`] decided, for [`confirm`] to settle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Candidate {
    /// Nothing is asked and text is drawn.
    #[default]
    None,
    /// The terminal is asked whether it draws this.
    Asked(Protocol),
    /// This is drawn without asking.
    Forced(Protocol),
}

/// Whether a support-table cell draws right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// It was seen to draw right in every step of its screen mode.
    Enabled,
    /// It was seen to draw wrongly, or was not seen.
    Disabled,
}

/// The candidate for this session: what is asked, what is drawn without
/// asking, or nothing.
///
/// Pure, so every platform and environment is tested from any host: what the
/// terminal and tmux would say is passed in, never found out here.
#[must_use]
pub fn select(
    env: &dyn Fn(&str) -> Option<String>,
    platform: Platform,
    setting: Setting,
) -> Candidate {
    if quiet(env, setting) {
        return Candidate::None;
    }
    let drawn = |protocol| {
        support(env, platform, setting.screen, protocol, setting.passthrough) == Support::Enabled
    };
    match setting.images {
        Wanted::Forced(protocol) if drawn(protocol) => Candidate::Forced(protocol),
        Wanted::Off | Wanted::Forced(_) => Candidate::None,
        // A multiplexer answers some questions itself and passes others on, so
        // what it says is not what the terminal outside it draws.
        Wanted::Auto
            if matches!(
                place(env),
                Place::Tmux | Place::Multiplexed | Place::RemoteMultiplexed
            ) =>
        {
            Candidate::None
        }
        Wanted::Auto => match outer(env) {
            Some(Outer::Kitty) if drawn(Protocol::Kitty) => Candidate::Asked(Protocol::Kitty),
            Some(Outer::Wezterm) if drawn(Protocol::Iterm2) => Candidate::Asked(Protocol::Iterm2),
            Some(Outer::Kitty | Outer::Wezterm) | None => Candidate::None,
        },
    }
}

/// Whether nothing may be written to ask: output redirected or input not a
/// terminal, `NO_COLOR` set, or colour turned off.
fn quiet(env: &dyn Fn(&str) -> Option<String>, setting: Setting) -> bool {
    !setting.terminal || !setting.colour || env("NO_COLOR").is_some_and(|set| !set.is_empty())
}

/// Whether `protocol` draws right in `screen` mode where this session runs.
///
/// The cells of the terminal support table that drew right in every step of
/// their mode. Every judgement in it was made on Linux, under a virtual X
/// display, so a platform it does not cover is no cell, and neither is a
/// terminal it does not identify.
#[must_use]
pub fn support(
    env: &dyn Fn(&str) -> Option<String>,
    platform: Platform,
    screen: ScreenMode,
    protocol: Protocol,
    passthrough: Passthrough,
) -> Support {
    let covered = match platform {
        Platform::Linux | Platform::Macos => true,
        Platform::Windows | Platform::FreeBsd | Platform::Other => false,
    };
    let row = match place(env) {
        Place::Here => outer(env).map(|outer| (Row::Here, outer)),
        Place::Remote => outer(env).map(|outer| (Row::Remote, outer)),
        // tmux passing sequences through is the row; tmux holding them back,
        // `screen` and Zellij are rows where nothing drew.
        Place::Tmux if passthrough == Passthrough::On => outer(env).map(|outer| (Row::Tmux, outer)),
        Place::Tmux | Place::Multiplexed | Place::RemoteMultiplexed => None,
    };
    let Some(row) = row.filter(|_| covered) else {
        return Support::Disabled;
    };
    if ENABLED.contains(&(row.0, row.1, screen, protocol)) {
        Support::Enabled
    } else {
        Support::Disabled
    }
}

/// The cells that drew right: where, inside what, in which mode, with what.
///
/// Kitty over SSH did not answer inside the patience, and `WezTerm` in native
/// mode lost a picture's top after a resize, so neither is here.
const ENABLED: [(Row, Outer, ScreenMode, Protocol); 6] = [
    (
        Row::Here,
        Outer::Kitty,
        ScreenMode::Fullscreen,
        Protocol::Kitty,
    ),
    (Row::Here, Outer::Kitty, ScreenMode::Native, Protocol::Kitty),
    (
        Row::Here,
        Outer::Wezterm,
        ScreenMode::Fullscreen,
        Protocol::Iterm2,
    ),
    (Row::Tmux, Outer::Kitty, ScreenMode::Native, Protocol::Kitty),
    (
        Row::Tmux,
        Outer::Wezterm,
        ScreenMode::Fullscreen,
        Protocol::Iterm2,
    ),
    (
        Row::Remote,
        Outer::Wezterm,
        ScreenMode::Fullscreen,
        Protocol::Iterm2,
    ),
];

/// Where the session's terminal is, as far as a support-table row goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// On this machine, drawn on directly.
    Here,
    /// Behind tmux, which passes sequences through.
    Tmux,
    /// At the far end of SSH.
    Remote,
}

/// Where the session is, read from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// On the terminal itself.
    Here,
    /// Inside tmux, on this machine.
    Tmux,
    /// Inside `screen` or Zellij.
    Multiplexed,
    /// Reached over SSH.
    Remote,
    /// Inside a multiplexer reached over SSH, which no row covers.
    RemoteMultiplexed,
}

/// The terminal the session is drawn on, by the variables the support table
/// identified it by. Kitty's are the more particular, so they are read first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outer {
    /// Kitty or Ghostty.
    Kitty,
    /// `WezTerm` or iTerm2.
    Wezterm,
}

/// `name`, where it is set to something.
fn set(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name).filter(|value| !value.is_empty())
}

fn place(env: &dyn Fn(&str) -> Option<String>) -> Place {
    let term = set(env, "TERM").unwrap_or_default();
    let tmux = set(env, "TMUX").is_some();
    let multiplexed = tmux
        || set(env, "STY").is_some()
        || set(env, "ZELLIJ").is_some()
        || term.starts_with("tmux")
        || term.starts_with("screen");
    let remote = ["SSH_TTY", "SSH_CONNECTION", "SSH_CLIENT"]
        .iter()
        .any(|name| set(env, name).is_some());
    match (multiplexed, remote) {
        (true, true) => Place::RemoteMultiplexed,
        (true, false) if tmux => Place::Tmux,
        (true, false) => Place::Multiplexed,
        (false, true) => Place::Remote,
        (false, false) => Place::Here,
    }
}

fn outer(env: &dyn Fn(&str) -> Option<String>) -> Option<Outer> {
    let term = set(env, "TERM").unwrap_or_default();
    let program = set(env, "TERM_PROGRAM").unwrap_or_default();
    if set(env, "KITTY_WINDOW_ID").is_some()
        || term.contains("kitty")
        || term.contains("ghostty")
        || program == "ghostty"
    {
        Some(Outer::Kitty)
    } else if program == "WezTerm" || program == "iTerm.app" || set(env, "LC_TERMINAL").is_some() {
        Some(Outer::Wezterm)
    } else {
        None
    }
}

/// How long the exchange waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    /// For the first cursor position, which every terminal answers.
    pub patience: Duration,
    /// For the candidate's reply and the cursor position behind it.
    pub reply: Duration,
}

impl Deadline {
    /// What a start waits: a terminal that answers at all answers the cursor
    /// position in well under 8 ms, and one that answers it may take longer
    /// over the candidate's question, which a slow link pays once.
    pub const STARTUP: Self = Self {
        patience: Duration::from_millis(8),
        reply: Duration::from_secs(2),
    };
}

/// A picture's bytes, encoded for one protocol, and the cells it covers.
pub struct Payload {
    protocol: Protocol,
    bytes: Vec<u8>,
    columns: u16,
    rows: u16,
}

impl Payload {
    /// `bytes`, encoded for `protocol` from a picture `width` by `height`
    /// pixels, to be placed in cells the size of `cell`.
    #[must_use]
    pub fn new(
        protocol: Protocol,
        bytes: Vec<u8>,
        (width, height): (u32, u32),
        cell: Cell,
    ) -> Self {
        let across = |pixels: u32, size: u16| {
            let cells = pixels.div_ceil(u32::from(size.max(1)));
            u16::try_from(cells).unwrap_or(u16::MAX)
        };
        Self {
            protocol,
            bytes,
            columns: across(width, cell.width),
            rows: across(height, cell.height),
        }
    }

    /// The protocol the bytes are encoded for.
    #[must_use]
    pub const fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// The encoded bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The cells the picture covers, across and down.
    #[must_use]
    pub const fn cells(&self) -> (u16, u16) {
        (self.columns, self.rows)
    }
}

// The bytes are a picture, and a picture in a log is megabytes of noise.
impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Payload")
            .field("protocol", &self.protocol)
            .field("bytes", &self.bytes.len())
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .finish()
    }
}

/// The longest sequence whose bytes are kept to be read. A longer one is still
/// consumed to its end, and is read as no reply this exchange asked for.
#[cfg(unix)]
const LONGEST: usize = 1024;

/// The image a Kitty query names, so its reply is told from any other.
#[cfg(unix)]
const QUERY_IMAGE: &str = "i=31";

/// What a run of bytes from the terminal was, by its shape.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// A byte outside any sequence: something typed.
    Typed,
    /// A cursor position report, `CSI row ; column R`.
    CursorPosition,
    /// Kitty's reply to the query this exchange writes, saying it can or not.
    Graphics {
        /// Whether it said `OK`.
        ok: bool,
    },
    /// iTerm2's report of its cell size.
    CellSize,
    /// The primary device attributes.
    Attributes {
        /// Whether they include Sixel.
        sixel: bool,
    },
    /// Any other sequence, complete or cut short by what followed it.
    Other,
}

/// Where the reader is inside a sequence.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Outside one.
    Ground,
    /// After `ESC`.
    Escape,
    /// After `ESC` and an intermediate byte, such as the `(` that picks a
    /// character set.
    EscapeIntermediate,
    /// After `ESC O`, which one more byte ends.
    Shift,
    /// After `CSI`.
    Control,
    /// Inside a string: an OSC, DCS, APC, PM or SOS.
    String(Kind),
    /// After an `ESC` inside a string, which `\` makes its end.
    StringEscape(Kind),
}

/// Which string a reader is in.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Operating system command, which `BEL` may end too.
    Osc,
    /// Application program command.
    Apc,
    /// Device control string, privacy message or start of string.
    Data,
}

/// Bytes from the terminal, read one at a time into [`Shape`]s.
///
/// Every byte belongs to exactly one shape or to the sequence still being
/// read, so nothing is ever left over to be read as a key. Bounded: a
/// sequence keeps at most [`LONGEST`] of its bytes, however long it runs.
#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct Replies {
    state: State,
    held: Vec<u8>,
    spilled: bool,
}

#[cfg(unix)]
impl Replies {
    pub(crate) const fn new() -> Self {
        Self {
            state: State::Ground,
            held: Vec::new(),
            spilled: false,
        }
    }

    /// Whether the reader is outside every sequence, so nothing read so far is
    /// waiting on a byte still to come.
    pub(crate) fn between(&self) -> bool {
        self.state == State::Ground
    }

    /// Reads `byte`, and says what it ended, if anything.
    pub(crate) fn feed(&mut self, byte: u8) -> Option<Shape> {
        match self.state {
            State::Ground if byte == ESC => self.enter(State::Escape),
            State::Ground => Some(Shape::Typed),
            State::Escape => self.escaped(byte),
            State::EscapeIntermediate => match byte {
                0x20..=0x2f => None,
                0x30..=0x7e => Some(self.ended(Shape::Other)),
                _ => Some(self.cut(byte)),
            },
            State::Shift if byte == ESC || cancels(byte) => Some(self.cut(byte)),
            State::Shift => Some(self.ended(Shape::Other)),
            State::Control => self.control(byte),
            State::String(kind) => self.string(kind, byte),
            State::StringEscape(kind) if byte == b'\\' => {
                let shape = self.read(kind);
                Some(self.ended(shape))
            }
            // The escape ended the string, and begins whatever comes next.
            State::StringEscape(_) => {
                self.clear();
                self.state = State::Escape;
                let next = self.escaped(byte);
                // An escape and one byte is a sequence ended as soon as it
                // began, so it is the same thing cut short as the string.
                if next.is_some() && self.state == State::Ground {
                    return next;
                }
                Some(Shape::Other)
            }
        }
    }

    /// After `ESC`.
    fn escaped(&mut self, byte: u8) -> Option<Shape> {
        match byte {
            b'[' => self.enter(State::Control),
            b']' => self.enter(State::String(Kind::Osc)),
            b'_' => self.enter(State::String(Kind::Apc)),
            b'P' | b'^' | b'X' => self.enter(State::String(Kind::Data)),
            b'O' => self.enter(State::Shift),
            0x20..=0x2f => self.enter(State::EscapeIntermediate),
            // A lone escape: the next one may begin a sequence of its own.
            ESC => Some(Shape::Other),
            _ => Some(self.ended(Shape::Other)),
        }
    }

    /// Inside `CSI`.
    fn control(&mut self, byte: u8) -> Option<Shape> {
        match byte {
            0x20..=0x3f => {
                self.hold(byte);
                None
            }
            0x40..=0x7e => {
                let shape = self.answered(byte);
                Some(self.ended(shape))
            }
            ESC | 0x18 | 0x1a | 0x80..=0xff => Some(self.cut(byte)),
            // Any other control byte is carried out where it stands, and is
            // no part of the sequence.
            _ => None,
        }
    }

    /// Inside a string.
    fn string(&mut self, kind: Kind, byte: u8) -> Option<Shape> {
        match byte {
            ESC => {
                self.state = State::StringEscape(kind);
                None
            }
            BEL if kind == Kind::Osc => {
                let shape = self.read(kind);
                Some(self.ended(shape))
            }
            0x18 | 0x1a => Some(self.cut(byte)),
            _ => {
                self.hold(byte);
                None
            }
        }
    }

    /// A sequence cut short by `byte`: an escape begins the next one, and
    /// anything else ends it.
    fn cut(&mut self, byte: u8) -> Shape {
        self.clear();
        self.state = if byte == ESC {
            State::Escape
        } else {
            State::Ground
        };
        Shape::Other
    }

    fn enter(&mut self, state: State) -> Option<Shape> {
        self.clear();
        self.state = state;
        None
    }

    fn ended(&mut self, shape: Shape) -> Shape {
        self.clear();
        self.state = State::Ground;
        shape
    }

    fn clear(&mut self) {
        self.held.clear();
        self.spilled = false;
    }

    fn hold(&mut self, byte: u8) {
        if self.held.len() < LONGEST {
            self.held.push(byte);
        } else {
            self.spilled = true;
        }
    }

    /// What the `CSI` sequence held, ended by `last`, was.
    fn answered(&self, last: u8) -> Shape {
        if self.spilled {
            return Shape::Other;
        }
        let held = self.held.as_slice();
        let numbers = |text: &[u8]| !text.is_empty() && text.iter().all(u8::is_ascii_digit);
        match (last, held.split_first()) {
            (b'R', _) => match held.iter().position(|byte| *byte == b';') {
                Some(at)
                    if numbers(held.get(..at).unwrap_or_default())
                        && numbers(held.get(at + 1..).unwrap_or_default()) =>
                {
                    Shape::CursorPosition
                }
                _ => Shape::Other,
            },
            (b'c', Some((b'?', rest))) if rest.split(|byte| *byte == b';').all(numbers) => {
                Shape::Attributes {
                    sixel: rest.split(|byte| *byte == b';').any(|part| part == b"4"),
                }
            }
            _ => Shape::Other,
        }
    }

    /// What the string held was.
    fn read(&self, kind: Kind) -> Shape {
        if self.spilled {
            return Shape::Other;
        }
        let held = self.held.as_slice();
        match kind {
            Kind::Osc if held.starts_with(b"1337;ReportCellSize=") => Shape::CellSize,
            Kind::Apc => match held.strip_prefix(b"G") {
                Some(body) => {
                    let (keys, message) = match body.iter().position(|byte| *byte == b';') {
                        Some(at) => (
                            body.get(..at).unwrap_or_default(),
                            body.get(at + 1..).unwrap_or_default(),
                        ),
                        None => (body, &b""[..]),
                    };
                    if keys
                        .split(|byte| *byte == b',')
                        .any(|key| key == QUERY_IMAGE.as_bytes())
                    {
                        Shape::Graphics {
                            ok: message == b"OK",
                        }
                    } else {
                        Shape::Other
                    }
                }
                None => Shape::Other,
            },
            Kind::Osc | Kind::Data => Shape::Other,
        }
    }
}

#[cfg(unix)]
const ESC: u8 = 0x1b;
#[cfg(unix)]
const BEL: u8 = 0x07;

/// `CAN` and `SUB`, which call off any sequence they arrive inside.
#[cfg(unix)]
const fn cancels(byte: u8) -> bool {
    matches!(byte, 0x18 | 0x1a)
}

/// What the session draws pictures with, settled from the candidate [`select`]
/// named: text for none; a forced protocol as it is, fitted to the window's
/// cell or to [`Cell::FALLBACK`] where the window reports no pixels; and a
/// candidate only where the window reports its pixels and the terminal answers
/// the exchange with the candidate's yes.
#[must_use]
pub fn confirm(candidate: Candidate, deadline: Deadline) -> Capability {
    confirmed(candidate, window_cell(), |protocol| {
        asked(protocol, deadline)
    })
}

/// The window's cell, from the pixel fields of its window-size report. Asks
/// the terminal nothing; read again after a resize.
#[must_use]
pub fn window_cell() -> Option<Cell> {
    crossterm::terminal::window_size()
        .ok()
        .and_then(|size| cell_of(&size))
}

/// The cell a window-size report gives.
fn cell_of(size: &crossterm::terminal::WindowSize) -> Option<Cell> {
    Cell::measured(size.columns, size.rows, size.width, size.height)
}

/// [`confirm`]'s rules, with the window's cell and the exchange passed in.
fn confirmed(
    candidate: Candidate,
    cell: Option<Cell>,
    ask: impl FnOnce(Protocol) -> bool,
) -> Capability {
    match candidate {
        Candidate::None => Capability::Text,
        Candidate::Forced(protocol) => Capability::Graphics {
            protocol,
            cell: cell.unwrap_or(Cell::FALLBACK),
        },
        Candidate::Asked(protocol) => match cell {
            Some(cell) if ask(protocol) => Capability::Graphics { protocol, cell },
            _ => Capability::Text,
        },
    }
}

/// Whether the terminal this process runs in says yes to `protocol`.
#[cfg(unix)]
fn asked(protocol: Protocol, deadline: Deadline) -> bool {
    terminal_trx::terminal()
        .is_ok_and(|mut terminal| exchanged(terminal.lock(), protocol, deadline))
}

/// Windows draws text this release, so its terminal is asked nothing.
#[cfg(not(unix))]
const fn asked(_: Protocol, _: Deadline) -> bool {
    false
}

/// Bytes one exchange reads at most, whatever arrives: far more than the
/// replies it asks for and a late answer to the background question come to,
/// and few enough that a terminal sending without end is left at once.
#[cfg(unix)]
const HEARD: usize = 16 * 1024;

/// The cursor-position question, which every terminal answers.
#[cfg(unix)]
const CURSOR: &[u8] = b"\x1b[6n";

/// Each protocol's own question: Kitty's query of a one-pixel image named by
/// [`QUERY_IMAGE`], iTerm2's request for its cell size, and the primary device
/// attributes, which list Sixel where it is drawn.
#[cfg(unix)]
const fn question(protocol: Protocol) -> &'static [u8] {
    match protocol {
        Protocol::Kitty => b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\",
        Protocol::Iterm2 => b"\x1b]1337;ReportCellSize\x07",
        Protocol::Sixel => b"\x1b[c",
    }
}

/// Whether `shape` is `protocol`'s yes.
#[cfg(unix)]
const fn says_yes(protocol: Protocol, shape: Shape) -> bool {
    matches!(
        (protocol, shape),
        (Protocol::Kitty, Shape::Graphics { ok: true })
            | (Protocol::Iterm2, Shape::CellSize)
            | (Protocol::Sixel, Shape::Attributes { sixel: true })
    )
}

/// The exchange, on the terminal `io`: the cursor position alone within the
/// patience, then, only if that was answered, `protocol`'s question and the
/// cursor position again within the reply wait. A yes counts only ahead of
/// that cursor position; one behind it is read and consumed for the patience
/// again. Any failure of the terminal is a no.
#[cfg(unix)]
fn exchanged<T: AsFd + io::Read + io::Write>(
    io: T,
    protocol: Protocol,
    deadline: Deadline,
) -> bool {
    let Ok(Some(mut mode)) = Mode::enter(io) else {
        return false;
    };
    let Ok(mut ear) = Ear::new(&mode.io) else {
        return false;
    };

    let started = Instant::now();
    if say(&mut mode.io, CURSOR).is_err() {
        return false;
    }
    let answered = ear.listen(
        &mut mode.io,
        started + deadline.patience,
        started + deadline.reply,
        |shape| shape == Shape::CursorPosition,
    );
    if answered != Heard::Decided {
        return false;
    }

    let asked = Instant::now();
    if say(&mut mode.io, &[question(protocol), CURSOR].concat()).is_err() {
        return false;
    }
    let mut placed = false;
    let mut yes = false;
    let heard = ear.listen(
        &mut mode.io,
        asked + deadline.reply,
        asked + deadline.reply + deadline.reply,
        |shape| {
            if shape == Shape::CursorPosition {
                placed = true;
            } else if !placed && says_yes(protocol, shape) {
                yes = true;
            }
            placed
        },
    );
    if heard == Heard::Decided && !yes {
        let behind = Instant::now();
        ear.listen(
            &mut mode.io,
            behind + deadline.patience,
            behind + deadline.reply,
            |shape| says_yes(protocol, shape),
        );
    }
    yes && heard != Heard::Spent
}

/// Writes a question whole.
#[cfg(unix)]
fn say(io: &mut impl io::Write, bytes: &[u8]) -> io::Result<()> {
    io.write_all(bytes)?;
    io.flush()
}

/// The terminal mode the exchange reads in: raw, so a reply arrives a byte at
/// a time and nothing is echoed, and with no minimum and no timer, so a read of
/// an empty queue returns at once. What was there before is put back from
/// `Drop`, on every return; where `io` is not a terminal, nothing is set.
#[cfg(unix)]
struct Mode<T: AsFd> {
    io: T,
    before: rustix::termios::Termios,
}

#[cfg(unix)]
impl<T: AsFd> Mode<T> {
    fn enter(io: T) -> io::Result<Option<Self>> {
        use rustix::termios::{OptionalActions, SpecialCodeIndex};

        if !rustix::termios::isatty(&io) {
            return Ok(None);
        }
        let before = rustix::termios::tcgetattr(&io)?;
        let mut raw = before.clone();
        raw.make_raw();
        raw.special_codes[SpecialCodeIndex::VMIN] = 0;
        raw.special_codes[SpecialCodeIndex::VTIME] = 0;
        rustix::termios::tcsetattr(&io, OptionalActions::Now, &raw)?;
        Ok(Some(Self { io, before }))
    }
}

#[cfg(unix)]
impl<T: AsFd> Drop for Mode<T> {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(
            &self.io,
            rustix::termios::OptionalActions::Now,
            &self.before,
        );
    }
}

/// How a wait for replies ended.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heard {
    /// What was waited for arrived, and nothing is left half read.
    Decided,
    /// The wait ran out between sequences.
    Lapsed,
    /// The exchange is over: the ceiling on bytes or on time was reached, or
    /// the terminal failed.
    Spent,
}

/// What the exchange reads replies with: a readiness wait on the terminal and
/// the reader that tells each reply by its shape, kept across the questions so
/// a sequence split between them is read whole.
#[cfg(unix)]
struct Ear {
    poll: mio::Poll,
    events: mio::Events,
    replies: Replies,
    heard: usize,
}

#[cfg(unix)]
impl Ear {
    fn new(io: &impl AsFd) -> io::Result<Self> {
        use std::os::fd::AsRawFd as _;

        let poll = mio::Poll::new()?;
        poll.registry().register(
            &mut mio::unix::SourceFd(&io.as_fd().as_raw_fd()),
            mio::Token(0),
            mio::Interest::READABLE,
        )?;
        Ok(Self {
            poll,
            events: mio::Events::with_capacity(4),
            replies: Replies::new(),
            heard: 0,
        })
    }

    /// Reads every byte that arrives until `decide` has said yes to a shape,
    /// or `until` has passed; a sequence still arriving then is read to its
    /// end, until `ceiling` at the latest.
    fn listen(
        &mut self,
        io: &mut impl io::Read,
        until: Instant,
        ceiling: Instant,
        mut decide: impl FnMut(Shape) -> bool,
    ) -> Heard {
        let mut decided = false;
        let mut chunk = [0_u8; 512];
        loop {
            // A wakeup is for what is new, so what has arrived is read until
            // the queue is empty.
            loop {
                match io.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        for byte in chunk.get(..read).unwrap_or_default() {
                            if let Some(shape) = self.replies.feed(*byte) {
                                decided |= decide(shape);
                            }
                        }
                        self.heard = self.heard.saturating_add(read);
                        if self.heard >= HEARD {
                            return Heard::Spent;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => return Heard::Spent,
                }
            }
            let between = self.replies.between();
            if decided && between {
                return Heard::Decided;
            }
            let limit = if between { until } else { ceiling };
            let Some(left) = limit.checked_duration_since(Instant::now()) else {
                return if between { Heard::Lapsed } else { Heard::Spent };
            };
            match self.poll.poll(&mut self.events, Some(left)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Heard::Spent,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment holding exactly these variables.
    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let held: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();

        move |wanted| {
            held.iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.clone())
        }
    }

    /// A terminal at both ends, colour on, in `screen` mode, wanting `images`.
    fn wanting(images: Wanted, screen: ScreenMode) -> Setting {
        Setting {
            images,
            screen,
            colour: true,
            terminal: true,
            passthrough: Passthrough::Unknown,
        }
    }

    const KITTY: &[(&str, &str)] = &[("TERM", "xterm-kitty"), ("KITTY_WINDOW_ID", "1")];
    const WEZTERM: &[(&str, &str)] = &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WezTerm")];
    const TMUX_IN_KITTY: &[(&str, &str)] = &[
        ("TERM", "tmux-256color"),
        ("TERM_PROGRAM", "tmux"),
        ("TMUX", "/tmp/tmux-1000/default,1234,0"),
        ("KITTY_WINDOW_ID", "1"),
    ];
    const TMUX_IN_WEZTERM: &[(&str, &str)] = &[
        ("TERM", "tmux-256color"),
        ("TERM_PROGRAM", "tmux"),
        ("TMUX", "/tmp/tmux-1000/default,1234,0"),
        ("LC_TERMINAL", "WezTerm"),
    ];
    const SSH_FROM_KITTY: &[(&str, &str)] = &[("TERM", "xterm-kitty"), ("SSH_TTY", "/dev/pts/3")];
    const SSH_FROM_WEZTERM: &[(&str, &str)] = &[
        ("TERM", "xterm-256color"),
        ("SSH_TTY", "/dev/pts/3"),
        ("LC_TERMINAL", "WezTerm"),
    ];

    /// Variables and their values, as a test spells an environment.
    type Pairs = &'static [(&'static str, &'static str)];

    /// Every place the table has a row for, with whether tmux passes through.
    fn places() -> Vec<(&'static str, Pairs, Passthrough)> {
        vec![
            ("R1", KITTY, Passthrough::Unknown),
            ("R2", WEZTERM, Passthrough::Unknown),
            ("R3", &[("TERM", "foot")], Passthrough::Unknown),
            ("R4K", TMUX_IN_KITTY, Passthrough::On),
            ("R4W", TMUX_IN_WEZTERM, Passthrough::On),
            ("R5K", TMUX_IN_KITTY, Passthrough::Off),
            ("R5W", TMUX_IN_WEZTERM, Passthrough::Off),
            ("R6K", SSH_FROM_KITTY, Passthrough::Unknown),
            ("R6W", SSH_FROM_WEZTERM, Passthrough::Unknown),
            ("RX", &[("TERM", "xterm")], Passthrough::Unknown),
            (
                "VC",
                &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")],
                Passthrough::Unknown,
            ),
        ]
    }

    const MODES: [(ScreenMode, &str); 2] = [
        (ScreenMode::Fullscreen, "fullscreen"),
        (ScreenMode::Native, "native"),
    ];
    const PROTOCOLS: [(Protocol, &str); 3] = [
        (Protocol::Kitty, "kitty"),
        (Protocol::Iterm2, "iterm2"),
        (Protocol::Sixel, "sixel"),
    ];

    /// The cells the support table enabled; every other one is disabled.
    const ENABLED: [&str; 6] = [
        "R1-fullscreen-kitty",
        "R1-native-kitty",
        "R2-fullscreen-iterm2",
        "R4K-native-kitty",
        "R4W-fullscreen-iterm2",
        "R6W-fullscreen-iterm2",
    ];

    #[test]
    fn the_support_table_enables_exactly_the_six_cells_that_drew_right() {
        let mut enabled = Vec::new();
        for (row, pairs, passthrough) in places() {
            for (mode, mode_name) in MODES {
                for (protocol, protocol_name) in PROTOCOLS {
                    let env = environment(pairs);
                    if support(&env, Platform::Linux, mode, protocol, passthrough)
                        == Support::Enabled
                    {
                        enabled.push(format!("{row}-{mode_name}-{protocol_name}"));
                    }
                }
            }
        }
        assert_eq!(enabled, ENABLED);
    }

    #[test]
    fn windows_freebsd_and_any_other_system_draw_text_even_when_a_protocol_is_forced() {
        for platform in [Platform::Windows, Platform::FreeBsd, Platform::Other] {
            for (mode, _) in MODES {
                for (protocol, _) in PROTOCOLS {
                    let env = environment(KITTY);
                    assert_eq!(
                        select(&env, platform, wanting(Wanted::Forced(protocol), mode)),
                        Candidate::None,
                        "{platform:?} {mode:?} {protocol:?}"
                    );
                    assert_eq!(
                        select(&env, platform, wanting(Wanted::Auto, mode)),
                        Candidate::None
                    );
                }
            }
        }
        // The same environment on a system the table covers does draw.
        let env = environment(KITTY);
        assert_eq!(
            select(
                &env,
                Platform::Macos,
                wanting(Wanted::Forced(Protocol::Kitty), ScreenMode::Native)
            ),
            Candidate::Forced(Protocol::Kitty)
        );
    }

    #[test]
    fn a_protocol_forced_over_a_disabled_cell_draws_text() {
        for (row, pairs, passthrough) in places() {
            for (mode, mode_name) in MODES {
                for (protocol, protocol_name) in PROTOCOLS {
                    let env = environment(pairs);
                    let mut setting = wanting(Wanted::Forced(protocol), mode);
                    setting.passthrough = passthrough;
                    let cell = format!("{row}-{mode_name}-{protocol_name}");
                    let expected = if ENABLED.contains(&cell.as_str()) {
                        Candidate::Forced(protocol)
                    } else {
                        Candidate::None
                    };
                    assert_eq!(select(&env, Platform::Linux, setting), expected, "{cell}");
                }
            }
        }
    }

    #[test]
    fn auto_asks_only_where_the_environment_names_an_enabled_candidate() {
        let asked = |pairs: &[(&str, &str)], mode| {
            select(
                &environment(pairs),
                Platform::Linux,
                wanting(Wanted::Auto, mode),
            )
        };
        assert_eq!(
            asked(KITTY, ScreenMode::Native),
            Candidate::Asked(Protocol::Kitty)
        );
        assert_eq!(
            asked(KITTY, ScreenMode::Fullscreen),
            Candidate::Asked(Protocol::Kitty)
        );
        assert_eq!(
            asked(WEZTERM, ScreenMode::Fullscreen),
            Candidate::Asked(Protocol::Iterm2)
        );
        assert_eq!(
            asked(SSH_FROM_WEZTERM, ScreenMode::Fullscreen),
            Candidate::Asked(Protocol::Iterm2)
        );
        // Seen to lose a picture's top after a resize in native mode.
        assert_eq!(asked(WEZTERM, ScreenMode::Native), Candidate::None);
        // Over SSH, Kitty did not answer in time.
        assert_eq!(asked(SSH_FROM_KITTY, ScreenMode::Native), Candidate::None);
        // Ghostty and iTerm2 are named by the same rules as Kitty and WezTerm.
        assert_eq!(
            asked(&[("TERM", "xterm-ghostty")], ScreenMode::Native),
            Candidate::Asked(Protocol::Kitty)
        );
        assert_eq!(
            asked(&[("TERM_PROGRAM", "ghostty")], ScreenMode::Native),
            Candidate::Asked(Protocol::Kitty)
        );
        assert_eq!(
            asked(&[("TERM_PROGRAM", "iTerm.app")], ScreenMode::Fullscreen),
            Candidate::Asked(Protocol::Iterm2)
        );
        // Nothing names these.
        assert_eq!(
            asked(&[("TERM", "xterm-256color")], ScreenMode::Fullscreen),
            Candidate::None
        );
        assert_eq!(
            asked(&[("TERM", "foot")], ScreenMode::Fullscreen),
            Candidate::None
        );
        assert_eq!(asked(&[], ScreenMode::Fullscreen), Candidate::None);
    }

    #[test]
    fn auto_names_nothing_inside_a_multiplexer_whatever_tmux_would_pass() {
        let inside: [&[(&str, &str)]; 5] = [
            TMUX_IN_KITTY,
            &[("KITTY_WINDOW_ID", "1"), ("STY", "1234.pts-0.host")],
            &[("KITTY_WINDOW_ID", "1"), ("ZELLIJ", "0")],
            &[("KITTY_WINDOW_ID", "1"), ("TERM", "screen-256color")],
            &[("KITTY_WINDOW_ID", "1"), ("TERM", "tmux-256color")],
        ];
        for pairs in inside {
            for passthrough in [Passthrough::On, Passthrough::Off, Passthrough::Unknown] {
                let mut setting = wanting(Wanted::Auto, ScreenMode::Native);
                setting.passthrough = passthrough;
                assert_eq!(
                    select(&environment(pairs), Platform::Linux, setting),
                    Candidate::None,
                    "{pairs:?} {passthrough:?}"
                );
            }
        }
    }

    #[test]
    fn a_multiplexer_reached_over_ssh_has_no_row_so_draws_text() {
        let env = environment(&[
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("KITTY_WINDOW_ID", "1"),
            ("SSH_CONNECTION", "10.0.0.1 22 10.0.0.2 50000"),
        ]);
        let mut setting = wanting(Wanted::Forced(Protocol::Kitty), ScreenMode::Native);
        setting.passthrough = Passthrough::On;
        assert_eq!(select(&env, Platform::Linux, setting), Candidate::None);
    }

    #[test]
    fn off_draws_text_wherever_a_picture_would_draw() {
        for (mode, _) in MODES {
            assert_eq!(
                select(
                    &environment(KITTY),
                    Platform::Linux,
                    wanting(Wanted::Off, mode)
                ),
                Candidate::None
            );
        }
    }

    #[test]
    fn the_four_places_nothing_may_be_asked_name_no_candidate_whatever_is_wanted() {
        let quiet = |setting: Setting, pairs: &[(&str, &str)]| {
            select(&environment(pairs), Platform::Linux, setting)
        };
        let with_no_colour: Vec<(&str, &str)> =
            KITTY.iter().copied().chain([("NO_COLOR", "1")]).collect();
        for images in [Wanted::Auto, Wanted::Forced(Protocol::Kitty)] {
            let usual = wanting(images, ScreenMode::Native);
            // Output redirected, or input not a terminal: both are `terminal`.
            assert_eq!(
                quiet(
                    Setting {
                        terminal: false,
                        ..usual
                    },
                    KITTY
                ),
                Candidate::None
            );
            assert_eq!(quiet(usual, &with_no_colour), Candidate::None);
            assert_eq!(
                quiet(
                    Setting {
                        colour: false,
                        ..usual
                    },
                    KITTY
                ),
                Candidate::None
            );
            // And the same session with none of them does ask.
            assert_ne!(quiet(usual, KITTY), Candidate::None);
        }
        // An empty `NO_COLOR` is not set, as the convention says.
        let empty: Vec<(&str, &str)> = KITTY.iter().copied().chain([("NO_COLOR", "")]).collect();
        assert_eq!(
            quiet(wanting(Wanted::Auto, ScreenMode::Native), &empty),
            Candidate::Asked(Protocol::Kitty)
        );
    }

    #[test]
    fn tmux_passes_a_forced_protocol_only_where_its_server_said_on() {
        for (passthrough, expected) in [
            (Passthrough::On, Candidate::Forced(Protocol::Kitty)),
            (Passthrough::Off, Candidate::None),
            (Passthrough::Unknown, Candidate::None),
        ] {
            let mut setting = wanting(Wanted::Forced(Protocol::Kitty), ScreenMode::Native);
            setting.passthrough = passthrough;
            assert_eq!(
                select(&environment(TMUX_IN_KITTY), Platform::Linux, setting),
                expected,
                "{passthrough:?}"
            );
        }
        // `screen` and Zellij have no passthrough row at all.
        for pairs in [
            &[("KITTY_WINDOW_ID", "1"), ("STY", "1.pts-0.host")][..],
            &[("KITTY_WINDOW_ID", "1"), ("ZELLIJ", "0")][..],
        ] {
            let mut setting = wanting(Wanted::Forced(Protocol::Kitty), ScreenMode::Native);
            setting.passthrough = Passthrough::On;
            assert_eq!(
                select(&environment(pairs), Platform::Linux, setting),
                Candidate::None
            );
        }
    }

    #[test]
    fn the_outer_terminal_is_named_only_by_the_variables_the_table_names() {
        // WezTerm inside tmux is known only by what it leaves behind that the
        // table read; its pane variable is not one of them.
        let env = environment(&[
            ("TERM", "tmux-256color"),
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("WEZTERM_PANE", "0"),
        ]);
        let mut setting = wanting(Wanted::Forced(Protocol::Iterm2), ScreenMode::Fullscreen);
        setting.passthrough = Passthrough::On;
        assert_eq!(select(&env, Platform::Linux, setting), Candidate::None);

        // A forwarded `TERM_PROGRAM` counts as much as an inherited one.
        let env = environment(&[
            ("SSH_CLIENT", "10.0.0.1 50000 22"),
            ("TERM_PROGRAM", "WezTerm"),
        ]);
        assert_eq!(
            select(
                &env,
                Platform::Linux,
                wanting(Wanted::Auto, ScreenMode::Fullscreen)
            ),
            Candidate::Asked(Protocol::Iterm2)
        );
    }

    #[test]
    fn a_window_reporting_its_pixels_gives_its_cell_and_a_nought_gives_none() {
        assert_eq!(
            Cell::measured(80, 24, 720, 432),
            Some(Cell {
                width: 9,
                height: 18
            })
        );
        assert_eq!(
            Cell::measured(96, 29, 960, 640),
            Some(Cell {
                width: 10,
                height: 22
            })
        );
        assert_eq!(Cell::measured(80, 24, 0, 0), None);
        assert_eq!(Cell::measured(80, 24, 720, 0), None);
        assert_eq!(Cell::measured(0, 0, 720, 432), None);
        // Fewer pixels than cells is no cell either.
        assert_eq!(Cell::measured(80, 24, 40, 432), None);
    }

    #[test]
    fn a_payload_covers_every_cell_its_picture_reaches_into() {
        let payload = Payload::new(Protocol::Kitty, vec![1, 2, 3], (100, 40), Cell::FALLBACK);
        assert_eq!(payload.cells(), (13, 3));
        assert_eq!(payload.protocol(), Protocol::Kitty);
        assert_eq!(payload.bytes(), &[1, 2, 3]);
        let exact = Payload::new(Protocol::Iterm2, Vec::new(), (80, 32), Cell::FALLBACK);
        assert_eq!(exact.cells(), (10, 2));
        // A picture too big to count in cells is held to the most there are.
        let huge = Payload::new(
            Protocol::Sixel,
            Vec::new(),
            (u32::MAX, u32::MAX),
            Cell {
                width: 1,
                height: 1,
            },
        );
        assert_eq!(huge.cells(), (u16::MAX, u16::MAX));
        // Its bytes stay out of a log.
        assert!(!format!("{payload:?}").contains("[1, 2, 3]"));
    }

    // -----------------------------------------------------------------------
    // The reply parser: a hostile corpus, then generated input.

    #[cfg(unix)]
    mod replies {
        use super::*;

        /// Every shape `bytes` makes, and the reader after them.
        fn shapes(bytes: &[u8]) -> (Vec<Shape>, Replies) {
            let mut replies = Replies::new();
            let shapes = bytes
                .iter()
                .filter_map(|byte| replies.feed(*byte))
                .collect();
            (shapes, replies)
        }

        /// Every shape `bytes` makes, which must end outside every sequence.
        fn complete(bytes: &[u8]) -> Vec<Shape> {
            let (shapes, replies) = shapes(bytes);
            assert!(replies.between(), "{bytes:?} left a sequence open");
            shapes
        }

        const OK: Shape = Shape::Graphics { ok: true };

        #[test]
        fn each_reply_the_exchange_asks_for_is_known_by_its_shape() {
            assert_eq!(complete(b"\x1b[12;40R"), [Shape::CursorPosition]);
            assert_eq!(complete(b"\x1b[1;1R"), [Shape::CursorPosition]);
            assert_eq!(complete(b"\x1b_Gi=31;OK\x1b\\"), [OK]);
            assert_eq!(complete(b"\x1b_Gi=31,q=0;OK\x1b\\"), [OK]);
            assert_eq!(
                complete(b"\x1b_Gi=31;ENOTSUPPORTED:no direct\x1b\\"),
                [Shape::Graphics { ok: false }]
            );
            assert_eq!(
                complete(b"\x1b]1337;ReportCellSize=22.0;10.0;1.0\x07"),
                [Shape::CellSize]
            );
            assert_eq!(
                complete(b"\x1b]1337;ReportCellSize=16;8\x1b\\"),
                [Shape::CellSize]
            );
            assert_eq!(
                complete(b"\x1b[?62;4;22c"),
                [Shape::Attributes { sixel: true }]
            );
            assert_eq!(complete(b"\x1b[?4c"), [Shape::Attributes { sixel: true }]);
            assert_eq!(
                complete(b"\x1b[?62;22c"),
                [Shape::Attributes { sixel: false }]
            );
            assert_eq!(
                complete(b"\x1b[?64;44c"),
                [Shape::Attributes { sixel: false }]
            );
            assert_eq!(complete(b"ab"), [Shape::Typed, Shape::Typed]);
        }

        #[test]
        fn a_reply_nobody_asked_for_is_consumed_and_named_nothing() {
            // The background colour, answered after its own wait gave up.
            assert_eq!(
                complete(b"\x1b]11;rgb:1c1c/1c1c/1c1c\x1b\\"),
                [Shape::Other]
            );
            assert_eq!(complete(b"\x1b]11;rgb:1c1c/1c1c/1c1c\x07"), [Shape::Other]);
            // Another image's reply, and Kitty's without its image.
            assert_eq!(complete(b"\x1b_Gi=310;OK\x1b\\"), [Shape::Other]);
            assert_eq!(complete(b"\x1b_Gi=3;OK\x1b\\"), [Shape::Other]);
            assert_eq!(complete(b"\x1b_G;OK\x1b\\"), [Shape::Other]);
            assert_eq!(complete(b"\x1b_Xi=31;OK\x1b\\"), [Shape::Other]);
            // Shaped almost like a cursor position.
            assert_eq!(complete(b"\x1b[?12;1R"), [Shape::Other]);
            assert_eq!(complete(b"\x1b[R"), [Shape::Other]);
            assert_eq!(complete(b"\x1b[12R"), [Shape::Other]);
            assert_eq!(complete(b"\x1b[1;2;3R"), [Shape::Other]);
            assert_eq!(complete(b"\x1b[;1R"), [Shape::Other]);
            assert_eq!(complete(b"\x1b[62;4c"), [Shape::Other]);
            assert_eq!(
                complete(b"\x1b]1337;ReportVariable=abc\x07"),
                [Shape::Other]
            );
            // A device control string is ended by `ST` alone.
            assert_eq!(complete(b"\x1bP1$r0m\x1b\\"), [Shape::Other]);
            assert_eq!(complete(b"\x1bPab\x07cd\x1b\\"), [Shape::Other]);
            assert_eq!(
                complete(b"\x1b^private\x1b\\\x1bXstart\x1b\\"),
                [Shape::Other, Shape::Other]
            );
            // Keys, as a terminal spells them.
            assert_eq!(complete(b"\x1ba"), [Shape::Other]);
            assert_eq!(complete(b"\x1bOP"), [Shape::Other]);
            assert_eq!(complete(b"\x1b(B"), [Shape::Other]);
            assert_eq!(complete(b"\x1b[1;5A"), [Shape::Other]);
        }

        #[test]
        fn an_escape_inside_a_sequence_ends_it_and_begins_the_next() {
            assert_eq!(
                complete(b"\x1b]11;rgb:1c1c\x1b[12;1R"),
                [Shape::Other, Shape::CursorPosition]
            );
            assert_eq!(
                complete(b"\x1b[12;\x1b[3;4R"),
                [Shape::Other, Shape::CursorPosition]
            );
            assert_eq!(
                complete(b"\x1b_Gi=31;OK\x1b_Gi=31;OK\x1b\\"),
                [Shape::Other, OK]
            );
            // An escape in a string followed by a key is one thing cut short.
            assert_eq!(complete(b"\x1b]abc\x1ba"), [Shape::Other]);
            assert_eq!(
                complete(b"\x1b]abc\x1b\x1b[1;1R"),
                [Shape::Other, Shape::CursorPosition]
            );
        }

        #[test]
        fn a_cancel_ends_any_sequence_and_what_follows_is_typed() {
            for open in [
                &b"\x1b"[..],
                b"\x1b[12",
                b"\x1b]11;rgb",
                b"\x1b_Gi=31",
                b"\x1bP1$",
                b"\x1b(",
                b"\x1bO",
                b"\x1b]x\x1b",
            ] {
                for cancel in [0x18_u8, 0x1a] {
                    let mut bytes = open.to_vec();
                    bytes.push(cancel);
                    bytes.push(b'a');
                    assert_eq!(complete(&bytes), [Shape::Other, Shape::Typed], "{bytes:?}");
                }
            }
        }

        #[test]
        fn bytes_no_sequence_may_hold_end_it() {
            assert_eq!(complete(b"\x1b[1\xffa"), [Shape::Other, Shape::Typed]);
            // A control byte inside a sequence is carried out where it stands.
            assert_eq!(complete(b"\x1b[1\x00;2R"), [Shape::CursorPosition]);
            assert_eq!(complete(b"\x1b[1;2\x7fR"), [Shape::CursorPosition]);
            // Not UTF-8, outside a sequence: typed, byte by byte.
            assert_eq!(complete(b"\xc3\x28"), [Shape::Typed, Shape::Typed]);
        }

        #[test]
        fn a_sequence_still_arriving_is_not_between() {
            for open in [
                &b"\x1b"[..],
                b"\x1b[",
                b"\x1b[12;",
                b"\x1b]11;",
                b"\x1b]11;x\x1b",
                b"\x1b_G",
                b"\x1bO",
                b"\x1b(",
            ] {
                let (shapes, replies) = shapes(open);
                assert!(shapes.is_empty(), "{open:?}");
                assert!(!replies.between(), "{open:?}");
            }
            let (shapes, replies) = shapes(b"\x1b\x1b");
            assert_eq!(shapes, [Shape::Other]);
            assert!(!replies.between());
        }

        #[test]
        fn a_sequence_of_any_length_is_held_to_its_ceiling_and_read_to_its_end() {
            for (open, fill, close) in [
                (&b"\x1b]11;"[..], b'x', &b"\x07"[..]),
                (b"\x1b_Gi=31;", b'O', b"\x1b\\"),
                (b"\x1bP", b'q', b"\x1b\\"),
                (b"\x1b[", b'1', b"R"),
                (b"\x1b[?", b'4', b"c"),
                (b"\x1b(", b' ', b"B"),
            ] {
                let mut replies = Replies::new();
                let mut seen = Vec::new();
                let body = std::iter::repeat_n(fill, 100_000);
                for byte in open
                    .iter()
                    .copied()
                    .chain(body)
                    .chain(close.iter().copied())
                {
                    seen.extend(replies.feed(byte));
                    assert!(replies.held.len() <= LONGEST);
                }
                assert_eq!(seen, [Shape::Other], "{open:?}");
                assert!(replies.between());
            }
        }

        #[test]
        fn a_long_sequence_does_not_keep_its_spill_into_the_next() {
            let mut bytes = b"\x1b]".to_vec();
            bytes.extend(std::iter::repeat_n(b'x', 5000));
            bytes.extend(b"\x07\x1b[3;4R");
            assert_eq!(complete(&bytes), [Shape::Other, Shape::CursorPosition]);
            let mut bytes = b"\x1b[".to_vec();
            bytes.extend(std::iter::repeat_n(b'1', 5000));
            bytes.extend(b"\x1b_Gi=31;OK\x1b\\");
            assert_eq!(complete(&bytes), [Shape::Other, OK]);
        }

        // -----------------------------------------------------------------------
        // Generated replies: every case is made from the seed and its index
        // alone, so a failure names the case that reproduces it.

        const SEED: u64 = 0x5eed_7e9c_ca11;
        const DEFAULT_CASES: u64 = 4096;
        const CASES_VARIABLE: &str = "CRUCIBLE_GENERATED_INPUTS";

        fn cases() -> u64 {
            std::env::var(CASES_VARIABLE)
                .ok()
                .and_then(|text| text.parse().ok())
                .unwrap_or(DEFAULT_CASES)
        }

        #[derive(Debug, PartialEq, Eq)]
        enum Ended<T> {
            Returned(T),
            Panicked,
            Hung,
        }

        /// `work` on a thread of its own, given `deadline` to answer.
        fn guarded<T: Send + 'static>(
            deadline: Duration,
            work: impl FnOnce() -> T + Send + 'static,
        ) -> Ended<T> {
            let (answer, heard) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = answer.send(work());
            });
            match heard.recv_timeout(deadline) {
                Ok(value) => Ended::Returned(value),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ended::Hung,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Ended::Panicked,
            }
        }

        #[test]
        fn a_planted_panic_and_a_planted_hang_are_found_by_the_generated_runner() {
            assert_eq!(
                guarded(Duration::from_secs(5), || -> u8 { panic!("planted") }),
                Ended::Panicked
            );
            let begun = std::time::Instant::now();
            let ended = guarded(Duration::from_millis(200), move || {
                while begun.elapsed() < Duration::from_secs(3) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
            assert_eq!(ended, Ended::Hung);
        }

        /// xorshift64*: small, and the same on every platform.
        struct Rng(u64);

        impl Rng {
            fn for_case(index: u64) -> Self {
                let mut rng = Rng(SEED ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
                for _ in 0..4 {
                    rng.next();
                }
                rng
            }

            fn next(&mut self) -> u64 {
                self.0 ^= self.0 >> 12;
                self.0 ^= self.0 << 25;
                self.0 ^= self.0 >> 27;
                self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
            }

            fn below(&mut self, bound: usize) -> usize {
                let bound = u64::try_from(bound).unwrap_or(u64::MAX).max(1);
                usize::try_from((self.next() >> 11) % bound).unwrap_or(0)
            }

            fn number(&mut self) -> String {
                self.below(100_000).to_string()
            }
        }

        /// One whole reply, with the shape it must be read as.
        fn reply(rng: &mut Rng) -> (Vec<u8>, Shape) {
            let st = if rng.below(2) == 0 { "\x07" } else { "\x1b\\" };
            match rng.below(8) {
                0 => (
                    format!("\x1b[{};{}R", rng.number(), rng.number()).into_bytes(),
                    Shape::CursorPosition,
                ),
                1 => (b"\x1b_Gi=31;OK\x1b\\".to_vec(), OK),
                2 => (
                    format!("\x1b_Gi=31;E{}\x1b\\", rng.number()).into_bytes(),
                    Shape::Graphics { ok: false },
                ),
                3 => (
                    format!(
                        "\x1b]1337;ReportCellSize={};{}{st}",
                        rng.number(),
                        rng.number()
                    )
                    .into_bytes(),
                    Shape::CellSize,
                ),
                4 => (
                    format!("\x1b[?{};4;{}c", rng.number(), rng.number()).into_bytes(),
                    Shape::Attributes { sixel: true },
                ),
                5 => (
                    format!(
                        "\x1b]11;rgb:{}/{}/{}{st}",
                        rng.number(),
                        rng.number(),
                        rng.number()
                    )
                    .into_bytes(),
                    Shape::Other,
                ),
                6 => (
                    format!("\x1b[{};{}H", rng.number(), rng.number()).into_bytes(),
                    Shape::Other,
                ),
                _ => (
                    vec![b'a' + u8::try_from(rng.below(26)).unwrap_or(0)],
                    Shape::Typed,
                ),
            }
        }

        /// A run of bytes no terminal would send on purpose.
        fn noise(rng: &mut Rng) -> Vec<u8> {
            const PIECES: &[&[u8]] = &[
                b"\x1b",
                b"\x1b[",
                b"\x1b]",
                b"\x1b_G",
                b"\x1bP",
                b"\x1b^",
                b"\x1bX",
                b"\x1bO",
                b"\x1b(",
                b"\x18",
                b"\x1a",
                b"\x07",
                b"\x1b\\",
                b";",
                b"?",
                b"R",
                b"c",
                b"i=31",
                b"OK",
                b"1337;ReportCellSize=",
                b"\x00",
                b"\x7f",
                b"\x9b",
                b"\xff",
                b"\xc3",
            ];
            let mut bytes = Vec::new();
            for _ in 0..rng.below(64) {
                if rng.below(8) == 0 {
                    let long = rng.below(3000);
                    bytes.extend(std::iter::repeat_n(b'x', long));
                } else if rng.below(4) == 0 {
                    bytes.push(u8::try_from(rng.below(256)).unwrap_or(0));
                } else if let Some(piece) = PIECES.get(rng.below(PIECES.len())) {
                    bytes.extend_from_slice(piece);
                }
            }
            bytes
        }

        /// What case `index` must hold to, or why it did not.
        fn generated(index: u64) -> Result<(), String> {
            let mut rng = Rng::for_case(index);

            // Noise of any kind: read whole, held to the ceiling, and a cancel
            // always brings the reader back outside every sequence.
            let bytes = noise(&mut rng);
            let mut replies = Replies::new();
            let mut made = 0_usize;
            for byte in &bytes {
                made += usize::from(replies.feed(*byte).is_some());
                if replies.held.len() > LONGEST {
                    return Err(format!("held {} bytes", replies.held.len()));
                }
            }
            if made > bytes.len() {
                return Err(format!("{made} shapes from {} bytes", bytes.len()));
            }
            replies.feed(0x18);
            if !replies.between() {
                return Err(format!("a cancel after {bytes:?} left a sequence open"));
            }

            // Whole replies, one after another, are read back as exactly what
            // they were, after any noise a cancel ended.
            let mut stream = Vec::new();
            let mut expected = Vec::new();
            for _ in 0..rng.below(16) {
                let (bytes, shape) = reply(&mut rng);
                stream.extend(bytes);
                expected.push(shape);
            }
            let read: Vec<Shape> = stream
                .iter()
                .filter_map(|byte| replies.feed(*byte))
                .collect();
            if read != expected || !replies.between() {
                return Err(format!("{stream:?} read as {read:?}, not {expected:?}"));
            }
            Ok(())
        }

        #[test]
        fn generated_replies_and_noise_are_read_whole_and_bounded() {
            let total = cases();
            println!("seed {SEED:#x}, {total} inputs ({CASES_VARIABLE} raises the count)");
            for index in 0..total {
                match guarded(Duration::from_secs(30), move || generated(index)) {
                    Ended::Returned(Ok(())) => {}
                    Ended::Returned(Err(why)) => panic!("case {index} (seed {SEED:#x}): {why}"),
                    Ended::Panicked => panic!("case {index} (seed {SEED:#x}): panicked"),
                    Ended::Hung => panic!("case {index} (seed {SEED:#x}): no answer in 30 s"),
                }
            }
        }
    }

    #[test]
    fn a_forced_protocol_is_used_unasked_in_the_windows_cell_or_eight_by_sixteen() {
        let cell = Cell {
            width: 9,
            height: 18,
        };
        let unasked = |_: Protocol| -> bool { panic!("a forced protocol was asked about") };

        assert_eq!(
            confirmed(Candidate::Forced(Protocol::Kitty), Some(cell), unasked),
            Capability::Graphics {
                protocol: Protocol::Kitty,
                cell
            }
        );
        assert_eq!(
            confirmed(Candidate::Forced(Protocol::Sixel), None, unasked),
            Capability::Graphics {
                protocol: Protocol::Sixel,
                cell: Cell::FALLBACK
            }
        );
    }

    #[test]
    fn a_candidate_is_asked_about_only_where_the_window_reports_its_pixels() {
        let cell = Cell {
            width: 9,
            height: 18,
        };
        let unasked = |_: Protocol| -> bool { panic!("a window with no pixels was asked about") };

        assert_eq!(
            confirmed(Candidate::Asked(Protocol::Kitty), None, unasked),
            Capability::Text
        );
        assert_eq!(
            confirmed(
                Candidate::Asked(Protocol::Iterm2),
                Some(cell),
                |asked| asked == Protocol::Iterm2
            ),
            Capability::Graphics {
                protocol: Protocol::Iterm2,
                cell
            }
        );
        assert_eq!(
            confirmed(Candidate::Asked(Protocol::Kitty), Some(cell), |_| false),
            Capability::Text
        );
    }

    #[test]
    fn no_candidate_is_text_and_asks_nothing() {
        let cell = Cell {
            width: 9,
            height: 18,
        };
        assert_eq!(
            confirmed(Candidate::None, Some(cell), |_| panic!(
                "nothing was to be asked"
            )),
            Capability::Text
        );
    }

    #[test]
    fn the_cell_is_read_from_the_pixel_fields_of_the_window_size_report() {
        let report = |columns, rows, width, height| crossterm::terminal::WindowSize {
            rows,
            columns,
            width,
            height,
        };

        assert_eq!(
            cell_of(&report(80, 24, 800, 480)),
            Some(Cell {
                width: 10,
                height: 20
            })
        );
        assert_eq!(cell_of(&report(80, 24, 0, 0)), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_kitty_question_names_the_image_its_reply_is_known_by() {
        let asked = question(Protocol::Kitty);
        assert!(
            asked
                .windows(QUERY_IMAGE.len())
                .any(|part| part == QUERY_IMAGE.as_bytes()),
            "{asked:?}"
        );
    }

    /// The pseudo-terminal cases: each test owns a pty, plays the terminal on
    /// its master side, and runs the exchange on the other.
    #[cfg(unix)]
    mod exchange {
        use std::fs::File;
        use std::io::{self, Read as _, Write as _};
        use std::os::fd::AsFd;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, mpsc};
        use std::thread;
        use std::time::{Duration, Instant};

        use rustix::termios::{LocalModes, OptionalActions, SpecialCodeIndex};

        use super::super::{Deadline, Mode, Protocol, Replies, Shape, exchanged, question};

        /// A wait the cases can tell apart under a loaded machine: the
        /// patience is long enough to answer within, and the reply wait long
        /// enough past it to tell which one ran out.
        const WAIT: Deadline = Deadline {
            patience: Duration::from_millis(100),
            reply: Duration::from_millis(600),
        };

        const CURSOR: &[u8] = b"\x1b[6n";
        const CURSOR_AT: &[u8] = b"\x1b[12;40R";
        const KITTY_YES: &[u8] = b"\x1b_Gi=31;OK\x1b\\";
        const ITERM2_YES: &[u8] = b"\x1b]1337;ReportCellSize=17.0;8.0\x1b\\";
        const SIXEL_YES: &[u8] = b"\x1b[?62;4;22c";

        /// The longest any case waits for the played terminal.
        const GIVE_UP: Duration = Duration::from_secs(10);

        /// A pseudo-terminal pair, as `(terminal, process)`: the side the test
        /// answers on, and the side the exchange reads.
        fn pty() -> (File, File) {
            use std::os::unix::ffi::OsStrExt as _;
            use std::os::unix::fs::OpenOptionsExt as _;

            let master = rustix::pty::openpt(
                rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY,
            )
            .expect("a pty");
            rustix::pty::grantpt(&master).expect("the pty is granted");
            rustix::pty::unlockpt(&master).expect("the pty is unlocked");
            let name = rustix::pty::ptsname(&master, Vec::new()).expect("the pty's name");
            let slave = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOCTTY)
                .open(std::ffi::OsStr::from_bytes(name.as_bytes()))
                .expect("the pty's other side");
            (File::from(master), slave)
        }

        /// The mode the session's key reader finds what is left in: raw, and a
        /// read of an empty queue returning at once.
        fn as_the_session_reads(slave: &File) {
            let mut mode = rustix::termios::tcgetattr(slave).expect("the pty's mode");
            mode.make_raw();
            mode.special_codes[SpecialCodeIndex::VMIN] = 0;
            mode.special_codes[SpecialCodeIndex::VTIME] = 0;
            rustix::termios::tcsetattr(slave, OptionalActions::Now, &mode).expect("set");
        }

        /// One question the played terminal waits for, and its answers, each
        /// after its own pause in milliseconds.
        struct Turn {
            asked: Vec<u8>,
            answers: Vec<(u64, Vec<u8>)>,
        }

        fn turn(asked: &[u8], answers: &[(u64, &[u8])]) -> Turn {
            Turn {
                asked: asked.to_vec(),
                answers: answers
                    .iter()
                    .map(|(pause, bytes)| (*pause, bytes.to_vec()))
                    .collect(),
            }
        }

        /// The candidate's question as the exchange writes it.
        fn asking(protocol: Protocol) -> Vec<u8> {
            [question(protocol), CURSOR].concat()
        }

        /// Reads what is waiting on `file`, which is non-blocking.
        fn take(file: &mut File, into: &mut Vec<u8>) {
            let mut chunk = [0_u8; 4096];
            loop {
                match file.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(read) => into.extend_from_slice(chunk.get(..read).unwrap_or_default()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => return,
                }
            }
        }

        /// Writes all of `bytes`, waiting while the pty is full.
        fn tell(file: &mut File, mut bytes: &[u8], until: Instant) {
            while !bytes.is_empty() && Instant::now() < until {
                match file.write(bytes) {
                    Ok(written) => bytes = bytes.get(written..).unwrap_or_default(),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => panic!("the played terminal could not answer: {error}"),
                }
            }
        }

        /// Plays a terminal on `master`: waits for each turn's question in
        /// order and gives its answers, then hears on until the exchange is
        /// done. Sends back everything it heard.
        fn play(
            mut master: File,
            turns: Vec<Turn>,
            said: Arc<AtomicBool>,
            done: Arc<AtomicBool>,
        ) -> mpsc::Receiver<Vec<u8>> {
            let (send, heard) = mpsc::channel();
            thread::spawn(move || {
                rustix::fs::fcntl_setfl(&master, rustix::fs::OFlags::NONBLOCK)
                    .expect("the played side does not block");
                let until = Instant::now() + GIVE_UP;
                let mut heard = Vec::new();
                let mut from = 0;
                for turn in turns {
                    loop {
                        take(&mut master, &mut heard);
                        let found = heard
                            .get(from..)
                            .unwrap_or_default()
                            .windows(turn.asked.len())
                            .position(|part| part == turn.asked.as_slice());
                        if let Some(at) = found {
                            from += at + turn.asked.len();
                            break;
                        }
                        if Instant::now() > until {
                            said.store(true, Ordering::SeqCst);
                            let _ = send.send(heard);
                            return;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    for (pause, bytes) in turn.answers {
                        thread::sleep(Duration::from_millis(pause));
                        tell(&mut master, &bytes, until);
                    }
                }
                said.store(true, Ordering::SeqCst);
                while !done.load(Ordering::SeqCst) && Instant::now() < until {
                    take(&mut master, &mut heard);
                    thread::sleep(Duration::from_millis(5));
                }
                thread::sleep(Duration::from_millis(20));
                take(&mut master, &mut heard);
                let _ = send.send(heard);
            });
            heard
        }

        /// What the exchange left on `slave` for the key reader, read once the
        /// played terminal has said everything and the pty has gone quiet.
        fn left(slave: &mut File, said: &AtomicBool) -> Vec<u8> {
            let until = Instant::now() + GIVE_UP;
            let mut left = Vec::new();
            let mut quiet = Instant::now();
            while Instant::now() < until {
                let before = left.len();
                take(slave, &mut left);
                if left.len() != before {
                    quiet = Instant::now();
                }
                if said.load(Ordering::SeqCst) && quiet.elapsed() > Duration::from_millis(100) {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
            left
        }

        /// Every key the session would read from what was left: whatever is
        /// not a cursor position, which the key reader drops, and a sequence
        /// left unfinished.
        fn keys(left: &[u8]) -> Vec<Shape> {
            let mut replies = Replies::new();
            let mut keys: Vec<Shape> = left
                .iter()
                .filter_map(|byte| replies.feed(*byte))
                .filter(|shape| *shape != Shape::CursorPosition)
                .collect();
            if !replies.between() {
                keys.push(Shape::Other);
            }
            keys
        }

        fn count(heard: &[u8], question: &[u8]) -> usize {
            heard
                .windows(question.len())
                .filter(|part| *part == question)
                .count()
        }

        /// One exchange against a terminal playing `turns`.
        struct Ran {
            yes: bool,
            took: Duration,
            heard: Vec<u8>,
            left: Vec<u8>,
        }

        fn run(protocol: Protocol, turns: Vec<Turn>) -> Ran {
            let (master, mut slave) = pty();
            as_the_session_reads(&slave);
            let said = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            let heard = play(master, turns, Arc::clone(&said), Arc::clone(&done));

            let started = Instant::now();
            let yes = exchanged(&slave, protocol, WAIT);
            let took = started.elapsed();
            done.store(true, Ordering::SeqCst);

            let left = left(&mut slave, &said);
            let heard = heard
                .recv_timeout(GIVE_UP)
                .expect("the played terminal finished");
            Ran {
                yes,
                took,
                heard,
                left,
            }
        }

        #[test]
        fn a_terminal_that_answers_confirms_its_candidate_and_leaves_nothing_to_read() {
            for (protocol, yes) in [
                (Protocol::Kitty, KITTY_YES),
                (Protocol::Iterm2, ITERM2_YES),
                (Protocol::Sixel, SIXEL_YES),
            ] {
                let ran = run(
                    protocol,
                    vec![
                        turn(CURSOR, &[(0, CURSOR_AT)]),
                        turn(&asking(protocol), &[(0, yes), (0, CURSOR_AT)]),
                    ],
                );
                assert!(ran.yes, "{protocol:?} answered yes and was not confirmed");
                assert_eq!(count(&ran.heard, &asking(protocol)), 1, "{protocol:?}");
                assert_eq!(
                    keys(&ran.left),
                    Vec::<Shape>::new(),
                    "{protocol:?}: {:?}",
                    ran.left
                );
                assert!(ran.left.is_empty(), "{protocol:?}: {:?}", ran.left);
            }
        }

        #[test]
        fn a_cursor_position_after_the_patience_is_no_answer_and_nothing_more_is_asked() {
            let ran = run(Protocol::Kitty, vec![turn(CURSOR, &[(350, CURSOR_AT)])]);

            assert!(!ran.yes);
            assert_eq!(count(&ran.heard, CURSOR), 1, "{:?}", ran.heard);
            assert_eq!(count(&ran.heard, question(Protocol::Kitty)), 0);
            assert!(ran.took < Duration::from_millis(300), "{:?}", ran.took);
            assert_eq!(
                ran.left, CURSOR_AT,
                "the late answer is the key reader's to drop"
            );
            assert_eq!(keys(&ran.left), Vec::<Shape>::new());
        }

        #[test]
        fn a_terminal_that_never_answers_costs_the_patience_alone() {
            let ran = run(Protocol::Kitty, vec![turn(CURSOR, &[])]);

            assert!(!ran.yes);
            assert_eq!(count(&ran.heard, CURSOR), 1, "{:?}", ran.heard);
            assert_eq!(count(&ran.heard, question(Protocol::Kitty)), 0);
            assert!(ran.took >= WAIT.patience, "{:?}", ran.took);
            assert!(ran.took < WAIT.reply, "{:?}", ran.took);
            assert!(ran.left.is_empty(), "{:?}", ran.left);
        }

        #[test]
        fn a_yes_behind_its_cursor_position_is_a_no_and_is_consumed() {
            let ran = run(
                Protocol::Kitty,
                vec![
                    turn(CURSOR, &[(0, CURSOR_AT)]),
                    turn(&asking(Protocol::Kitty), &[(0, CURSOR_AT), (5, KITTY_YES)]),
                ],
            );

            assert!(!ran.yes);
            assert_eq!(count(&ran.heard, &asking(Protocol::Kitty)), 1);
            assert!(ran.left.is_empty(), "{:?}", ran.left);
        }

        #[test]
        fn a_reply_split_at_its_escape_within_the_wait_is_read_whole() {
            let ran = run(
                Protocol::Kitty,
                vec![
                    turn(CURSOR, &[(0, b"\x1b"), (30, b"[12;40R")]),
                    turn(
                        &asking(Protocol::Kitty),
                        &[(0, b"\x1b_Gi=31;OK\x1b"), (30, b"\\"), (0, CURSOR_AT)],
                    ),
                ],
            );

            assert!(ran.yes);
            assert!(ran.left.is_empty(), "{:?}", ran.left);
        }

        #[test]
        fn a_cursor_position_still_arriving_when_the_patience_ends_is_read_to_its_end() {
            let ran = run(
                Protocol::Kitty,
                vec![
                    turn(CURSOR, &[(0, b"\x1b["), (250, b"12;40R")]),
                    turn(&asking(Protocol::Kitty), &[(0, KITTY_YES), (0, CURSOR_AT)]),
                ],
            );

            assert!(ran.yes, "a cursor position begun in time is an answer");
            assert!(ran.left.is_empty(), "{:?}", ran.left);
        }

        #[test]
        fn a_yes_still_arriving_when_the_reply_wait_ends_is_read_to_its_end() {
            let ran = run(
                Protocol::Kitty,
                vec![
                    turn(CURSOR, &[(0, CURSOR_AT)]),
                    turn(
                        &asking(Protocol::Kitty),
                        &[(0, b"\x1b_Gi=31;OK\x1b"), (800, b"\\"), (0, CURSOR_AT)],
                    ),
                ],
            );

            assert!(ran.yes, "a yes begun in time is a yes");
            assert_eq!(keys(&ran.left), Vec::<Shape>::new(), "{:?}", ran.left);
        }

        #[test]
        fn a_late_background_answer_is_consumed_wherever_it_lands() {
            let ran = run(
                Protocol::Kitty,
                vec![
                    turn(
                        CURSOR,
                        &[(0, b"\x1b]11;rgb:0000/0000/0000\x1b\\"), (0, CURSOR_AT)],
                    ),
                    turn(
                        &asking(Protocol::Kitty),
                        &[
                            (0, b"\x1b]11;rgb:ffff/ffff/ffff\x07"),
                            (0, KITTY_YES),
                            (0, CURSOR_AT),
                        ],
                    ),
                ],
            );

            assert!(ran.yes);
            assert!(ran.left.is_empty(), "{:?}", ran.left);
        }

        #[test]
        fn a_terminal_that_sends_without_end_is_left_long_before_the_wait_runs_out() {
            let mut flood = b"\x1b]".to_vec();
            flood.extend(vec![b'a'; 64 * 1024]);
            let ran = run(
                Protocol::Kitty,
                vec![Turn {
                    asked: CURSOR.to_vec(),
                    answers: vec![(0, flood)],
                }],
            );

            assert!(!ran.yes);
            assert_eq!(count(&ran.heard, CURSOR), 1);
            assert!(ran.took < Duration::from_millis(500), "{:?}", ran.took);
        }

        #[test]
        fn the_mode_is_put_back_when_the_exchange_returns_early() {
            let (_master, slave) = pty();
            let before = rustix::termios::tcgetattr(&slave).expect("the pty's mode");
            let mut inside = None;

            let returned = (|| -> io::Result<()> {
                let _mode = Mode::enter(&slave)?
                    .ok_or_else(|| io::Error::other("the pty is not a terminal"))?;
                inside = Some(rustix::termios::tcgetattr(&slave)?);
                let _: u8 = "an early return".parse().map_err(io::Error::other)?;
                Ok(())
            })();

            assert!(returned.is_err(), "the case did not return early");
            let inside = inside.expect("the mode was entered");
            assert!(!inside.local_modes.contains(LocalModes::ICANON));
            assert!(!inside.local_modes.contains(LocalModes::ECHO));
            assert_eq!(inside.special_codes[SpecialCodeIndex::VMIN], 0);

            let after = rustix::termios::tcgetattr(&slave).expect("the pty's mode");
            assert_eq!(after.local_modes, before.local_modes);
            assert_eq!(after.input_modes, before.input_modes);
            assert_eq!(after.output_modes, before.output_modes);
            assert_eq!(
                after.special_codes[SpecialCodeIndex::VMIN],
                before.special_codes[SpecialCodeIndex::VMIN]
            );
        }

        #[test]
        fn no_mode_is_set_where_the_other_end_is_not_a_terminal() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/null")
                .expect("the null device");
            assert!(matches!(Mode::enter(file.as_fd()), Ok(None)));
        }
    }
}
