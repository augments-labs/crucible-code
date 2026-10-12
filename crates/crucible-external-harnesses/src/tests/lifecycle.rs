//! An account operation is kept before it acts, and one scope runs one at a
//! time.

use crucible_execution::{Admission, Ending};

use super::Owned;
use crate::{Contended, Coordinator, Journal, Stage, Work, needs_recovery};

fn coordinator(at: &Owned) -> Coordinator {
    let journal = Journal::of(at.0.join("journal"), "scope").expect("a short scope");
    Coordinator::restore(journal, "claude", Admission::Enabled).expect("a short name")
}

/// What a crash between two stages leaves is the stage written before it,
/// and the next process to start finds the instance needing a check.
#[test]
fn an_operation_cut_off_between_stages_leaves_its_instance_needing_a_check() {
    let at = Owned::new("cut-off");
    let mut first = coordinator(&at);
    let mut slot = first
        .begin(Work::Connect, Some("profile"), 1)
        .expect("nothing else holds the scope");
    slot.advance(Stage::Mutating).expect("kept");
    // Nothing a dropped slot does reaches the journal: this is all a killed
    // process would have left.
    drop(slot);
    drop(first);

    let mut restarted = coordinator(&at);
    assert!(needs_recovery(&restarted.lifecycle()));
    let journal = Journal::of(at.0.join("journal"), "scope").expect("a short scope");
    let open = journal.read().expect("readable").open.expect("still open");
    assert_eq!(open.stage(), Stage::Mutating);
    // Only a check may begin on what nobody saw end.
    let refused = restarted.begin(Work::SignOut, None, 2).map(|_| ());
    assert!(matches!(refused, Err(Contended::Refused(_))), "{refused:?}");
    restarted
        .begin(Work::Connect, None, 2)
        .expect("a connect checks first")
        .finish(Ending::Succeeded)
        .expect("kept");
    let entries = journal.read().expect("readable");
    assert!(entries.open.is_none());
    assert_eq!(
        entries
            .history
            .front()
            .and_then(crate::journal::Record::ended),
        Some(Ending::Unknown),
        "the cut-off operation is recorded as nobody saw it end"
    );
}

/// Two hosts on one profile share its sign-in, and a second operation begun
/// while the first is under way is refused rather than interleaved with it.
#[test]
fn two_coordinators_on_one_scope_run_one_operation_at_a_time() {
    let at = Owned::new("serialized");
    let mut first = coordinator(&at);
    let mut second = coordinator(&at);
    let slot = first
        .begin(Work::Connect, None, 1)
        .expect("nothing else holds the scope");

    assert!(matches!(
        second.begin(Work::Connect, None, 1),
        Err(Contended::Busy)
    ));

    slot.finish(Ending::Succeeded).expect("kept");
    second
        .begin(Work::Disconnect, None, 1)
        .expect("the scope is free again")
        .finish(Ending::Succeeded)
        .expect("kept");
}
