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
//! right for the same boxes and lines. The engine lays a diagram out once whatever its room, and a drawing
//! wider than the room is refused for it only after, so each is laid out at a
//! common terminal's width, where it may be refused as too wide, and in a
//! window wide enough to draw it, in both glyph sets. Two milliseconds is the
//! budget for text laid out on the drawing thread: a tenth of the first
//! frame's twenty, so a block closing beside a streaming answer still leaves
//! the frame its room.
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
use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Draws, Room};
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

/// The rooms a diagram is laid out in: a common terminal's width with a
/// frame's gutter, and a window wide enough to draw any diagram at the caps.
const DIAGRAM_ROOMS: &[usize] = &[WIDE + 2, 402];

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

/// A label of `columns` columns for box or line `index`: an even one in
/// characters two columns wide, an odd one in words.
fn label(index: usize, columns: usize) -> String {
    let wide = index.is_multiple_of(2);
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

/// What writes one kind of diagram: its caps, how its boxes are set out, and
/// the columns of each label.
type Write = fn(Caps, Shape, usize) -> String;

/// The longest source `write` makes in `shape`, with labels of at most
/// `caps.label` columns, or as many as the bytes leave room for where labels
/// are not capped, that `caps.bytes` holds.
fn diagram_at_the_caps(caps: Caps, shape: Shape, write: Write) -> String {
    let most = caps.label.unwrap_or(caps.bytes);
    (1..=most)
        .rev()
        .map(|columns| write(caps, shape, columns))
        .find(|source| source.len() <= caps.bytes)
        .unwrap_or_default()
}

/// A flowchart from top to bottom with as many boxes as `shape` sets out and
/// every line its caps hold, each line labelled.
fn flowchart(caps: Caps, shape: Shape, columns: usize) -> String {
    let shapes = [
        ("[\"", "\"]"),
        ("(\"", "\")"),
        ("{\"", "\"}"),
        ("{{\"", "\"}}"),
    ];
    let boxes = shape.boxes(caps.nodes);
    let mut source = String::from("flowchart TD\n");
    for (index, (open, close)) in (0..boxes).zip(shapes.iter().cycle()) {
        let _ = writeln!(source, "N{index}{open}{}{close}", label(index, columns));
    }
    for (from, to) in shape.lines(boxes, caps.edges) {
        let _ = writeln!(source, "N{from} -->|{}| N{to}", label(from + to, columns));
    }
    source
}

/// A state diagram with as many states as `shape` sets out and every
/// transition its caps hold, its start marker among the boxes and its first
/// transition, each other transition labelled.
fn states(caps: Caps, shape: Shape, columns: usize) -> String {
    let named = shape.boxes(caps.nodes.saturating_sub(1));
    let mut source = String::from("stateDiagram-v2\n");
    for index in 0..named {
        let _ = writeln!(source, "state \"{}\" as S{index}", label(index, columns));
    }
    source.push_str("[*] --> S0\n");
    for (from, to) in shape.lines(named, caps.edges.saturating_sub(1)) {
        let _ = writeln!(source, "S{from} --> S{to} : {}", label(from + to, columns));
    }
    source
}

/// A state diagram whose states after the first sit inside one composite
/// state: the start marker into the first, a transition from it into the
/// composite, and every other transition the caps hold among the states
/// inside, each labelled.
fn composite(caps: Caps, shape: Shape, columns: usize) -> String {
    // The start marker, the first state and the composite are boxes too.
    let inner = shape.boxes(caps.nodes.saturating_sub(3));
    let mut source = String::from("stateDiagram-v2\n");
    let _ = writeln!(source, "state \"{}\" as S0", label(0, columns));
    source.push_str("[*] --> S0\n");
    let _ = writeln!(source, "S0 --> G : {}", label(1, columns));
    source.push_str("state G {\n");
    for index in 0..inner {
        let _ = writeln!(
            source,
            "state \"{}\" as T{index}",
            label(index + 1, columns)
        );
    }
    for (from, to) in shape.lines(inner, caps.edges.saturating_sub(2)) {
        let _ = writeln!(source, "T{from} --> T{to} : {}", label(from + to, columns));
    }
    source.push_str("}\n");
    source
}

/// An entity-relationship diagram with as many entities as `shape` sets out
/// and every relationship its caps hold, each entity with two attributes and
/// each relationship labelled.
fn entities(caps: Caps, shape: Shape, columns: usize) -> String {
    let kinds = ["||--o{", "||--|{", "}o--o{", "|o--||"];
    let boxes = shape.boxes(caps.nodes);
    let mut source = String::from("erDiagram\n");
    for index in 0..boxes {
        let named = label(index, columns).replace(' ', "_");
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
            label(from + to, columns)
        );
    }
    source
}

fn cases() -> Vec<Case> {
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
        Case {
            name: "a flowchart at its caps",
            input: Input::Diagram(diagram_at_the_caps(
                Caps::FLOWCHART,
                Shape::Spread,
                flowchart,
            )),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest flowchart inside its caps",
            input: Input::Diagram(diagram_at_the_caps(Caps::FLOWCHART, Shape::Few, flowchart)),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest flowchart found when its caps were set",
            input: Input::Diagram(COSTLIEST_FLOWCHART.to_owned()),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "a state diagram at its caps",
            input: Input::Diagram(diagram_at_the_caps(Caps::STATE, Shape::Spread, states)),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest state diagram inside its caps",
            input: Input::Diagram(diagram_at_the_caps(Caps::STATE, Shape::Few, states)),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest composite state inside its caps",
            input: Input::Diagram(diagram_at_the_caps(Caps::STATE, Shape::Few, composite)),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest state diagram found when its caps were set",
            input: Input::Diagram(COSTLIEST_STATE.to_owned()),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "an er diagram at its caps",
            input: Input::Diagram(diagram_at_the_caps(Caps::ENTITIES, Shape::Spread, entities)),
            rooms: DIAGRAM_ROOMS,
        },
        Case {
            name: "the costliest er diagram inside its caps",
            input: Input::Diagram(diagram_at_the_caps(Caps::ENTITIES, Shape::Few, entities)),
            rooms: DIAGRAM_ROOMS,
        },
    ]
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
    }
}

/// What a reading that did not answer as its case says would have timed.
fn misread(case: &Case) -> ProbeError {
    match case.input {
        Input::Refused(_) => ProbeError::Drawn(case.name),
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
            lay(case, &room).ok_or_else(|| misread(case))?;
        }
        for _ in 0..RUNS {
            let started = Instant::now();
            let rows = lay(case, &room);
            readings.push(started.elapsed());
            if rows.unwrap_or(0) == 0 {
                return Err(misread(case));
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
