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
//! sequence that is still arriving when the wait runs out is read to its end.
//! A cursor position that arrives after the exchange gave up is one shape the
//! key reader drops itself. What a user types during the exchange is consumed
//! with the replies, which costs at most the exchange's own few milliseconds of
//! typing.
//!
//! **The table is what was seen.** [`support`] holds the cells of the terminal
//! support table that drew right in every step of their screen mode; every
//! other cell is disabled, because a disabled cell is output that was seen to
//! draw wrongly or was never seen at all. A terminal is identified only by what
//! the table identified it by.

use std::fmt;
use std::time::Duration;

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
const LONGEST: usize = 1024;

/// The image a Kitty query names, so its reply is told from any other.
const QUERY_IMAGE: &str = "i=31";

/// What a run of bytes from the terminal was, by its shape.
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
#[derive(Debug)]
pub(crate) struct Replies {
    state: State,
    held: Vec<u8>,
    spilled: bool,
}

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

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// `CAN` and `SUB`, which call off any sequence they arrive inside.
const fn cancels(byte: u8) -> bool {
    matches!(byte, 0x18 | 0x1a)
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
