//! Generated input: formulas written whole from the TeX this crate knows, the
//! same formulas cut, spliced and littered, and noise made of TeX's own
//! characters, each drawn in a line and as a block into a room of its own.
//! None may come back as a panic, a hang, a row wider than its room, a line
//! broken inside a span, or a character a terminal would obey; and a formula
//! written whole must be drawn, in a line always and as a block wherever it
//! fits.
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
use crucible_tui::drawing::{Draws, Laid, Room, Run};
use crucible_tui::{Glyphs, Row};
use crucible_tui_math::Formulas;
use std::fmt::Write as _;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const SEED: u64 = 0x5eed_7e70_f0c0;
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

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from.get(self.below(from.len()))
            .copied()
            .unwrap_or_default()
    }
}

/// What a formula's simplest atoms are: letters, digits, and the commands
/// that stand for one character.
const ATOMS: &[&str] = &[
    "x", "y", "n", "a", "b", "2", "1", "0", r"\alpha", r"\beta", r"\pi", r"\theta", r"\infty",
];

/// What stands between two atoms.
const OPERATORS: &[&str] = &[
    " + ", " - ", " = ", r" \pm ", r" \leq ", r" \geq ", r" \neq ", r" \cdot ", r" \to ", r" \in ",
    " < ", " ",
];

/// The environments a written formula may hold, whose cells are formulas.
const MATRICES: &[&str] = &["pmatrix", "bmatrix", "vmatrix", "matrix"];

/// A base with at most one subscript and one superscript: `x^2^3` is not TeX.
fn scripted(rng: &mut Rng, depth: usize) -> String {
    let mut out = rng.pick(ATOMS).to_owned();
    if rng.chance(4) {
        let _ = write!(out, "_{{{}}}", expression(rng, depth + 1));
    }
    if rng.chance(4) {
        let _ = write!(out, "^{{{}}}", expression(rng, depth + 1));
    }
    out
}

fn term(rng: &mut Rng, depth: usize) -> String {
    // Below the deepest level only atoms, so a written formula stays inside
    // the caps.
    if depth >= 3 {
        return rng.pick(ATOMS).to_owned();
    }
    let inner = |rng: &mut Rng| expression(rng, depth + 1);
    match rng.below(10) {
        0 => format!(r"\frac{{{}}}{{{}}}", inner(rng), inner(rng)),
        1 => format!(r"\sqrt{{{}}}", inner(rng)),
        2 => format!(r"\left( {} \right)", inner(rng)),
        3 => format!(
            r"\text{{{}}}",
            rng.pick(&["if ", "otherwise", "for all ", "x"])
        ),
        4 => format!(r"\sum_{{{}}}^{{{}}} {}", inner(rng), inner(rng), inner(rng)),
        5 if depth == 0 => {
            let name = rng.pick(MATRICES);
            let columns = 1 + rng.below(3);
            let rows: Vec<String> = (0..=rng.below(3))
                .map(|_| {
                    (0..columns)
                        .map(|_| expression(rng, 2))
                        .collect::<Vec<_>>()
                        .join(" & ")
                })
                .collect();
            format!(r"\begin{{{name}}} {} \end{{{name}}}", rows.join(r" \\ "))
        }
        6 if depth == 0 => {
            let rows: Vec<String> = (0..=rng.below(2))
                .map(|_| format!("{} & {}", expression(rng, 2), expression(rng, 2)))
                .collect();
            format!(r"\begin{{cases}} {} \end{{cases}}", rows.join(r" \\ "))
        }
        _ => scripted(rng, depth),
    }
}

/// A formula written whole from the TeX this crate knows.
fn expression(rng: &mut Rng, depth: usize) -> String {
    let mut out = term(rng, depth);
    for _ in 0..rng.below(4) {
        out.push_str(rng.pick(OPERATORS));
        out.push_str(&term(rng, depth));
    }
    out
}

/// Pieces of TeX and of what a terminal obeys.
const PIECES: &[&str] = &[
    "x",
    " ",
    "{",
    "}",
    "^",
    "_",
    "&",
    r"\\",
    "\\",
    "$",
    "%",
    "#",
    r"\frac",
    r"\sqrt",
    r"\left(",
    r"\right)",
    r"\begin{pmatrix}",
    r"\end{pmatrix}",
    r"\begin{cases}",
    r"\end{cases}",
    r"\text{",
    r"\input{/etc/passwd}",
    r"\documentclass{article}",
    r"\unknown",
    "漢字",
    "e\u{301}",
    "\u{1b}[2J",
    "\u{1b}]8;;http://127.0.0.1:9/\u{7}",
    "\u{0}",
    "\u{7f}",
    "\u{85}",
    "\u{202e}",
    "\u{2066}",
    "\u{200b}",
    "\u{feff}",
    "\n",
    "\r",
    "\t",
];

fn noise(rng: &mut Rng) -> String {
    let mut out = String::new();
    for _ in 0..rng.below(80) {
        out.push_str(rng.pick(PIECES));
    }
    out
}

/// A written formula, then cut, spliced or littered.
fn damaged(rng: &mut Rng) -> String {
    let mut out = expression(rng, 0);
    for _ in 0..=rng.below(4) {
        let mut at = rng.below(out.len() + 1);
        while !out.is_char_boundary(at) {
            at -= 1;
        }
        match rng.below(3) {
            0 => out.truncate(at),
            1 => out.insert_str(at, rng.pick(PIECES)),
            _ => {
                let mut to = rng.below(out.len() + 1).max(at);
                while !out.is_char_boundary(to) {
                    to -= 1;
                }
                out.replace_range(at..to.max(at), "");
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Written,
    Damaged,
    Noise,
}

const FAMILIES: [Family; 3] = [Family::Written, Family::Damaged, Family::Noise];

#[derive(Debug)]
struct Case {
    source: String,
    /// Whether it is written whole, and must be drawn.
    whole: bool,
    room: Room,
}

fn case(index: u64) -> (Family, Case) {
    let mut rng = Rng::for_case(index);
    let family = FAMILIES
        .get(usize::try_from(index % 3).unwrap_or(0))
        .copied()
        .unwrap_or(Family::Noise);
    let (source, whole) = match family {
        Family::Written => (expression(&mut rng, 0), true),
        Family::Damaged => (damaged(&mut rng), false),
        Family::Noise => (noise(&mut rng), false),
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
    (
        family,
        Case {
            source,
            whole,
            room,
        },
    )
}

// ---------------------------------------------------------------------------
// The judge.

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

fn check_runs(runs: &[Run]) -> Result<(), String> {
    for run in runs {
        if let Some(found) = run.text.chars().find(|character| obeyed(*character)) {
            return Err(format!(
                "U+{:04X} in a span: {:?}",
                u32::from(found),
                run.text
            ));
        }
    }
    Ok(())
}

/// What draws a formula: this crate's drawer, or a planted stand-in the run
/// must catch.
type Drawer = fn(&str, &Room) -> (Option<Vec<Run>>, Result<Laid, Undrawn>);

fn this_crate(source: &str, room: &Room) -> (Option<Vec<Run>>, Result<Laid, Undrawn>) {
    (Formulas.inline(source, room), Formulas.block(source, room))
}

fn judge(case: &Case, drawer: Drawer) -> Result<(), String> {
    let room = case.room;
    let (inline, block) = drawer(&case.source, &room);
    match inline {
        Some(runs) => check_runs(&runs)?,
        None if case.whole => return Err("a formula written whole was not drawn in a line".into()),
        None => {}
    }
    match block {
        Ok(laid) => check_rows(&laid.rows, room.columns),
        // Too wide for its room is the one refusal a written formula may meet.
        Err(Undrawn::TooWide) => Ok(()),
        Err(undrawn) if case.whole => Err(format!(
            "a formula written whole was not drawn: {undrawn:?}"
        )),
        Err(_) => Ok(()),
    }
}

/// The cases `from` to `to`, each with a deadline, the first failure first.
fn run(from: u64, to: u64, drawer: Drawer) -> Result<(), String> {
    for index in from..to {
        let (family, case) = case(index);
        let source = case.source.clone();
        let began = Instant::now();
        match guarded(DEADLINE, move || judge(&case, drawer)) {
            Ended::Returned(Ok(())) => {}
            Ended::Returned(Err(why)) => {
                return Err(format!(
                    "case {index} ({family:?}, seed {SEED:#x}): {why}\n{source:?}"
                ));
            }
            Ended::Panicked => {
                return Err(format!(
                    "case {index} ({family:?}, seed {SEED:#x}): panicked\n{source:?}"
                ));
            }
            Ended::Hung => {
                return Err(format!(
                    "case {index} ({family:?}, seed {SEED:#x}): no answer in {:?}\n{source:?}",
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
    assert_ne!(format!("{:?}", case(9)), format!("{:?}", case(12)));
}

#[test]
fn every_family_of_case_is_made_within_a_run() {
    let made: Vec<Family> = (0..DEFAULT_CASES).map(|index| case(index).0).collect();
    for family in FAMILIES {
        assert!(made.contains(&family), "no {family:?} in a run");
    }
}

#[test]
fn a_written_run_holds_every_construct_the_grammar_knows() {
    let written: String = (0..DEFAULT_CASES)
        .map(case)
        .filter(|(family, _)| *family == Family::Written)
        .map(|(_, case)| case.source)
        .collect();
    for construct in [
        r"\frac{",
        r"\sqrt{",
        r"\left(",
        r"\text{",
        r"\sum_{",
        "^{",
        "_{",
        r"\begin{pmatrix}",
        r"\begin{cases}",
        " & ",
        r" \\ ",
    ] {
        assert!(written.contains(construct), "no {construct} in a run");
    }
}

#[test]
fn a_planted_drawer_that_panics_is_found_by_the_generated_run() {
    fn panics_on_a_fraction(
        source: &str,
        room: &Room,
    ) -> (Option<Vec<Run>>, Result<Laid, Undrawn>) {
        assert!(!source.contains(r"\frac{"), "planted: a drawer that panics");
        this_crate(source, room)
    }
    let found = run(0, 256, panics_on_a_fraction).expect_err("the planted panic");
    assert!(found.contains("panicked"), "{found}");
}

#[test]
fn a_planted_drawer_that_refuses_what_it_knows_is_found_by_the_generated_run() {
    fn refuses_a_root(source: &str, room: &Room) -> (Option<Vec<Run>>, Result<Laid, Undrawn>) {
        if source.contains(r"\sqrt{") {
            return (None, Err(Undrawn::NotText));
        }
        this_crate(source, room)
    }
    let found = run(0, 256, refuses_a_root).expect_err("the planted refusal");
    assert!(found.contains("written whole was not drawn"), "{found}");
}

#[test]
fn generated_formulas_are_drawn_or_shown_as_written_within_their_room() {
    let total = cases();
    println!("seed {SEED:#x}, {total} inputs ({CASES_VARIABLE} raises the count)");
    if let Err(why) = run(0, total, this_crate) {
        panic!("{why}");
    }
}
