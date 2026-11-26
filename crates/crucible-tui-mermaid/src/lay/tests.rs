//! The engine's reading of a state, entity-relationship, sequence, gantt or
//! git graph diagram held to the scan's: a reading that leaves out or changes
//! what the scan read is refused, and one alike is held. And a flowchart's or
//! a state diagram's drawing in colour, its colours taken out, held to the
//! engine's drawing of it without colour, and refused where its colours
//! leave a cell unaccounted for or set a label across another line.

use std::fs;

use mermaid_text::{RenderOptions, parser, render_with_options, to_ascii};

use super::{
    SPACING, Unlaid, entities_alike, gantt_alike, git_graph_alike, labels, sequence_alike,
    state_alike,
};
use crate::Kind;
use crate::scan::scan;

const STATES: &str = "stateDiagram-v2
[*] --> Still
Still --> Moving : push
Moving --> Still : stop
Moving --> Crash
Crash --> [*]
";

const ENTITIES: &str = "erDiagram
CUSTOMER ||--o{ ORDER : places
ORDER ||--|{ CUSTOMER : bills
PRODUCT {
    string sku
}
";

const SEQUENCE: &str = "sequenceDiagram
participant U as User
U->>S: request
S-->>U: answer
";

const GANTT: &str = "gantt
section Build
Design :a1, 2026-01-05, 3d
Code :after a1, 5d
";

const GIT_GRAPH: &str = "gitGraph
commit
branch feature
commit tag: \"v1\"
checkout main
commit id: \"fix\"
merge feature
";

/// Whether the engine's reading of `read_as`, a state diagram, is held to
/// the scan's reading of `written`; `None` where either does not read it.
fn states_held(written: &str, read_as: &str) -> Option<bool> {
    let scanned = scan(written).ok()?;
    let graph = parser::state::parse(read_as).ok()?;
    Some(state_alike(&graph, &scanned.read))
}

/// Whether the engine's reading of `read_as`, an entity-relationship
/// diagram, is held to the scan's reading of `written`; `None` where either
/// does not read it.
fn entities_held(written: &str, read_as: &str) -> Option<bool> {
    let scanned = scan(written).ok()?;
    let diagram = parser::er::parse(read_as).ok()?;
    Some(entities_alike(&diagram, &scanned.read))
}

/// Whether the engine's reading of `read_as` is held to the scan's reading
/// of `written`, each a diagram of the kind `written` opens with; `None`
/// where either does not read it.
fn timeline_held(written: &str, read_as: &str) -> Option<bool> {
    let scanned = scan(written).ok()?;
    let read = &scanned.read;
    match written.lines().next()? {
        "sequenceDiagram" => parser::sequence::parse(read_as)
            .ok()
            .map(|diagram| sequence_alike(&diagram, read)),
        "gantt" => parser::gantt::parse(read_as)
            .ok()
            .map(|diagram| gantt_alike(&diagram, read)),
        "gitGraph" => parser::git_graph::parse(read_as)
            .ok()
            .map(|graph| git_graph_alike(&graph, read)),
        _ => None,
    }
}

#[test]
fn a_timeline_read_alike_is_held() {
    for source in [SEQUENCE, GANTT, GIT_GRAPH] {
        assert_eq!(timeline_held(source, source), Some(true), "{source}");
    }
}

#[test]
fn a_timeline_reading_that_leaves_out_or_changes_what_was_read_is_refused() {
    for (source, from, to) in [
        (SEQUENCE, "S-->>U: answer\n", ""),
        (SEQUENCE, ": answer", ": reply"),
        (SEQUENCE, "as User", "as Someone"),
        (SEQUENCE, "S-->>U", "U-->>S"),
        (
            SEQUENCE,
            "S-->>U: answer\n",
            "S-->>U: answer\nNote right of U: a note\n",
        ),
        (GANTT, "Code :after a1, 5d\n", ""),
        (GANTT, "Code :", "Coding :"),
        (GANTT, "section Build", "section Make"),
        (GIT_GRAPH, "commit id: \"fix\"\n", ""),
        (GIT_GRAPH, "tag: \"v1\"", "tag: \"v2\""),
        (GIT_GRAPH, "id: \"fix\"", "id: \"mend\""),
        (GIT_GRAPH, "feature", "other"),
        (GIT_GRAPH, "checkout main\n", ""),
        (GIT_GRAPH, "merge feature\n", "cherry-pick id: \"c1\"\n"),
    ] {
        let changed = source.replace(from, to);
        assert_ne!(changed, source, "{from}");
        assert_eq!(timeline_held(source, &changed), Some(false), "{changed}");
    }
}

#[test]
fn a_state_diagram_read_alike_is_held() {
    assert_eq!(states_held(STATES, STATES), Some(true));
}

#[test]
fn a_state_reading_that_leaves_out_a_transition_is_refused() {
    let dropped = STATES.replace("Moving --> Still : stop\n", "");
    assert_eq!(states_held(STATES, &dropped), Some(false));
}

#[test]
fn a_state_reading_that_labels_a_transition_otherwise_is_refused() {
    let relabelled = STATES.replace(": push", ": pull");
    assert_eq!(states_held(STATES, &relabelled), Some(false));
}

#[test]
fn an_entity_relationship_diagram_read_alike_is_held() {
    assert_eq!(entities_held(ENTITIES, ENTITIES), Some(true));
}

#[test]
fn an_entity_reading_that_leaves_out_an_entity_is_refused() {
    let dropped = ENTITIES.replace("PRODUCT {\n    string sku\n}\n", "");
    assert_eq!(entities_held(ENTITIES, &dropped), Some(false));
}

#[test]
fn an_entity_reading_that_names_an_entity_otherwise_is_refused() {
    let renamed = ENTITIES.replace("PRODUCT", "ITEM");
    assert_eq!(entities_held(ENTITIES, &renamed), Some(false));
}

#[test]
fn an_entity_reading_that_leaves_out_a_relationship_is_refused() {
    let dropped = ENTITIES.replace("ORDER ||--|{ CUSTOMER : bills\n", "");
    assert_eq!(entities_held(ENTITIES, &dropped), Some(false));
}

/// The flowcharts and state diagrams of the hostile corpus, and this
/// module's state diagram, as the scan keeps them for the engine, each with
/// the count of the lines the engine reads in it.
fn with_lines() -> Vec<(String, usize)> {
    let corpus = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");
    let mut entries: Vec<_> = fs::read_dir(corpus)
        .expect("the corpus is listed")
        .map(|entry| entry.expect("a corpus entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mmd"))
        .collect();
    entries.sort();
    let written = entries
        .iter()
        .map(|path| fs::read_to_string(path).expect("a corpus entry is read"));
    std::iter::once(STATES.to_owned())
        .chain(written)
        .filter_map(|written| {
            let scanned = scan(&written).ok()?;
            let graph = match scanned.kind {
                Kind::Flowchart => parser::flowchart::parse(&scanned.source),
                Kind::State => parser::state::parse(&scanned.source),
                _ => return None,
            };
            let lines = graph.ok()?.edges.len();
            Some((scanned.source, lines))
        })
        .collect()
}

/// The engine's drawing of `source`, as the layout hands it the source, its
/// rows' trailing blanks taken off, or its error.
fn drawing(source: &str, ascii: bool, color: bool) -> Result<Vec<String>, String> {
    let options = RenderOptions {
        max_width: Some(120),
        ascii,
        gaps_override: Some(SPACING),
        color,
        ..RenderOptions::default()
    };
    render_with_options(source, &options)
        .map(|drawn| drawn.lines().map(|row| row.trim_end().to_owned()).collect())
        .map_err(|error| format!("{error:?}"))
}

#[test]
fn a_drawing_in_colour_its_colours_taken_out_is_the_drawing_without_colour() {
    let sources = with_lines();
    assert!(sources.len() >= 10, "{} diagrams", sources.len());
    for (source, lines) in sources {
        for ascii in [false, true] {
            let plain = drawing(&source, ascii, false);
            // Painted as the layout paints it: with the glyphs the engine
            // knows, put in plain glyphs once held.
            let painted = drawing(&labels::painted(&source, lines), false, true);
            let held = painted.and_then(|painted| {
                labels::held(&painted.join("\n"), &vec![false; lines])
                    .map(|held| if ascii { to_ascii(&held) } else { held })
                    .map(|held| held.lines().map(str::to_owned).collect())
                    .map_err(|unlaid| format!("{unlaid:?}"))
            });
            assert_eq!(held, plain, "ascii {ascii}:\n{source}");
        }
    }
}

/// A drawing in colour as cells, row by row: each cell's glyph, and its
/// colour read as one number, or `None` where it has none. A wide glyph is
/// one cell.
fn cells(drawn: &str) -> Vec<Vec<(char, Option<u32>)>> {
    let mut colour = None;
    let mut rows = vec![Vec::new()];
    let mut rest = drawn;
    while let Some(character) = rest.chars().next() {
        if character == '\x1b' {
            let (sequence, after) = rest.split_once('m').expect("an escape ends");
            colour = sequence.strip_prefix("\x1b[38;2;").map(|body| {
                body.split(';')
                    .map(|part| part.parse::<u32>().expect("a colour's part"))
                    .fold(0, |colour, part| colour << 8 | part)
            });
            rest = after;
            continue;
        }
        rest = rest.get(character.len_utf8()..).unwrap_or_default();
        match rows.last_mut() {
            Some(row) if character != '\n' => row.push((character, colour)),
            _ => rows.push(Vec::new()),
        }
    }
    rows
}

/// `cells` drawn in colour as the engine draws it.
fn coloured(cells: &[Vec<(char, Option<u32>)>]) -> String {
    let rows: Vec<String> = cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|&(glyph, colour)| match colour {
                    Some(colour) => format!(
                        "\x1b[38;2;{};{};{}m{glyph}\x1b[0m",
                        colour >> 16,
                        colour >> 8 & 0xff,
                        colour & 0xff
                    ),
                    None => glyph.to_string(),
                })
                .collect()
        })
        .collect();
    rows.join("\n")
}

/// Where `text` stands in `cells`: its row, and the first of its cells.
fn at(cells: &[Vec<(char, Option<u32>)>], text: &str) -> (usize, usize) {
    let text: Vec<char> = text.chars().collect();
    cells
        .iter()
        .enumerate()
        .find_map(|(row, cells)| {
            let glyphs: Vec<char> = cells.iter().map(|&(glyph, _)| glyph).collect();
            let from = glyphs.windows(text.len()).position(|run| run == text)?;
            Some((row, from))
        })
        .expect("the text is drawn")
}

/// A flowchart whose last line runs back up to its first box, and is the one
/// labelled.
const BACK: &str = "graph TD
    A[alpha] --> B[beta]
    B --> C[gamma]
    C -->|back| A
";

/// [`BACK`] drawn in colour as the layout paints it, as cells.
fn back() -> Vec<Vec<(char, Option<u32>)>> {
    let painted = labels::painted(BACK, 3);
    let drawn = drawing(&painted, false, true).expect("the engine draws it");
    cells(&drawn.join("\n"))
}

#[test]
fn a_labelled_drawing_with_a_stroke_left_unpainted_where_a_back_edge_joins_its_box_is_refused() {
    let labelled = [false, false, true];
    let mut drawn = back();
    let shown = coloured(&drawn);
    assert!(labels::held(&shown, &labelled).is_ok(), "{shown}");
    // The glyphs that join the line running back to its source's box, which
    // the engine draws without the line's colour where the join is off the
    // path it routed.
    let (row, from) = at(&drawn, "├└┘");
    for cell in drawn
        .get_mut(row)
        .and_then(|row| row.get_mut(from + 1..from + 3))
        .expect("the join is drawn")
    {
        cell.1 = None;
    }
    let shown = coloured(&drawn);
    assert_eq!(
        labels::held(&shown, &labelled),
        Err(Unlaid::Misplaced),
        "{shown}"
    );
}

#[test]
fn a_labelled_drawing_with_a_line_colour_on_a_cell_that_holds_no_line_is_refused() {
    let labelled = [false, false, true];
    // The labelled line's colour on a box's text, and on a blank.
    for (text, offset) in [("beta", 0), ("│ beta", 1)] {
        let mut drawn = back();
        let (row, from) = at(&drawn, text);
        let cell = drawn
            .get_mut(row)
            .and_then(|row| row.get_mut(from + offset))
            .expect("the text is drawn");
        cell.1 = Some(5);
        let shown = coloured(&drawn);
        assert_eq!(
            labels::held(&shown, &labelled),
            Err(Unlaid::Misplaced),
            "{text}\n{shown}"
        );
    }
}

/// A flowchart the engine draws with its first line's label set over the
/// third line's stroke running down, as near its own line as the line it
/// crosses.
const ACROSS: &str = "graph TB
    N1([\"한국어\"]) -.->|選ぶ?| N0((\"check it\"))
    N0 -. a & b .-> N1
    N1 -.->|한국어| N0
    N1 -->|done| N0
";

#[test]
fn a_label_set_across_another_line_on_its_row_is_refused_and_one_between_two_lines_is_held() {
    // Its own line running down through it, and another line's stroke
    // running into it from each side on its row.
    let across = [
        vec![(' ', None), ('│', Some(1)), (' ', None), (' ', None)],
        vec![
            ('─', Some(3)),
            ('g', Some(2)),
            ('o', Some(2)),
            ('─', Some(3)),
        ],
        vec![(' ', None), ('│', Some(1)), (' ', None), (' ', None)],
    ];
    let shown = coloured(&across);
    assert_eq!(
        labels::held(&shown, &[true, false]),
        Err(Unlaid::Misplaced),
        "{shown}"
    );
    // A label between its own line and another, as near each.
    let between = [
        vec![('│', Some(1)), (' ', None), (' ', None), ('│', Some(3))],
        vec![
            ('│', Some(1)),
            ('g', Some(2)),
            ('o', Some(2)),
            ('│', Some(3)),
        ],
        vec![('│', Some(1)), (' ', None), (' ', None), ('│', Some(3))],
    ];
    let shown = coloured(&between);
    assert!(labels::held(&shown, &[true, false]).is_ok(), "{shown}");
    // Set over another line running down, held to the first line's label
    // alone, since the others are nearer another line than their own, it is
    // held as near its own line as the other.
    let drawn = drawing(&labels::painted(ACROSS, 4), false, true).expect("the engine draws it");
    let shown = drawn.join("\n");
    assert!(
        labels::held(&shown, &[true, false, false, false]).is_ok(),
        "{shown}"
    );
}
