//! The hostile corpus: diffs that are forged, truncated, past their caps or
//! not diffs at all, lines that carry escapes and overrides, and paths that
//! would mislead a reader.
//!
//! Every entry of `tests/corpus/` is drawn or shown as written as the table
//! below records, inside a deadline, without a panic, and every row it draws
//! fits the width it was drawn at and holds nothing a terminal would obey. A
//! runner that finds faults is only worth something once a planted fault has
//! been found by it, so a drawer that panics and one that does quadratic work
//! are each run through the same checks and must be caught.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a fixture not made or a check failed is the test's failure, in a helper too"
)]

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Draws, Laid, Room};
use crucible_tui::{Glyphs, Row};
use crucible_tui_diff::{Changes, MOST_BYTES, MOST_LINES};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");

/// Long enough for one draw of the largest entry in a debug build on a loaded
/// machine, and short against work that grows with the square of its input.
const DEADLINE: Duration = Duration::from_secs(5);

/// The width the record is drawn at: a common terminal's.
const WIDTH: usize = 80;

/// The widths a change is drawn at, both ways: a window narrowed from wide to
/// its narrowest and one widened back.
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

/// What draws a fence's text into a room: this crate's drawer, or a planted
/// stand-in the checks must catch.
type Drawer = fn(&str, &Room) -> Result<Laid, Undrawn>;

fn this_crate(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    Changes.block(source, room)
}

fn panics_on_a_rename(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    assert!(
        !source.contains("rename from"),
        "planted: a drawer that panics"
    );
    Changes.block(source, room)
}

/// Compares every byte with every byte after it before drawing: nothing a
/// small fixture shows, and seconds at the caps. It gives up on its own after
/// a while, so a runner that fails to cut it off cannot hang the suite.
fn quadratic(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    let begun = Instant::now();
    let bytes = source.as_bytes();
    let mut alike = 0_usize;
    for (at, byte) in bytes.iter().enumerate() {
        if begun.elapsed() > DEADLINE * 2 {
            break;
        }
        for other in bytes.iter().skip(at) {
            alike += usize::from(std::hint::black_box(byte == other));
        }
    }
    std::hint::black_box(alike);
    Changes.block(source, room)
}

// ---------------------------------------------------------------------------
// The corpus.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Drawn,
    Undrawn(Undrawn),
}

use Expect::{Drawn, Undrawn as Shown};

/// Each file of the corpus and what becomes of it. A file added without a row,
/// or a row left without its file, fails the check beside this table.
///
/// A file that is not a diff, or holds headers and nothing under them, is
/// shown as written with no refusal: it was never a change. One past a cap of
/// lines or files is shown as written and says so, whether its files open with
/// `diff --git` or `+++` and whether they hold lines or a binary note.
/// Everything else is drawn, however its counts were forged or its hunks cut
/// short: a hunk is read for what it holds, never for what it claims.
const ENTRIES: &[(&str, Expect)] = &[
    ("binary.diff", Drawn),
    ("empty.diff", Shown(Undrawn::NotHere)),
    ("far-hunks.diff", Drawn),
    ("forged-counts.diff", Drawn),
    ("headers-inside-a-hunk.diff", Drawn),
    ("headers-only.diff", Shown(Undrawn::NotHere)),
    ("hostile-lines.diff", Drawn),
    ("many-pairs.diff", Drawn),
    ("new-and-deleted.diff", Drawn),
    ("no-newline-at-end.diff", Drawn),
    ("one-long-pair.diff", Drawn),
    ("past-files-binary.diff", Shown(Undrawn::PastCap)),
    ("past-files.diff", Shown(Undrawn::PastCap)),
    ("past-lines.diff", Shown(Undrawn::PastCap)),
    ("path-bidi.diff", Drawn),
    ("path-controls.diff", Drawn),
    ("path-isolates.diff", Drawn),
    ("path-overlong.diff", Drawn),
    ("path-private-use.diff", Drawn),
    ("path-wide.diff", Drawn),
    ("prose.txt", Shown(Undrawn::NotHere)),
    ("rename.diff", Drawn),
    ("truncated-hunk.diff", Drawn),
    ("two-files.diff", Drawn),
];

fn corpus_files() -> Vec<String> {
    let mut files: Vec<String> = fs::read_dir(CORPUS)
        .expect("the corpus directory")
        .map(|entry| {
            entry
                .expect("a corpus entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    files
}

fn source(name: &str) -> String {
    fs::read_to_string(Path::new(CORPUS).join(name)).expect("a corpus file")
}

fn room(columns: usize, glyphs: Glyphs) -> Room {
    Room { columns, glyphs }
}

/// Whether a terminal would act on `character` rather than draw it, or draw it
/// so the characters around it read as something they are not: a control or
/// a format character. The line and paragraph separators are neither, and the
/// screen draws each as a column, as `crucible-tui` does everywhere; the
/// joiners are kept, since an emoji sequence needs them.
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

/// Everything a drawn change is held to at `columns`: no row past it, and no
/// row holding a character a terminal would obey.
fn check_rows(name: &str, rows: &[Row], columns: usize) -> Result<(), String> {
    for row in rows {
        if row.columns() > columns {
            return Err(format!(
                "{name}: a row of {} columns at {columns}: {:?}",
                row.columns(),
                row.text()
            ));
        }
        if let Some(found) = row.text().chars().find(|character| obeyed(*character)) {
            return Err(format!(
                "{name}: U+{:04X} drawn at {columns}: {:?}",
                u32::from(found),
                row.text()
            ));
        }
    }
    Ok(())
}

/// Every entry drawn by `drawer` at [`WIDTH`], each inside [`DEADLINE`], and
/// what went other than the record says.
fn what_a_drawer_does_with_the_corpus(drawer: Drawer) -> Vec<String> {
    let mut wrong = Vec::new();
    for (name, expect) in ENTRIES {
        let text = source(name);
        let ended = guarded(DEADLINE, move || {
            drawer(&text, &room(WIDTH, Glyphs::Unicode))
        });
        let drawn = match ended {
            Ended::Returned(drawn) => drawn,
            Ended::Panicked => {
                wrong.push(format!("{name}: panicked"));
                continue;
            }
            Ended::Hung => {
                wrong.push(format!("{name}: no answer in {DEADLINE:?}"));
                continue;
            }
        };
        match (expect, drawn) {
            (Drawn, Ok(laid)) => {
                if let Err(why) = check_rows(name, &laid.rows, WIDTH) {
                    wrong.push(why);
                }
            }
            (Shown(expected), Err(got)) if *expected == got => {}
            (expected, got) => wrong.push(format!("{name}: expected {expected:?}, got {got:?}")),
        }
    }
    wrong
}

#[test]
fn a_planted_panic_is_found_by_the_runner() {
    let ended = guarded(Duration::from_secs(5), || -> u8 {
        panic!("planted: a parser that panics")
    });
    assert_eq!(ended, Ended::Panicked);
}

#[test]
fn a_planted_unbounded_loop_is_found_by_the_runner() {
    let begun = Instant::now();
    let ended = guarded(Duration::from_millis(300), move || {
        // Unbounded as far as the runner can tell. It ends itself after a few
        // seconds, so a runner that fails to cut it off cannot hang the suite.
        while begun.elapsed() < Duration::from_secs(4) {
            thread::sleep(Duration::from_millis(1));
        }
    });
    assert_eq!(ended, Ended::Hung);
}

#[test]
fn every_file_of_the_corpus_is_drawn_or_shown_as_written_as_recorded() {
    let wrong = what_a_drawer_does_with_the_corpus(this_crate);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn no_file_of_the_corpus_is_missing_from_the_record_or_the_record_from_the_corpus() {
    let recorded: Vec<String> = ENTRIES.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(corpus_files(), recorded);
}

#[test]
fn a_planted_drawer_that_panics_is_found_by_the_corpus_run() {
    let wrong = what_a_drawer_does_with_the_corpus(panics_on_a_rename);
    assert!(
        wrong.iter().any(|why| why == "rename.diff: panicked"),
        "{wrong:#?}"
    );
}

#[test]
fn a_planted_drawer_doing_quadratic_work_is_found_by_the_corpus_run() {
    let wrong = what_a_drawer_does_with_the_corpus(quadratic);
    assert!(
        wrong.iter().any(|why| why.contains("no answer")),
        "{wrong:#?}"
    );
}

/// The largest diff the reader takes: as many lines as it keeps, in as many
/// bytes as it reads, every removed line paired with an added one so each is
/// read word by word.
fn at_the_caps() -> String {
    let mut text =
        String::from("--- a/at/the/caps.rs\n+++ b/at/the/caps.rs\n@@ -1,1000 +1,1000 @@\n");
    let pairs = (MOST_LINES - 8) / 2;
    let room = (MOST_BYTES - text.len()) / (pairs * 2) - 2;
    for side in ['-', '+'] {
        for at in 0..pairs {
            let words = format!("{side}let value_{at} = call(alpha, beta, {side}{at}); ");
            let line: String = words.chars().cycle().take(room).collect();
            text.push_str(line.trim_end());
            text.push('\n');
        }
    }
    text
}

#[test]
fn a_diff_at_the_caps_is_drawn_inside_the_deadline_and_the_planted_quadratic_one_is_not() {
    let worst = at_the_caps();
    assert!(worst.len() <= MOST_BYTES, "{} bytes", worst.len());
    let same = worst.clone();
    let ended = guarded(DEADLINE, move || {
        this_crate(&same, &room(WIDTH, Glyphs::Unicode)).map(|laid| laid.rows.len())
    });
    assert!(
        matches!(ended, Ended::Returned(Ok(rows)) if rows > 0),
        "{ended:?}"
    );
    let ended = guarded(DEADLINE, move || {
        quadratic(&worst, &room(WIDTH, Glyphs::Unicode)).map(|laid| laid.rows.len())
    });
    assert_eq!(ended, Ended::Hung);
}

#[test]
fn every_drawn_file_fits_each_width_from_the_widest_down_and_from_the_narrowest_up() {
    for (name, expect) in ENTRIES {
        if *expect != Drawn {
            continue;
        }
        let text = source(name);
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let mut narrowing = Vec::new();
            for columns in (NARROWEST..=WIDEST).rev() {
                let laid = Changes.block(&text, &room(columns, glyphs)).expect(name);
                if let Err(why) = check_rows(name, &laid.rows, columns) {
                    panic!("{why}");
                }
                narrowing.push((columns, laid.rows));
            }
            let mut widening = Vec::new();
            for columns in NARROWEST..=WIDEST {
                let laid = Changes.block(&text, &room(columns, glyphs)).expect(name);
                widening.push((columns, laid.rows));
            }
            widening.reverse();
            assert_eq!(
                narrowing, widening,
                "{name}: a width reached from the other side drew other rows"
            );
        }
    }
}

#[test]
fn a_misleading_path_is_drawn_with_what_would_mislead_replaced() {
    for (name, dropped) in [
        ("path-bidi.diff", "\u{202e}"),
        ("path-isolates.diff", "\u{2066}"),
        ("path-controls.diff", "\u{1b}"),
        ("path-private-use.diff", "\u{e000}"),
    ] {
        let laid = Changes
            .block(&source(name), &room(WIDEST, Glyphs::Unicode))
            .expect(name);
        let header = laid.rows.first().expect(name).text();
        assert!(header.starts_with("╭─ "), "{name}: {header:?}");
        assert!(!header.contains(dropped), "{name}: {header:?}");
        assert!(header.contains('\u{fffd}'), "{name}: {header:?}");
    }
}

#[test]
fn a_path_too_long_for_any_window_is_shortened_from_its_start() {
    let laid = Changes
        .block(
            &source("path-overlong.diff"),
            &room(WIDEST, Glyphs::Unicode),
        )
        .expect("an overlong path");
    let header = laid.rows.first().expect("a header").text();
    assert!(header.starts_with("╭─ …"), "{header:?}");
    assert!(header.contains("d/f.txt"), "{header:?}");
}

#[test]
fn a_diff_written_with_carriage_returns_is_drawn_as_one_written_without() {
    // Kept out of the corpus: the repository checks text out with its line
    // endings made plain, which would take the carriage returns away.
    let with = "--- a/x.txt\r\n+++ b/x.txt\r\n@@ -1,2 +1,2 @@\r\n kept\r\n-old\r\n+new\r\n";
    let without = with.replace('\r', "");
    let drawn = |text: &str| {
        Changes
            .block(text, &room(WIDTH, Glyphs::Unicode))
            .expect("a diff")
            .rows
    };
    assert_eq!(drawn(with), drawn(&without));
}

// ---------------------------------------------------------------------------
// The corpus in a child process, which must print nothing.

/// Draws the corpus at every width, and says nothing. It is run by its
/// parent, which reads what it printed.
fn the_whole_corpus() -> usize {
    let mut drawn = 0_usize;
    for (name, _) in ENTRIES {
        let text = source(name);
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for columns in NARROWEST..=WIDEST {
                drop(Changes.block(&text, &room(columns, glyphs)));
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
