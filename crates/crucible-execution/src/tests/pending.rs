//! A decision settles only the action it was composed for.

use crucible_types::TurnId;

use crate::{Decision, Lasting, PENDINGS, Pending, Pendings, SessionRef, Stale, Text};

fn session(name: &str) -> SessionRef {
    SessionRef::new(name).expect("a short name")
}

fn permission() -> Pending {
    Pending::Permission {
        tool: Text::new("bash"),
        summary: Text::new("ls"),
    }
}

fn running() -> Pendings {
    let mut table = Pendings::new().expect("a fresh table");
    table.begin(session("one"), TurnId::FIRST);
    table
}

/// An answer composed for a program that has since been started again is
/// refused as stale, even though the new program has a request open under the
/// same number.
#[test]
fn a_decision_carrying_a_stale_generation_is_refused() {
    let mut table = running();
    let old = table.raise(permission()).expect("room for one");
    table.relaunch().expect("a fresh generation");
    table.begin(session("one"), TurnId::FIRST);
    let new = table.raise(permission()).expect("room for one");
    assert_eq!(
        old.request(),
        new.request(),
        "the restart reused the number"
    );

    assert_eq!(
        table.settle(&old, &Decision::Allow(Lasting::Once)),
        Err(Stale::Generation)
    );
    assert_eq!(
        table.settle(&new, &Decision::Allow(Lasting::Once)),
        Ok(permission()),
        "the live request is still open and settles"
    );
}

/// A decision for a turn that has ended is refused, and says so.
#[test]
fn a_decision_for_an_ended_turn_is_refused() {
    let mut table = running();
    let asked = table.raise(permission()).expect("room for one");
    table.end();
    table.begin(session("one"), TurnId::FIRST.next());

    assert_eq!(table.settle(&asked, &Decision::Deny), Err(Stale::Turn));
}

/// A second decision for one already settled is refused.
#[test]
fn a_duplicate_decision_is_refused() {
    let mut table = running();
    let asked = table.raise(permission()).expect("room for one");

    assert_eq!(table.settle(&asked, &Decision::Deny), Ok(permission()));
    assert_eq!(table.settle(&asked, &Decision::Deny), Err(Stale::Settled));
}

/// An answer to a permission, or leave given to a question, is refused and
/// leaves the action open.
#[test]
fn a_decision_of_the_wrong_kind_is_refused_and_leaves_it_open() {
    let mut table = running();
    let asked = table.raise(permission()).expect("room for one");

    assert_eq!(
        table.settle(&asked, &Decision::Answer(Text::new("yes"))),
        Err(Stale::Kind)
    );
    assert_eq!(table.settle(&asked, &Decision::Deny), Ok(permission()));

    let question = Pending::Question {
        prompt: Text::new("which?"),
    };
    let asked = table.raise(question.clone()).expect("room for one");
    assert_eq!(
        table.settle(&asked, &Decision::Allow(Lasting::Session)),
        Err(Stale::Kind)
    );
    assert_eq!(
        table.settle(&asked, &Decision::Answer(Text::new("that"))),
        Ok(question)
    );
}

/// A decision composed for another session, or another instance, is refused.
#[test]
fn a_decision_from_elsewhere_is_refused() {
    let mut table = running();
    let mut other = Pendings::new().expect("a fresh table");
    other.begin(session("one"), TurnId::FIRST);
    let theirs = other.raise(permission()).expect("room for one");
    assert_eq!(table.settle(&theirs, &Decision::Deny), Err(Stale::Instance));

    let mut elsewhere = running();
    let asked = elsewhere.raise(permission()).expect("room for one");
    elsewhere.end();
    elsewhere.begin(session("two"), TurnId::FIRST);
    assert_eq!(
        elsewhere.settle(&asked, &Decision::Deny),
        Err(Stale::Session)
    );
}

/// The table holds at most its ceiling, and raises nothing outside a turn.
#[test]
fn the_table_is_bounded_and_needs_a_turn() {
    let mut idle = Pendings::new().expect("a fresh table");
    assert!(idle.raise(permission()).is_err(), "no turn is running");

    let mut table = running();
    for _ in 0..PENDINGS {
        table.raise(permission()).expect("room");
    }
    assert!(table.raise(permission()).is_err(), "the table is full");
}
