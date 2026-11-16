//! The hostile corpus: formulas left open, past their caps, carrying escapes
//! and overrides, naming commands nothing knows, or whole documents; and
//! formulas that name a file, a pipe or an address.
//!
//! Every entry of `tests/corpus/` is drawn or shown as written as the table
//! below records, inside a deadline, without a panic, and every row it draws
//! fits the width it was drawn at and holds nothing a terminal would obey.
//! Every template of `tests/corpus/references/` is drawn as though what it
//! names were not there, or refused whole, and what it names records nothing.
//! A runner that finds faults is only worth something once a planted fault
//! has been found by it, so a drawer that panics, one that does quadratic
//! work, one that uses what a reference names, one that reads it, one that
//! connects to it and one that prints are each run through the same checks
//! and must be caught.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a fixture not made or a check failed is the test's failure, in a helper too"
)]

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Drawers, Draws, Laid, Room};
use crucible_tui::testing::read as answered;
use crucible_tui::{Glyphs, Row};
use crucible_tui_math::{Formulas, MOST_BYTES, MOST_COLUMNS, MOST_DEPTH};
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");

/// Long enough for a debug build on a loaded machine, and short against a
/// hang.
const DEADLINE: Duration = Duration::from_secs(5);
const WIDTH: usize = 80;
const NARROWEST: usize = 8;
const WIDEST: usize = 160;

// ---------------------------------------------------------------------------
// The runner: a panic and a hang are findings, not the test's own end.

#[derive(Debug, PartialEq, Eq)]
enum Ended<T> {
    Returned(T),
    Panicked,
    Hung,
}

/// `work` on a thread of its own, given `deadline` to answer. A thread that
/// outlives its deadline is left behind: nothing can stop it.
fn guarded<T: Send + 'static>(
    deadline: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Ended<T> {
    let (answer, heard) = mpsc::channel();
    thread::spawn(move || {
        // A send that fails means the runner has already given up.
        let _ = answer.send(work());
    });
    match heard.recv_timeout(deadline) {
        Ok(value) => Ended::Returned(value),
        Err(RecvTimeoutError::Timeout) => Ended::Hung,
        Err(RecvTimeoutError::Disconnected) => Ended::Panicked,
    }
}

/// What draws a formula into a room: this crate's drawer, or a planted
/// stand-in the run must catch.
type Drawer = fn(&str, &Room) -> Result<Laid, Undrawn>;

fn this_crate(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    Formulas.block(source, room)
}

fn panics_on_a_fraction(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    assert!(!source.contains(r"\frac"), "planted: a drawer that panics");
    Formulas.block(source, room)
}

/// Work that grows as the square of the source: what a reader that rescans
/// from the start for every byte does. A formula of a few hundred bytes costs
/// it little and one at the byte cap costs it a great deal. It stops working
/// half a minute after it was first called, so a runner that fails to cut it
/// off cannot hang the suite and the threads a runner leaves behind stop
/// spending.
fn quadratic(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    static FIRST: OnceLock<Instant> = OnceLock::new();
    let first = *FIRST.get_or_init(Instant::now);
    let bytes = source.as_bytes();
    let mut same = 0_u64;
    for (at, left) in bytes.iter().enumerate() {
        if first.elapsed() > Duration::from_secs(30) {
            break;
        }
        for right in bytes.iter().skip(at) {
            for _ in 0..8 {
                if std::hint::black_box(left) == std::hint::black_box(right) {
                    same = same.wrapping_add(1);
                }
            }
        }
    }
    std::hint::black_box(same);
    Formulas.block(source, room)
}

// ---------------------------------------------------------------------------
// The corpus and what each entry is.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Drawn,
    Shown(Undrawn),
}

use Expect::{Drawn, Shown};

/// Each entry of the corpus and what it is at [`WIDTH`] columns.
const ENTRIES: &[(&str, Expect)] = &[
    ("aligned.tex", Drawn),
    ("bidi-override.tex", Shown(Undrawn::NotText)),
    ("cases.tex", Drawn),
    ("comment.tex", Shown(Undrawn::NotText)),
    ("control-escape.tex", Shown(Undrawn::NotText)),
    ("deep-nesting.tex", Shown(Undrawn::PastCap)),
    ("document.tex", Shown(Undrawn::NotText)),
    ("dollar-inside-text.tex", Shown(Undrawn::NotText)),
    ("empty.tex", Shown(Undrawn::NotHere)),
    ("integral.tex", Drawn),
    ("matrix.tex", Drawn),
    ("nested-fractions.tex", Drawn),
    ("osc-link.tex", Shown(Undrawn::NotText)),
    ("past-bytes.tex", Shown(Undrawn::PastCap)),
    ("quadratic.tex", Drawn),
    ("stray-close.tex", Shown(Undrawn::NotText)),
    ("tall-matrix.tex", Shown(Undrawn::PastCap)),
    ("tikz.tex", Shown(Undrawn::NotText)),
    ("too-wide.tex", Shown(Undrawn::TooWide)),
    ("unclosed-environment.tex", Shown(Undrawn::NotText)),
    ("unclosed-group.tex", Shown(Undrawn::NotText)),
    ("unknown-command.tex", Shown(Undrawn::NotText)),
    ("wide-characters.tex", Drawn),
    ("zero-width.tex", Shown(Undrawn::NotText)),
];

fn corpus_files() -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(CORPUS)
        .expect("the corpus")
        .map(|entry| entry.expect("an entry"))
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

fn source(name: &str) -> String {
    fs::read_to_string(Path::new(CORPUS).join(name)).expect(name)
}

fn room(columns: usize, glyphs: Glyphs) -> Room {
    Room { columns, glyphs }
}

/// Whether a terminal would act on `character` rather than draw it, or draw it
/// so the characters around it read as something they are not.
fn obeyed(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{61c}'
                | '\u{200b}'
                | '\u{200e}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}'
                | '\u{feff}'
        )
}

fn check_rows(rows: &[Row], columns: usize) -> Result<(), String> {
    for row in rows {
        if row.columns() > columns {
            return Err(format!(
                "a row of {} columns at {columns}: {:?}",
                row.columns(),
                row.text()
            ));
        }
        if let Some(found) = row.text().chars().find(|character| obeyed(*character)) {
            return Err(format!(
                "U+{:04X} drawn: {:?}",
                u32::from(found),
                row.text()
            ));
        }
    }
    Ok(())
}

/// Every entry drawn at every width from [`NARROWEST`] to [`WIDEST`] in both
/// sets, each inside the deadline, and at [`WIDTH`] as recorded. What went
/// wrong, in words.
fn what_a_drawer_does_with_the_corpus(drawer: Drawer) -> Vec<String> {
    let mut wrong = Vec::new();
    for (name, expect) in ENTRIES {
        let text = source(name);
        let expect = *expect;
        let ended = guarded(DEADLINE, move || {
            let mut found = Vec::new();
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                for columns in NARROWEST..=WIDEST {
                    let drawn = drawer(&text, &room(columns, glyphs));
                    if let Ok(laid) = &drawn
                        && let Err(why) = check_rows(&laid.rows, columns)
                    {
                        found.push(why);
                    }
                    if columns == WIDTH {
                        let was = match drawn {
                            Ok(_) => Drawn,
                            Err(why) => Shown(why),
                        };
                        if was != expect {
                            found.push(format!("{was:?} at {WIDTH}, not {expect:?}"));
                        }
                    }
                }
            }
            found
        });
        match ended {
            Ended::Returned(found) => {
                wrong.extend(found.into_iter().map(|why| format!("{name}: {why}")));
            }
            Ended::Panicked => wrong.push(format!("{name}: panicked")),
            Ended::Hung => wrong.push(format!("{name}: no answer in {DEADLINE:?}")),
        }
    }
    wrong
}

#[test]
fn a_planted_panic_is_found_by_the_runner() {
    let ended = guarded(Duration::from_secs(5), || -> u8 {
        panic!("planted: a reader that panics")
    });
    assert_eq!(ended, Ended::Panicked);
}

#[test]
fn a_planted_unbounded_loop_is_found_by_the_runner() {
    let begun = Instant::now();
    let ended = guarded(Duration::from_millis(300), move || {
        // It ends itself after a few seconds, so a runner that fails to cut it
        // off cannot hang the suite.
        while begun.elapsed() < Duration::from_secs(4) {
            thread::sleep(Duration::from_millis(1));
        }
    });
    assert_eq!(ended, Ended::Hung);
}

#[test]
fn every_entry_of_the_corpus_is_drawn_or_shown_as_written_as_recorded() {
    let wrong = what_a_drawer_does_with_the_corpus(this_crate);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn the_corpus_and_its_record_name_the_same_files() {
    let recorded: Vec<String> = ENTRIES.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(corpus_files(), recorded);
}

#[test]
fn a_planted_panic_is_found_by_the_corpus_run() {
    let wrong = what_a_drawer_does_with_the_corpus(panics_on_a_fraction);
    assert!(
        wrong.iter().any(|line| line == "quadratic.tex: panicked"),
        "{wrong:#?}"
    );
}

#[test]
fn a_planted_quadratic_drawer_is_found_by_the_corpus_run() {
    let wrong = what_a_drawer_does_with_the_corpus(quadratic);
    assert!(
        wrong
            .iter()
            .any(|line| line.starts_with("past-bytes.tex: no answer")),
        "{wrong:#?}"
    );
}

#[test]
fn a_formula_at_the_byte_cap_is_drawn_inside_the_deadline_and_the_planted_quadratic_is_not() {
    let at_the_cap: String = "x + ".repeat(MOST_BYTES / 4);
    assert_eq!(at_the_cap.len(), MOST_BYTES);
    for (drawer, inside) in [(this_crate as Drawer, true), (quadratic as Drawer, false)] {
        let text = at_the_cap.clone();
        let ended = guarded(DEADLINE, move || {
            let mut drawn = 0_usize;
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                for columns in NARROWEST..=WIDEST {
                    drawn += usize::from(drawer(&text, &room(columns, glyphs)).is_ok());
                }
            }
            drawn
        });
        assert_eq!(matches!(ended, Ended::Returned(_)), inside, "{ended:?}");
    }
}

#[test]
fn a_continued_fraction_nested_thirty_deep_is_drawn_in_a_line_and_answered_as_a_block_inside_the_deadline()
 {
    const DEEP: usize = 30;
    let mut text = String::from("x = ");
    text.push_str(&r"1 + \frac{1}{".repeat(DEEP));
    text.push('y');
    text.push_str(&"}".repeat(DEEP));
    let ended = guarded(DEADLINE, move || {
        let mut in_a_line = Vec::new();
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let room = room(WIDEST, glyphs);
            in_a_line.push(Formulas.inline(&text, &room).is_some());
            // Too tall to lay out at its depth: what is asked is an answer.
            drop(Formulas.block(&text, &room));
        }
        in_a_line
    });
    assert_eq!(ended, Ended::Returned(vec![true, true]));
}

/// `levels` matrices each inside the last, the innermost holding as many
/// letters as the byte cap leaves room for.
fn nested_matrices(levels: usize) -> String {
    let (open, close, letter) = (r"\begin{matrix}", r"\end{matrix}", r"\alpha");
    let ends = levels * (open.len() + close.len());
    let letters = MOST_BYTES.saturating_sub(ends) / letter.len();
    format!(
        "{}{}{}",
        open.repeat(levels),
        letter.repeat(letters),
        close.repeat(levels)
    )
}

#[test]
fn matrices_nested_to_the_byte_cap_are_answered_inside_the_deadline_and_drawn_only_within_the_depth()
 {
    // As deep as a letter inside them is read, and as deep as the byte cap
    // allows, which is more than twice past it. Each level of the first is
    // laid out over the whole formula, so it is drawn at the narrowest, the
    // usual and the widest room; the second is refused before its end is
    // looked for, so it is answered at every width between.
    let within = nested_matrices(MOST_DEPTH - 1);
    let past = nested_matrices(MOST_BYTES / r"\begin{matrix}\end{matrix}".len());
    assert!(within.len() <= MOST_BYTES && past.len() <= MOST_BYTES);
    assert!(past.matches(r"\begin").count() > 2 * MOST_DEPTH);
    let ended = guarded(DEADLINE, move || {
        let mut answered = Vec::new();
        let every: Vec<usize> = (NARROWEST..=WIDEST).collect();
        for (text, widths) in [(&within, vec![NARROWEST, WIDTH, WIDEST]), (&past, every)] {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                for &columns in &widths {
                    let room = room(columns, glyphs);
                    drop(Formulas.inline(text, &room));
                    drop(this_crate(text, &room));
                }
                answered.push(this_crate(text, &room(MOST_COLUMNS + 2, glyphs)).err());
            }
        }
        answered
    });
    let past_cap = Some(Undrawn::PastCap);
    assert_eq!(ended, Ended::Returned(vec![None, None, past_cap, past_cap]));
}

#[test]
fn a_formula_in_a_fence_of_an_answer_is_drawn_and_its_refusal_shown_as_written() {
    let drawers = Drawers::default().with(Arc::new(Formulas));
    let rows = answered(
        &["```math\n", &source("quadratic.tex"), "```\n"],
        &drawers,
        room(WIDTH, Glyphs::Unicode),
    );
    let texts: Vec<String> = rows
        .iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect();
    assert!(texts.iter().any(|row| row == "┌─ math"), "{texts:#?}");
    assert!(texts.iter().any(|row| row.contains("x = ─")), "{texts:#?}");

    let rows = answered(
        &["```latex\n", &source("document.tex"), "```\n"],
        &drawers,
        room(WIDTH, Glyphs::Unicode),
    );
    let texts: Vec<String> = rows
        .iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect();
    assert!(
        texts
            .iter()
            .any(|row| row.contains("not drawn as text") && row.contains("shown as written")),
        "{texts:#?}"
    );
    assert!(
        texts
            .iter()
            .any(|row| row.contains(r"\documentclass{article}")),
        "{texts:#?}"
    );
}

// ---------------------------------------------------------------------------
// The sentinels: what a reference points at.

static MADE: AtomicUsize = AtomicUsize::new(0);

/// One sentinel test at a time: each listens and makes files of its own, and
/// a planted resolver must find its own sentinel and no other test's.
fn alone() -> MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A sentinel that records being touched. It records from a thread of its own,
/// and `end` stops it.
struct Watch {
    seen: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    fn seen(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }

    /// Waits up to `within` for the sentinel to record something.
    fn seen_within(&self, within: Duration) -> bool {
        let begun = Instant::now();
        while begun.elapsed() < within {
            if self.seen() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        self.seen()
    }

    fn end(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A TCP listener on loopback, in every platform's reach: it records an
/// accepted connection.
struct Listener {
    port: u16,
    watch: Watch,
}

impl Listener {
    fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port");
        listener
            .set_nonblocking(true)
            .expect("a listener that does not block");
        let port = listener.local_addr().expect("an address").port();
        let seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (heard, told) = (Arc::clone(&seen), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            while !told.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok(_) => heard.store(true, Ordering::SeqCst),
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            port,
            watch: Watch {
                seen,
                stop,
                thread: Some(thread),
            },
        }
    }
}

/// A named pipe the file system calls a file: on Unix a FIFO whose writer
/// records an open, on Windows a pipe whose server records a connect.
struct Pipe {
    path: PathBuf,
    watch: Watch,
}

#[cfg(unix)]
impl Pipe {
    fn new(dir: &Path) -> Self {
        let path = dir.join("sentinel.fifo");
        let made = Command::new("mkfifo").arg(&path).status().expect("mkfifo");
        assert!(made.success(), "a FIFO to point at");
        let seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (heard, told, at) = (Arc::clone(&seen), Arc::clone(&stop), path.clone());
        // Opening a FIFO for writing waits for a reader: it returns only when
        // something opens the FIFO to read it.
        let thread = thread::spawn(move || {
            if fs::OpenOptions::new().write(true).open(at).is_ok() && !told.load(Ordering::SeqCst) {
                heard.store(true, Ordering::SeqCst);
            }
        });
        Self {
            path,
            watch: Watch {
                seen,
                stop,
                thread: Some(thread),
            },
        }
    }

    /// The writer waits in `open` for a reader, and a reader of our own lets it
    /// go. It is told to stop first, so it records nothing for ours.
    fn end(&mut self) {
        self.watch.end();
        let at = self.path.clone();
        let release = thread::spawn(move || {
            // Blocks for ever if the writer has already gone: a thread left
            // behind, not a hang of the test.
            drop(fs::File::open(at));
        });
        let begun = Instant::now();
        while begun.elapsed() < Duration::from_secs(2)
            && self
                .watch
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
        {
            thread::sleep(Duration::from_millis(5));
        }
        drop(release);
    }
}

#[cfg(windows)]
impl Pipe {
    fn new(_dir: &Path) -> Self {
        use tokio::net::windows::named_pipe::ServerOptions;
        let path = PathBuf::from(format!(
            r"\\.\pipe\crucible-tui-math-{}-{}",
            std::process::id(),
            MADE.fetch_add(1, Ordering::SeqCst)
        ));
        let seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (heard, told, at) = (Arc::clone(&seen), Arc::clone(&stop), path.clone());
        let (ready, created) = mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime for the pipe");
            runtime.block_on(async {
                let server = ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(&at)
                    .expect("a named pipe to point at");
                ready.send(()).expect("the test is waiting");
                while !told.load(Ordering::SeqCst) {
                    // A connect that is cut off here is still a connect to
                    // wait for on the next turn.
                    let waited =
                        tokio::time::timeout(Duration::from_millis(20), server.connect()).await;
                    if matches!(waited, Ok(Ok(()))) {
                        heard.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            });
        });
        created
            .recv_timeout(Duration::from_secs(10))
            .expect("the pipe is created");
        Self {
            path,
            watch: Watch {
                seen,
                stop,
                thread: Some(thread),
            },
        }
    }

    fn end(&mut self) {
        self.watch.end();
    }
}

/// A valid formula in a file, which would change what is drawn if it were
/// read in where a reference names it.
const SENTINEL_TEX: &str = r"\alpha + \beta";

/// The file, the pipe and the address a reference can name.
struct Sentinels {
    dir: PathBuf,
    tex: PathBuf,
    listener: Listener,
    pipe: Pipe,
}

impl Sentinels {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "crucible-tui-math-sentinels-{}-{}",
            std::process::id(),
            MADE.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).expect("a directory for the sentinels");
        let tex = dir.join("sentinel.tex");
        fs::write(&tex, SENTINEL_TEX).expect("a sentinel");
        let pipe = Pipe::new(&dir);
        Self {
            dir,
            tex,
            listener: Listener::new(),
            pipe,
        }
    }

    /// A template with the sentinels put in. `None` for a template whose
    /// sentinel this platform has no way to make.
    fn fill(&self, template: &str) -> Option<String> {
        let pipe = self.pipe.path.display().to_string();
        let mut out = template.to_owned();
        for (name, value) in [
            ("@TEX@", Some(self.tex.display().to_string())),
            ("@PORT@", Some(self.listener.port.to_string())),
            ("@PIPE@", cfg!(windows).then(|| pipe.clone())),
            ("@FIFO@", cfg!(unix).then_some(pipe)),
        ] {
            if out.contains(name) {
                out = out.replace(name, &value?);
            }
        }
        assert!(
            ["@TEX@", "@PORT@", "@PIPE@", "@FIFO@"]
                .iter()
                .all(|name| !out.contains(name)),
            "a placeholder no sentinel fills: {out}"
        );
        Some(out)
    }

    /// The sentinels that recorded something, by name.
    fn touched(&self, within: Duration) -> Vec<&'static str> {
        let mut touched = Vec::new();
        if self.listener.watch.seen_within(within) {
            touched.push("a connection was accepted on 127.0.0.1");
        }
        if self.pipe.watch.seen_within(within) {
            touched.push(if cfg!(windows) {
                "the named pipe was connected to"
            } else {
                "the FIFO was opened"
            });
        }
        touched
    }
}

impl Drop for Sentinels {
    fn drop(&mut self) {
        self.listener.watch.end();
        self.pipe.end();
        if let Some(thread) = self.listener.watch.thread.take() {
            drop(thread.join());
        }
        drop(fs::remove_dir_all(&self.dir));
    }
}

/// The templates of `tests/corpus/references/`, by file name.
fn templates() -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = fs::read_dir(Path::new(CORPUS).join("references"))
        .expect("the references")
        .map(|entry| {
            let entry = entry.expect("a reference");
            let text = fs::read_to_string(entry.path()).expect("a reference as text");
            (entry.file_name().to_string_lossy().into_owned(), text)
        })
        .collect();
    found.sort();
    found
}

/// The template as it is with the reference in it, and as it is with the
/// reference taken out.
fn with_and_without(filled: &str) -> (String, String) {
    let (Some((before, rest)), Some((_, after))) = (
        filled.split_once("[[reference]]"),
        filled.split_once("[[/reference]]"),
    ) else {
        panic!("a template without its reference marked: {filled}");
    };
    let (inside, _) = rest.split_once("[[/reference]]").expect("a closed marker");
    (
        format!("{before}{inside}{after}"),
        format!("{before}{after}"),
    )
}

/// What a way of drawing a formula does with its text.
type Resolver = dyn Fn(&Sentinels, &str) -> Result<Laid, Undrawn>;

fn drawn_alone(_: &Sentinels, tex: &str) -> Result<Laid, Undrawn> {
    Formulas.block(tex, &room(WIDTH, Glyphs::Unicode))
}

/// Every reference refused whole, or drawn as though it were not there. What
/// went wrong, in words.
fn what_a_drawer_does_with_the_references(sentinels: &Sentinels, drawer: &Resolver) -> Vec<String> {
    let mut wrong = Vec::new();
    let mut read = 0;
    for (name, template) in templates() {
        let Some(filled) = sentinels.fill(&template) else {
            continue;
        };
        let (with, without) = with_and_without(&filled);
        let (with, without) = (drawer(sentinels, &with), drawer(sentinels, &without));
        let rows = |laid: &Laid| -> Vec<String> { laid.rows.iter().map(Row::text).collect() };
        match (&with, &without) {
            // Refused whole: shown as written, naming what it names, opening
            // nothing.
            (Err(_), _) => {}
            (Ok(with), Ok(without)) if rows(with) == rows(without) => {}
            _ => wrong.push(format!(
                "{name}: drew {:?} where the reference removed draws {:?}",
                with.as_ref().map(rows),
                without.as_ref().map(rows)
            )),
        }
        read += 1;
    }
    // The four templates every platform fills, and the pipe of its own.
    assert!(read >= 5, "only {read} templates read");
    wrong
}

#[test]
fn every_reference_is_refused_or_drawn_as_if_it_were_not_there_and_what_it_names_records_nothing() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &drawn_alone);
    assert!(wrong.is_empty(), "{wrong:#?}");
    let touched = sentinels.touched(Duration::from_millis(300));
    assert!(touched.is_empty(), "{touched:?}");
}

// Stand-ins for a resolver that does what this crate must not, wrapped around
// the real drawing so that the same checks judge them.

/// Reads the file a reference names and puts it in the formula.
fn uses_what_it_reads(sentinels: &Sentinels, tex: &str) -> Result<Laid, Undrawn> {
    let named = format!(r"\input{{{}}}", sentinels.tex.display());
    let tex = tex.replace(
        &named,
        &fs::read_to_string(&sentinels.tex).expect("the sentinel"),
    );
    drawn_alone(sentinels, &tex)
}

/// Opens what a reference names and throws away what it finds.
fn reads_and_discards(sentinels: &Sentinels, tex: &str) -> Result<Laid, Undrawn> {
    for path in [&sentinels.pipe.path, &sentinels.tex] {
        if tex.contains(&path.display().to_string()) {
            drop(fs::read(path));
        }
    }
    drawn_alone(sentinels, tex)
}

/// Connects to the address a reference names.
fn connects(sentinels: &Sentinels, tex: &str) -> Result<Laid, Undrawn> {
    if tex.contains(&format!("127.0.0.1:{}", sentinels.listener.port)) {
        drop(TcpStream::connect(("127.0.0.1", sentinels.listener.port)));
    }
    drawn_alone(sentinels, tex)
}

#[test]
fn a_planted_resolver_that_uses_the_sentinel_changes_the_output_and_is_found() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &uses_what_it_reads);
    assert!(
        wrong.iter().any(|line| line.starts_with("input-file.tex")),
        "{wrong:#?}"
    );
}

#[test]
fn a_planted_resolver_that_reads_and_discards_is_found_by_the_pipe_that_was_opened() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &reads_and_discards);
    assert!(
        wrong.is_empty(),
        "discarding what is read changes no output, so the pipe is what finds it: {wrong:#?}"
    );
    let touched = sentinels.touched(Duration::from_secs(5));
    let expected = if cfg!(windows) {
        "the named pipe was connected to"
    } else {
        "the FIFO was opened"
    };
    assert!(touched.contains(&expected), "{touched:?}");
}

#[test]
fn a_planted_resolver_that_connects_is_found_by_the_listener_that_accepted() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &connects);
    assert!(wrong.is_empty(), "{wrong:#?}");
    let touched = sentinels.touched(Duration::from_secs(5));
    assert!(
        touched.contains(&"a connection was accepted on 127.0.0.1"),
        "{touched:?}"
    );
}

// ---------------------------------------------------------------------------
// The corpus in a child process, which must print nothing.

/// Draws the corpus and the references at every width, in a line and in a
/// block, and says nothing. It is run by its parent, which reads what it
/// printed.
fn the_whole_corpus() -> usize {
    let sentinels = Sentinels::new();
    let mut texts: Vec<String> = ENTRIES.iter().map(|(name, _)| source(name)).collect();
    texts.extend(
        templates()
            .iter()
            .filter_map(|(_, template)| sentinels.fill(template))
            .map(|filled| with_and_without(&filled).0),
    );
    let mut drawn = 0_usize;
    for text in &texts {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for columns in NARROWEST..=WIDEST {
                drop(Formulas.block(text, &room(columns, glyphs)));
                drop(Formulas.inline(text, &room(columns, glyphs)));
                drawn += 1;
            }
        }
    }
    drawn
}

#[test]
#[ignore = "run by its parent, in a child process"]
fn child_draws_the_corpus() {
    assert!(the_whole_corpus() > ENTRIES.len());
}

// The stand-in for a dependency that prints: it draws the same corpus and then
// writes to the standard error behind an allow.
#[test]
#[ignore = "run by its parent, in a child process, to be found printing"]
fn child_draws_the_corpus_and_prints() {
    assert!(the_whole_corpus() > ENTRIES.len());
    #[allow(
        clippy::print_stderr,
        reason = "planted: the parent must find this line"
    )]
    {
        eprintln!("planted: a dependency that prints a diagnostic");
    }
}

/// What a test binary prints around one test that prints nothing.
fn only_the_harness_printed(stdout: &str) -> bool {
    stdout.lines().all(|line| {
        line.is_empty()
            || line == "running 1 test"
            || (line.starts_with("test child_draws_the_corpus") && line.ends_with(" ... ok"))
            || line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;")
    })
}

struct Child {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Child {
    fn was_silent(&self) -> bool {
        self.stderr.is_empty() && only_the_harness_printed(&self.stdout)
    }
}

fn run_child(test: &str) -> Child {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--ignored",
            "--exact",
            test,
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .expect("a child process");
    Child {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn the_whole_corpus_drawn_in_a_child_process_prints_nothing() {
    let child = run_child("child_draws_the_corpus");
    assert!(child.ok, "{}\n{}", child.stdout, child.stderr);
    assert!(
        child.stdout.contains("test child_draws_the_corpus ... ok"),
        "the child did not run the corpus: {}",
        child.stdout
    );
    assert!(
        child.was_silent(),
        "stdout: {:?}, stderr: {:?}",
        child.stdout,
        child.stderr
    );
}

#[test]
fn a_planted_diagnostic_behind_an_allow_is_found_by_the_child_process_check() {
    let child = run_child("child_draws_the_corpus_and_prints");
    assert!(child.ok, "{}\n{}", child.stdout, child.stderr);
    assert!(!child.was_silent(), "the planted line was not seen");
    assert!(child.stderr.contains("planted"), "{}", child.stderr);
}
