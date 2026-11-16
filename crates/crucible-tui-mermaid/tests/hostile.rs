//! The hostile corpus: diagrams cut short, past their caps, carrying escapes
//! and overrides, written in a statement nothing reads, or of a kind not
//! drawn as text; and diagrams whose links, directives and styles name a
//! file, a pipe or an address.
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
use crucible_tui_mermaid::{Caps, Diagrams, MOST_BYTES};
use std::fmt::Write as _;
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
const DEADLINE: Duration = Duration::from_secs(10);
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

/// What draws a diagram into a room: this crate's drawer, or a planted
/// stand-in the run must catch.
type Drawer = fn(&str, &Room) -> Result<Laid, Undrawn>;

fn this_crate(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    Diagrams.block(source, room)
}

fn panics_on_a_decision(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    assert!(
        !source.contains("{Is it?}"),
        "planted: a drawer that panics"
    );
    Diagrams.block(source, room)
}

/// Work that grows as the square of the source: what a reader that rescans
/// from the start for every byte does. A diagram of a few hundred bytes costs
/// it little and one past the byte cap costs it a great deal. It stops
/// working half a minute after it was first called, so a runner that fails
/// to cut it off cannot hang the suite and the threads a runner leaves behind
/// stop spending.
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
    Diagrams.block(source, room)
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
    ("bidi-override.mmd", Drawn),
    ("box-drawing-label.mmd", Drawn),
    ("class.mmd", Shown(Undrawn::Unsupported("class diagram"))),
    ("click-and-style.mmd", Drawn),
    ("control-escape.mmd", Drawn),
    ("cut-class-first-line.mmd", Shown(Undrawn::NotText)),
    (
        "cut-class-label.mmd",
        Shown(Undrawn::Unsupported("class diagram")),
    ),
    ("cut-er-first-line.mmd", Shown(Undrawn::NotText)),
    ("cut-er-label.mmd", Shown(Undrawn::NotText)),
    ("cut-flowchart-first-line.mmd", Shown(Undrawn::NotText)),
    ("cut-flowchart-label.mmd", Shown(Undrawn::NotText)),
    ("cut-state-first-line.mmd", Shown(Undrawn::NotText)),
    ("cut-state-label.mmd", Shown(Undrawn::NotText)),
    ("empty.mmd", Shown(Undrawn::NotHere)),
    ("er.mmd", Drawn),
    ("flowchart-lr.mmd", Drawn),
    ("flowchart.mmd", Drawn),
    ("gantt.mmd", Shown(Undrawn::Unsupported("gantt"))),
    ("init-and-comments.mmd", Drawn),
    ("long-label.mmd", Shown(Undrawn::PastCap)),
    ("past-boxes.mmd", Shown(Undrawn::PastCap)),
    ("past-bytes.mmd", Shown(Undrawn::PastCap)),
    ("past-lines.mmd", Shown(Undrawn::PastCap)),
    (
        "sequence.mmd",
        Shown(Undrawn::Unsupported("sequence diagram")),
    ),
    ("state-v1.mmd", Drawn),
    ("state.mmd", Drawn),
    ("subgraph.mmd", Shown(Undrawn::Unsupported("groups"))),
    ("too-wide.mmd", Shown(Undrawn::TooWide)),
    ("unknown-statement.mmd", Shown(Undrawn::NotText)),
    ("wide-characters.mmd", Drawn),
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
    let wrong = what_a_drawer_does_with_the_corpus(panics_on_a_decision);
    assert!(
        wrong.iter().any(|line| line == "flowchart.mmd: panicked"),
        "{wrong:#?}"
    );
}

#[test]
fn a_planted_quadratic_drawer_is_found_by_the_corpus_run() {
    let wrong = what_a_drawer_does_with_the_corpus(quadratic);
    assert!(
        wrong
            .iter()
            .any(|line| line.starts_with("past-bytes.mmd: no answer")),
        "{wrong:#?}"
    );
}

/// An entity-relationship diagram at its kind's caps: every entity and every
/// relationship it may hold, its attributes and labels filling the bytes the
/// largest byte cap leaves.
fn at_the_caps() -> String {
    let Caps {
        bytes,
        nodes,
        edges,
        ..
    } = Caps::ENTITIES;
    let mut lines = vec!["erDiagram".to_owned()];
    for at in 0..edges {
        let (from, to) = (at % nodes, (at * 7 + 1) % nodes);
        lines.push(format!("E{from} ||--o{{ E{to} : \"漢字 and words\""));
    }
    let mut text = lines.join("\n");
    let mut entity = 0;
    while text.len() + 40 < bytes {
        let _ = write!(text, "\nE{} {{\nstring 字段_{entity}\n}}", entity % nodes);
        entity += 1;
    }
    text
}

#[test]
fn a_diagram_at_the_caps_is_answered_inside_the_deadline_and_the_planted_quadratic_is_not() {
    let text = at_the_caps();
    assert!(text.len() <= MOST_BYTES && text.len() + 40 >= Caps::ENTITIES.bytes);
    for (drawer, inside) in [(this_crate as Drawer, true), (quadratic as Drawer, false)] {
        let text = text.clone();
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

/// The entries the record says are drawn.
fn drawable() -> Vec<(&'static str, String)> {
    ENTRIES
        .iter()
        .filter(|(_, expect)| *expect == Drawn)
        .map(|(name, _)| (*name, source(name)))
        .collect()
}

#[test]
fn every_drawable_entry_is_drawn_alike_every_time_in_one_process() {
    for (name, text) in drawable() {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let first = this_crate(&text, &room(WIDEST, glyphs)).map(|laid| laid.rows);
            assert!(first.is_ok(), "{name}: {first:?}");
            for _ in 0..64 {
                let again = this_crate(&text, &room(WIDEST, glyphs)).map(|laid| laid.rows);
                assert_eq!(again, first, "{name} in {glyphs:?}");
            }
        }
    }
}

#[test]
fn under_ascii_every_row_of_the_corpus_is_ascii_or_written_in_its_source() {
    let mut wrong = Vec::new();
    for (name, _) in ENTRIES {
        let text = source(name);
        for columns in NARROWEST..=WIDEST {
            let Ok(laid) = this_crate(&text, &room(columns, Glyphs::Ascii)) else {
                continue;
            };
            for row in &laid.rows {
                let strays: String = row
                    .text()
                    .chars()
                    .filter(|character| !character.is_ascii() && !text.contains(*character))
                    .collect();
                if !strays.is_empty() {
                    wrong.push(format!(
                        "{name} at {columns}: {strays:?} in {:?}",
                        row.text()
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_diagram_in_a_fence_of_an_answer_is_drawn_and_its_refusal_shown_as_written() {
    let drawers = Drawers::default().with(Arc::new(Diagrams));
    let texts = |name: &str| -> Vec<String> {
        answered(
            &["```mermaid\n", &source(name), "```\n"],
            &drawers,
            room(WIDTH, Glyphs::Unicode),
        )
        .iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect()
    };

    let drawn = texts("flowchart.mmd");
    assert!(
        drawn.iter().any(|row| row == "┌─ mermaid · flowchart"),
        "{drawn:#?}"
    );
    assert!(
        drawn.iter().any(|row| row.contains("Rethink")),
        "{drawn:#?}"
    );
    assert!(!drawn.iter().any(|row| row.contains("-->")), "{drawn:#?}");

    let refused = texts("class.mmd");
    assert!(
        refused.iter().any(|row| row.contains("class diagram")
            && row.contains("not drawn as text")
            && row.contains("shown as written")),
        "{refused:#?}"
    );
    assert!(
        refused.iter().any(|row| row.contains("Animal <|-- Duck")),
        "{refused:#?}"
    );
}

#[test]
fn a_fence_cut_inside_its_first_line_or_a_label_is_shown_as_written() {
    let drawers = Drawers::default().with(Arc::new(Diagrams));
    for name in ENTRIES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| name.starts_with("cut-"))
    {
        let text = source(name);
        // The answer ends inside the fence: the stream was cut there.
        let rows: Vec<String> = answered(
            &["```mermaid\n", &text],
            &drawers,
            room(WIDTH, Glyphs::Unicode),
        )
        .iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect();
        let last = text.lines().last().unwrap_or_default().trim();
        assert!(
            rows.iter().any(|row| row.contains(last)),
            "{name}: {last:?} not shown in {rows:#?}"
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.starts_with("┌─ mermaid · ") && !row.contains("shown as written")),
            "{name}: drawn from a cut fence: {rows:#?}"
        );
    }
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
            r"\\.\pipe\crucible-tui-mermaid-{}-{}",
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

/// A valid statement in a file, which would change what is drawn if it were
/// read in where a reference names it: a box the diagram does not have.
const SENTINEL_MMD: &str = "A --> Elsewhere";

/// The file, the pipe and the address a reference can name.
struct Sentinels {
    dir: PathBuf,
    mmd: PathBuf,
    listener: Listener,
    pipe: Pipe,
}

impl Sentinels {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "crucible-tui-mermaid-sentinels-{}-{}",
            std::process::id(),
            MADE.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).expect("a directory for the sentinels");
        let mmd = dir.join("sentinel.mmd");
        fs::write(&mmd, SENTINEL_MMD).expect("a sentinel");
        let pipe = Pipe::new(&dir);
        Self {
            dir,
            mmd,
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
            ("@MMD@", Some(self.mmd.display().to_string())),
            ("@PORT@", Some(self.listener.port.to_string())),
            ("@PIPE@", cfg!(windows).then(|| pipe.clone())),
            ("@FIFO@", cfg!(unix).then_some(pipe)),
        ] {
            if out.contains(name) {
                out = out.replace(name, &value?);
            }
        }
        assert!(
            ["@MMD@", "@PORT@", "@PIPE@", "@FIFO@"]
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

/// What a way of drawing a diagram does with its source.
type Resolver = dyn Fn(&Sentinels, &str) -> Result<Laid, Undrawn>;

fn drawn_alone(_: &Sentinels, source: &str) -> Result<Laid, Undrawn> {
    Diagrams.block(source, &room(WIDTH, Glyphs::Unicode))
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

/// Reads the file a link names and puts what it holds in the diagram.
fn uses_what_it_reads(sentinels: &Sentinels, source: &str) -> Result<Laid, Undrawn> {
    let named = format!("click A \"{}\"", sentinels.mmd.display());
    let source = source.replace(
        &named,
        &fs::read_to_string(&sentinels.mmd).expect("the sentinel"),
    );
    drawn_alone(sentinels, &source)
}

/// Opens what a link names and throws away what it finds.
fn reads_and_discards(sentinels: &Sentinels, source: &str) -> Result<Laid, Undrawn> {
    for path in [&sentinels.pipe.path, &sentinels.mmd] {
        if source.contains(&path.display().to_string()) {
            drop(fs::read(path));
        }
    }
    drawn_alone(sentinels, source)
}

/// Connects to the address a link, a directive or a style names.
fn connects(sentinels: &Sentinels, source: &str) -> Result<Laid, Undrawn> {
    if source.contains(&format!("127.0.0.1:{}", sentinels.listener.port)) {
        drop(TcpStream::connect(("127.0.0.1", sentinels.listener.port)));
    }
    drawn_alone(sentinels, source)
}

#[test]
fn a_planted_resolver_that_uses_the_sentinel_changes_the_output_and_is_found() {
    let _alone = alone();
    let sentinels = Sentinels::new();
    let wrong = what_a_drawer_does_with_the_references(&sentinels, &uses_what_it_reads);
    assert!(
        wrong.iter().any(|line| line.starts_with("click-file.mmd")),
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

/// Draws the corpus and the references at every width, and says nothing. It
/// is run by its parent, which reads what it printed.
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
                drop(Diagrams.block(text, &room(columns, glyphs)));
                let _ = Diagrams.label("mermaid", text.lines().next().unwrap_or_default());
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
