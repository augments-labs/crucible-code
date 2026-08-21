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

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rustix::pty::{self, OpenptFlags};
use rustix::termios::{self, OptionalActions, Winsize};

use crate::screen::Screen;
use crate::vendor::Vendor;

/// How long the terminal must go without a byte before the screen is settled.
///
/// Generous, because the alternative to waiting long enough is a test that
/// fails one run in twenty and then gets switched off.
const QUIET: Duration = Duration::from_millis(400);

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
    /// The directory this case was given to itself.
    scratch: PathBuf,
}

impl Watched {
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
    /// `ansi` because the screen keeps text and drops every colour it is told,
    /// so what a snapshot is worth is the rows, and the sixteen a terminal
    /// already has are the fewest escapes to get to them.
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

    /// Starts crucible in a window that size and waits for it to finish
    /// drawing.
    fn started(case: &str, columns: u16, rows: u16, vendor: Option<&Vendor>, keyed: bool) -> Self {
        Self::configured(case, columns, rows, &document(vendor, None), keyed)
    }

    fn configured(case: &str, columns: u16, rows: u16, document: &str, keyed: bool) -> Self {
        // One flat directory per case, so the last thing a case does can take
        // the whole of what it made with it.
        let scratch = std::env::temp_dir().join(format!(
            "crucible-whole-screen-{}-{case}",
            std::process::id()
        ));
        let home = scratch.join("home");

        let workspace = working(&scratch);
        fs::create_dir_all(&home).expect("a scratch home directory");
        fs::create_dir_all(&workspace).expect("a scratch working directory");
        fs::write(home.join("config.json"), document).expect("a configuration file");

        let (terminal, inside) = pair(columns, rows);
        let child = start(&scratch, &home, &workspace, keyed, inside);
        let (sender, bytes) = mpsc::channel();
        let reading = terminal
            .try_clone()
            .expect("a second handle on the terminal");
        std::thread::spawn(move || read(reading, &sender));

        let mut window = Self {
            terminal,
            child,
            bytes,
            screen: Screen::new(columns as usize, rows as usize),
            scratch,
        };
        window.settle("crucible started", Some(READY));
        window
    }

    /// Types `keys` and waits for the screen to stop changing.
    pub(crate) fn types(&mut self, keys: &str) {
        self.terminal
            .write_all(keys.as_bytes())
            .expect("keys go to the terminal");
        self.settle(&format!("{keys:?} was typed"), None);
    }

    /// Types `keys` and waits until the screen contains `wanted` and settles.
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
    /// land and stops at the first one carrying `wanted`.
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

    /// Reads frames until `wanted` is on screen.
    pub(crate) fn catches(&mut self, step: &str, wanted: &str) {
        let deadline = Instant::now() + CEILING;

        while !self.picture().contains(wanted) {
            match self.bytes.recv_timeout(QUIET) {
                Ok(bytes) => self.screen.feed(&bytes),
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
                "no {wanted:?} was ever drawn after {step}, in {CEILING:?}\n{}",
                self.picture()
            );
        }
    }

    /// Changes the size of the window, the way dragging its corner would.
    ///
    /// The kernel is what tells crucible: setting the size on the near side of
    /// the pair raises `SIGWINCH` in the session on the far side of it.
    pub(crate) fn resize(&mut self, columns: u16, rows: u16) {
        termios::tcsetwinsize(&self.terminal, size(columns, rows)).expect("a new window size");
        self.screen.resize(columns as usize, rows as usize);
        self.settle(&format!("the window became {columns}x{rows}"), None);
    }

    /// The screen, ready to be compared against the one checked in beside it.
    pub(crate) fn picture(&self) -> String {
        self.screen.picture()
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
        let deadline = Instant::now() + CEILING;
        let mut arrived = false;

        loop {
            match self.bytes.recv_timeout(QUIET) {
                Ok(bytes) => {
                    self.screen.feed(&bytes);
                    arrived = true;
                }
                Err(RecvTimeoutError::Timeout) if self.ready(arrived, wanted) => break,
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
                wanted.map_or(String::new(), |mark| format!(" — no {mark:?} on it")),
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
    }

    /// Whether a quiet screen is one worth looking at yet.
    fn ready(&self, arrived: bool, wanted: Option<&str>) -> bool {
        wanted.map_or(arrived, |mark| self.picture().contains(mark))
    }
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

/// Opens the pair, sets its size, and puts the far side in raw mode.
fn pair(columns: u16, rows: u16) -> (File, File) {
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

    let mut mode = termios::tcgetattr(&inside).expect("the far side has a mode");
    mode.make_raw();
    termios::tcsetattr(&inside, OptionalActions::Now, &mode).expect("the far side goes raw");
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
fn start(scratch: &Path, home: &Path, workspace: &Path, keyed: bool, inside: File) -> Child {
    let second = inside.try_clone().expect("a second handle on the far side");
    let third = inside.try_clone().expect("a third handle on the far side");

    Command::new("setsid")
        .arg("--ctty")
        .arg(env!("CARGO_BIN_EXE_crucible"))
        // Answering fixtures opt into their model exactly as a person does on
        // the command line. A configured credential makes a provider
        // reachable; it must not silently choose what answers.
        .args(keyed.then_some(["--model", "anthropic/claude-test-1"]).into_iter().flatten())
        .current_dir(workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", scratch)
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .env("CRUCIBLE_CODE_HOME", home)
        .envs(keyed.then_some(("ANTHROPIC_API_KEY", "not-a-key-and-nothing-reads-it")))
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

/// A window size, in the shape the terminal takes one.
fn size(columns: u16, rows: u16) -> Winsize {
    Winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}
