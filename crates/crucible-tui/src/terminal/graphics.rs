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
}
