//! Generated input: diffs written whole, damaged, forged or made of noise, and
//! edits a tool hands over with numbers and paths no file has, each drawn into
//! a room of its own. None may come back as a panic, a hang, a row wider than
//! its room or a row holding a character a terminal would obey.
//!
//! Every case is made from a seed and its index alone, so a failure names the
//! case that reproduces it, and the run says its seed and its count. A run
//! makes [`DEFAULT_CASES`] cases; the variable named by [`CASES_VARIABLE`]
//! makes more of them for a longer run.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a fixture not made or a check failed is the test's failure, in a helper too"
)]

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Draws, Laid, Room};
use crucible_tui::{Glyphs, Row};
use crucible_tui_diff::{Change, Changes, File, MOST_LINES, Side};
use std::fmt::Write as _;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const SEED: u64 = 0x5eed_d1ff_0fc0;
const DEFAULT_CASES: u64 = 4096;
const CASES_VARIABLE: &str = "CRUCIBLE_GENERATED_INPUTS";

/// Long enough for the slowest case in a debug build on a loaded machine, and
/// short against a hang.
const DEADLINE: Duration = Duration::from_secs(30);

fn cases() -> u64 {
    std::env::var(CASES_VARIABLE)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(DEFAULT_CASES)
}

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

#[test]
fn a_planted_panic_is_found_by_the_generated_runner() {
    let ended = guarded(Duration::from_secs(5), || -> u8 {
        panic!("planted: a reader that panics")
    });
    assert_eq!(ended, Ended::Panicked);
}

#[test]
fn a_planted_unbounded_loop_is_found_by_the_generated_runner() {
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

// ---------------------------------------------------------------------------
// The generator.

/// xorshift64*: small, and the same on every platform.
struct Rng(u64);

impl Rng {
    /// The generator for case `index`: no case depends on another.
    fn for_case(index: u64) -> Self {
        let mut rng = Rng(SEED ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        // The first outputs of nearby seeds are alike; spend them.
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

    /// A number below `bound`, or nought for a bound of nought.
    fn below(&mut self, bound: usize) -> usize {
        let bound = u64::try_from(bound).unwrap_or(u64::MAX).max(1);
        usize::try_from((self.next() >> 11) % bound).unwrap_or(0)
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> Option<&'a T> {
        from.get(self.below(from.len()))
    }
}

/// Pieces of text a line or a path is made of: words, wide and combining
/// characters, tabs, and every kind a terminal would obey or a reader be
/// misled by.
const PIECES: &[&str] = &[
    "word",
    "let x = 1;",
    " ",
    "  ",
    "\t",
    "fn",
    "(",
    ")",
    ".",
    "_",
    "漢字",
    "e\u{301}",
    "\u{1f468}\u{200d}\u{1f469}",
    "\u{1b}[2J",
    "\u{1b}]8;;http://127.0.0.1:9/\u{7}",
    "\u{7}",
    "\u{0}",
    "\u{7f}",
    "\u{85}",
    "\u{202e}",
    "\u{2066}",
    "\u{200b}",
    "\u{feff}",
    "\u{e000}",
    "\u{f0000}",
    "\u{2028}",
    "\r",
    "@@",
    "+++",
    "---",
    "-",
    "+",
    "/",
    "a/",
    "b/",
];

fn text(rng: &mut Rng, most: usize) -> String {
    let mut text = String::new();
    for _ in 0..rng.below(most + 1) {
        text.push_str(rng.pick(PIECES).copied().unwrap_or_default());
    }
    text
}

/// A line's text as it may appear in a diff: no line break inside it.
fn line_text(rng: &mut Rng, most: usize) -> String {
    text(rng, most).replace(['\n', '\r'], " ")
}

fn path(rng: &mut Rng) -> String {
    let mut path = String::new();
    for _ in 0..rng.below(6) {
        path.push_str(&line_text(rng, 3).replace(' ', "_"));
        path.push('/');
    }
    if rng.chance(8) {
        path.push_str(&"long/".repeat(rng.below(400)));
    }
    path.push_str(&line_text(rng, 2));
    path.push_str(
        rng.pick(&[".rs", ".py", ".md", "", ".txt"])
            .copied()
            .unwrap_or_default(),
    );
    path
}

fn number(rng: &mut Rng) -> usize {
    match rng.below(6) {
        0 => usize::MAX - rng.below(4),
        1 => 0,
        2 => rng.below(10),
        _ => rng.below(100_000),
    }
}

fn side(rng: &mut Rng) -> Side {
    *rng.pick(&[Side::Kept, Side::Removed, Side::Added])
        .unwrap_or(&Side::Kept)
}

/// One file of a diff as text, with hunk headers that count what they hold.
fn written_file(rng: &mut Rng, out: &mut String) {
    let name = path(rng).replace(['\n', '\r'], " ");
    let _ = writeln!(out, "diff --git a/{name} b/{name}");
    if rng.chance(6) {
        let _ = writeln!(out, "rename from {}\nrename to {name}", path(rng));
    }
    if rng.chance(10) {
        let _ = writeln!(out, "Binary files a/{name} and b/{name} differ");
        return;
    }
    let _ = writeln!(out, "--- a/{name}\n+++ b/{name}");
    let mut at = 1 + rng.below(50);
    for _ in 0..=rng.below(4) {
        let lines: Vec<(char, String)> = (0..=rng.below(40))
            .map(|_| {
                let sign = *rng.pick(&[' ', '-', '+']).unwrap_or(&' ');
                (sign, line_text(rng, 8))
            })
            .collect();
        let old = lines.iter().filter(|(sign, _)| *sign != '+').count();
        let new = lines.iter().filter(|(sign, _)| *sign != '-').count();
        let section = if rng.chance(2) {
            line_text(rng, 4)
        } else {
            String::new()
        };
        let _ = writeln!(out, "@@ -{at},{old} +{at},{new} @@ {section}");
        for (sign, text) in lines {
            let _ = writeln!(out, "{sign}{text}");
        }
        at += old + rng.below(200);
    }
}

fn written(rng: &mut Rng) -> String {
    let mut out = String::new();
    for _ in 0..=rng.below(4) {
        written_file(rng, &mut out);
    }
    out
}

/// A written diff, then cut, spliced, forged or littered.
fn damaged(rng: &mut Rng) -> String {
    let whole = written(rng);
    let mut lines: Vec<String> = whole.lines().map(str::to_owned).collect();
    for _ in 0..=rng.below(6) {
        let at = rng.below(lines.len() + 1);
        match rng.below(5) {
            0 => lines.truncate(at),
            1 => lines.insert(at.min(lines.len()), text(rng, 12)),
            2 => {
                let forged = format!(
                    "@@ -{},{} +{},{} @@{}",
                    number(rng),
                    number(rng),
                    number(rng),
                    number(rng),
                    line_text(rng, 3)
                );
                lines.insert(at.min(lines.len()), forged);
            }
            3 => {
                if let Some(line) = lines.get(at).cloned() {
                    lines.insert(at, line);
                }
            }
            _ => {
                if at < lines.len() {
                    lines.remove(at);
                }
            }
        }
    }
    let mut out = lines.join(if rng.chance(4) { "\r\n" } else { "\n" });
    if rng.chance(3) {
        // A cut that may land inside a character is cut back to one.
        let mut end = rng.below(out.len() + 1);
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// Lines that begin as a diff's lines do, in any order, with anything after.
fn noise(rng: &mut Rng) -> String {
    const STARTS: &[&str] = &[
        "@@ ",
        "--- ",
        "+++ ",
        "+",
        "-",
        " ",
        "diff --git ",
        "rename from ",
        "rename to ",
        "Binary files ",
        "GIT binary patch",
        "\\ No newline at end of file",
        "",
    ];
    let mut out = String::new();
    for _ in 0..rng.below(60) {
        out.push_str(rng.pick(STARTS).copied().unwrap_or_default());
        out.push_str(&text(rng, 6));
        out.push('\n');
    }
    out
}

/// An edit as a tool hands it over: numbered lines in any order, a path or
/// none, and lines it never kept.
fn tool_edit(rng: &mut Rng) -> Change {
    let mut files = Vec::new();
    for _ in 0..=rng.below(3) {
        let named = if rng.chance(4) { None } else { Some(path(rng)) };
        let count = if rng.chance(16) {
            MOST_LINES + rng.below(64)
        } else {
            rng.below(80)
        };
        let mut at = number(rng);
        let lines: Vec<(usize, Side, String)> = (0..count)
            .map(|_| {
                at = if rng.chance(8) {
                    number(rng)
                } else {
                    at.saturating_add(1)
                };
                (at, side(rng), text(rng, 10))
            })
            .collect();
        let mut file = File::numbered(
            named.as_deref(),
            lines
                .iter()
                .map(|(at, side, text)| (*at, *side, text.as_str())),
        );
        if rng.chance(3) {
            file = file.dropping(number(rng));
        }
        if rng.chance(5) {
            file = file.counted(number(rng), number(rng));
        }
        files.push(file);
    }
    Change::new(files)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Written,
    Damaged,
    Noise,
    ToolEdit,
}

const FAMILIES: [Family; 4] = [
    Family::Written,
    Family::Damaged,
    Family::Noise,
    Family::ToolEdit,
];

/// What a case is drawn from.
#[derive(Debug)]
enum Input {
    /// A fence's text, which must be drawn when it is a diff written whole.
    Text { source: String, whole: bool },
    /// A tool's edit, which is always drawn.
    Edit(Change),
}

#[derive(Debug)]
struct Case {
    input: Input,
    room: Room,
}

fn case(index: u64) -> (Family, Case) {
    let mut rng = Rng::for_case(index);
    let family = FAMILIES
        .get(usize::try_from(index % 4).unwrap_or(0))
        .copied()
        .unwrap_or(Family::Noise);
    let input = match family {
        Family::Written => Input::Text {
            source: written(&mut rng),
            whole: true,
        },
        Family::Damaged => Input::Text {
            source: damaged(&mut rng),
            whole: false,
        },
        Family::Noise => Input::Text {
            source: noise(&mut rng),
            whole: false,
        },
        Family::ToolEdit => Input::Edit(tool_edit(&mut rng)),
    };
    let glyphs = if rng.chance(3) {
        Glyphs::Ascii
    } else {
        Glyphs::Unicode
    };
    let room = Room {
        columns: rng.below(201),
        glyphs,
    };
    (family, Case { input, room })
}

// ---------------------------------------------------------------------------
// The judge.

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

/// What draws a fence's text into a room: this crate's drawer, or a planted
/// stand-in the run must catch.
type Drawer = fn(&str, &Room) -> Result<Laid, Undrawn>;

fn this_crate(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    Changes.block(source, room)
}

fn judge(case: Case, drawer: Drawer) -> Result<(), String> {
    let room = case.room;
    match case.input {
        Input::Edit(change) => check_rows(&Changes.draw(&change, &room).rows, room.columns),
        Input::Text { source, whole } => match drawer(&source, &room) {
            Ok(laid) => check_rows(&laid.rows, room.columns),
            Err(undrawn) if whole => {
                Err(format!("a diff written whole was not drawn: {undrawn:?}"))
            }
            Err(_) => Ok(()),
        },
    }
}

/// The cases `from` to `to`, each with a deadline, the first failure first.
fn run(from: u64, to: u64, drawer: Drawer) -> Result<(), String> {
    for index in from..to {
        let (family, case) = case(index);
        let began = Instant::now();
        match guarded(DEADLINE, move || judge(case, drawer)) {
            Ended::Returned(Ok(())) => {}
            Ended::Returned(Err(why)) => {
                return Err(format!("case {index} ({family:?}, seed {SEED:#x}): {why}"));
            }
            Ended::Panicked => {
                return Err(format!(
                    "case {index} ({family:?}, seed {SEED:#x}): panicked"
                ));
            }
            Ended::Hung => {
                return Err(format!(
                    "case {index} ({family:?}, seed {SEED:#x}): no answer in {:?}",
                    began.elapsed()
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn the_same_case_is_made_from_the_same_seed_and_index() {
    for index in [0, 1, 7, 100, 4095] {
        assert_eq!(
            format!("{:?}", case(index)),
            format!("{:?}", case(index)),
            "case {index}"
        );
    }
    assert_ne!(format!("{:?}", case(10)), format!("{:?}", case(14)));
}

#[test]
fn every_family_of_case_is_made_within_a_run() {
    let made: Vec<Family> = (0..DEFAULT_CASES).map(|index| case(index).0).collect();
    for family in FAMILIES {
        assert!(made.contains(&family), "no {family:?} in a run");
    }
}

#[test]
fn a_planted_drawer_that_panics_is_found_by_the_generated_run() {
    fn panics_on_a_hunk(source: &str, room: &Room) -> Result<Laid, Undrawn> {
        assert!(!source.contains("@@ -"), "planted: a drawer that panics");
        Changes.block(source, room)
    }
    let found = run(0, 64, panics_on_a_hunk).expect_err("the planted panic");
    assert!(found.contains("panicked"), "{found}");
}

#[test]
fn generated_diffs_and_edits_are_drawn_or_shown_as_written_within_their_room() {
    let total = cases();
    println!("seed {SEED:#x}, {total} inputs ({CASES_VARIABLE} raises the count)");
    if let Err(why) = run(0, total, this_crate) {
        panic!("{why}");
    }
}
