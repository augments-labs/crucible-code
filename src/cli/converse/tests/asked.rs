//! What a question offers, and what the answer to it leaves behind.
//!
//! The turns either side of a question are the parent module's subject. This is
//! the moment in the middle, where the loop is drawing and waiting at once.

use crucible_core::{Settled, ToolArgs, ToolCall, ToolId};

use super::*;
use crate::cli::fake::{Fixed, changing, running};
use crate::cli::sample::Sample;

/// The whole loop under terms of the test's own: what an answer leaves behind
/// depends on where those terms point.
fn answering(terms: &Terms, rounds: Vec<Vec<Delta>>, offered: Tools, typed: &str) -> String {
    let runner = scripted(Script::new(rounds), offered);

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, terms, &mut input).expect("the loop to finish");

    renderer.terminal().written().to_string()
}

fn tools(tool: Fixed) -> Tools {
    let mut offered = Tools::new();
    offered.add(Box::new(tool));
    offered
}

/// The call the script below made, as the engine will be asked about it.
fn asking(name: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new("a"),
        name: name.into(),
        args: ToolArgs::new("{}"),
    }
}

fn calling(name: &str) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: name.into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

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
