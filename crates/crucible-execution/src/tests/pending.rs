//! A decision settles only the action it was composed for.

use crucible_types::TurnId;

use crate::{
    Decision, Detail, Lasting, PENDINGS, Pending, Pendings, SessionRef, Stale, TEXT_BYTES, Text,
    Unraised,
};

fn session(name: &str) -> SessionRef {
    SessionRef::new(name).expect("a short name")
}

fn permission() -> Pending {
    Pending::Permission {
        tool: Text::new("bash"),
        summary: Text::new("ls"),
        detail: Detail::Command {
            sent: Text::new("ls"),
            left_running: false,
        },
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
    assert_eq!(idle.raise(permission()), Err(Unraised::Idle));

    let mut table = running();
    for _ in 0..PENDINGS {
        table.raise(permission()).expect("room");
    }
    assert_eq!(table.raise(permission()), Err(Unraised::Full));
}

/// A question that would show something cut short is not raised, whichever
/// part was cut: a yes to part of a line is not a yes to the line.
#[test]
fn a_question_shown_cut_short_is_not_raised() {
    let long = "x".repeat(TEXT_BYTES + 1);
    let mut table = running();
    let cut = [
        Pending::Permission {
            tool: Text::new("bash"),
            summary: Text::new("run curl"),
            detail: Detail::Command {
                sent: Text::new(&long),
                left_running: false,
            },
        },
        Pending::Permission {
            tool: Text::new("web_search"),
            summary: Text::new("reach search.example"),
            detail: Detail::Network {
                sent: Text::new(&long),
            },
        },
        Pending::Permission {
            tool: Text::new("bash"),
            summary: Text::new(&long),
            detail: Detail::Other,
        },
        Pending::Permission {
            tool: Text::new(&long),
            summary: Text::new("run ls"),
            detail: Detail::Other,
        },
        Pending::Question {
            prompt: Text::new(&long),
        },
    ];
    for pending in cut {
        assert_eq!(
            table.raise(pending.clone()),
            Err(Unraised::Cut),
            "{pending:?}"
        );
    }
    table.raise(permission()).expect("nothing cut was kept");
}
