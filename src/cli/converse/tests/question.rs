//! What a question offers, and what the answer to it leaves behind.
//!
//! The turns either side of a question are the parent module's subject.
//! This is the moment in the middle, where the loop is drawing and waiting
//! at once — and it is enough of a subject to be read on its own.

use super::*;

#[test]
fn a_question_asked_mid_turn_is_answered_from_the_same_input() {
    // The turn blocks on the answer while the loop is drawing its events.
    // Both are on the one channel, so this deadlocks if they are not.
    let written = conversing(
        vec![calling("write"), saying("changed it")],
        tools(Fixed::new("write", changing())),
        "edit it\ny\n",
    );

    assert!(written.contains("wants to change"), "{written}");
    assert!(written.contains("changed it"), "{written}");
}

#[test]
fn refusing_a_tool_ends_the_turn_where_the_user_can_see_why() {
    let written = conversing(
        vec![calling("write")],
        tools(Fixed::new("write", changing())),
        "edit it\nn\n",
    );

    assert!(written.contains("write was not allowed"), "{written}");
}

#[test]
fn a_question_left_unanswered_at_end_of_input_is_refused() {
    // The input ends mid-question. Nothing consented, so nothing runs, and
    // the loop still returns instead of waiting on a pipe that is closed.
    let written = conversing(
        vec![calling("write")],
        tools(Fixed::new("write", changing())),
        "edit it\n",
    );

    assert!(written.contains("was not allowed"), "{written}");
}

#[test]
fn a_window_that_changed_while_a_question_stood_is_news_rather_than_nothing() {
    // The one read in the session with no clock on it: the question stands
    // until somebody decides, which makes it the longest stretch a window can
    // change over without anybody noticing. Passed over with the arrows, the
    // renderer keeps a size the screen no longer has and every frame for the
    // rest of the turn rewinds by that many rows -- over rows it never drew.
    assert!(matches!(heard(Pressed::Resized), Heard::Resized));
}

#[test]
fn a_key_that_answers_nothing_is_not_read_as_an_answer() {
    // An arrow through a list there is none of, a click, a mode step. The
    // question is still standing after each of them.
    for arrived in [
        Pressed::Up,
        Pressed::Down,
        Pressed::Cycle,
        Pressed::Clicked { row: 4, column: 2 },
        Pressed::Ignored,
    ] {
        assert!(matches!(heard(arrived), Heard::Ignored), "{arrived:?}");
    }
}

#[test]
fn every_way_out_of_a_question_leaves_the_tool_unrun() {
    // Escape, Ctrl-C and Ctrl-D are the way out of everything else in a
    // session, so they are the way out of this; return is a line nobody typed.
    // Every one of them is a refusal rather than a key to wait past, or the
    // question would stand with no way to say no to it.
    for arrived in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
        Pressed::Key(Key::Enter),
    ] {
        let Heard::Said(said) = heard(arrived) else {
            panic!("{arrived:?} was not an answer");
        };

        assert_eq!(verdict(Some(&said), true), (Verdict::Deny, Remember::Never));
    }
}

#[test]
fn an_answer_of_always_is_on_the_disk_before_the_next_turn_is_asked_for() {
    // The whole path, end to end: the rule the question offered is the rule
    // that reaches the file, and the file is the one crucible reads at start-up.
    let sample = Sample::new("converse-always");
    let terms = Terms {
        remembering: crucible_config::local(&sample.root()),
        ..plain()
    };

    let written = answering(
        &terms,
        vec![calling("bash"), saying("done")],
        tools(Fixed::new("bash", running("ls"))),
        "go\na\n",
    );

    assert!(written.contains("remembered bash(ls)"), "{written}");
    assert!(
        matches!(
            sample.settles(&asking("bash"), &running("ls")),
            Settled::Approved(_)
        ),
        "the rule is not in {}",
        crucible_config::local(&sample.root()).display()
    );
}

#[test]
fn a_call_no_rule_can_be_written_for_writes_nothing_when_always_is_typed() {
    // The question did not offer `always`, so the word is one the prompt has
    // no answer for. Nothing runs and nothing is written down — the failure
    // that leaves the user to answer again rather than one that quietly
    // widens what a file allows.
    let sample = Sample::new("converse-unwritable");
    let terms = Terms {
        remembering: crucible_config::local(&sample.root()),
        ..plain()
    };

    let written = answering(
        &terms,
        vec![calling("write")],
        tools(Fixed::new("write", changing())),
        "go\na\n",
    );

    assert!(written.contains("was not allowed"), "{written}");
    assert!(
        !crucible_config::local(&sample.root()).exists(),
        "a file was written for a call no rule can be minted from"
    );
}

#[test]
fn yes_allows_this_call_only() {
    assert_eq!(
        verdict(Some("y\n"), true),
        (Verdict::Allow, Remember::Never)
    );
    assert_eq!(
        verdict(Some("yes"), true),
        (Verdict::Allow, Remember::Never)
    );
}

#[test]
fn session_allows_calls_like_it_until_crucible_exits() {
    assert_eq!(
        verdict(Some("s\n"), true),
        (Verdict::Allow, Remember::Session)
    );
    assert_eq!(
        verdict(Some("session"), true),
        (Verdict::Allow, Remember::Session)
    );
}

#[test]
fn always_allows_calls_like_it_from_now_on() {
    // The answer that costs a file. It is a different word from `session`
    // because it is a different promise, and one of the two outlives the
    // process that made it.
    assert_eq!(
        verdict(Some("a\n"), true),
        (Verdict::Allow, Remember::Always)
    );
    assert_eq!(
        verdict(Some("always"), true),
        (Verdict::Allow, Remember::Always)
    );
}

#[test]
fn always_is_not_an_answer_where_no_rule_can_be_written() {
    // The question did not offer it, so it is a word the user typed at a
    // prompt that has no such answer — and the failure worth having is the
    // one where nothing runs.
    assert_eq!(
        verdict(Some("a\n"), false),
        (Verdict::Deny, Remember::Never)
    );
    assert_eq!(
        verdict(Some("always"), false),
        (Verdict::Deny, Remember::Never)
    );
}

#[test]
fn anything_else_is_a_refusal_that_is_remembered_about_nothing() {
    // Including the empty line, which is what someone types when they meant
    // to read the question first. A refusal covers the call it refused and no
    // other, so there is nothing for a duration to hold.
    for answer in ["n", "no", "", "\n", "yeah", "Y E S", "1"] {
        assert_eq!(
            verdict(Some(answer), true),
            (Verdict::Deny, Remember::Never),
            "{answer:?}"
        );
    }
}

#[test]
fn end_of_input_is_a_refusal() {
    // A pipe that closed mid-question cannot consent to anything.
    assert_eq!(verdict(None, true), (Verdict::Deny, Remember::Never));
}
