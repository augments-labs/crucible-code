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
//! width, at the narrowest a drawer is given, and in both glyph sets.
//!
//! A formula is laid out on the same thread when its block closes, and drawn
//! in a line as the line is read: a block at the formula drawer's byte cap, a
//! matrix as tall as its rows allow, a continued fraction as deep as those
//! rows allow, which lays every level out again under the next, matrices
//! nested as deep as the drawer reads, each level looking for its own end,
//! matrices nested past that to the byte cap, timed to their refusal, a span
//! at the byte cap and a span nested as deep as the drawer reads. A formula wider
//! than its room is refused for it rather than drawn, so each is laid out in
//! a room as wide as the drawer draws, in both glyph sets.
//!
//! A diagram is laid out on the same thread when its fence closes: for each
//! kind drawn as text, every box and every line its caps hold, each label as
//! long as the label cap and the byte cap leave room for, half of them in
//! characters two columns wide; and for each kind the costliest shape, four
//! boxes with every line the caps allow going out from the first to each of
//! the others and back again, and each label as long as those caps leave
//! room for, and for a state diagram the same inside a composite state. The
//! engine's cost follows the lines and the labels rather than the boxes, so
//! these are what a cap loosened on lines or labels makes dearer; each is
//! built from the caps themselves, never written out, so a loosened cap is
//! timed at what it admits. The costliest flowchart and state diagram found
//! when the caps were set are timed beside them as they were written. A
//! flowchart is laid top to bottom, which costs the engine more than left to
//! right for the same boxes and lines. A sequence diagram is timed with every
//! participant and every message its caps hold, and with its messages among
//! four participants; a gantt with every task its caps hold, in four sections
//! and in one, spanning as many days as it may and still be drawn a day to a
//! cell; and a git graph with every commit its caps hold, on every branch
//! they allow and on three, each id and tag as long as the byte cap leaves
//! room for. The engine lays a diagram out once whatever its room, and a drawing
//! wider than the room is refused for it only after, so each is laid out at a
//! common terminal's width, where it may be refused as too wide, and in a
//! window wide enough to draw it, in both glyph sets. Two milliseconds is the
//! budget for text laid out on the drawing thread: a tenth of the first
//! frame's twenty, so a block closing beside a streaming answer still leaves
//! the frame its room.
//!
//! A diagram is also drawn as a picture of the same layout, and a picture is
//! made on the same thread: laid out, rasterised and encoded as a PNG. For each
//! kind the costliest picture its caps hold is timed through all three, and a
//! sequence diagram's also with its messages among four participants, every
//! label in Latin letters, since a diagram holding a letter the picture's font
//! lacks keeps its text drawing and is never rasterised, and each label as long
//! as the caps leave room for, so a picture carries as many letters as the caps
//! let in, and every letter is outlines the rasteriser draws. It is rasterised
//! in every slot's ink, as a theme hands them, at the widest the most pixels a
//! picture may hold allow for its proportions, in a window wide enough to draw
//! it, since a picture is drawn at its own size whatever the room, and in both
//! glyph sets. A picture the drawer or the rasteriser refuses fails the probe
//! rather than timing the refusal. Thirty milliseconds is the budget for a
//! picture: half as much again as the first frame's whole budget, which a
//! picture spends once, when its fence closes.
//!
//! The reported number is the slowest text case's 95th percentile, with each
//! case's median and its slowest reading kept as evidence, and the slowest
//! picture's 95th percentile kept beside them with its limit. Every reading is one call,
//! timed with the input already built: the reading is the drawer's, not the
//! fixture's.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crucible_raster::{MOST_DECODED, png, rasterise};
use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Draws, Room};
use crucible_tui::{Glyphs, Slot};
use crucible_tui_diff::{
    Change, Changes, FOLD, File, MOST_BYTES, MOST_FILES, MOST_LINES, PAIRS, SYNTAX_BYTES,
    SYNTAX_LINES, Side, TOKENS,
};
use crucible_tui_math::{Formulas, MOST_COLUMNS, MOST_DEPTH, MOST_ROWS};
use crucible_tui_mermaid::{Caps, Diagrams};

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

/// The budget for a picture, in milliseconds: laid out, rasterised and
/// encoded, on a quiet machine and on a shared runner alike.
const PICTURE_LIMIT: f64 = 30.0;

/// Readings taken of each case, after the ones that warm it.
const RUNS: usize = 101;

/// Readings thrown away first, so the first call's allocations and caches are
/// not charged to the drawer.
const WARM: usize = 5;

/// A common terminal's width.
const WIDE: usize = 80;

/// The narrowest a drawer is given a block to lay out in.
const NARROW: usize = 8;

/// The rooms a change is laid out in.
const CHANGE_ROOMS: &[usize] = &[WIDE, NARROW];

/// The room a formula is laid out in: as wide as the formula drawer draws,
/// its gutter counted, so a formula at its column cap is drawn and the
/// reading is of a layout rather than of a refusal for the room.
const FORMULA_ROOMS: &[usize] = &[MOST_COLUMNS + 2];

/// A window wide enough to draw any diagram at the caps.
const WINDOW: usize = 402;

/// The rooms a diagram is laid out in: a common terminal's width with a
/// frame's gutter, and a window wide enough to draw any diagram at the caps.
const DIAGRAM_ROOMS: &[usize] = &[WIDE + 2, WINDOW];

/// The room a picture is made in: a window wide enough to draw any diagram at
/// the caps. A picture is drawn at its layout's own size whatever the room, so
/// a narrower one only refuses it as too wide, and a reading of that would
/// time the refusal.
const PICTURE_ROOMS: &[usize] = &[WINDOW];

/// The days a timed gantt's tasks start within: two short of the most the
/// engine draws a day to a cell, so a task two days long ends inside them.
const GANTT_DAYS: usize = 58;

/// The boxes the costliest diagram of a kind has: few enough that every line
/// its caps allow runs among the same boxes, out from one and back, which is
/// the shape the engine took longest over when the caps were set.
const FEW: usize = 4;

/// The costliest flowchart found when its caps were set, timed as written
/// beside the one built from the caps: five boxes, every line the caps allow,
/// two of them going back up to the first, and each label at the label cap.
const COSTLIEST_FLOWCHART: &str = "flowchart TD
N0{{\"0漢字漢字漢字漢\"}}
N1{\"1word word word \"}
N2{\"2漢字漢字漢字漢\"}
N3(\"3word word word \")
N4([\"4漢字漢字漢字漢\"])
N0 ---|1word word word | N1
N0 ---|2漢字漢字漢字漢| N2
N2 -->|5word word word | N3
N0 -->|4漢字漢字漢字漢| N4
N2 -->|2漢字漢字漢字漢| N0
N4 ==>|4漢字漢字漢字漢| N0
";

/// The costliest state diagram found when its caps were set, timed as
/// written beside the ones built from the caps: three states inside a
/// composite one, every transition the caps allow, each going round them, and
/// each label at the label cap.
const COSTLIEST_STATE: &str = "stateDiagram-v2
state \"0漢字漢字漢\" as S0
[*] --> S0
S0 --> G : 1word word w
state G {
state \"1word word w\" as T0
state \"2漢字漢字漢\" as T1
state \"3word word w\" as T2
T0 --> T1 : 1word word w
T1 --> T2 : 3word word w
T2 --> T0 : 2漢字漢字漢
T0 --> T1 : 1word word w
T1 --> T0 : 1word word w
T2 --> T0 : 2漢字漢字漢
}
";

/// The deepest a continued fraction in a line is read: one level more and the
/// formula drawer reads its nesting as past what it holds.
const DEEPEST_SPAN: usize = 32;

#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("bench-drawing: {0}")]
    Io(#[from] io::Error),
    #[error("bench-drawing: CRUCIBLE_BENCH_RUNNER is {0:?}; want it unset, or `shared`")]
    Runner(String),
    #[error("bench-drawing: {0} was not drawn; a reading of it would time a refusal")]
    Undrawn(&'static str),
    #[error("bench-drawing: {0} was drawn; a reading of it would time a layout, not its refusal")]
    Drawn(&'static str),
    #[error(
        "bench-drawing: {0} was not made a picture ({1}); a reading of it would time a refusal"
    )]
    Unpictured(&'static str, String),
}

/// What a case hands the drawer.
enum Input {
    /// A unified diff, read and laid out as a fenced block is.
    Diff(String),
    /// A change already built, laid out as a tool's edit is.
    Change(Change),
    /// A formula, laid out as display math is.
    Formula(String),
    /// A formula, drawn in a line as an inline span is.
    Span(String),
    /// A formula display math must refuse, timed to its refusal.
    Refused(String),
    /// A mermaid diagram, laid out whole and then drawn, or refused as wider
    /// than its room.
    Diagram(String),
    /// A mermaid diagram drawn as a picture: laid out, rasterised and encoded.
    Picture(String),
}

struct Case {
    name: &'static str,
    input: Input,
    /// The columns it is laid out at, in each glyph set.
    rooms: &'static [usize],
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

/// A formula of aligned rows, each a sum of fractions holding scripts and a
/// root, in as many rows as the formula drawer's bytes hold.
fn aligned_at_the_byte_cap() -> String {
    let term = r"\frac{\alpha_{k}+\beta}{\sqrt{\gamma}}";
    let row = format!("y &= {}", [term; 6].join(" + "));
    let ends = r"\begin{aligned}  \end{aligned}".len();
    let rows = (crucible_tui_math::MOST_BYTES - ends) / (row.len() + r" \\ ".len());
    format!(
        r"\begin{{aligned}} {} \end{{aligned}}",
        vec![row; rows].join(r" \\ ")
    )
}

/// A matrix as tall as the formula drawer lays out, between bracket pieces
/// as tall as it.
fn the_tallest_matrix() -> String {
    let rows: Vec<String> = (0..MOST_ROWS)
        .map(|row| {
            (0..4)
                .map(|column| format!(r"\alpha_{{{row}{column}}}"))
                .collect::<Vec<_>>()
                .join(" & ")
        })
        .collect();
    format!(r"\begin{{pmatrix}} {} \end{{pmatrix}}", rows.join(r" \\ "))
}

/// A fraction under a fraction `levels` deep, each level a sum beside it.
fn continued_fraction(levels: usize) -> String {
    format!(
        "x = {}y{}",
        r"1 + \frac{1}{".repeat(levels),
        "}".repeat(levels)
    )
}

/// A continued fraction as deep as the formula drawer's rows allow: two rows a
/// level, its rule and the row over it, and the innermost row.
fn the_deepest_block() -> String {
    continued_fraction((MOST_ROWS - 1) / 2)
}

/// A sum of fractions of roots and scripts in a line, in as many terms as the
/// formula drawer's bytes hold.
fn a_span_at_the_byte_cap() -> String {
    let term = r"\frac{\sqrt{\alpha_{1}}}{\beta^{2}} + ";
    format!(
        "{}y",
        term.repeat((crucible_tui_math::MOST_BYTES - 1) / term.len())
    )
}

/// `levels` matrices each inside the last, the innermost holding as many
/// letters as the formula drawer's bytes leave room for.
fn nested_matrices(levels: usize) -> String {
    let (open, close, letter) = (r"\begin{matrix}", r"\end{matrix}", r"\alpha");
    let ends = levels * (open.len() + close.len());
    let letters = crucible_tui_math::MOST_BYTES.saturating_sub(ends) / letter.len();
    format!(
        "{}{}{}",
        open.repeat(levels),
        letter.repeat(letters),
        close.repeat(levels)
    )
}

/// The letters a generated diagram's labels are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Letters {
    /// Half the labels in characters two columns wide, which cost a text
    /// layout the most for their columns.
    Mixed,
    /// Every label in Latin words: a picture is made only of a diagram whose
    /// every letter its font has.
    Latin,
}

/// A label of `columns` columns for box or line `index`: in `letters`, an
/// even one in characters two columns wide where they are mixed, and any
/// other in words.
fn label(index: usize, columns: usize, letters: Letters) -> String {
    let wide = letters == Letters::Mixed && index.is_multiple_of(2);
    let head = index.to_string();
    let rest = columns.saturating_sub(head.len());
    let tail: String = if wide {
        "漢字".chars().cycle().take(rest / 2).collect()
    } else {
        "word ".chars().cycle().take(rest).collect()
    };
    format!("{head}{}", tail.trim_end())
}

/// How a generated diagram's boxes are set out and joined.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// Every box the caps hold, each after the first reached from one before
    /// it, and the lines past those joining boxes again.
    Spread,
    /// [`FEW`] boxes with every line the caps hold running among them: out
    /// from the first to each of the others, then back from each, the last
    /// first, then out again.
    Few,
}

impl Shape {
    /// The boxes set out where the caps leave room for `most`.
    fn boxes(self, most: usize) -> usize {
        match self {
            Self::Spread => most,
            Self::Few => FEW.min(most),
        }
    }

    /// The lines between `boxes` boxes, `edges` of them.
    fn lines(self, boxes: usize, edges: usize) -> Vec<(usize, usize)> {
        match self {
            Self::Spread => {
                let reached = boxes.saturating_sub(1).max(1);
                (0..edges)
                    .map(|line| {
                        let to = line % reached + 1;
                        (line * 5 % to, to)
                    })
                    .collect()
            }
            Self::Few => {
                let boxes = boxes.max(2);
                let out = (1..boxes).map(|to| (0, to));
                let back = (1..boxes).rev().map(|from| (from, 0));
                let round: Vec<(usize, usize)> = out.chain(back).collect();
                round.iter().copied().cycle().take(edges).collect()
            }
        }
    }
}

/// What writes one kind of diagram: its caps, how its boxes are set out, the
/// letters of its labels and the columns of each.
type Write = fn(Caps, Shape, Letters, usize) -> String;

/// The longest source `write` makes in `shape`, with labels in `letters` of at
/// most `caps.label` columns, or as many as the bytes leave room for where
/// labels are not capped, that `caps.bytes` holds.
fn diagram_at_the_caps(caps: Caps, shape: Shape, letters: Letters, write: Write) -> String {
    widest_held(caps, shape, letters, write)
        .map(|columns| write(caps, shape, letters, columns))
        .unwrap_or_default()
}

/// The most columns a label of `write`'s in `letters` may take in `shape`, up to
/// `caps.label` or, where labels are not capped, `caps.bytes`, with the
/// source still within `caps.bytes`; `None` where a label of one column is
/// already past it. A source never shortens as its labels widen, so the
/// columns are found by halving: a search a column at a time builds a source
/// for each, and under a loosened byte cap that alone takes longer than the
/// probe may.
fn widest_held(caps: Caps, shape: Shape, letters: Letters, write: Write) -> Option<usize> {
    let held = |columns: usize| write(caps, shape, letters, columns).len() <= caps.bytes;
    if !held(1) {
        return None;
    }
    let (mut low, mut high) = (1, caps.label.unwrap_or(caps.bytes).max(1));
    while low < high {
        let middle = high - (high - low) / 2;
        if held(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Some(low)
}

/// A flowchart from top to bottom with as many boxes as `shape` sets out and
/// every line its caps hold, each line labelled.
fn flowchart(caps: Caps, shape: Shape, letters: Letters, columns: usize) -> String {
    let shapes = [
        ("[\"", "\"]"),
        ("(\"", "\")"),
        ("{\"", "\"}"),
        ("{{\"", "\"}}"),
    ];
    let boxes = shape.boxes(caps.nodes);
    let mut source = String::from("flowchart TD\n");
    for (index, (open, close)) in (0..boxes).zip(shapes.iter().cycle()) {
        let _ = writeln!(
            source,
            "N{index}{open}{}{close}",
            label(index, columns, letters)
        );
    }
    for (from, to) in shape.lines(boxes, caps.edges) {
        let _ = writeln!(
            source,
            "N{from} -->|{}| N{to}",
            label(from + to, columns, letters)
        );
    }
    source
}

/// A state diagram with as many states as `shape` sets out and every
/// transition its caps hold, its start marker among the boxes and its first
/// transition, each other transition labelled.
fn states(caps: Caps, shape: Shape, letters: Letters, columns: usize) -> String {
    let named = shape.boxes(caps.nodes.saturating_sub(1));
    let mut source = String::from("stateDiagram-v2\n");
    for index in 0..named {
        let _ = writeln!(
            source,
            "state \"{}\" as S{index}",
            label(index, columns, letters)
        );
    }
    source.push_str("[*] --> S0\n");
    for (from, to) in shape.lines(named, caps.edges.saturating_sub(1)) {
        let _ = writeln!(
            source,
            "S{from} --> S{to} : {}",
            label(from + to, columns, letters)
        );
    }
    source
}

/// A state diagram whose states after the first sit inside one composite
/// state: the start marker into the first, a transition from it into the
/// composite, and every other transition the caps hold among the states
/// inside, each labelled.
fn composite(caps: Caps, shape: Shape, letters: Letters, columns: usize) -> String {
    // The start marker, the first state and the composite are boxes too.
    let inner = shape.boxes(caps.nodes.saturating_sub(3));
    let mut source = String::from("stateDiagram-v2\n");
    let _ = writeln!(source, "state \"{}\" as S0", label(0, columns, letters));
    source.push_str("[*] --> S0\n");
    let _ = writeln!(source, "S0 --> G : {}", label(1, columns, letters));
    source.push_str("state G {\n");
    for index in 0..inner {
        let _ = writeln!(
            source,
            "state \"{}\" as T{index}",
            label(index + 1, columns, letters)
        );
    }
    for (from, to) in shape.lines(inner, caps.edges.saturating_sub(2)) {
        let _ = writeln!(
            source,
            "T{from} --> T{to} : {}",
            label(from + to, columns, letters)
        );
    }
    source.push_str("}\n");
    source
}

/// An entity-relationship diagram with as many entities as `shape` sets out
/// and every relationship its caps hold, each entity with two attributes and
/// each relationship labelled.
fn entities(caps: Caps, shape: Shape, letters: Letters, columns: usize) -> String {
    let kinds = ["||--o{", "||--|{", "}o--o{", "|o--||"];
    let boxes = shape.boxes(caps.nodes);
    let mut source = String::from("erDiagram\n");
    for index in 0..boxes {
        let named = label(index, columns, letters).replace(' ', "_");
        let _ = writeln!(source, "E{index} {{\nstring {named}\nint id\n}}");
    }
    for ((from, to), kind) in shape
        .lines(boxes, caps.edges)
        .into_iter()
        .zip(kinds.iter().cycle())
    {
        let _ = writeln!(
            source,
            "E{from} {kind} E{to} : \"{}\"",
            label(from + to, columns, letters)
        );
    }
    source
}

/// A sequence diagram with as many participants as `shape` sets out, each
/// declared with a label, and every message its caps hold, each labelled.
fn sequence(caps: Caps, shape: Shape, letters: Letters, columns: usize) -> String {
    let arrows = ["->>", "-->>", "->", "-->"];
    let participants = shape.boxes(caps.nodes);
    let mut source = String::from("sequenceDiagram\n");
    for index in 0..participants {
        let _ = writeln!(
            source,
            "participant P{index} as {}",
            label(index, columns, letters)
        );
    }
    for ((from, to), arrow) in shape
        .lines(participants, caps.edges)
        .into_iter()
        .zip(arrows.iter().cycle())
    {
        let _ = writeln!(
            source,
            "P{from}{arrow}P{to}: {}",
            label(from + to, columns, letters)
        );
    }
    source
}

/// A gantt with every section and task its caps hold: four sections where
/// `shape` spreads them and one where it keeps them few, each task two days
/// long and starting on its own day within [`GANTT_DAYS`], each section and
/// task named.
fn gantt(caps: Caps, shape: Shape, letters: Letters, columns: usize) -> String {
    let sections = match shape {
        Shape::Spread => FEW,
        Shape::Few => 1,
    };
    let tasks = caps.nodes.saturating_sub(sections);
    let mut source = String::from("gantt\ndateFormat YYYY-MM-DD\n");
    for index in 0..tasks {
        if index % tasks.div_ceil(sections).max(1) == 0 {
            let _ = writeln!(source, "section {}", label(index, columns, letters));
        }
        let day = index * 2 % GANTT_DAYS;
        let (month, date) = if day < 31 {
            (1, day + 1)
        } else {
            (2, day - 30)
        };
        let _ = writeln!(
            source,
            "{} :2026-{month:02}-{date:02}, 2d",
            label(index + 1, columns, letters)
        );
    }
    source
}

/// A git graph with every commit its caps hold: one on `main`, then for each
/// branch `shape` sets out a commit on it with a tag and one on `main`, then a
/// merge of each, then commits on `main` to the cap; each commit's id and tag
/// as long as `columns`.
fn git_graph(caps: Caps, shape: Shape, letters: Letters, columns: usize) -> String {
    let branches = shape.boxes(caps.edges.saturating_add(1)).saturating_sub(1);
    let mut source = String::from("gitGraph\n");
    let mut commits = 0;
    let named = |at: usize| label(at, columns, letters);
    commit(&mut source, &mut commits, named, false);
    for branch in 0..branches {
        let _ = writeln!(source, "branch b{branch}");
        commit(&mut source, &mut commits, named, true);
        source.push_str("checkout main\n");
        commit(&mut source, &mut commits, named, false);
    }
    for branch in 0..branches {
        let _ = writeln!(source, "merge b{branch}");
    }
    for _ in commits.saturating_add(branches)..caps.nodes {
        commit(&mut source, &mut commits, named, false);
    }
    source
}

/// One commit of a git graph written to `source`, its id the next of
/// `commits` as `label` writes it, with a tag `label` writes where `tagged`.
fn commit(source: &mut String, commits: &mut usize, label: impl Fn(usize) -> String, tagged: bool) {
    let _ = write!(source, "commit id: \"{}\"", label(*commits));
    if tagged {
        let _ = write!(source, " tag: \"{}\"", label(*commits + 1));
    }
    source.push('\n');
    *commits += 1;
}

/// Every case, in the order its readings are reported.
fn cases() -> Vec<Case> {
    let mut cases = blocks();
    cases.extend(diagrams());
    cases.extend(pictures());
    cases
}

/// The changes and formulas a drawer lays out at their caps.
fn blocks() -> Vec<Case> {
    vec![
        Case {
            name: "one file at the caps",
            input: Input::Diff(at_the_caps(1)),
            rooms: CHANGE_ROOMS,
        },
        Case {
            name: "every file a change holds",
            input: Input::Diff(at_the_caps(MOST_FILES)),
            rooms: CHANGE_ROOMS,
        },
        Case {
            name: "the longest lines a change holds",
            input: Input::Diff(the_longest_lines()),
            rooms: CHANGE_ROOMS,
        },
        Case {
            name: "word pairs at the caps",
            input: Input::Diff(word_pairs_at_the_caps()),
            rooms: CHANGE_ROOMS,
        },
        Case {
            name: "colour at the caps",
            input: Input::Diff(colour_at_the_caps()),
            rooms: CHANGE_ROOMS,
        },
        Case {
            name: "every cap at once",
            input: Input::Diff(every_cap_at_once()),
            rooms: CHANGE_ROOMS,
        },
        Case {
            name: "a tool's edit at the caps",
            input: Input::Change(a_tools_edit()),
            rooms: CHANGE_ROOMS,
        },
        Case {
            name: "a formula at the byte cap",
            input: Input::Formula(aligned_at_the_byte_cap()),
            rooms: FORMULA_ROOMS,
        },
        Case {
            name: "a matrix at the row cap",
            input: Input::Formula(the_tallest_matrix()),
            rooms: FORMULA_ROOMS,
        },
        Case {
            name: "a continued fraction at the row cap",
            input: Input::Formula(the_deepest_block()),
            rooms: FORMULA_ROOMS,
        },
        Case {
            name: "matrices nested as deep as a formula is read",
            input: Input::Formula(nested_matrices(MOST_DEPTH - 1)),
            rooms: FORMULA_ROOMS,
        },
        Case {
            name: "matrices nested past the depth cap to the byte cap",
            input: Input::Refused(nested_matrices(
                crucible_tui_math::MOST_BYTES / r"\begin{matrix}\end{matrix}".len(),
            )),
            rooms: FORMULA_ROOMS,
        },
        Case {
            name: "a span at the byte cap",
            input: Input::Span(a_span_at_the_byte_cap()),
            rooms: FORMULA_ROOMS,
        },
        Case {
            name: "a span at the depth cap",
            input: Input::Span(continued_fraction(DEEPEST_SPAN)),
            rooms: FORMULA_ROOMS,
        },
    ]
}

/// The diagrams of each kind laid out as text at their caps.
fn diagrams() -> Vec<Case> {
    vec![
        Case {
            name: "a flowchart at its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::FLOWCHART,
                Shape::Spread,
                Letters::Mixed,
                flowchart,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest flowchart inside its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::FLOWCHART,
                Shape::Few,
                Letters::Mixed,
                flowchart,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest flowchart found when its caps were set",
            input: Input::Diagram(COSTLIEST_FLOWCHART.to_owned()),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a state diagram at its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::STATE,
                Shape::Spread,
                Letters::Mixed,
                states,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest state diagram inside its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::STATE,
                Shape::Few,
                Letters::Mixed,
                states,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest composite state inside its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::STATE,
                Shape::Few,
                Letters::Mixed,
                composite,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest state diagram found when its caps were set",
            input: Input::Diagram(COSTLIEST_STATE.to_owned()),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "an er diagram at its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::ENTITIES,
                Shape::Spread,
                Letters::Mixed,
                entities,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest er diagram inside its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::ENTITIES,
                Shape::Few,
                Letters::Mixed,
                entities,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a sequence diagram at its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::SEQUENCE,
                Shape::Spread,
                Letters::Mixed,
                sequence,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a sequence diagram's messages among few participants",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::SEQUENCE,
                Shape::Few,
                Letters::Mixed,
                sequence,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a gantt at its caps in four sections",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::GANTT,
                Shape::Spread,
                Letters::Mixed,
                gantt,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a gantt at its caps in one section",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::GANTT,
                Shape::Few,
                Letters::Mixed,
                gantt,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a git graph at its caps on every branch",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::GIT_GRAPH,
                Shape::Spread,
                Letters::Mixed,
                git_graph,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a git graph at its caps on few branches",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::GIT_GRAPH,
                Shape::Few,
                Letters::Mixed,
                git_graph,
            )),
            rooms: DIAGRAM_ROOMS,
        },
    ]
}

/// The costliest picture of each kind: laid out, rasterised and encoded.
fn pictures() -> Vec<Case> {
    vec![
        Case {
            name: "a flowchart's picture at its caps",
            input: Input::Picture(diagram_at_the_caps(
                Caps::FLOWCHART,
                Shape::Spread,
                Letters::Latin,
                flowchart,
            )),
            rooms: PICTURE_ROOMS,
        },
        Case {
            name: "a state diagram's picture at its caps",
            input: Input::Picture(diagram_at_the_caps(
                Caps::STATE,
                Shape::Spread,
                Letters::Latin,
                states,
            )),
            rooms: PICTURE_ROOMS,
        },
        Case {
            name: "an er diagram's picture at its caps",
            input: Input::Picture(diagram_at_the_caps(
                Caps::ENTITIES,
                Shape::Spread,
                Letters::Latin,
                entities,
            )),
            rooms: PICTURE_ROOMS,
        },
        Case {
            name: "a sequence diagram's picture at its caps",
            input: Input::Picture(diagram_at_the_caps(
                Caps::SEQUENCE,
                Shape::Spread,
                Letters::Latin,
                sequence,
            )),
            rooms: PICTURE_ROOMS,
        },
        Case {
            name: "a sequence diagram's picture among few participants",
            input: Input::Picture(diagram_at_the_caps(
                Caps::SEQUENCE,
                Shape::Few,
                Letters::Latin,
                sequence,
            )),
            rooms: PICTURE_ROOMS,
        },
        Case {
            name: "a gantt's picture at its caps",
            input: Input::Picture(diagram_at_the_caps(
                Caps::GANTT,
                Shape::Spread,
                Letters::Latin,
                gantt,
            )),
            rooms: PICTURE_ROOMS,
        },
        Case {
            name: "a git graph's picture at its caps",
            input: Input::Picture(diagram_at_the_caps(
                Caps::GIT_GRAPH,
                Shape::Spread,
                Letters::Latin,
                git_graph,
            )),
            rooms: PICTURE_ROOMS,
        },
    ]
}

/// The slot after `slot` in the order they are declared, so that every
/// slot's ink can be walked: a slot added is a case here before this probe
/// compiles, and goes where it is declared.
fn after(slot: Slot) -> Option<Slot> {
    match slot {
        Slot::Plain => Some(Slot::Accent),
        Slot::Accent => Some(Slot::Strong),
        Slot::Strong => Some(Slot::Bold),
        Slot::Bold => Some(Slot::Pointed),
        Slot::Pointed => Some(Slot::Quiet),
        Slot::Quiet => Some(Slot::Cut),
        Slot::Cut => Some(Slot::Link),
        Slot::Link => Some(Slot::Emphasis),
        Slot::Emphasis => Some(Slot::BoldEmphasis),
        Slot::BoldEmphasis => Some(Slot::Struck),
        Slot::Struck => Some(Slot::Code),
        Slot::Code => Some(Slot::AllowEdits),
        Slot::AllowEdits => Some(Slot::FullAccess),
        Slot::FullAccess => Some(Slot::Doing),
        Slot::Doing => Some(Slot::DoingMark),
        Slot::DoingMark => Some(Slot::Done),
        Slot::Done => Some(Slot::DoneMark),
        Slot::DoneMark => Some(Slot::Trouble),
        Slot::Trouble => Some(Slot::Removed),
        Slot::Removed => Some(Slot::RemovedNumber),
        Slot::RemovedNumber => Some(Slot::Added),
        Slot::Added => Some(Slot::AddedNumber),
        Slot::AddedNumber => Some(Slot::Prompt),
        Slot::Prompt => Some(Slot::PromptMark),
        Slot::PromptMark => Some(Slot::Comment),
        Slot::Comment => Some(Slot::Keyword),
        Slot::Keyword => Some(Slot::Str),
        Slot::Str => Some(Slot::Number),
        Slot::Number => Some(Slot::Name),
        Slot::Name => Some(Slot::Operator),
        Slot::Operator => Some(Slot::Path),
        Slot::Path => Some(Slot::Revision),
        Slot::Revision => Some(Slot::Figure),
        Slot::Figure => Some(Slot::Heading),
        Slot::Heading => Some(Slot::Bullet),
        Slot::Bullet => Some(Slot::Ordinal),
        Slot::Ordinal => Some(Slot::Quote),
        Slot::Quote => Some(Slot::ArgumentPath),
        Slot::ArgumentPath => Some(Slot::ArgumentAddress),
        Slot::ArgumentAddress => Some(Slot::ArgumentCommand),
        Slot::ArgumentCommand => Some(Slot::RemovedWord),
        Slot::RemovedWord => Some(Slot::AddedWord),
        Slot::AddedWord => Some(Slot::RemovedCount),
        Slot::RemovedCount => Some(Slot::AddedCount),
        Slot::AddedCount => None,
    }
}

/// Every slot's class with an ink of its own, as a theme hands a picture its
/// inks: a style sheet of every class is what a picture is rasterised under.
fn inks() -> Vec<(&'static str, [u8; 3])> {
    let mut inks = Vec::new();
    let mut slot = Some(Slot::Plain);
    while let Some(at) = slot {
        let shade = u8::try_from(inks.len() * 5 % 256).unwrap_or(0);
        inks.push((at.class(), [shade, 128, 255 - shade]));
        slot = after(at);
    }
    inks
}

/// The value of `name` on the outermost element of `svg`, as a number.
fn attribute(svg: &str, name: &str) -> Option<u64> {
    let opened = svg.find('>')?;
    let head = svg.get(..opened)?;
    let key = format!(" {name}=\"");
    let start = head.find(&key)? + key.len();
    let rest = head.get(start..)?;
    rest.get(..rest.find('"')?)?.parse().ok()
}

/// The widest a picture `width` by `height` across may be rasterised at:
/// the most pixels, at its proportions and with the height rounded up as the
/// rasteriser rounds it, that [`MOST_DECODED`] holds.
fn widest(width: u64, height: u64) -> Option<u32> {
    let most = u64::try_from(MOST_DECODED).ok()? / 4;
    let held = |across: u64| {
        across.saturating_mul((across.saturating_mul(height)).div_ceil(width.max(1))) <= most
    };
    let (mut low, mut high) = (1_u64, most);
    while low < high {
        let middle = high - (high - low) / 2;
        if held(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    u32::try_from(low).ok()
}

/// `text` drawn as a picture in `room`: laid out, rasterised in every slot's
/// ink as wide as [`widest`] allows, and encoded as a PNG; the PNG's bytes,
/// or why it was not made.
fn picture(text: &str, room: &Room) -> Result<usize, String> {
    let svg = Diagrams
        .picture(text, room)
        .map_err(|refused| format!("drawer: {refused:?}"))?;
    let svg = svg.as_str();
    let across = attribute(svg, "width")
        .zip(attribute(svg, "height"))
        .and_then(|(width, height)| widest(width, height))
        .ok_or("drawer: no size")?;
    let pixels = rasterise(svg.as_bytes(), across, &inks())
        .map_err(|refused| format!("raster at {across} across: {refused:?}"))?;
    png(&pixels)
        .map(|encoded| encoded.len())
        .map_err(|refused| format!("png: {refused:?}"))
}

/// One layout of `case` in `room`, with the rows it laid out.
fn lay(case: &Case, room: &Room) -> Option<usize> {
    match &case.input {
        Input::Diff(text) => Changes.block(text, room).ok().map(|laid| laid.rows.len()),
        Input::Change(change) => Some(Changes.draw(change, room).rows.len()),
        Input::Formula(text) => Formulas.block(text, room).ok().map(|laid| laid.rows.len()),
        Input::Span(text) => Formulas.inline(text, room).map(|runs| runs.len()),
        // One row stands for the refusal, and none for a formula drawn.
        Input::Refused(text) => Formulas.block(text, room).err().map(|_| 1),
        // A diagram refused as wider than its room was laid out whole first,
        // and one row stands for that refusal.
        Input::Diagram(text) => match Diagrams.block(text, room) {
            Ok(laid) => Some(laid.rows.len()),
            Err(Undrawn::TooWide) => Some(1),
            Err(_) => None,
        },
        Input::Picture(text) => picture(text, room).ok(),
    }
}

/// What a reading in `room` that did not answer as its case says would have
/// timed.
fn misread(case: &Case, room: &Room) -> ProbeError {
    match &case.input {
        Input::Refused(_) => ProbeError::Drawn(case.name),
        Input::Picture(text) => {
            ProbeError::Unpictured(case.name, picture(text, room).err().unwrap_or_default())
        }
        Input::Diff(_)
        | Input::Change(_)
        | Input::Formula(_)
        | Input::Span(_)
        | Input::Diagram(_) => ProbeError::Undrawn(case.name),
    }
}

fn time(case: &Case) -> Result<Timed, ProbeError> {
    let rooms = [Glyphs::Unicode, Glyphs::Ascii]
        .into_iter()
        .flat_map(|glyphs| {
            case.rooms
                .iter()
                .map(move |&columns| Room { columns, glyphs })
        });
    let mut readings = Vec::with_capacity(RUNS * 2 * case.rooms.len());
    for room in rooms {
        for _ in 0..WARM {
            lay(case, &room).ok_or_else(|| misread(case, &room))?;
        }
        for _ in 0..RUNS {
            let started = Instant::now();
            let rows = lay(case, &room);
            readings.push(started.elapsed());
            if rows.unwrap_or(0) == 0 {
                return Err(misread(case, &room));
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

    let pictured: Vec<bool> = cases()
        .iter()
        .map(|case| matches!(case.input, Input::Picture(_)))
        .collect();
    let mut worst = 0.0_f64;
    let mut worst_picture = 0.0_f64;
    let mut evidence = String::new();
    for (index, case) in timed.iter().enumerate() {
        let p95 = case.at(0.95);
        if pictured.get(index).copied().unwrap_or(false) {
            worst_picture = worst_picture.max(p95);
        } else {
            worst = worst.max(p95);
        }
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
    let _ = writeln!(
        line,
        "{worst:.3} ms {limit:.1} picture_p95={worst_picture:.3} picture_limit={PICTURE_LIMIT:.1}{evidence}"
    );
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()?;

    let held = worst <= limit && worst_picture <= PICTURE_LIMIT;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The columns a search a column at a time down from the most finds: the
    /// first whose source `caps.bytes` holds.
    fn widest_a_column_at_a_time(caps: Caps, shape: Shape, write: Write) -> Option<usize> {
        let most = caps.label.unwrap_or(caps.bytes);
        (1..=most)
            .rev()
            .find(|&columns| write(caps, shape, Letters::Mixed, columns).len() <= caps.bytes)
    }

    /// Whether halving finds the columns a search a column at a time finds
    /// for `write` in `shape` at `caps`, and some.
    fn halved_alike(caps: Caps, shape: Shape, write: Write) {
        let halved = widest_held(caps, shape, Letters::Mixed, write);
        assert!(halved.is_some(), "{shape:?} holds no label");
        assert_eq!(
            halved,
            widest_a_column_at_a_time(caps, shape, write),
            "{shape:?}"
        );
    }

    #[test]
    fn halving_finds_a_flowchart_s_columns_at_its_caps() {
        halved_alike(Caps::FLOWCHART, Shape::Spread, flowchart);
        halved_alike(Caps::FLOWCHART, Shape::Few, flowchart);
    }

    #[test]
    fn halving_finds_a_state_diagram_s_columns_at_its_caps() {
        halved_alike(Caps::STATE, Shape::Spread, states);
        halved_alike(Caps::STATE, Shape::Few, states);
        halved_alike(Caps::STATE, Shape::Spread, composite);
        halved_alike(Caps::STATE, Shape::Few, composite);
    }

    #[test]
    fn halving_finds_an_er_diagram_s_columns_at_its_caps_with_every_entity() {
        halved_alike(Caps::ENTITIES, Shape::Spread, entities);
    }

    #[test]
    fn halving_finds_an_er_diagram_s_columns_at_its_caps_with_few_entities() {
        halved_alike(Caps::ENTITIES, Shape::Few, entities);
    }

    #[test]
    fn halving_finds_a_sequence_diagram_s_columns_at_its_caps() {
        halved_alike(Caps::SEQUENCE, Shape::Spread, sequence);
        halved_alike(Caps::SEQUENCE, Shape::Few, sequence);
    }

    #[test]
    fn halving_finds_a_gantt_s_columns_at_its_caps() {
        halved_alike(Caps::GANTT, Shape::Spread, gantt);
        halved_alike(Caps::GANTT, Shape::Few, gantt);
    }

    #[test]
    fn halving_finds_a_git_graph_s_columns_at_its_caps() {
        halved_alike(Caps::GIT_GRAPH, Shape::Spread, git_graph);
        halved_alike(Caps::GIT_GRAPH, Shape::Few, git_graph);
    }

    /// Every diagram a case times is drawn, or refused only as wider than its
    /// room: a refusal for any other reason would time the refusal.
    #[test]
    fn every_diagram_timed_is_laid_out() {
        for case in cases() {
            if let Input::Diagram(text) = &case.input {
                for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                    for &columns in case.rooms {
                        let laid = Diagrams.block(text, &Room { columns, glyphs });
                        assert!(
                            matches!(laid, Ok(_) | Err(Undrawn::TooWide)),
                            "{} at {columns} in {glyphs:?}: {laid:?}\n{text}",
                            case.name
                        );
                    }
                }
            }
        }
    }

    /// Every diagram a picture case times is made a picture by the drawer,
    /// at a size whose widest raster the rasteriser's pixels hold: a refusal
    /// would time the refusal, and one the rasteriser makes fails the probe.
    #[test]
    fn every_picture_timed_is_drawn_as_a_picture() {
        let mut pictures = 0;
        for case in cases() {
            if let Input::Picture(text) = &case.input {
                pictures += 1;
                for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                    for &columns in case.rooms {
                        let svg = Diagrams.picture(text, &Room { columns, glyphs });
                        let sized = svg.as_ref().ok().and_then(|svg| {
                            let svg = svg.as_str();
                            attribute(svg, "width").zip(attribute(svg, "height"))
                        });
                        assert!(
                            sized.is_some_and(|(width, height)| widest(width, height).is_some()),
                            "{} at {columns} in {glyphs:?}: {svg:?}\n{text}",
                            case.name
                        );
                    }
                }
            }
        }
        assert_eq!(
            pictures, 7,
            "one picture for each kind, and a sequence diagram's among few participants"
        );
    }

    /// The widest a picture is rasterised at keeps its pixels inside what the
    /// rasteriser holds, and one more column would not.
    #[test]
    fn the_widest_raster_is_the_most_the_rasteriser_holds() {
        let most = MOST_DECODED as u64 / 4;
        for (width, height) in [(3216, 16), (3216, 4000), (8, 16), (1, 100_000)] {
            let across = u64::from(widest(width, height).unwrap_or(0));
            let pixels = |across: u64| across * (across * height).div_ceil(width);
            assert!(pixels(across) <= most, "{width}x{height} at {across}");
            assert!(pixels(across + 1) > most, "{width}x{height} at {across}");
        }
    }

    /// Every slot's class is an ink a picture is rasterised under, once.
    #[test]
    fn every_slot_is_an_ink_once() {
        let inks = inks();
        let mut classes: Vec<&str> = inks.iter().map(|(class, _)| *class).collect();
        classes.sort_unstable();
        classes.dedup();
        assert_eq!(classes.len(), inks.len());
        assert!(classes.contains(&Slot::Plain.class()));
        assert!(classes.contains(&Slot::AddedCount.class()));
    }
}
