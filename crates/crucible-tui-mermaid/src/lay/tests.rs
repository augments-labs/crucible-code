//! The engine's reading of a state, entity-relationship, sequence, gantt or
//! git graph diagram held to the scan's: a reading that leaves out or changes
//! what the scan read is refused, and one alike is held.

use mermaid_text::parser;

use super::{entities_alike, gantt_alike, git_graph_alike, sequence_alike, state_alike};
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
