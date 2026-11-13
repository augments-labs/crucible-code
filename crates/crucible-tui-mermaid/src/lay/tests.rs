//! The engine's reading of a state or entity-relationship diagram held to the
//! scan's: a reading that leaves out or changes what the scan read is
//! refused, and one alike is held.

use mermaid_text::parser;

use super::{entities_alike, state_alike};
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
