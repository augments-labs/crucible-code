//! A decision reaches the agent's wire only for the run that asked.

use crucible_execution::{Decision, Lasting, Pending, SessionRef, Stale, Text};
use crucible_types::TurnId;

use crate::Asked;

fn question() -> Pending {
    Pending::Question {
        prompt: Text::new("Which branch?"),
    }
}

fn session() -> SessionRef {
    SessionRef::new("one").expect("a short name")
}

/// A program numbers its questions afresh each run, so the second run asks
/// under the first run's name. A decision composed for the first run is
/// refused as stale rather than sent under that name to a question it was
/// never about.
#[test]
fn a_decision_for_a_run_since_restarted_is_refused_though_its_wire_name_is_reused() {
    let mut asked = Asked::new().expect("a fresh table");
    asked.begin(session(), TurnId::FIRST);
    let old = asked.raise("1", question()).expect("room for one");
    assert!(asked.relaunch(), "a fresh run");
    asked.begin(session(), TurnId::FIRST);
    asked.raise("1", question()).expect("room for one");

    assert_eq!(
        asked.settle(&old, &Decision::Allow(Lasting::Once)).err(),
        Some(Stale::Generation)
    );
}
