//! crucible, running in a terminal of a known size.
//!
//! The pair is opened here, the real binary is started against the far side of
//! it, keystrokes go in and bytes come back. Nothing is mocked: what the
//! [`Screen`] draws is what a terminal would have drawn.
//!
//! Three things about the arrangement are not obvious and all three are load
//! bearing.
//!
//! The child is started through `setsid --ctty` rather than directly. It needs
//! this pty as its *controlling* terminal, because `crossterm` reads the window
//! size by opening `/dev/tty` and only falls back to standard output when that
//! fails — a child that kept the developer's terminal would lay its frames out
//! for whatever size that window happens to be, and the same test would draw
//! something different on every machine. It is also what makes the kernel
//! deliver `SIGWINCH` when the size changes, so the resize case is a resize
//! rather than a simulation of one. Claiming a controlling terminal from inside
//! a child means `setsid` and `TIOCSCTTY` between the fork and the exec, which
//! is `unsafe` and denied in this workspace, so the one program whose whole job
//! is to do that safely does it instead.
//!
//! The far side is put in raw mode here, before the child starts. Otherwise the
//! kernel's own line discipline rewrites what crucible wrote — `\r\n` arrives
//! as `\r\r\n` — and the screen would be asserting on the kernel's version of
//! the frame rather than on crucible's.
//!
//! The far side is then closed. While anything in this process still holds it
//! open, a read of the near side blocks forever instead of ending when the
//! child does, and a test that hangs is worse than one that fails.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags};
use rustix::pty::{self, OpenptFlags};
use rustix::termios::{self, OptionalActions, Winsize};

use crate::screen::{Profile, Screen};
use crate::vendor::Vendor;

/// How long the terminal must go without a byte before the screen is settled.
///
/// Generous, because the alternative to waiting long enough is a test that
/// fails one run in twenty and then gets switched off.
const QUIET: Duration = Duration::from_millis(400);

/// Long enough to cross more than one quarter-second face of the working mark.
const TURN_BEATS: Duration = Duration::from_secs(1);

/// What an active turn may spend of the CPU for each second it is watched.
///
/// An active marker may wake on its four beats, but not burn a scheduler slice
/// between them. The allowance covers everything the process does while the
/// faces turn: drawing them, the terminal work around them, and supervising the
/// command the held turn left running. The key echo is timed on its own rather
/// than charged here.
///
/// A hundred and twenty milliseconds, measured rather than chosen. A watched
/// second charges 37 to 66 ms from run to run and has reached 89 ms, and forty
/// failed eight of twelve runs alone on a quiet host. The largest part is the
/// runtime supervising the held command on its 20 ms poll; the mark is four
/// frames and under 400 bytes a second. What this still catches is a turn that
/// spins, charged about a second for each second watched, and a supervision
/// poll tightened to a millisecond, charged about 200 ms. What it cannot catch
/// is a draw that merely doubles: that moved the charge by about 2 ms, well
/// inside the run-to-run spread.
const ACTIVE_CPU_PER_SECOND: Duration = Duration::from_millis(120);

/// How long a settled prompt is watched.
const IDLE_SAMPLE: Duration = Duration::from_secs(1);

/// What a settled prompt may spend of the CPU for each second it is watched.
///
/// Ten milliseconds, one `/proc` clock tick at the usual `USER_HZ` of 100, which is
/// what the tick count this replaced allowed. Read to the nanosecond rather than
/// counted in ticks, a process that keeps waking while nobody types shows up as
/// the time it spent, not as whether a tick boundary happened to fall inside the
/// sample.
const IDLE_CPU_PER_SECOND: Duration = Duration::from_millis(10);

/// How long one step may take before the screen is called stuck.
const CEILING: Duration = Duration::from_secs(20);

/// What is on screen once crucible has finished opening a session.
///
/// The opening is several frames with real work between them — a directory of
/// session logs read, a runner assembled — and quiet alone cannot tell the gap
/// between two of them from the end of the last. This is the row drawn under
/// the box, which is the last thing written before crucible waits for a key.
const READY: &str = "ask mode on";

/// The model a case with something to ask asks for.
///
/// It reaches [`Vendor`], which answers whatever it is asked, so the name only
/// has to be one — but it is on the screen, so it is written to look like one
/// rather than like a test fixture.
const MODEL: &str = "claude-test-1";

/// The configuration a case is given, pointed at `vendor` where there is one
/// and holding `allowed` where a case has a call to let through.
///
/// `updates.check` reaches GitHub, and a test that asks a network for anything
/// is a test that fails when the network does. Written rather than left to the
/// default so that the answer is in the tree next to the cases.
///
/// This is crucible's *own* configuration file, in the home this run was given.
/// That matters for `baseUrl`: it is refused in the file a checkout carries, so
/// a case that wrote it there would be testing the refusal instead.
fn document(vendor: Option<&Vendor>, allowed: Option<&str>) -> String {
    let providers = vendor.map_or_else(String::new, |vendor| {
        format!(
            ",\n  \"providers\": {{\n    \"anthropic\": {{\n      \
             \"model\": \"{MODEL}\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}",
            vendor.address()
        )
    });
    let rules = allowed.map_or_else(String::new, |rule| {
        format!(",\n  \"permissions\": {{\n    \"allow\": [\"{rule}\"]\n  }}")
    });

    format!("{{\n  \"updates\": {{\n    \"check\": \"never\"\n  }}{providers}{rules}\n}}\n")
}

/// Adds the one host-dependent choice every screen fixture makes.
///
/// These cases exercise terminal composition through real child processes, not
/// Linux namespace support. The setting is written into the disposable
/// user-owned document so the fixture does not depend on the application default
/// and no project layer is allowed to weaken confinement.
fn fixture_document(document: &str) -> String {
    let rest = document
        .strip_prefix("{\n")
        .expect("a whole-screen configuration object");
    format!("{{\n  \"sandbox\": {{\"enabled\": false}},\n{rest}")
}

/// A kept tail no session started by a case here can reach past, in tokens.
///
/// `keep` is how many tokens of recent turns survive a recap word for word.
/// Set this high and every turn is inside it, so compaction finds no middle to
/// replace and does only the other thing it can do.
const NO_MIDDLE: u64 = 100_000_000;

/// How long the path of every case's scratch directory is, in bytes.
///
/// The workspace root is told to the model, so the length of the directory a
/// case works in is in every count of what a request carries — and `/context`
/// draws those counts to the token. Left to the host, the same case would read
/// one figure where the temporary directory is `/tmp`, another under a longer
/// one, and another again the day this run's process id gains a digit. The
/// scratch directory's name is padded out to this length instead, so every
/// machine and every run sends the same number of bytes. Room for a temporary
/// directory as long as a sandbox gives, while the padded name stays inside the
/// 255 bytes one name may hold where the temporary directory is short.
const SCRATCH_LENGTH: usize = 240;

/// The directory `case` is given, which holds everything it makes.
///
/// One flat directory per case, so the last thing a case does can take the
/// whole of what it made with it.
pub(crate) fn scratch(case: &str) -> PathBuf {
    let mut named = std::env::temp_dir()
        .join(format!(
            "crucible-whole-screen-{}-{case}-",
            std::process::id()
        ))
        .into_os_string();
    let length = named.len();
    assert!(
        length <= SCRATCH_LENGTH,
        "{} is {length} bytes, past the {SCRATCH_LENGTH} every case's scratch directory is \
         padded to; run with a shorter TMPDIR",
        named.display()
    );
    named.push("-".repeat(SCRATCH_LENGTH.saturating_sub(length)));
    PathBuf::from(named)
}

/// The directory a case is given to work in, below the one it is given.
///
/// Deep enough that everything drawing it shortens it, and shortens it past the
/// segment holding this run's process id. A path drawn whole would put that
/// number into the snapshot, which would then be a picture of one run rather
/// than of this program. The row at the top of the window is the widest thing
/// that says it, so the segments below the scratch directory have to fill that
/// row on their own.
fn working(scratch: &Path) -> PathBuf {
    scratch
        .join("a-directory-to-be-working-in")
        .join("and-something-below-that-again")
        .join("workspace")
}

/// What a case starts crucible with beyond what every case is given.
#[derive(Debug, Default)]
pub(crate) struct Launch<'a> {
    /// The whole configuration file, written as it is.
    pub(crate) document: &'a str,
    /// More of the environment, beside what every case sets.
    pub(crate) env: &'a [(&'a str, &'a str)],
    /// The command line after the program's name.
    pub(crate) args: &'a [&'a str],
    /// A home an earlier run left, copied in before the configuration file is
    /// written over it.
    pub(crate) home: Option<&'a Path>,
}

/// Copies the tree at `from` into `to`, file by file.
fn copied(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("a directory to copy into");
    for entry in fs::read_dir(from).expect("a home to copy").flatten() {
        let path = entry.path();
        let into = to.join(entry.file_name());
        if path.is_dir() {
            copied(&path, &into);
        } else {
            fs::copy(&path, &into).expect("a file copied");
        }
    }
}

/// Optional behavior of the terminal side of one whole-screen case.
struct TerminalFixture<'a> {
    columns: u16,
    rows: u16,
    reply: Option<&'a [u8]>,
    /// Whether the launch draws in the terminal's own buffer rather than on
    /// a screen of its own.
    native: bool,
    /// How the terminal reflows and counts columns.
    profile: Profile,
    /// Whether the far side starts in the mode a new terminal opens in,
    /// echoing and reading whole lines, rather than raw.
    cooked: bool,
}

impl TerminalFixture<'_> {
    fn coloured(&self) -> bool {
        self.reply.is_some()
    }
}

/// crucible, and the terminal it is drawing into.
#[derive(Debug)]
pub(crate) struct Watched {
    /// The near side of the pair: what a terminal emulator would hold.
    terminal: File,
    /// The process being watched.
    child: Child,
    /// What has been read off the terminal and not yet drawn.
    bytes: Receiver<Vec<u8>>,
    /// What it has drawn.
    screen: Screen,
    /// A size the window took whose mark has not come back off the terminal.
    marked: Option<(u16, u16)>,
    /// Bytes read off the terminal that may begin the mark, held until the
    /// rest of it, or something else, arrives.
    carry: Vec<u8>,
    /// Whether fixed palette proofs crossed the PTY. Payloads do not survive.
    light_seen: bool,
    dark_seen: bool,
    /// The directory this case was given to itself.
    scratch: PathBuf,
}

impl Watched {
    /// Starts crucible as though an exact-colour terminal answered that its
    /// background is white. The reply is sent over the PTY in response to the
    /// process's real OSC 11 query; no style owner is called by the fixture.
    pub(crate) fn on_light_terminal(case: &str, columns: u16, rows: u16) -> Self {
        Self::configured_with_terminal(
            case,
            &document(None, None),
            false,
            &TerminalFixture {
                columns,
                rows,
                reply: Some(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?1;2c"),
                native: false,
                profile: Profile::default(),
                cooked: false,
            },
            None,
        )
    }

    /// Starts crucible in a window that size and waits for it to finish
    /// drawing.
    ///
    /// `case` names the directory this run is given, so two cases running at
    /// once share no session log, no configuration and no working directory.
    ///
    /// No provider: this is crucible with nothing to answer, which is every
    /// case about what is on screen before a turn.
    pub(crate) fn open(case: &str, columns: u16, rows: u16) -> Self {
        Self::started(case, columns, rows, None, false)
    }

    /// The same, with a provider on this machine that answers what it is asked.
    ///
    /// A turn is what most of the renderer's arithmetic is *for* — the live
    /// tail, the footing standing under a streaming answer, the box taking
    /// typing while one runs — and none of it was reachable from here while the
    /// address a request goes to was a constant.
    pub(crate) fn answering(case: &str, columns: u16, rows: u16, vendor: &Vendor) -> Self {
        Self::started(case, columns, rows, Some(vendor), true)
    }

    /// The same, on a terminal that counts columns as `profile` says rather
    /// than as most terminals now do.
    pub(crate) fn answering_on(
        case: &str,
        columns: u16,
        rows: u16,
        vendor: &Vendor,
        profile: Profile,
    ) -> Self {
        Self::configured_with_terminal(
            case,
            &document(Some(vendor), None),
            true,
            &TerminalFixture {
                columns,
                rows,
                reply: None,
                native: false,
                profile,
                cooked: false,
            },
            None,
        )
    }

    /// The same again, in native mode: crucible drawing at the foot of the
    /// terminal's own buffer rather than on a screen of its own.
    ///
    /// The terminal it is given is the one that keeps a scrollback and
    /// rewraps, since a native frame moves relatively and lets finished rows
    /// scroll off the top; a fullscreen one would refuse the first frame. A
    /// case started this way proves it ran in native mode with
    /// [`Self::assert_never_alternate`] and not with its rows, which look the
    /// same on either screen until something scrolls.
    pub(crate) fn native(case: &str, columns: u16, rows: u16, vendor: &Vendor) -> Self {
        Self::native_on(case, columns, rows, vendor, Profile::default())
    }

    /// The same, on a terminal that reflows and counts columns as `profile`
    /// says rather than as most terminals now do.
    pub(crate) fn native_on(
        case: &str,
        columns: u16,
        rows: u16,
        vendor: &Vendor,
        profile: Profile,
    ) -> Self {
        let document = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
             \"output\": {{\"screen\": \"native\"}},\n  \
             \"providers\": {{\n    \"anthropic\": {{\n      \
             \"model\": \"{MODEL}\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
            vendor.address()
        );

        Self::configured_with_terminal(
            case,
            &document,
            true,
            &TerminalFixture {
                columns,
                rows,
                reply: None,
                native: true,
                profile,
                cooked: false,
            },
            None,
        )
    }

    /// The same again, on a terminal crucible writes colour to.
    ///
    /// Every other case here runs with `NO_COLOR` set, which is what keeps a
    /// picture readable — and it is also what leaves the markdown reader
    /// unreached, since a run with no colour to put a marker into keeps the
    /// marker instead. So the one thing this whole file exists to check about
    /// an answer — that what a person sees is what the model meant, drawn by
    /// the terminal they are actually in — was the one thing no case here
    /// could see. This asks for colour outright, which outranks `NO_COLOR`.
    ///
    /// `ansi` because these cases are read through [`Self::picture`], which
    /// shows the rows and not what they were drawn in, and the sixteen a
    /// terminal already has are the fewest escapes to get to them.
    pub(crate) fn in_colour(case: &str, columns: u16, rows: u16, vendor: &Vendor) -> Self {
        let document = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
             \"output\": {{\"color\": \"always\", \"theme\": \"ansi\"}},\n  \
             \"providers\": {{\n    \"anthropic\": {{\n      \
             \"model\": \"{MODEL}\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
            vendor.address()
        );

        Self::configured(case, columns, rows, &document, true)
    }

    /// The same again, in the colours crucible draws with when nothing chooses
    /// a theme, for a case read with [`Self::picture_in_colour`].
    ///
    /// Colour is asked for outright, which outranks the `NO_COLOR` the harness
    /// sets, and the theme is left to its default. Nothing else differs from
    /// the launch the case copies: `vendor` and `rule` are what it was given,
    /// and a case with a vendor has the variable a key is read from, as every
    /// case that reaches one does.
    pub(crate) fn in_default_colours(
        case: &str,
        size: (u16, u16),
        vendor: Option<&Vendor>,
        rule: Option<&str>,
    ) -> Self {
        let plain = document(vendor, rule);
        let rest = plain
            .strip_prefix("{\n")
            .expect("a whole-screen configuration object");
        let document = format!("{{\n  \"output\": {{\"color\": \"always\"}},\n{rest}");
        let (columns, rows) = size;

        Self::configured(case, columns, rows, &document, vendor.is_some())
    }

    /// A provider to reach and nothing to sign a request with.
    ///
    /// The machine `/login` is for: a vendor is there and configured, and this
    /// run started without the variable that would have chosen it, so the only
    /// way to a turn is through the command. Both halves matter — a case with
    /// no vendor could not tell a key that arrived from one that never did.
    pub(crate) fn keyless(case: &str, columns: u16, rows: u16, vendor: &Vendor) -> Self {
        Self::started(case, columns, rows, Some(vendor), false)
    }

    /// The same again, with one rule standing over the call the case makes.
    ///
    /// A rule rather than `fullAccess`, because what this reaches is the
    /// transcript a call leaves behind: a mode that allowed everything would
    /// put the case in front of a screen no ordinary run meets, and the
    /// question a call is asked about has its own cases already.
    pub(crate) fn allowing(
        case: &str,
        columns: u16,
        rows: u16,
        vendor: &Vendor,
        rule: &str,
    ) -> Self {
        let document = document(Some(vendor), Some(rule));
        Self::configured(case, columns, rows, &document, true)
    }

    /// The same, drawn with the glyphs and on the screen `drawn` names: the two
    /// settings a panel has to look right under in every combination, paired
    /// because a case always names both.
    ///
    /// Both are written as the configuration spells them, so a case reads as
    /// the setting it is about. A native terminal is the one that keeps a
    /// scrollback, as [`Self::native`] says.
    pub(crate) fn allowing_drawn(
        case: &str,
        size: (u16, u16),
        vendor: &Vendor,
        rule: &str,
        drawn: (&str, &str),
    ) -> Self {
        let (glyphs, screen) = drawn;
        let document = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
             \"output\": {{\"glyphs\": \"{glyphs}\", \"screen\": \"{screen}\"}},\n  \
             \"permissions\": {{\"allow\": [\"{rule}\"]}},\n  \
             \"providers\": {{\n    \"anthropic\": {{\n      \
             \"model\": \"{MODEL}\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
            vendor.address()
        );
        let (columns, rows) = size;

        Self::configured_with_terminal(
            case,
            &document,
            true,
            &TerminalFixture {
                columns,
                rows,
                reply: None,
                native: screen == "native",
                profile: Profile::default(),
                cooked: false,
            },
            None,
        )
    }

    /// Colour, rules for the calls a case makes, and no middle to recap.
    ///
    /// Three things at once, which no other case here needs together. Colour,
    /// because the markdown reader is unreached without it. Rules, because the
    /// case makes calls and the question a call is asked has its own cases
    /// already. And `keep` set past anything a session started here can reach,
    /// so that asking for room finds no older middle worth replacing and the
    /// clearing of old tool output is the whole of what compaction does — which
    /// is the one thing this arrangement exists to put on a screen.
    pub(crate) fn pruning_in_colour(
        case: &str,
        columns: u16,
        rows: u16,
        vendor: &Vendor,
        rules: &[&str],
    ) -> Self {
        let allowed = rules
            .iter()
            .map(|rule| format!("\"{rule}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let document = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
             \"output\": {{\"color\": \"always\", \"theme\": \"ansi\"}},\n  \
             \"permissions\": {{\"allow\": [{allowed}]}},\n  \
             \"compaction\": {{\"keep\": {NO_MIDDLE}}},\n  \
             \"providers\": {{\n    \"anthropic\": {{\n      \
             \"model\": \"{MODEL}\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
            vendor.address()
        );

        Self::configured(case, columns, rows, &document, true)
    }

    /// A provider that answers, and a session that is asked about the moment
    /// it is picked up.
    ///
    /// `askOnResume` is set to one token so any session at all is large enough
    /// to be worth the question. What the case is about is the panel and what
    /// follows it, not the figure that decides whether they appear.
    pub(crate) fn asking_on_resume(case: &str, columns: u16, rows: u16, vendor: &Vendor) -> Self {
        let document = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
             \"compaction\": {{\"askOnResume\": 1}},\n  \
             \"providers\": {{\n    \"anthropic\": {{\n      \
             \"model\": \"{MODEL}\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
            vendor.address()
        );

        Self::configured(case, columns, rows, &document, true)
    }

    /// A provider that answers, with the compaction tail held to one token.
    ///
    /// The default keeps twenty thousand, which every turn here fits inside —
    /// a recap would stand in place of nothing. One token keeps only the turn
    /// being taken, so a session of three short turns still has a middle to
    /// replace, which is the thing these cases are about.
    pub(crate) fn compacting(case: &str, columns: u16, rows: u16, vendor: &Vendor) -> Self {
        let document = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \"compaction\": {{\"keep\": 1, \"askOnResume\": 1}},\n  \"providers\": {{\n    \"anthropic\": {{\n      \"model\": \"{MODEL}\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
            vendor.address()
        );

        Self::configured(case, columns, rows, &document, true)
    }

    /// A provider, model and effort remembered after their credential left.
    pub(crate) fn unavailable(case: &str, columns: u16, rows: u16) -> Self {
        let document = concat!(
            "{\n",
            "  \"updates\": {\"check\": \"never\"},\n",
            "  \"provider\": \"openai\",\n",
            "  \"providers\": {\"openai\": {\"model\": \"gpt-5.6-sol\", \"effort\": \"high\"}}\n",
            "}\n"
        );
        Self::configured(case, columns, rows, document, false)
    }

    /// Crucible with nothing to answer, as [`Self::open`] starts it but with
    /// no reply from the terminal, in a terminal that starts as a new one
    /// does, echoing and reading whole lines, so that whether crucible put it
    /// back that way can be asked once it is gone.
    pub(crate) fn cooked(case: &str, columns: u16, rows: u16) -> Self {
        Self::configured_with_terminal(
            case,
            &document(None, None),
            false,
            &TerminalFixture {
                columns,
                rows,
                reply: None,
                native: false,
                profile: Profile::default(),
                cooked: true,
            },
            None,
        )
    }

    /// Starts crucible in a window that size and waits for it to finish
    /// drawing.
    fn started(case: &str, columns: u16, rows: u16, vendor: Option<&Vendor>, keyed: bool) -> Self {
        Self::configured(case, columns, rows, &document(vendor, None), keyed)
    }

    pub(crate) fn configured(
        case: &str,
        columns: u16,
        rows: u16,
        document: &str,
        keyed: bool,
    ) -> Self {
        Self::configured_with_terminal(
            case,
            document,
            keyed,
            &TerminalFixture {
                columns,
                rows,
                reply: None,
                native: false,
                profile: Profile::default(),
                cooked: false,
            },
            None,
        )
    }

    /// Starts crucible as `launch` says, in a window that size.
    pub(crate) fn launched(case: &str, columns: u16, rows: u16, launch: &Launch<'_>) -> Self {
        Self::configured_with_terminal(
            case,
            launch.document,
            false,
            &TerminalFixture {
                columns,
                rows,
                reply: None,
                native: false,
                profile: Profile::default(),
                cooked: false,
            },
            Some(launch),
        )
    }

    fn configured_with_terminal(
        case: &str,
        document: &str,
        keyed: bool,
        terminal: &TerminalFixture<'_>,
        launch: Option<&Launch<'_>>,
    ) -> Self {
        let scratch = scratch(case);
        let home = scratch.join("home");

        let workspace = working(&scratch);
        fs::create_dir_all(&home).expect("a scratch home directory");
        fs::create_dir_all(&workspace).expect("a scratch working directory");
        if let Some(earlier) = launch.and_then(|launch| launch.home) {
            copied(earlier, &home);
        }
        let written = match launch {
            Some(_) => document.to_owned(),
            None => fixture_document(document),
        };
        fs::write(home.join("config.json"), written).expect("a configuration file");

        // A checkout, because that is what crucible is run in. Only one fact
        // about it is read at startup and only one case turns on it -- but a
        // fixture with no remote is a fixture where a whole seam is switched
        // off, and switched off is not the state anybody ships in.
        let git = workspace.join(".git");
        fs::create_dir_all(&git).expect("a scratch .git directory");
        fs::write(
            git.join("config"),
            concat!(
                "[remote \"origin\"]\n",
                "\turl = https://github.com/augments-labs/crucible-code.git\n"
            ),
        )
        .expect("a git configuration file");

        let (mut near, inside) = pair(terminal.columns, terminal.rows, terminal.cooked);
        let child = start(&scratch, keyed, terminal, launch, inside);
        if let Some(reply) = terminal.reply {
            near.write_all(reply)
                .expect("the terminal background reply goes to crucible");
            near.flush().expect("the terminal reply is flushed");
        }
        let (sender, bytes) = mpsc::channel();
        let reading = near.try_clone().expect("a second handle on the terminal");
        std::thread::spawn(move || read(reading, &sender));

        let (columns, rows) = (terminal.columns as usize, terminal.rows as usize);
        let screen = if terminal.native {
            Screen::native_on(columns, rows, terminal.profile)
        } else {
            Screen::fullscreen_on(columns, rows, terminal.profile)
        };
        let mut window = Self {
            terminal: near,
            child,
            bytes,
            screen,
            marked: None,
            carry: Vec::new(),
            light_seen: false,
            dark_seen: false,
            scratch,
        };
        window.settle("crucible started", Some(READY));
        window
    }

    /// Clicks one zero-based cell and waits for the screen to settle.
    pub(crate) fn clicks(&mut self, row: usize, column: usize) {
        let x = column + 1;
        let y = row + 1;
        self.types(&format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m"));
    }

    /// Moves the pointer, no button held, onto one zero-based cell and waits
    /// for the screen to settle.
    pub(crate) fn hovers(&mut self, row: usize, column: usize) {
        let x = column + 1;
        let y = row + 1;
        self.types(&format!("\x1b[<35;{x};{y}M"));
    }

    /// Drags the left button between two zero-based cells and waits for the
    /// screen to settle.
    pub(crate) fn drags(&mut self, from: (usize, usize), to: (usize, usize)) {
        let (from_y, from_x) = (from.0 + 1, from.1 + 1);
        let (to_y, to_x) = (to.0 + 1, to.1 + 1);
        self.types(&format!(
            "\x1b[<0;{from_x};{from_y}M\x1b[<32;{to_x};{to_y}M\x1b[<0;{to_x};{to_y}m"
        ));
    }

    /// Types `keys` and waits for the screen to stop changing.
    pub(crate) fn types(&mut self, keys: &str) {
        self.terminal
            .write_all(keys.as_bytes())
            .expect("keys go to the terminal");
        self.settle(&format!("{keys:?} was typed"), None);
    }

    /// Writes `report` — a wheel notch, a click — as a terminal would send it,
    /// and waits one quiet period for whatever crucible does with it, which may
    /// rightly be nothing.
    ///
    /// [`Self::types`] waits for a byte to come back, because every key is
    /// drawn for. A report crucible asked for no mode to receive is not, so a
    /// step that insisted on one would time out proving the thing it meant to
    /// check; a case that wants to know the screen did not change compares the
    /// picture before with the picture after.
    pub(crate) fn reports(&mut self, report: &str) {
        self.terminal
            .write_all(report.as_bytes())
            .expect("a report goes to the terminal");
        self.settle_for(&format!("{report:?} was reported"), Awaited::Nothing);
    }

    /// Clicks the left button on the zero-based cell and reads until `wanted`
    /// is on screen, without waiting for the screen to go still.
    ///
    /// The click half of [`Self::types_and_catches`]: what the click opens can
    /// stand under an answer that is still arriving, and a screen being
    /// redrawn on the stream's beat is one [`Self::settle`] never hears go
    /// quiet.
    pub(crate) fn clicks_catching(&mut self, row: usize, column: usize, wanted: &str) {
        let x = column + 1;
        let y = row + 1;
        self.terminal
            .write_all(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").as_bytes())
            .expect("the click goes to the terminal");
        self.catches("a click", wanted);
    }

    /// Types `keys` and waits until the screen contains `wanted` and settles.
    ///
    /// `wanted` has to be something only this step can draw. A mark the screen
    /// already carries — the permission row, a heading, an answer worded the
    /// same as the one before it — leaves the quiet doing the waiting on its
    /// own, and quiet is the one thing a step in flight can produce: the gap
    /// between the keys being echoed and the answer starting to arrive is
    /// silence, and on a loaded machine it outlasts [`QUIET`]. The step then
    /// returns on the turn before it, and whatever is typed next lands in a
    /// session that is still answering.
    pub(crate) fn types_until(&mut self, keys: &str, wanted: &str) {
        self.terminal
            .write_all(keys.as_bytes())
            .expect("keys go to the terminal");
        self.settle(&format!("{keys:?} was typed"), Some(wanted));
    }

    /// Types `keys` and reads until `wanted` is on screen, without waiting for
    /// the screen to go still.
    ///
    /// [`Self::settle`] cannot be what a case about a screen redrawn on a beat
    /// waits on: the thing it is watching is the thing that keeps bytes
    /// arriving, so the quiet it waits for never comes and the step fails at
    /// [`CEILING`] with the picture it wanted on it. This reads frames as they
    /// land and stops at the first finished one carrying `wanted`.
    ///
    /// It terminates for the same reason `settle` does — every pass either
    /// takes bytes off a stream a stopped process cannot add to, or waits
    /// [`QUIET`] for one, and [`CEILING`] bounds the wait either way.
    pub(crate) fn types_and_catches(&mut self, keys: &str, wanted: &str) {
        self.terminal
            .write_all(keys.as_bytes())
            .expect("keys go to the terminal");
        self.catches(&format!("{keys:?} was typed"), wanted);
    }

    /// Watches a running turn cross more than one face, then proves a key is
    /// drawn before the next face can be mistaken for its response.
    ///
    /// The caller first waits for an answer's last word, so the provider has
    /// gone silent and is only keeping the request open. Bytes arriving here are
    /// therefore the application's own active-turn frames. The faces themselves
    /// come from the shipped binary rather than from [`Screen`]'s interpretation
    /// of one final picture.
    ///
    /// What the turn spends of the CPU is read across the faces alone. The key's
    /// echo is timed on its own, so a slow echo is reported as one rather than as
    /// an animation over its allowance.
    pub(crate) fn turns_then_echoes(&mut self, key: &str) {
        let before = Spent::by(self.child.id());
        let started = Instant::now();
        let deadline = started + TURN_BEATS;
        let mut faces = Vec::new();

        while Instant::now() < deadline {
            match self
                .bytes
                .recv_timeout(QUIET.min(deadline.saturating_duration_since(Instant::now())))
            {
                Ok(bytes) => {
                    self.feed(&bytes);
                    if let Some(face) = working_face(&self.picture())
                        && !faces.contains(&face)
                    {
                        faces.push(face);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("crucible left the terminal while its working mark was watched")
                }
            }
        }

        let charged = Spent::by(self.child.id()).since(&before);
        let watched = started.elapsed();

        assert!(
            faces.len() >= 2,
            "the held turn drew fewer than two working faces: {faces:?}\n{}",
            self.picture()
        );
        let allowed = ACTIVE_CPU_PER_SECOND.mul_f64(watched.as_secs_f64());
        assert!(
            charged <= allowed,
            "the active animation spent at least {charged:?} of CPU in {watched:?}, allowed {allowed:?}"
        );

        self.terminal
            .write_all(key.as_bytes())
            .expect("a key goes to the running turn");
        let sent = Instant::now();
        let echoed_by = sent + QUIET;
        while !self.picture().contains(key) && Instant::now() < echoed_by {
            match self
                .bytes
                .recv_timeout(echoed_by.saturating_duration_since(Instant::now()))
            {
                Ok(bytes) => self.feed(&bytes),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("crucible left the terminal before the key was echoed")
                }
            }
        }
        let latency = sent.elapsed();
        assert!(
            self.picture().contains(key),
            "{key:?} was not drawn within {QUIET:?} during the held turn\n{}",
            self.picture()
        );
        assert!(
            latency <= QUIET,
            "{key:?} took {latency:?} to reach the held-turn box; allowed {QUIET:?}"
        );
    }

    /// Proves every synchronized frame of a streamed answer preserves the
    /// already-visible prefix and never blanks the active answer panel.
    pub(crate) fn streams_without_flicker(&mut self, prompt: &str, words: &[&str]) {
        self.terminal
            .write_all(format!("{prompt}\r").as_bytes())
            .expect("the prompt goes to the terminal");

        let mut seen = 0;
        let mut active = 0;
        while seen < words.len() {
            let bytes = self
                .bytes
                .recv_timeout(CEILING)
                .expect("the streamed answer presents another frame");
            self.feed(&bytes);
            if !self.screen.is_holding() {
                let picture = self.picture();
                let now = words
                    .iter()
                    .take_while(|word| picture.contains(**word))
                    .count();
                if now > 0 {
                    active += 1;
                    assert!(
                        now >= seen,
                        "a streamed prefix went backwards from {seen} to {now}"
                    );
                    seen = now;
                } else if seen > 0 {
                    panic!("a streamed answer became blank between visible prefixes");
                }
            }
        }
        self.settle("the streamed answer", words.last().copied());
        assert!(
            active >= words.len(),
            "only {active} streamed presentation frames carried {} prefixes",
            words.len()
        );
    }

    /// Proves a real terminal query selected the light table in emitted bytes.
    pub(crate) fn uses_light_theme(&self) {
        assert!(
            self.screen
                .commands()
                .iter()
                .any(|command| command == "11;?"),
            "crucible never asked the terminal for its background"
        );
        assert!(self.light_seen, "the exact light accent was never emitted");
        assert!(
            !self.dark_seen,
            "the dark accent followed a light background reply"
        );
    }

    /// Proves the ordinary prompt stays byte-quiet and spends no more than ten
    /// milliseconds of CPU a second once its opening frame has settled.
    ///
    /// Linux exposes child CPU accounting in `/proc`; this whole test target is
    /// Linux-only, so reading it here adds no portability fiction. Active turns
    /// intentionally redraw and are measured separately by [`Self::turns_then_echoes`].
    pub(crate) fn stays_idle(&mut self) {
        let before = Spent::by(self.child.id());
        let started = Instant::now();
        let ended = started + IDLE_SAMPLE;
        let mut written = 0;

        while Instant::now() < ended {
            match self
                .bytes
                .recv_timeout(ended.saturating_duration_since(Instant::now()))
            {
                Ok(bytes) => {
                    written += bytes.len();
                    self.feed(&bytes);
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("crucible left the terminal while its settled prompt was watched")
                }
            }
        }

        let charged = Spent::by(self.child.id()).since(&before);
        let watched = started.elapsed();
        assert_eq!(
            written,
            0,
            "the settled prompt wrote {written} bytes in {IDLE_SAMPLE:?}\n{}",
            self.picture()
        );
        let allowed = IDLE_CPU_PER_SECOND.mul_f64(watched.as_secs_f64());
        assert!(
            charged <= allowed,
            "the settled prompt spent at least {charged:?} of CPU in {watched:?}, allowed {allowed:?}"
        );
    }

    /// Reads frames until `wanted` is on screen in a frame that has finished
    /// being written, as [`Screen::shows`] has it: a read that ends inside a
    /// frame would otherwise hand a case half a box.
    pub(crate) fn catches(&mut self, step: &str, wanted: &str) {
        self.catches_where(step, &format!("{wanted:?}"), |picture| {
            picture.contains(wanted)
        });
    }

    /// The same, for a screen a piece of text cannot name: reads frames until
    /// a finished one is a picture `drawn` holds of. `named` says what that
    /// is when none ever was.
    pub(crate) fn catches_where(&mut self, step: &str, named: &str, drawn: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + CEILING;

        while !self.screen.shows_where(&drawn) {
            match self.bytes.recv_timeout(QUIET) {
                Ok(bytes) => self.feed(&bytes),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "crucible left the terminal while {step}\n{}",
                        self.picture()
                    )
                }
            }

            assert!(
                Instant::now() < deadline,
                "no {named} was ever drawn after {step}, in {CEILING:?}{}\n{}",
                if self.screen.is_holding() {
                    " — the frame it is in was never finished"
                } else {
                    ""
                },
                self.picture()
            );
        }
    }

    /// Types `keys` and holds what follows to [`Self::never_draws`].
    pub(crate) fn types_and_never_draws(&mut self, keys: &str, unwanted: &str, held: Duration) {
        self.terminal
            .write_all(keys.as_bytes())
            .expect("keys go to the terminal");
        self.never_draws(&format!("{keys:?} was typed"), unwanted, held);
    }

    /// Reads what crucible writes for `held`, failing as soon as `unwanted` is
    /// on screen, in a finished frame or not: what a key that should leave part
    /// of the screen alone is held to after `step`.
    ///
    /// No frame can say that something will never be drawn, so this watches
    /// for longer than crucible takes to draw what the key would wrongly have
    /// done. It waits for neither a byte nor quiet, since a running turn
    /// redraws on its beat and a key that rightly does nothing draws nothing.
    pub(crate) fn never_draws(&mut self, step: &str, unwanted: &str, held: Duration) {
        let ended = Instant::now() + held;

        loop {
            assert!(
                !self.picture().contains(unwanted),
                "{unwanted:?} was drawn after {step}\n{}",
                self.picture()
            );
            let left = ended.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            match self.bytes.recv_timeout(left) {
                Ok(bytes) => self.feed(&bytes),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "crucible left the terminal while {step}\n{}",
                        self.picture()
                    )
                }
            }
        }
    }

    /// Changes the size of the window, the way dragging its corner would.
    ///
    /// The kernel is what tells crucible: setting the size on the near side of
    /// the pair raises `SIGWINCH` in the session on the far side of it. The
    /// screen is told at the point in crucible's output where the size took
    /// effect, since a frame on its way was drawn for the old one and what it
    /// lets through at the old size is counted from there. That point is
    /// marked by writing [`MARK`] to the far side of the pair once the size is
    /// set: the mark joins crucible's output in the order the kernel took the
    /// two, so everything before it was written before the mark and everything
    /// after it was written after the size was set. It errs only by what
    /// crucible wrote between the two calls, a few microseconds apart, and
    /// only toward counting that as written before — never toward charging
    /// the new size for a byte written before it. A settled screen is one
    /// crucible has drawn for, which [`Self::settle_for`] holds it to.
    pub(crate) fn resize(&mut self, columns: u16, rows: u16) {
        termios::tcsetwinsize(&self.terminal, size(columns, rows)).expect("a new window size");

        let named = pty::ptsname(&self.terminal, Vec::new()).expect("the far side has a name");
        let far = rustix::fs::open(
            OsStr::from_bytes(named.as_bytes()),
            OFlags::WRONLY | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("the far side opens again");
        File::from(far)
            .write_all(MARK)
            .expect("the mark goes in behind what crucible wrote");

        self.marked = Some((columns, rows));
        self.settle(&format!("the window became {columns}x{rows}"), None);
    }

    /// Sends crucible `signal`, reads the terminal until crucible has let go
    /// of it, and says how the process ended and what it wrote on the way.
    ///
    /// By name through `kill`, for the reason the child is started through
    /// `setsid`: raising a signal at another process is a call this workspace
    /// does not write by hand. `setsid` became crucible rather than starting
    /// it, so the process the signal reaches is the one being watched.
    ///
    /// Nothing here waits on a clock to decide the process is gone. The reader
    /// thread's channel closes when the last handle on the far side of the pair
    /// does, which is the process ending; [`CEILING`] only bounds a case that
    /// never gets there.
    ///
    /// What it wrote is handed back as it was written, because the screen
    /// keeps text and drops the modes a terminal is put in — and whether those
    /// were handed back is the thing a process ending has to be asked.
    pub(crate) fn ends_on(&mut self, signal: &str) -> (ExitStatus, String) {
        let sent = Command::new("kill")
            .args(["-s", signal, &self.child.id().to_string()])
            .status()
            .expect("kill is on the path");
        assert!(sent.success(), "{signal} never reached crucible");
        self.lets_go(signal)
    }

    /// Types `keys` that end the session, `/exit` or a second Ctrl+C, and
    /// reads the terminal until crucible has let go of it, as [`Self::ends_on`]
    /// does for a signal.
    pub(crate) fn ends_after(&mut self, keys: &str) -> (ExitStatus, String) {
        self.terminal
            .write_all(keys.as_bytes())
            .expect("keys go to the terminal");
        self.lets_go(&format!("{keys:?}"))
    }

    /// Reads until the process has ended, and says how it ended and what it
    /// wrote on the way; `cause` names what ended it in a failure.
    fn lets_go(&mut self, cause: &str) -> (ExitStatus, String) {
        let deadline = Instant::now() + CEILING;
        let mut wrote = Vec::new();
        loop {
            match self.bytes.recv_timeout(QUIET) {
                Ok(bytes) => {
                    wrote.extend_from_slice(&bytes);
                    self.feed(&bytes);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            assert!(
                Instant::now() < deadline,
                "crucible outlived {cause} by {CEILING:?}\n{}",
                self.picture()
            );
        }

        let ended = self.child.wait().expect("crucible ended");
        (ended, String::from_utf8_lossy(&wrote).into_owned())
    }

    /// Whether the terminal carries a character written past the last column
    /// on to the next row, as the last sequence that set it left it.
    pub(crate) fn wraps(&self) -> bool {
        self.screen.wraps()
    }

    /// Whether the terminal echoes what is typed, and whether it hands over
    /// whole lines rather than each key: the two modes a shell is left
    /// unusable without.
    pub(crate) fn modes(&self) -> (bool, bool) {
        let mode = termios::tcgetattr(&self.terminal).expect("the terminal has a mode");
        (
            mode.local_modes.contains(termios::LocalModes::ECHO),
            mode.local_modes.contains(termios::LocalModes::ICANON),
        )
    }

    /// Every session log this run left behind, one after another.
    ///
    /// Read after the process is gone, which is the only moment the question
    /// these logs answer can be asked: what is on the disk once nothing is left
    /// to put more there.
    pub(crate) fn recorded(&self) -> String {
        fn gather(directory: &Path, into: &mut String) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    gather(&path, into);
                } else if path.extension().is_some_and(|kind| kind == "jsonl") {
                    into.push_str(&fs::read_to_string(&path).unwrap_or_default());
                }
            }
        }

        let mut logs = String::new();
        gather(&self.home().join("sessions"), &mut logs);
        logs
    }

    /// The screen, ready to be compared against the one checked in beside it.
    pub(crate) fn picture(&self) -> String {
        self.screen.picture()
    }

    /// The same, with what each span was drawn in under its row.
    pub(crate) fn picture_in_colour(&self) -> String {
        self.screen.picture_in_colour()
    }

    /// The rows that scrolled off the top of a native window, oldest first,
    /// drawn the way [`Self::picture`] draws its rows: what a reader could
    /// scroll back to, read on from by the picture.
    pub(crate) fn scrollback(&self) -> String {
        self.screen.scrollback()
    }

    /// Fails when crucible entered the alternate screen.
    ///
    /// Every native case calls this, because it is what proves the case ran
    /// in native mode: a fullscreen launch enters the alternate screen before
    /// its first frame, and a native one never does, while the rows of a short
    /// session look the same on either.
    pub(crate) fn assert_never_alternate(&self) {
        assert!(
            !self.screen.entered_alternate(),
            "crucible entered the alternate screen, so this was not native mode\n{}",
            self.picture()
        );
    }

    /// crucible's own home for this case, where what a command wrote down
    /// lands.
    ///
    /// A case that reads it is asserting on the half of a command that outlives
    /// the process, which no picture can show: the screen says a thing was
    /// written, and the file is whether it was.
    pub(crate) fn home(&self) -> PathBuf {
        self.scratch.join("home")
    }

    /// The directory this run was given to work in.
    ///
    /// A case with a call in it puts the file that call is about here, which it
    /// can do after crucible has started: what has to exist by then is the
    /// directory, and what the tool goes looking for is not read until the turn
    /// asks for it.
    pub(crate) fn workspace(&self) -> PathBuf {
        working(&self.scratch)
    }

    /// The command strings crucible wrote, which is where a hyperlink is.
    pub(crate) fn commands(&self) -> &[String] {
        self.screen.commands()
    }

    /// Turns the two fixed palette proofs into booleans, then draws the bytes.
    /// No extra copy of any terminal payload survives this call, beyond the
    /// few bytes held while a mark is awaited.
    ///
    /// While a size the window took has not come back as its mark, the bytes
    /// are read for the mark first: what precedes it is drawn, the screen is
    /// told the size, and what follows is drawn after. A read ends wherever
    /// the kernel filled the buffer, so a tail that could begin the mark is
    /// held until the next read says whether it did.
    fn feed(&mut self, bytes: &[u8]) {
        const LIGHT: &[u8] = b"\x1b[38;2;13;107;98m";
        const DARK: &[u8] = b"\x1b[38;2;18;137;127m";
        self.light_seen |= bytes.windows(LIGHT.len()).any(|window| window == LIGHT);
        self.dark_seen |= bytes.windows(DARK.len()).any(|window| window == DARK);

        let Some((columns, rows)) = self.marked else {
            self.screen.feed(bytes);
            return;
        };

        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(bytes);

        if let Some(at) = data.windows(MARK.len()).position(|window| window == MARK) {
            self.marked = None;
            self.screen.feed(data.get(..at).unwrap_or_default());
            self.screen.resize(columns as usize, rows as usize);
            self.screen
                .feed(data.get(at + MARK.len()..).unwrap_or_default());
        } else {
            let held = (1..MARK.len())
                .rev()
                .find(|length| {
                    MARK.get(..*length)
                        .is_some_and(|opening| data.ends_with(opening))
                })
                .unwrap_or(0);
            self.carry = data.split_off(data.len() - held);
            self.screen.feed(&data);
        }
    }

    /// Reads until the screen settles, and fails plainly when it never does.
    ///
    /// Settled means two things at once: the terminal has gone [`QUIET`], and
    /// the screen is far enough along to be looked at — which is `wanted` on
    /// screen where a step has something it is waiting for, and otherwise a
    /// single byte having arrived since whatever was last sent. Quiet alone is
    /// not enough for a step drawn in several frames with work between them,
    /// because the gap between two frames and the end of the last one look the
    /// same from here; `wanted` is what tells them apart.
    ///
    /// Quiet is measured in bytes rather than in visible change, which is the
    /// stricter of the two: a frame that redrew the same rows still has to be
    /// read, because the sequences inside it are half of what is asserted.
    ///
    /// It terminates because every turn of the loop either takes bytes off a
    /// stream a stopped process cannot add to, or waits [`QUIET`] for one, and
    /// [`CEILING`] bounds the whole wait either way. Past it the step fails and
    /// says which one it was, rather than going on to compare a half-drawn
    /// screen against a picture and blaming the renderer for the difference.
    fn settle(&mut self, step: &str, wanted: Option<&str>) {
        self.settle_for(step, wanted.map_or(Awaited::AnyByte, Awaited::Mark));
    }

    /// [`Self::settle`], saying outright what a quiet screen has to show.
    fn settle_for(&mut self, step: &str, awaited: Awaited<'_>) {
        let deadline = Instant::now() + CEILING;
        let mut arrived = false;

        loop {
            match self.bytes.recv_timeout(QUIET) {
                Ok(bytes) => {
                    self.feed(&bytes);
                    arrived = true;
                }
                Err(RecvTimeoutError::Timeout) if self.ready(arrived, awaited) => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "crucible left the terminal while {step}\n{}",
                        self.picture()
                    )
                }
            }

            assert!(
                Instant::now() < deadline,
                "the screen never settled after {step}, in {CEILING:?}{}\n{}",
                match awaited {
                    Awaited::Mark(mark) => format!(" — no {mark:?} on it"),
                    Awaited::AnyByte | Awaited::Nothing => String::new(),
                },
                self.picture()
            );
        }

        assert!(
            self.screen.refusals().is_empty(),
            "crucible wrote what it does not promise to write, {step}:\n{}\n{}",
            self.screen.refusals().join("\n"),
            self.picture()
        );

        // A settled screen is one that has finished drawing, so a frame still
        // being held is one whose closing sequence is never coming. On a real
        // terminal that is a picture frozen until its own timeout gives up on
        // the frame, and it is the one failure a picture cannot show.
        assert!(
            !self.screen.is_holding(),
            "crucible held the screen for a frame it never finished, {step}\n{}",
            self.picture()
        );

        // And one that has drawn for the window it has: a size the window
        // took reaches the screen as its mark, and is held back from there
        // until crucible draws a frame for it, so a quiet screen still
        // waiting for one is a resize crucible never drew for — which no
        // picture shows, since the picture is still the old window.
        assert!(
            self.marked.is_none(),
            "the mark of the window's new size never came back off the terminal, {step}\n{}",
            self.picture()
        );
        if let Some((columns, rows)) = self.screen.awaiting() {
            panic!(
                "crucible went quiet without drawing for the window at {columns}x{rows}, {step}\n{}",
                self.picture()
            );
        }
    }

    /// Whether a quiet screen is one worth looking at yet.
    fn ready(&self, arrived: bool, awaited: Awaited<'_>) -> bool {
        match awaited {
            Awaited::AnyByte => arrived,
            Awaited::Mark(mark) => self.picture().contains(mark),
            Awaited::Nothing => true,
        }
    }
}

/// What a quiet screen has to show before a step is over.
#[derive(Debug, Clone, Copy)]
enum Awaited<'a> {
    /// A byte since the step began: what every key gets, since every key is
    /// drawn for.
    AnyByte,
    /// This text on screen, for a step drawn in several frames with work
    /// between them.
    Mark(&'a str),
    /// Nothing at all, for input crucible may rightly draw nothing for.
    Nothing,
}

/// The face at the front of the working row in `picture`, where there is one.
fn working_face(picture: &str) -> Option<char> {
    picture.lines().find_map(|line| {
        let face = line.trim_start_matches('|').trim_start().chars().next()?;
        matches!(face, '✳' | '✻' | '✺' | '✱').then_some(face)
    })
}

/// What a process has spent of the CPU, as Linux keeps two counts of it.
///
/// Each thread's `schedstat` says how long it has run, in nanoseconds, so what a
/// one-second window costs is read exactly rather than as how many `/proc` clock
/// ticks fell inside it. A thread that ends takes that count with it, though, and a
/// thread id reused inside the window reads as the thread that had it, so work in
/// short-lived threads can drop out of the sum. The process's user and system
/// ticks in `stat` keep what ended threads ran. Each of the two is rounded down on
/// its own, so a window they count `n` ticks across ran for more than `n - 2`
/// ticks, and one that ran `r` ticks is counted more than `r - 2`: work in threads
/// that end can go uncharged by up to four ticks a window. A window is charged
/// whichever reading is larger.
struct Spent {
    /// Nanoseconds each thread alive at the reading has run, by thread id.
    threads: BTreeMap<u32, u64>,
    /// User and system clock ticks for the whole process, ended threads included.
    ticks: u64,
}

impl Spent {
    /// Both counts for `pid`, now.
    fn by(pid: u32) -> Self {
        let mut threads = BTreeMap::new();
        for task in fs::read_dir(format!("/proc/{pid}/task"))
            .expect("Linux lists the threads of the watched process")
        {
            let task = task.expect("a thread of the watched process is listed");
            let thread = task
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
                .expect("a thread is listed by its id");
            // A thread that ended between the listing and this read leaves its
            // time only in the process's ticks, which still hold it. Any other
            // failure is not a thread that ended, and skipping it would charge a
            // live thread its whole life on the next reading.
            let schedstat = match fs::read_to_string(task.path().join("schedstat")) {
                Ok(schedstat) => schedstat,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) =>
                {
                    continue;
                }
                Err(error) => {
                    panic!("the schedstat of thread {thread} of {pid} could not be read: {error}")
                }
            };
            let ran = schedstat
                .split_whitespace()
                .next()
                .and_then(|field| field.parse::<u64>().ok())
                .expect("schedstat starts with the nanoseconds a thread has run");
            threads.insert(thread, ran);
        }
        assert!(
            threads.values().any(|ran| *ran > 0),
            "Linux reported no run time for any thread of {pid}, so its schedstat cannot measure this process"
        );

        Self {
            threads,
            ticks: ticks(pid),
        }
    }

    /// At least how much CPU the process spent between `earlier` and this reading.
    fn since(&self, earlier: &Self) -> Duration {
        let ran: u64 = self
            .threads
            .iter()
            .map(|(thread, ran)| {
                ran.saturating_sub(earlier.threads.get(thread).copied().unwrap_or(0))
            })
            .sum();
        let tick = 1_000_000_000 / rustix::param::clock_ticks_per_second();
        let whole = self.ticks.saturating_sub(earlier.ticks).saturating_sub(2);

        Duration::from_nanos(ran.max(tick.saturating_mul(whole)))
    }
}

/// User and system `/proc` clock ticks consumed by `pid`, ended threads included.
fn ticks(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .expect("Linux reports CPU accounting for the watched process");
    let (_, fields) = stat
        .rsplit_once(") ")
        .expect("/proc/<pid>/stat has a parenthesized process name");
    let mut fields = fields.split_whitespace();
    let user = fields
        .nth(11)
        .and_then(|field| field.parse::<u64>().ok())
        .expect("/proc/<pid>/stat has a user CPU field");
    let system = fields
        .next()
        .and_then(|field| field.parse::<u64>().ok())
        .expect("/proc/<pid>/stat has a system CPU field");
    user.saturating_add(system)
}

impl Drop for Watched {
    /// Takes the process and the directory back, however the case ended.
    ///
    /// In `Drop` rather than at the end of a case because the interesting exits
    /// are the other ones: a failed assertion unwinds, and a crucible left
    /// running would hold a terminal open and go on writing into a test run
    /// that has moved on.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.scratch);
    }
}

/// Opens the pair, sets its size, and puts the far side in raw mode unless
/// it is to start `cooked`.
fn pair(columns: u16, rows: u16, cooked: bool) -> (File, File) {
    let terminal = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .expect("a pseudo terminal");
    pty::grantpt(&terminal).expect("the far side is ours");
    pty::unlockpt(&terminal).expect("the far side is unlocked");

    let named = pty::ptsname(&terminal, Vec::new()).expect("the far side has a name");
    let inside = OpenOptions::new()
        .read(true)
        .write(true)
        .open(OsStr::from_bytes(named.as_bytes()))
        .expect("the far side opens");

    if !cooked {
        let mut mode = termios::tcgetattr(&inside).expect("the far side has a mode");
        mode.make_raw();
        termios::tcsetattr(&inside, OptionalActions::Now, &mode).expect("the far side goes raw");
    }
    termios::tcsetwinsize(&terminal, size(columns, rows)).expect("a window size");

    (File::from(terminal), inside)
}

/// Starts crucible against the far side of the pair, with nothing in its
/// environment that this test did not put there.
///
/// `HOME` is inside the scratch directory as well: `CRUCIBLE_CODE_HOME` is what
/// crucible reads, but a run that fell back for any reason must not fall back
/// into the developer's own files. `PATH` is the one thing carried over, since
/// it is how `setsid` is found.
///
/// `keyed` sets the variable a key is read from. It is not a key — the address
/// beside it is a socket on this machine, and what answers there wants nothing
/// signed — but crucible will not choose a provider without one, so a case with
/// something to ask needs the variable set to reach the provider at all.
fn start(
    scratch: &Path,
    keyed: bool,
    terminal: &TerminalFixture<'_>,
    launch: Option<&Launch<'_>>,
    inside: File,
) -> Child {
    let home = scratch.join("home");
    let workspace = working(scratch);
    let second = inside.try_clone().expect("a second handle on the far side");
    let third = inside.try_clone().expect("a third handle on the far side");

    Command::new("setsid")
        .arg("--ctty")
        .arg(env!("CARGO_BIN_EXE_crucible"))
        // Answering fixtures opt into their model exactly as a person does on
        // the command line. A configured credential makes a provider
        // reachable; it must not silently choose what answers.
        .args(keyed.then_some(["--model", "anthropic/claude-test-1"]).into_iter().flatten())
        .current_dir(&workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", scratch)
        .env("TERM", "xterm-256color")
        .envs(terminal.coloured().then_some(("COLORTERM", "truecolor")))
        .envs((!terminal.coloured()).then_some(("NO_COLOR", "1")))
        .env("CRUCIBLE_CODE_HOME", &home)
        .envs(keyed.then_some(("ANTHROPIC_API_KEY", "not-a-key-and-nothing-reads-it")))
        .envs(launch.map(|launch| launch.env).unwrap_or_default().iter().copied())
        .args(launch.map(|launch| launch.args).unwrap_or_default())
        .stdin(Stdio::from(inside))
        .stdout(Stdio::from(second))
        .stderr(Stdio::from(third))
        .spawn()
        .expect("setsid --ctty starts crucible; util-linux provides it")
}

/// Reads the terminal until it goes away, which is what killing crucible does.
///
/// A thread rather than a poll: `std` has no wait-with-timeout on a descriptor,
/// and a timeout is what makes "the screen has settled" something a test can
/// decide. The read ends when the last handle on the far side is closed, so
/// this ends with the process rather than outliving it.
fn read(mut terminal: File, sender: &Sender<Vec<u8>>) {
    let mut buffer = [0_u8; 8192];

    while let Ok(read) = terminal.read(&mut buffer) {
        if read == 0 {
            break;
        }
        if sender
            .send(buffer.get(..read).unwrap_or_default().to_vec())
            .is_err()
        {
            break;
        }
    }
}

/// What is written to the far side of the pair to mark, in crucible's
/// output, the point where the window took a new size.
///
/// An application program command, which crucible never writes and the screen
/// never sees: [`Watched::feed`] takes it out before drawing.
const MARK: &[u8] = b"\x1b_window\x1b\\";

/// A window size, in the shape the terminal takes one.
fn size(columns: u16, rows: u16) -> Winsize {
    Winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}
