//! When a signal is noted, and when it is left to do what it always did.

use super::{Ending, Told};
use crate::cli::Fatal;

#[test]
fn a_signal_sent_while_a_turn_runs_is_noted_and_the_process_is_still_here_to_read_it() {
    // The one test in this binary that installs the handlers, and it sends the
    // one signal: a second would be obeyed, which is the design, and would take
    // every other test in the process with it.
    let ending = Ending::listening(true);
    assert_eq!(ending.told(), None);

    #[cfg(unix)]
    {
        assert!(ending.listening, "the handlers were not installed");

        let turn = ending.turn();
        // Raised on this thread, and the handler has run by the time this
        // returns — so there is nothing to wait for and no clock in the test.
        signal_hook::low_level::raise(signal_hook::consts::SIGHUP).unwrap();

        assert_eq!(ending.told(), Some(Told(signal_hook::consts::SIGHUP)));
        drop(turn);
    }

    // Nothing is heard here, so nothing may be held back either: a stretch
    // that flipped the flag with no handler behind it would be harmless, and
    // one that claimed to be listening would not be.
    #[cfg(not(unix))]
    assert!(!ending.listening);
}

#[test]
fn a_wait_with_no_clock_is_not_begun_over_a_signal_already_noted() {
    let ending = Ending::deaf();
    let _turn = ending.turn();
    ending.tell(15);

    // Nothing reads a note during a wait on the keyboard, so beginning one
    // here would be a `kill` that waited for a keypress.
    assert!(matches!(
        ending.unclocked(),
        Err(Fatal::Ended(told)) if told == Told(15)
    ));
}

#[test]
fn a_wait_with_no_clock_begins_where_nothing_has_been_noted() {
    let ending = Ending::deaf();
    let _turn = ending.turn();

    assert!(ending.unclocked().is_ok());
}

#[test]
fn a_signal_noted_after_the_last_pass_ends_the_turn_once_it_is_written_down() {
    let ending = Ending::deaf();
    let turn = ending.turn();
    ending.tell(15);

    // Nothing had failed, so nothing had asked for the session to be finished
    // yet: the note is what asks, and it is asked before the note is handed on.
    let finished = std::cell::Cell::new(0);
    let ended = turn.over(Ok(()), || finished.set(finished.get() + 1));

    assert!(matches!(ended, Err(Fatal::Ended(told)) if told == Told(15)));
    assert_eq!(finished.get(), 1);
}

/// One that treats stretches as though handlers were behind it, with none
/// installed: what a stretch does to the flag can then be read off with no
/// signal sent and no clock, on every platform.
fn heard() -> Ending {
    Ending {
        listening: true,
        ..Ending::deaf()
    }
}

/// Whether a signal landing now would be obeyed where it lands.
fn at_once(ending: &Ending) -> bool {
    ending.at_once.load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn a_signal_is_noted_again_once_a_wait_with_no_clock_inside_a_turn_is_over() {
    let ending = heard();
    assert!(at_once(&ending), "between turns nothing reads a note");

    let turn = ending.turn();
    assert!(!at_once(&ending), "a turn notes a signal for its loop");

    let waiting = ending.unclocked().expect("nothing has been noted");
    assert!(at_once(&ending), "nothing reads a note at a wait on a key");

    // The turn goes on after the question is answered, with the answer still
    // held by the worker: a signal obeyed here would take it.
    drop(waiting);
    assert!(!at_once(&ending), "the rest of the turn lost its hearing");

    drop(turn);
    assert!(at_once(&ending), "the prompt after the turn reads no notes");
}

#[test]
fn once_one_signal_has_been_read_the_next_is_obeyed_where_it_lands() {
    let ending = heard();
    let turn = ending.turn();

    ending.tell(15);
    assert!(!at_once(&ending), "noting a signal is not reading it");
    assert_eq!(ending.told(), Some(Told(15)));
    assert!(
        at_once(&ending),
        "a turn that will not stop held the process"
    );

    // And it stays spent, through the stretch that was open when it was read.
    drop(turn);
    assert!(at_once(&ending));
}

#[test]
fn a_stretch_that_ends_over_a_note_nobody_read_does_not_go_back_to_noting() {
    let ending = heard();
    let turn = ending.turn();
    let waiting = ending.unclocked().expect("nothing has been noted");

    // Noted, and not yet read by anybody: what the stretch was before it began
    // is a turn that notes, and going back to that would leave the next signal
    // noted for a loop that may never look.
    ending.tell(1);
    drop(waiting);
    assert!(at_once(&ending));

    drop(turn);
    assert!(at_once(&ending));
}

#[test]
fn a_turn_that_failed_is_written_down_with_a_signal_obeyed_where_it_lands() {
    let ending = heard();
    let turn = ending.turn();

    // The wait for the log has no clock and nothing reads a note during it, so
    // a log that stopped answering would hold the process against every `kill`
    // if signals were still only being noted.
    let obeyed = std::cell::Cell::new(None);
    let failed = Err(Fatal::InputTooLong);
    let ended = turn.over(failed, || obeyed.set(Some(at_once(&ending))));

    assert!(obeyed.get().is_some(), "the failed turn was never finished");
    assert_eq!(obeyed.get(), Some(true), "the wait for the log was deaf");
    assert!(matches!(ended, Err(Fatal::InputTooLong)), "{ended:?}");
}

#[test]
fn a_signal_noted_beside_a_failure_is_how_the_turn_is_said_to_have_ended() {
    let ending = heard();
    let turn = ending.turn();
    ending.tell(1);

    // The loop lets a noted signal outrank a failure it already holds, and the
    // last look at the note answers the same way: a window that closed fails
    // the write and hangs up, and it is the hang-up the process should be seen
    // to have ended by.
    let finished = std::cell::Cell::new(0);
    let ended = turn.over(Err(Fatal::InputTooLong), || {
        finished.set(finished.get() + 1);
    });

    assert!(matches!(ended, Err(Fatal::Ended(told)) if told == Told(1)));
    assert!(finished.get() >= 1);
}
