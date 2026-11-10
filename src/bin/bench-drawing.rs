//! Time to lay out the worst block a drawer is handed, at its caps.
//!
//! A drawer runs on the drawing thread when its block closes, so whatever it
//! takes is a frame the screen did not draw. The hostile corpus shows a drawer
//! ends on every input it is given; this measures how long it takes on the
//! largest one it accepts, which is the frame a reader waits on when a model
//! hands over the biggest change the reader keeps.
//!
//! Each case is the most a drawer takes at once: a unified diff at the reader's
//! line and byte caps; the same bytes spread over as many files as a change
//! holds, so every file opens its own box and folds on its own; the longest
//! lines a change holds, each wider than any room; as many word pairs as a
//! change compares, each of as many words as a line may have and none in
//! common; as many lines and bytes as a change reads for colour; every one of
//! those caps reached in the one change; and a tool's edit with as many
//! numbered lines as a file keeps. Each is laid out at a common terminal's
//! width, at the narrowest a drawer is given, and in both glyph sets. Two
//! milliseconds is the budget for text laid out on the drawing thread: a tenth
//! of the first frame's twenty, so a block closing beside a streaming answer
//! still leaves the frame its room.
//!
//! The reported number is the slowest case's 95th percentile, with each case's
//! median and its slowest reading kept as evidence. Every reading is one call,
//! timed with the input already built: the reading is the drawer's, not the
//! fixture's.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crucible_tui::Glyphs;
use crucible_tui::drawing::{Draws, Room};
use crucible_tui_diff::{
    Change, Changes, FOLD, File, MOST_BYTES, MOST_FILES, MOST_LINES, PAIRS, SYNTAX_BYTES,
    SYNTAX_LINES, Side, TOKENS,
};

/// The budget on a quiet machine, in milliseconds.
const LIMIT: f64 = 2.0;

/// The limit on a shared CI runner, in milliseconds.
///
/// A runner shares its cores, and its clock reads whatever else it is doing
/// in the middle of a reading. The first frame's runner limit stands at seven
/// and a half times its budget for that reason, and this one at the same
/// ratio: it still stops a layout grown many times slower, and a smaller
/// slowdown is the quiet-machine run's to catch, under [`LIMIT`], before a
/// release.
const SHARED_RUNNER_LIMIT: f64 = 15.0;

/// Readings taken of each case, after the ones that warm it.
const RUNS: usize = 101;

/// Readings thrown away first, so the first call's allocations and caches are
/// not charged to the drawer.
const WARM: usize = 5;

/// A common terminal's width.
const WIDE: usize = 80;

/// The narrowest a drawer is given a block to lay out in.
const NARROW: usize = 8;

#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("bench-drawing: {0}")]
    Io(#[from] io::Error),
    #[error("bench-drawing: CRUCIBLE_BENCH_RUNNER is {0:?}; want it unset, or `shared`")]
    Runner(String),
    #[error("bench-drawing: {0} was not drawn; a reading of it would time a refusal")]
    Undrawn(&'static str),
}

/// What a case hands the drawer.
enum Input {
    /// A unified diff, read and laid out as a fenced block is.
    Diff(String),
    /// A change already built, laid out as a tool's edit is.
    Change(Change),
}

struct Case {
    name: &'static str,
    input: Input,
}

/// One case's readings, sorted.
struct Timed {
    name: &'static str,
    readings: Vec<Duration>,
}

impl Timed {
    fn at(&self, fraction: f64) -> f64 {
        // Readings are many and few enough that the index is exact in an f64,
        // and it is clamped to the last one.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "an index into at most a hundred and one readings"
        )]
        let index = ((self.readings.len() as f64 - 1.0) * fraction).round() as usize;
        self.readings
            .get(index)
            .or_else(|| self.readings.last())
            .map_or(0.0, |reading| reading.as_secs_f64() * 1000.0)
    }
}

/// A removed line and the added line paired with it, as words a word diff
/// has to walk.
fn line(side: char, at: usize, room: usize) -> String {
    let words = format!("{side}let value_{at} = call(alpha, beta, {side}{at}); ");
    let line: String = words.chars().cycle().take(room).collect();
    line.trim_end().to_owned()
}

/// The largest diff the reader takes, in `files` files: as many lines as it
/// reads, headers counted, in as many bytes as it reads, every removed line
/// paired with an added one.
fn at_the_caps(files: usize) -> String {
    // The reader refuses a diff of `MOST_LINES` lines or more, and each file
    // spends three of them on its headers.
    let pairs = (MOST_LINES - 1 - 3 * files) / 2 / files;
    let headers: Vec<String> = (0..files)
        .map(|file| {
            format!(
                "--- a/at/the/caps_{file}.rs\n+++ b/at/the/caps_{file}.rs\n@@ -1,{pairs} +1,{pairs} @@\n"
            )
        })
        .collect();
    let written: usize = headers.iter().map(String::len).sum();
    let room = (MOST_BYTES - written) / (pairs * 2 * files) - 2;
    let mut text = String::with_capacity(MOST_BYTES);
    for (file, header) in headers.iter().enumerate() {
        text.push_str(header);
        for side in ['-', '+'] {
            for at in 0..pairs {
                text.push_str(&line(side, file * pairs + at, room));
                text.push('\n');
            }
        }
    }
    text
}

/// A diff whose lines are as long as the reader's bytes allow: a line that did
/// not move, then one removed and the one that replaced it, each a third of
/// the bytes, so each is coloured, compared and clipped at its longest.
fn the_longest_lines() -> String {
    let header = "--- a/at/the/caps.rs\n+++ b/at/the/caps.rs\n@@ -1,2 +1,2 @@\n";
    let room = (MOST_BYTES - header.len()) / 3 - 2;
    let mut text = String::with_capacity(MOST_BYTES);
    text.push_str(header);
    for side in [' ', '-', '+'] {
        text.push(side);
        text.push_str(&line(side, 0, room));
        text.push('\n');
    }
    text
}

/// One file whose every shown pair has as many words as a pair is compared
/// at, with no word in common, so each comparison walks its longest path.
fn word_pairs(text: &mut String, file: usize, pairs: usize) {
    let words = TOKENS / 2;
    let _ = write!(
        text,
        "--- a/at/the/caps_{file}.txt\n+++ b/at/the/caps_{file}.txt\n@@ -1,{pairs} +1,{pairs} @@\n"
    );
    for side in ['-', '+'] {
        for at in 0..pairs {
            text.push(side);
            for word in 0..words {
                let _ = write!(text, "{side}{}", (at + word) % 10);
                text.push(' ');
            }
            text.pop();
            text.push('\n');
        }
    }
}

/// A diff of as many such files as hold the pairs a change compares, each
/// with as many pairs as a box shows without folding.
fn word_pairs_at_the_caps() -> String {
    let mut text = String::with_capacity(MOST_BYTES);
    for file in 0..PAIRS.div_ceil(FOLD / 2) {
        word_pairs(&mut text, file, FOLD / 2);
    }
    text
}

/// One file of lines that did not move, in a language with colours, as many
/// as a change reads for colour and as long as share the bytes it reads
/// between them, so every one of them is read: a line costs its reading for
/// itself and more for each byte, and this spends both.
fn coloured(text: &mut String) {
    let _ = write!(
        text,
        "--- a/at/the/colours.rs\n+++ b/at/the/colours.rs\n@@ -1,{SYNTAX_LINES} +1,{SYNTAX_LINES} @@\n"
    );
    for at in 0..SYNTAX_LINES {
        text.push(' ');
        text.push_str(&line(' ', at, SYNTAX_BYTES / SYNTAX_LINES));
        text.push('\n');
    }
}

/// A diff whose lines spend all of the colour a change reads.
fn colour_at_the_caps() -> String {
    let mut text = String::new();
    coloured(&mut text);
    text
}

/// Every cap spent in one change: the colour, then the word pairs, then as
/// many files more as the reader takes, each as long as a box shows without
/// folding, within the reader's lines and bytes.
fn every_cap_at_once() -> String {
    let mut text = String::with_capacity(MOST_BYTES);
    coloured(&mut text);
    let paired = PAIRS.div_ceil(FOLD / 2);
    for file in 0..paired {
        word_pairs(&mut text, 1 + file, FOLD / 2);
    }
    let rest = MOST_FILES.saturating_sub(1 + paired);
    let newlines = text.matches('\n').count();
    let written = text.len() + newlines;
    let Some(lines) = (MOST_LINES - 1 - newlines).checked_div(rest) else {
        return text;
    };
    let pairs = ((lines - 3) / 2).min(FOLD / 2);
    let room = (MOST_BYTES - written) / rest / (pairs * 2).max(1) - 8;
    for file in 0..rest {
        let _ = write!(
            text,
            "--- a/at/the/rest_{file}.rs\n+++ b/at/the/rest_{file}.rs\n@@ -1,{pairs} +1,{pairs} @@\n"
        );
        for side in ['-', '+'] {
            for at in 0..pairs {
                text.push_str(&line(side, at, room));
                text.push('\n');
            }
        }
    }
    text
}

/// A tool's edit keeping as many lines as a file holds: each removed line
/// beside the line that took its place, as a tool numbers them.
fn a_tools_edit() -> Change {
    let pairs = MOST_LINES / 2;
    let removed: Vec<String> = (0..pairs).map(|at| line('-', at, 72)).collect();
    let added: Vec<String> = (0..pairs).map(|at| line('+', at, 72)).collect();
    let lines = (0..pairs).flat_map(|at| {
        let number = at + 1;
        [
            (
                number,
                Side::Removed,
                removed.get(at).map_or("", String::as_str),
            ),
            (
                number,
                Side::Added,
                added.get(at).map_or("", String::as_str),
            ),
        ]
    });
    Change::new(vec![
        File::numbered(Some("src/at/the/caps.rs"), lines).counted(pairs, pairs),
    ])
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one file at the caps",
            input: Input::Diff(at_the_caps(1)),
        },
        Case {
            name: "every file a change holds",
            input: Input::Diff(at_the_caps(MOST_FILES)),
        },
        Case {
            name: "the longest lines a change holds",
            input: Input::Diff(the_longest_lines()),
        },
        Case {
            name: "word pairs at the caps",
            input: Input::Diff(word_pairs_at_the_caps()),
        },
        Case {
            name: "colour at the caps",
            input: Input::Diff(colour_at_the_caps()),
        },
        Case {
            name: "every cap at once",
            input: Input::Diff(every_cap_at_once()),
        },
        Case {
            name: "a tool's edit at the caps",
            input: Input::Change(a_tools_edit()),
        },
    ]
}

/// One layout of `case` in `room`, with the rows it laid out.
fn lay(case: &Case, room: &Room) -> Option<usize> {
    match &case.input {
        Input::Diff(text) => Changes.block(text, room).ok().map(|laid| laid.rows.len()),
        Input::Change(change) => Some(Changes.draw(change, room).rows.len()),
    }
}

fn time(case: &Case) -> Result<Timed, ProbeError> {
    let rooms = [Glyphs::Unicode, Glyphs::Ascii]
        .into_iter()
        .flat_map(|glyphs| [WIDE, NARROW].map(|columns| Room { columns, glyphs }));
    let mut readings = Vec::with_capacity(RUNS * 4);
    for room in rooms {
        for _ in 0..WARM {
            lay(case, &room).ok_or(ProbeError::Undrawn(case.name))?;
        }
        for _ in 0..RUNS {
            let started = Instant::now();
            let rows = lay(case, &room);
            readings.push(started.elapsed());
            if rows.unwrap_or(0) == 0 {
                return Err(ProbeError::Undrawn(case.name));
            }
        }
    }
    readings.sort_unstable();
    Ok(Timed {
        name: case.name,
        readings,
    })
}

/// The limit this run is held to: the quiet machine's, or the shared runner's
/// when `CRUCIBLE_BENCH_RUNNER` names one. Anything else is refused rather than
/// read as either, since a misspelt runner falling back to a limit would be a
/// budget nobody chose.
fn limit() -> Result<f64, ProbeError> {
    match std::env::var_os("CRUCIBLE_BENCH_RUNNER") {
        None => Ok(LIMIT),
        Some(named) if named == "shared" => Ok(SHARED_RUNNER_LIMIT),
        Some(other) => Err(ProbeError::Runner(other.to_string_lossy().into_owned())),
    }
}

/// Measures every case, and says whether the slowest held.
fn run() -> Result<bool, ProbeError> {
    let limit = limit()?;
    let timed = cases().iter().map(time).collect::<Result<Vec<_>, _>>()?;

    let mut worst = 0.0_f64;
    let mut evidence = String::new();
    for (index, case) in timed.iter().enumerate() {
        let p95 = case.at(0.95);
        worst = worst.max(p95);
        let _ = write!(
            evidence,
            " case{index}_p95={p95:.3} case{index}_median={:.3} case{index}_max={:.3}",
            case.at(0.5),
            case.at(1.0)
        );
    }

    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on inside a probe.
    let mut line = String::new();
    let _ = writeln!(line, "{worst:.3} ms {limit:.1}{evidence}");
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()?;

    let held = worst <= limit;
    if !held {
        let mut told = String::new();
        for case in &timed {
            let _ = writeln!(
                told,
                "    bench-drawing {}: median {:.3} ms, p95 {:.3} ms, slowest {:.3} ms",
                case.name,
                case.at(0.5),
                case.at(0.95),
                case.at(1.0)
            );
        }
        io::stderr().write_all(told.as_bytes())?;
    }
    Ok(held)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(problem) => {
            let mut line = String::new();
            let _ = writeln!(line, "    FAIL {problem}");
            let _ = io::stderr().write_all(line.as_bytes());
            ExitCode::FAILURE
        }
    }
}
