//! What a run of calls that only looked around says it did.

use super::Gathering;
use crucible_core::{Looking, ToolId, ToolOutput};

/// A run holding one of each, in the order the counters are said.
fn gathering(looking: &[Looking]) -> Gathering {
    let mut gathering = Gathering::default();
    for (at, one) in looking.iter().enumerate() {
        gathering.took(ToolId::new(format!("call-{at}")), *one, String::new());
    }
    gathering
}

#[test]
fn a_run_says_what_it_is_doing_while_it_is_going() {
    let gathering = gathering(&[Looking::Pattern, Looking::File, Looking::File]);

    assert_eq!(
        gathering.doing(),
        "Searching for 1 pattern, reading 2 files"
    );
}

#[test]
fn a_run_says_what_it_did_once_it_has_settled() {
    let gathering = gathering(&[Looking::Pattern, Looking::File, Looking::File]);

    assert_eq!(gathering.did(), "Searched for 1 pattern, read 2 files");
}

/// The order is the counters' own and never the order the calls arrived in, so
/// two runs holding the same work read the same way.
#[test]
fn the_counters_are_said_in_one_order_whatever_order_the_calls_came_in() {
    let backwards = gathering(&[
        Looking::Command,
        Looking::Directory,
        Looking::File,
        Looking::Pattern,
    ]);

    assert_eq!(
        backwards.did(),
        "Searched for 1 pattern, read 1 file, listed 1 directory, ran 1 command"
    );
}

/// A counter nothing was counted against is not said at all: a run that read
/// four files did not search for nothing, and a line saying so would be four
/// words of noise on every row.
#[test]
fn a_counter_with_nothing_against_it_is_not_said() {
    let gathering = gathering(&[Looking::File, Looking::File, Looking::File, Looking::File]);

    assert_eq!(gathering.doing(), "Reading 4 files");
    assert_eq!(gathering.did(), "Read 4 files");
}

/// Only the first, because the rest are the middle of a sentence.
#[test]
fn the_first_counter_opens_the_line_and_the_rest_do_not() {
    let gathering = gathering(&[Looking::Directory, Looking::Command]);

    assert_eq!(gathering.doing(), "Listing 1 directory, running 1 command");
    assert_eq!(gathering.did(), "Listed 1 directory, ran 1 command");
}

/// The plural is the counter's own, and `directory` is why it has to be.
#[test]
fn each_counter_carries_its_own_plural() {
    let one = gathering(&[Looking::Directory]);
    let two = gathering(&[Looking::Directory, Looking::Directory]);

    assert_eq!(one.did(), "Listed 1 directory");
    assert_eq!(two.did(), "Listed 2 directories");
}

#[test]
fn a_run_counts_the_calls_it_holds() {
    assert_eq!(gathering(&[]).len(), 0);
    assert_eq!(gathering(&[Looking::File]).len(), 1);
    assert_eq!(gathering(&[Looking::File, Looking::Pattern]).len(), 2);
}

/// Nothing to say where nothing was gathered, rather than a bare capital or a
/// stray comma.
#[test]
fn a_run_that_gathered_nothing_says_nothing() {
    assert_eq!(gathering(&[]).doing(), "");
    assert_eq!(gathering(&[]).did(), "");
}

/// The first call of a run waits to see whether a second one arrives, because a
/// run of one is not folded and its row is the row it always was.
#[test]
fn the_first_call_of_a_run_is_held_back_until_a_second_one_arrives() {
    let mut gathering = Gathering::default();

    assert!(
        gathering
            .took(ToolId::new("a"), Looking::File, "Read(one)".to_owned())
            .is_none()
    );

    let alone = gathering
        .took(ToolId::new("b"), Looking::File, "Read(two)".to_owned())
        .expect("the first call, let go now the run has a second");

    assert_eq!(alone.call, ToolId::new("a"));
    assert_eq!(alone.said, "Read(one)");

    // And once only: the run is settled, so nothing after it is held back.
    assert!(
        gathering
            .took(ToolId::new("c"), Looking::File, "Read(three)".to_owned())
            .is_none()
    );
}

/// What the held call came back with waits with it, and nothing else does.
#[test]
fn only_the_held_call_keeps_its_result_here() {
    let mut gathering = Gathering::default();
    gathering.took(ToolId::new("a"), Looking::File, "Read(one)".to_owned());

    assert!(
        gathering
            .answered(&ToolId::new("a"), ToolOutput::ok("one"))
            .is_none()
    );
    assert!(
        gathering
            .answered(&ToolId::new("b"), ToolOutput::ok("two"))
            .is_some_and(|given| given.text() == "two")
    );

    let alone = gathering.alone().expect("the call still held");
    assert!(alone.output.is_some_and(|output| output.text() == "one"));
}

/// A run knows which calls it counted, because the event that draws a result
/// arrives after the one that folded it.
#[test]
fn a_run_knows_which_calls_it_counted() {
    let mut gathering = Gathering::default();
    gathering.took(ToolId::new("a"), Looking::File, String::new());

    assert!(gathering.holds(&ToolId::new("a")));
    assert!(!gathering.holds(&ToolId::new("b")));
}

/// Taking a run empties it, so the next call opens a new one.
#[test]
fn taking_a_run_leaves_nothing_behind() {
    let mut gathering = Gathering::default();
    gathering.took(ToolId::new("a"), Looking::File, String::new());
    gathering.took(ToolId::new("b"), Looking::Pattern, String::new());

    let taken = gathering.taken();

    assert_eq!(taken.len(), 2);
    assert_eq!(gathering.len(), 0);
    assert_eq!(gathering.did(), "");
    assert!(!gathering.holds(&ToolId::new("a")));
}

/// The calls come back in the order they were made, which is the order a reader
/// opening the run reads them in.
#[test]
fn a_run_hands_back_its_calls_in_the_order_they_were_made() {
    let mut gathering = Gathering::default();
    gathering.took(ToolId::new("a"), Looking::File, String::new());
    gathering.took(ToolId::new("b"), Looking::Pattern, String::new());

    assert_eq!(gathering.calls(), [ToolId::new("a"), ToolId::new("b")]);
}

/// One call is not a run worth folding; two are.
#[test]
fn a_run_folds_once_it_holds_two() {
    let mut gathering = Gathering::default();
    assert!(!gathering.folds());

    gathering.took(ToolId::new("a"), Looking::File, String::new());
    assert!(!gathering.folds());

    gathering.took(ToolId::new("b"), Looking::File, String::new());
    assert!(gathering.folds());
}
