//! Generated input: flowcharts, state diagrams and entity-relationship
//! diagrams written whole inside their kind's caps from the forms this crate
//! reads, the same diagrams cut, spliced and littered, and noise made of
//! Mermaid's own pieces, each drawn into a room of its own. None may come back
//! as a panic, a hang, a row wider than its room, or a character a terminal
//! would obey; and a diagram written whole must be drawn wherever it fits.
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
use crucible_tui_mermaid::{Caps, Diagrams};
use std::fmt::Write as _;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const SEED: u64 = 0x5eed_3e7a_1d00;
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

    /// A number from `low` to `high`, both included.
    fn between(&mut self, low: usize, high: usize) -> usize {
        low + self.below(high.saturating_sub(low) + 1)
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

/// Labels within every kind's label cap: words, wide characters, and
/// punctuation a label may hold.
const LABELS: &[&str] = &[
    "start",
    "check it",
    "done",
    "retry later",
    "漢字",
    "한국어",
    "選ぶ?",
    "yes",
    "no",
    "a & b",
    "x = 1",
    "naïve",
];

/// A flowchart's shapes, as the text around a label.
const SHAPES: &[(&str, &str)] = &[
    ("[", "]"),
    ("(", ")"),
    ("((", "))"),
    ("([", "])"),
    ("[[", "]]"),
    ("[(", ")]"),
    ("{", "}"),
    ("{{", "}}"),
    ("[/", "/]"),
    ("[\\", "\\]"),
    (">", "]"),
];

/// A flowchart's lines with no label, and the forms of a line with one, the
/// label at `@`.
const LINKS: &[&str] = &[
    "-->", "---", "-.->", "-.-", "==>", "===", "--o", "--x", "<-->",
];
const LABELLED_LINKS: &[&str] = &[
    "-->|@|", "---|@|", "-- @ -->", "-. @ .->", "== @ ==>", "-.->|@|",
];

/// A flowchart written whole inside its caps.
fn flowchart(rng: &mut Rng) -> String {
    let caps = Caps::FLOWCHART;
    let boxes = rng.between(2, caps.nodes.min(10));
    let edges = rng.between(1, caps.edges);
    let mut out = format!(
        "{} {}\n",
        rng.pick(&["graph", "flowchart"]),
        rng.pick(&["TD", "TB", "LR", "RL", "BT"])
    );
    let mut named = vec![false; boxes];
    let name = |rng: &mut Rng, at: usize, named: &mut Vec<bool>| -> String {
        let mut text = format!("N{at}");
        if let Some(seen) = named.get_mut(at)
            && !*seen
        {
            *seen = true;
            if !rng.chance(4) {
                let (open, close) = SHAPES
                    .get(rng.below(SHAPES.len()))
                    .copied()
                    .unwrap_or(("[", "]"));
                let label = rng.pick(LABELS);
                let _ = write!(text, "{open}\"{label}\"{close}");
            }
        }
        text
    };
    for _ in 0..edges {
        let from = rng.below(boxes);
        let to = (from + 1 + rng.below(boxes - 1)) % boxes;
        let from = name(rng, from, &mut named);
        let to = name(rng, to, &mut named);
        let link = if rng.chance(3) {
            rng.pick(LABELLED_LINKS).replace('@', rng.pick(LABELS))
        } else {
            rng.pick(LINKS).to_owned()
        };
        let _ = writeln!(out, "    {from} {link} {to}");
    }
    for at in 0..boxes {
        if named.get(at).is_some_and(|seen| !seen) {
            let _ = writeln!(out, "    {}", name(rng, at, &mut named));
        }
    }
    if rng.chance(4) {
        out.push_str(
            "    %% a comment\n    click N0 \"https://example.com\"\n    style N0 fill:#f9f\n",
        );
    }
    out
}

/// A state diagram written whole inside its caps.
fn state(rng: &mut Rng) -> String {
    let caps = Caps::STATE;
    let states = rng.between(1, 6);
    let mut out = String::from(rng.pick(&["stateDiagram-v2\n", "stateDiagram\n"]));
    if rng.chance(3) {
        let _ = writeln!(out, "    direction {}", rng.pick(&["TB", "LR"]));
    }
    for at in 0..states {
        if rng.chance(3) {
            let _ = writeln!(out, "    state \"{}\" as S{at}", rng.pick(LABELS));
        }
    }
    // The lines from the start and to the end, and the three of a composite
    // state, leave the rest to the transitions between states.
    let _ = writeln!(out, "    [*] --> S0");
    for _ in 0..rng.below(caps.edges.saturating_sub(5)) {
        let from = rng.below(states);
        let to = rng.below(states);
        if from == to {
            continue;
        }
        if rng.chance(2) {
            let _ = writeln!(out, "    S{from} --> S{to} : {}", rng.pick(LABELS));
        } else {
            let _ = writeln!(out, "    S{from} --> S{to}");
        }
    }
    if rng.chance(2) {
        let _ = writeln!(out, "    S{} --> [*]", rng.below(states));
    }
    if rng.chance(3) {
        let _ = writeln!(
            out,
            "    state Inner {{\n        [*] --> Deep\n        Deep --> [*]\n    }}\n    S0 --> Inner"
        );
    }
    if rng.chance(3) {
        let _ = writeln!(
            out,
            "    note {} of S0 : {}",
            rng.pick(&["right", "left"]),
            rng.pick(LABELS)
        );
    }
    out
}

/// An entity-relationship diagram's ends of a line, left and right.
const LEFT_ENDS: &[&str] = &["||", "|o", "}o", "}|"];
const RIGHT_ENDS: &[&str] = &["||", "o|", "o{", "|{"];
const TYPES: &[&str] = &["string", "int", "float", "date", "bool"];

/// An entity-relationship diagram written whole inside its caps.
fn entities(rng: &mut Rng) -> String {
    let caps = Caps::ENTITIES;
    let entities = rng.between(2, caps.nodes.min(8));
    let mut out = String::from("erDiagram\n");
    for _ in 0..rng.between(1, caps.edges.min(10)) {
        let from = rng.below(entities);
        let to = (from + 1 + rng.below(entities - 1)) % entities;
        let line = format!(
            "{}{}{}",
            rng.pick(LEFT_ENDS),
            rng.pick(&["--", ".."]),
            rng.pick(RIGHT_ENDS)
        );
        let label = if rng.chance(2) {
            format!("\"{}\"", rng.pick(LABELS))
        } else {
            rng.pick(&["has", "places", "owns"]).to_owned()
        };
        let _ = writeln!(out, "    E{from} {line} E{to} : {label}");
    }
    for at in 0..entities {
        if rng.chance(2) {
            let _ = writeln!(out, "    E{at} {{");
            for field in 0..rng.between(1, 4) {
                let _ = writeln!(out, "        {} field_{field}", rng.pick(TYPES));
            }
            out.push_str("    }\n");
        }
    }
    out
}

/// A diagram of one of the kinds drawn as text, written whole.
fn written(rng: &mut Rng) -> String {
    match rng.below(3) {
        0 => flowchart(rng),
        1 => state(rng),
        _ => entities(rng),
    }
}

/// Pieces of Mermaid and of what a terminal obeys.
const PIECES: &[&str] = &[
    "A",
    " ",
    "\n",
    "-->",
    "---",
    "-.->",
    "==>",
    "|",
    "[",
    "]",
    "(",
    ")",
    "{",
    "}",
    "\"",
    ":",
    ";",
    "&",
    "%%",
    "%%{init: {\"theme\": \"dark\"}}%%",
    "graph LR",
    "flowchart TD",
    "stateDiagram-v2",
    "classDiagram",
    "erDiagram",
    "sequenceDiagram",
    "subgraph",
    "end",
    "[*]",
    "state ",
    "note right of ",
    "||--o{",
    "click A \"/etc/passwd\"",
    "style A fill:url(http://127.0.0.1:9/)",
    "漢字",
    "e\u{301}",
    "┌─┐",
    "\u{1b}[2J",
    "\u{1b}]8;;http://127.0.0.1:9/\u{7}",
    "\u{0}",
    "\u{7f}",
    "\u{85}",
    "\u{202e}",
    "\u{2066}",
    "\u{200b}",
    "\u{feff}",
    "\r",
    "\t",
];

fn noise(rng: &mut Rng) -> String {
    let mut out = String::new();
    for _ in 0..rng.below(120) {
        out.push_str(rng.pick(PIECES));
    }
    out
}

/// A written diagram, then cut, spliced or littered.
fn damaged(rng: &mut Rng) -> String {
    let mut out = written(rng);
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
        Family::Written => (written(&mut rng), true),
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

/// What draws a diagram: this crate's drawer, or a planted stand-in the run
/// must catch.
type Drawer = fn(&str, &Room) -> Result<Laid, Undrawn>;

fn this_crate(source: &str, room: &Room) -> Result<Laid, Undrawn> {
    Diagrams.block(source, room)
}

fn judge(case: &Case, drawer: Drawer) -> Result<(), String> {
    let room = case.room;
    match drawer(&case.source, &room) {
        Ok(laid) => check_rows(&laid.rows, room.columns),
        // Too wide for its room is the one refusal a written diagram may meet.
        Err(Undrawn::TooWide) => Ok(()),
        Err(undrawn) if case.whole => Err(format!(
            "a diagram written whole was not drawn: {undrawn:?}"
        )),
        Err(_) => Ok(()),
    }
}

/// The cases `from` to `to`, each with a deadline, the first failure first.
fn run(from: u64, to: u64, drawer: Drawer) -> Result<(), String> {
    for index in from..to {
        let (family, case) = case(index);
        let source = case.source.clone();
        let room = case.room;
        let began = Instant::now();
        let said = |why: &str| {
            format!(
                "case {index} ({family:?}, seed {SEED:#x}, {} columns, {:?}): {why}\n{source}",
                room.columns, room.glyphs
            )
        };
        match guarded(DEADLINE, move || judge(&case, drawer)) {
            Ended::Returned(Ok(())) => {}
            Ended::Returned(Err(why)) => return Err(said(&why)),
            Ended::Panicked => return Err(said("panicked")),
            Ended::Hung => return Err(said(&format!("no answer in {:?}", began.elapsed()))),
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
fn a_written_run_holds_every_kind_and_form_the_generator_knows() {
    let written: String = (0..DEFAULT_CASES)
        .map(case)
        .filter(|(family, _)| *family == Family::Written)
        .map(|(_, case)| case.source)
        .collect();
    for construct in [
        "graph ",
        "flowchart ",
        "stateDiagram-v2\n",
        "erDiagram\n",
        "((\"",
        "{{\"",
        "[/\"",
        "-->|",
        "-- ",
        "-. ",
        "== ",
        "<-->",
        "click ",
        "[*] --> ",
        "state \"",
        "state Inner {",
        "note right of",
        "direction LR",
        "}o..|{",
        " {\n        ",
    ] {
        assert!(written.contains(construct), "no {construct:?} in a run");
    }
}

#[test]
fn every_written_diagram_is_inside_its_kinds_caps() {
    for index in (0..DEFAULT_CASES).filter(|index| index % 3 == 0) {
        let (_, case) = case(index);
        let caps = if case.source.starts_with("erDiagram") {
            Caps::ENTITIES
        } else if case.source.starts_with("state") {
            Caps::STATE
        } else {
            Caps::FLOWCHART
        };
        assert!(
            case.source.len() <= caps.bytes,
            "case {index}: {}",
            case.source
        );
    }
}

/// A room wider than any written diagram is laid out.
const WIDE: usize = 400;

#[test]
fn every_written_diagram_is_drawn_in_a_room_wide_enough_for_it() {
    let mut wrong = Vec::new();
    for index in (0..DEFAULT_CASES).filter(|index| index % 3 == 0) {
        let (_, case) = case(index);
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let room = Room {
                columns: WIDE,
                glyphs,
            };
            if let Err(why) = this_crate(&case.source, &room) {
                wrong.push(format!(
                    "case {index} in {glyphs:?}: {why:?}\n{}",
                    case.source
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} not drawn, the first:\n{}",
        wrong.len(),
        wrong.first().map_or("", String::as_str)
    );
}

#[test]
fn a_planted_drawer_that_panics_is_found_by_the_generated_run() {
    fn panics_on_a_note(source: &str, room: &Room) -> Result<Laid, Undrawn> {
        assert!(!source.contains("note "), "planted: a drawer that panics");
        this_crate(source, room)
    }
    let found = run(0, 256, panics_on_a_note).expect_err("the planted panic");
    assert!(found.contains("panicked"), "{found}");
}

#[test]
fn a_planted_drawer_that_refuses_what_it_knows_is_found_by_the_generated_run() {
    fn refuses_a_hexagon(source: &str, room: &Room) -> Result<Laid, Undrawn> {
        if source.contains("{{\"") {
            return Err(Undrawn::NotText);
        }
        this_crate(source, room)
    }
    let found = run(0, 256, refuses_a_hexagon).expect_err("the planted refusal");
    assert!(found.contains("written whole was not drawn"), "{found}");
}

#[test]
fn generated_diagrams_are_drawn_or_shown_as_written_within_their_room() {
    let total = cases();
    println!("seed {SEED:#x}, {total} inputs ({CASES_VARIABLE} raises the count)");
    if let Err(why) = run(0, total, this_crate) {
        panic!("{why}");
    }
}
