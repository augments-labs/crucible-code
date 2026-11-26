//! The engine's reading of a state, entity-relationship, sequence, gantt or
//! git graph diagram held to the scan's: a reading that leaves out or changes
//! what the scan read is refused, and one alike is held. And a flowchart's or
//! a state diagram's drawing in colour, its colours taken out, held to the
//! engine's drawing of it without colour.

use std::fs;

use mermaid_text::{RenderOptions, parser, render_with_options};

use super::{
    SPACING, entities_alike, gantt_alike, git_graph_alike, labels, sequence_alike, state_alike,
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
            let painted = drawing(&labels::painted(&source, lines), ascii, true);
            let held = painted.and_then(|painted| {
                labels::held(&painted.join("\n"), &vec![false; lines])
                    .map(|held| held.lines().map(str::to_owned).collect())
                    .map_err(|unlaid| format!("{unlaid:?}"))
            });
            assert_eq!(held, plain, "ascii {ascii}:\n{source}");
        }
    }
}
