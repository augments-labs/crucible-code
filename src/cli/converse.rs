//! The loop: read a line, take a turn, draw what the turn does.
//!
//! The turn runs on its own thread and the terminal stays with this one. That
//! split is the whole reason a turn can stream while a question is waiting to
//! be answered, and it is why no lock appears anywhere on the render path: the
//! only thread that writes to the terminal is the one running this loop.
//!
//! Standard input is left in cooked mode for 0.0.1. The consequence worth
//! knowing: Ctrl-C during a turn ends the process, because catching a signal
//! would need `unsafe`, which this workspace forbids. The session log is
//! append-only and written as the turn goes, so `--continue` picks the
//! conversation up from wherever it stopped.

use std::io::BufRead;
use std::sync::mpsc::channel;
use std::thread;

use crucible_core::{Cancel, Event, Post as _, Verdict};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Terminal};

use super::Fatal;
use super::draw;
use super::seen::{Asking, Relay, Seen};

/// What the user types after.
const MARK: &str = "› ";

/// Reads prompts and takes turns until input ends.
///
/// `input` is standard input in a real run. It is a parameter so that a test
/// can drive the loop: the deadlock this file has to avoid is one that only
/// shows up when a whole turn runs, and a hardwired stdin makes that unrunnable.
pub(crate) fn converse<T: Terminal>(
    mut runner: Runner,
    renderer: &mut Renderer<T>,
    cancel: &Cancel,
    input: &mut dyn BufRead,
) -> Result<(), Fatal> {
    loop {
        draw::mark(renderer, MARK)?;

        let Some(prompt) = read(input)? else { break };
        if prompt.trim().is_empty() {
            continue;
        }

        runner = take(runner, renderer, cancel, input, prompt)?;
    }

    renderer.settle()?;
    Ok(())
}

/// One turn, start to finish.
///
/// The runner goes to the worker and comes back, which is what makes the
/// transcript and the permission memory survive a turn without being shared
/// between threads.
fn take<T: Terminal>(
    runner: Runner,
    renderer: &mut Renderer<T>,
    cancel: &Cancel,
    input: &mut dyn BufRead,
    prompt: String,
) -> Result<Runner, Fatal> {
    // Both channels are made fresh for this turn. A reply channel that outlived
    // its turn could hand the next question an answer meant for the last one.
    let (post, seen) = channel();
    let (reply, hear) = channel();

    let mut asking = Asking::new(post.clone(), hear);
    let relay = Relay::new(post);
    let running = cancel.clone();

    let working = thread::spawn(move || {
        let mut runner = runner;

        // The runner reports what happened and returns why it stopped; nothing
        // else has posted the failure, so this is where it becomes visible.
        if let Err(problem) = runner.turn(prompt.trim(), &mut asking, &relay, &running) {
            relay.post(Event::Failed { error: problem });
        }

        runner
    });

    // Ends when the worker drops both senders, which happens when the turn is
    // over. No sentinel event, and no way to leave the loop early and miss the
    // last delta.
    for one in seen {
        match one {
            Seen::Turn(event) => draw::event(renderer, event)?,
            Seen::Question { call, sensitivity } => {
                draw::question(renderer, &call, &sensitivity)?;
                // A worker that stopped waiting has already denied itself.
                let _ = reply.send(verdict(read(input)?.as_deref()));
            }
        }
    }

    working.join().map_err(|_| Fatal::Lost)
}

/// Reads one line, or `None` at end of input.
fn read(input: &mut dyn BufRead) -> Result<Option<String>, Fatal> {
    let mut line = String::new();

    match input.read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(line)),
        Err(problem) => Err(Fatal::Input(problem)),
    }
}

/// What an answer to a permission question means.
///
/// Anything unrecognised is a refusal, and so is end of input. The two ways to
/// say yes are both explicit; everything else, including a typo and a closed
/// pipe, leaves the tool unrun.
fn verdict(answer: Option<&str>) -> Verdict {
    match answer.map(str::trim) {
        Some("y" | "Y" | "yes") => Verdict::AllowOnce,
        Some("a" | "A" | "always") => Verdict::AllowSession,
        _ => Verdict::Deny,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crucible_core::{Delta, Sensitivity, StopReason, ToolId};
    use crucible_runner::{Model, Session, Tools};
    use crucible_tui::Recording;

    use super::*;
    use crate::cli::fake::{Fixed, Script};

    /// Runs the whole loop over a scripted provider and typed-ahead input.
    ///
    /// Returns what the terminal ended up with. A test that hangs here has
    /// found the deadlock this file exists to avoid, so every one of them is
    /// also a liveness check.
    fn conversing(rounds: Vec<Vec<Delta>>, offered: Tools, typed: &str) -> String {
        over(Script::new(rounds), offered, typed).0
    }

    /// The whole loop over one script: what the terminal ended up with, and how
    /// many requests the script was given.
    fn over(script: Script, offered: Tools, typed: &str) -> (String, usize) {
        let asked = script.asked();
        let runner = Runner::new(
            Box::new(script),
            offered,
            Model {
                name: "script".into(),
                max_tokens: 64,
                system: None,
            },
            Session::nowhere(),
        );

        let mut renderer = Renderer::new(Recording::new(80, 24)).expect("a recording terminal");
        let mut input = Cursor::new(typed.as_bytes().to_vec());

        converse(runner, &mut renderer, &Cancel::new(), &mut input).expect("the loop to finish");

        (
            renderer.terminal().written().to_string(),
            asked.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn tools(tool: Fixed) -> Tools {
        let mut offered = Tools::new();
        offered.add(Box::new(tool));
        offered
    }

    fn saying(text: &str) -> Vec<Delta> {
        vec![
            Delta::Text(text.into()),
            Delta::Stopped(StopReason::Yielded),
        ]
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
    fn a_turn_streams_what_the_model_said_and_the_loop_comes_back_for_more() {
        // The drain ends when the worker drops its senders. If it did not, this
        // test would hang instead of failing, which is the point of running the
        // real loop rather than asserting on a mock.
        let written = conversing(vec![saying("hello")], Tools::new(), "hi\n");

        assert!(written.contains("hello"), "{written}");
    }

    #[test]
    fn two_prompts_are_two_turns() {
        // The runner has to survive being handed to a thread and back, or the
        // second turn has no transcript to continue from.
        let written = conversing(
            vec![saying("first"), saying("second")],
            Tools::new(),
            "one\ntwo\n",
        );

        assert!(written.contains("first"), "{written}");
        assert!(written.contains("second"), "{written}");
    }

    #[test]
    fn a_blank_line_is_not_a_turn() {
        // Otherwise the return key alone sends an empty prompt and costs a
        // request. Counted at the provider rather than in what was drawn: the
        // renderer writes a line once live and again on its way to scrollback,
        // so counting appearances would count frames.
        let (written, asked) = over(
            Script::new(vec![saying("answered")]),
            Tools::new(),
            "\n   \nreal\n",
        );

        assert_eq!(asked, 1, "{written}");
        assert!(written.contains("answered"), "{written}");
    }

    #[test]
    fn a_question_asked_mid_turn_is_answered_from_the_same_input() {
        // The turn blocks on the answer while the loop is drawing its events.
        // Both are on the one channel, so this deadlocks if they are not.
        let written = conversing(
            vec![calling("write"), saying("changed it")],
            tools(Fixed::new("write", Sensitivity::MutatesFile)),
            "edit it\ny\n",
        );

        assert!(written.contains("wants to change a file"), "{written}");
        assert!(written.contains("changed it"), "{written}");
    }

    #[test]
    fn refusing_a_tool_ends_the_turn_where_the_user_can_see_why() {
        let written = conversing(
            vec![calling("write")],
            tools(Fixed::new("write", Sensitivity::MutatesFile)),
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
            tools(Fixed::new("write", Sensitivity::MutatesFile)),
            "edit it\n",
        );

        assert!(written.contains("was not allowed"), "{written}");
    }

    #[test]
    fn a_provider_that_fails_says_so_instead_of_ending_the_session() {
        // Nothing else posts the turn's own failure, so if the wiring drops it
        // the user gets a prompt back with no explanation and retypes the
        // thing that just failed.
        let (written, asked) = over(Script::refusing(), Tools::new(), "go\nagain\n");

        assert!(written.contains("HTTP 401"), "{written}");
        assert_eq!(asked, 2, "a failed turn does not end the session");
    }

    #[test]
    fn yes_allows_this_call_only() {
        assert_eq!(verdict(Some("y\n")), Verdict::AllowOnce);
        assert_eq!(verdict(Some("yes")), Verdict::AllowOnce);
    }

    #[test]
    fn always_allows_calls_like_it_for_the_rest_of_the_session() {
        assert_eq!(verdict(Some("a\n")), Verdict::AllowSession);
        assert_eq!(verdict(Some("always")), Verdict::AllowSession);
    }

    #[test]
    fn anything_else_is_a_refusal() {
        // Including the empty line, which is what someone types when they meant
        // to read the question first.
        for answer in ["n", "no", "", "\n", "yeah", "Y E S", "1"] {
            assert_eq!(verdict(Some(answer)), Verdict::Deny, "{answer:?}");
        }
    }

    #[test]
    fn end_of_input_is_a_refusal() {
        // A pipe that closed mid-question cannot consent to anything.
        assert_eq!(verdict(None), Verdict::Deny);
    }
}
