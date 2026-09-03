use super::{Ambiguity, NoRestart, Restarts};

#[test]
fn an_extension_that_finished_what_it_was_asked_is_started_again() {
    let mut restarts = Restarts::ceiling(2);

    let restarting = restarts
        .again(Ambiguity::Settled)
        .expect("a clean ending inside the ceiling");

    assert_eq!(restarting.nth(), 1);
    assert_eq!(restarts.spent(), 1);
    assert_eq!(restarts.left(), 1);
}

#[test]
fn each_restart_says_which_one_it_is() {
    let mut restarts = Restarts::ceiling(3);

    let numbered: Vec<u32> = (0..3)
        .map(|_| {
            restarts
                .again(Ambiguity::Settled)
                .expect("inside the ceiling")
                .nth()
        })
        .collect();

    assert_eq!(numbered, vec![1, 2, 3]);
}

#[test]
fn an_extension_that_has_used_its_ceiling_is_not_started_again() {
    let mut restarts = Restarts::ceiling(1);
    restarts.again(Ambiguity::Settled).expect("the only one");

    let refused = restarts
        .again(Ambiguity::Settled)
        .expect_err("the ceiling is reached");

    assert_eq!(refused, NoRestart::Spent { ceiling: 1 });
    assert_eq!(restarts.left(), 0);
}

#[test]
fn a_ceiling_of_none_refuses_the_first_restart() {
    let mut restarts = Restarts::ceiling(0);

    let refused = restarts
        .again(Ambiguity::Settled)
        .expect_err("nothing was allowed");

    assert_eq!(refused, NoRestart::Spent { ceiling: 0 });
    assert_eq!(restarts.spent(), 0);
}

#[test]
fn an_ending_that_left_a_call_outstanding_is_never_started_again() {
    let mut restarts = Restarts::ceiling(5);

    let refused = restarts
        .again(Ambiguity::Unsettled)
        .expect_err("what it had done cannot be known");

    assert_eq!(refused, NoRestart::Unsettled);
}

#[test]
fn an_unsettled_ending_is_refused_for_that_rather_than_for_its_budget() {
    // Both refusals apply at once. The one that is reported is the one that
    // says restarting would be unsafe, not the one that says it is merely out
    // of turns, because only the first stays true if somebody raises the
    // ceiling.
    let mut restarts = Restarts::ceiling(0);

    let refused = restarts
        .again(Ambiguity::Unsettled)
        .expect_err("both reasons hold");

    assert_eq!(refused, NoRestart::Unsettled);
}

#[test]
fn a_refused_restart_spends_nothing() {
    let mut restarts = Restarts::ceiling(2);

    let _ = restarts.again(Ambiguity::Unsettled);

    assert_eq!(restarts.spent(), 0);
    assert_eq!(restarts.left(), 2);
}

#[test]
fn a_permitted_restart_is_spent_whether_or_not_it_starts_anything() {
    // The caller is not asked afterwards how it went. A budget that only
    // counted the starts that worked would let a program which cannot start at
    // all be tried until the run ended.
    let mut restarts = Restarts::ceiling(1);

    let _ = restarts.again(Ambiguity::Settled);

    assert_eq!(restarts.left(), 0);
}

#[test]
fn each_refusal_says_which_of_the_two_it_is() {
    assert!(NoRestart::Unsettled.to_string().contains("cannot be known"),);
    assert!(
        NoRestart::Spent { ceiling: 3 }
            .to_string()
            .contains("all 3 of the restarts"),
    );
}
