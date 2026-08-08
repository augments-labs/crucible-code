//! Running what the model asked for.
//!
//! One rule shapes this file: **every call the transcript records has a result
//! recorded with it.** A provider refuses a conversation containing a request
//! with no answer, so a turn that stops half way through a round — because the
//! user cancelled, or said no — still writes a result for each remaining call
//! saying why there is nothing in it.

use std::sync::mpsc::Sender;

use crucible_core::{
    Ask, Cancel, Event, Permission, StopReason, ToolCall, ToolError, ToolOutput, ToolResult,
};

use crate::post;
use crate::tools::Tools;

/// What a call is answered with when the turn ended before it could run.
const NOT_RUN: &str = "not run: the turn ended first";

/// What a call is answered with when the user said no.
const DENIED: &str = "the user did not allow this";

/// What one round of calls decided about the turn.
pub(crate) enum Went {
    /// Every call ran. Ask the model again.
    On,
    /// The user cancelled part way through.
    Stopped(StopReason),
    /// The user refused this tool.
    Refused(Box<str>),
}

/// What one call produced.
enum Ran {
    /// Something to send back, whether or not the tool succeeded.
    Output(ToolOutput),
    /// The user cancelled.
    Stopped(StopReason),
    /// The user said no.
    Refused,
}

/// Everything a round of calls needs, gathered so the loop reads as one thing.
pub(crate) struct Work<'a> {
    /// What may be called.
    pub(crate) tools: &'a Tools,
    /// The session's memory of what was already allowed.
    pub(crate) permission: &'a mut Permission,
    /// How to put a call to the user.
    pub(crate) ask: &'a mut dyn Ask,
    /// Where progress is reported.
    pub(crate) events: &'a Sender<Event>,
    /// Whether the user has asked everything to stop.
    pub(crate) cancel: &'a Cancel,
}

impl Work<'_> {
    /// Runs `calls` in order, and answers every one of them.
    pub(crate) fn round(&mut self, calls: &[ToolCall]) -> (Vec<ToolResult>, Went) {
        let mut results = Vec::with_capacity(calls.len());
        let mut went = Went::On;

        for call in calls {
            let output = match went {
                Went::On => match self.one(call) {
                    Ran::Output(output) => output,
                    Ran::Stopped(stop) => {
                        went = Went::Stopped(stop);
                        ToolOutput::failed(NOT_RUN)
                    }
                    Ran::Refused => {
                        went = Went::Refused(call.name.clone());
                        ToolOutput::failed(DENIED)
                    }
                },
                // The turn is already over. The call is still answered, so the
                // transcript stays one a provider will accept.
                Went::Stopped(_) | Went::Refused(_) => ToolOutput::failed(NOT_RUN),
            };

            // Cloned because both halves need it: the renderer shows what the
            // tool produced, and the transcript sends it to the model. It is
            // one tool's output, so it does not grow with the transcript.
            post(
                self.events,
                Event::ToolFinished {
                    call: call.id.clone(),
                    output: output.clone(),
                },
            );

            results.push(ToolResult {
                id: call.id.clone(),
                output,
            });
        }

        (results, went)
    }

    /// Runs one call, if it is allowed to run.
    fn one(&mut self, call: &ToolCall) -> Ran {
        if self.cancel.requested() {
            return Ran::Stopped(StopReason::Cancelled);
        }

        let Some(tool) = self.tools.find(&call.name) else {
            // A name the model invented is something the model can correct, so
            // it goes back as a result rather than ending the turn.
            return Ran::Output(failure(&ToolError::Unknown(call.name.clone())));
        };

        let sensitivity = tool.sensitivity(&call.args);
        let Some(grant) = self.permission.decide(call, &sensitivity, self.ask) else {
            return Ran::Refused;
        };

        match tool.run(call.args.clone(), grant) {
            Ok(output) => Ran::Output(output),
            // Cancelling is not a result the model should reason about. The
            // user stopped the turn, so the turn stops.
            Err(ToolError::Cancelled(_)) => Ran::Stopped(StopReason::Cancelled),
            Err(problem) => Ran::Output(failure(&problem)),
        }
    }
}

/// A failure the model is meant to read and work around.
fn failure(problem: &ToolError) -> ToolOutput {
    ToolOutput::failed(problem.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};

    use crucible_core::{Sensitivity, ToolArgs, ToolId, Verdict};

    use super::*;
    use crate::fake::{Fixed, Says};

    /// One round, with everything it needed set up around it.
    struct Round {
        tools: Tools,
        permission: Permission,
        says: Says,
        cancel: Cancel,
        events: Sender<Event>,
        seen: Receiver<Event>,
    }

    impl Round {
        fn new(verdict: Verdict) -> Self {
            let (events, seen) = channel();

            Self {
                tools: Tools::new(),
                permission: Permission::new(),
                says: Says::new(verdict),
                cancel: Cancel::new(),
                events,
                seen,
            }
        }

        fn offering(mut self, tool: Fixed) -> Self {
            self.tools.add(Box::new(tool));
            self
        }

        fn round(&mut self, calls: &[ToolCall]) -> (Vec<ToolResult>, Went) {
            Work {
                tools: &self.tools,
                permission: &mut self.permission,
                ask: &mut self.says,
                events: &self.events,
                cancel: &self.cancel,
            }
            .round(calls)
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new("{}"),
        }
    }

    fn texts(results: &[ToolResult]) -> Vec<&str> {
        results.iter().map(|result| result.output.text()).collect()
    }

    #[test]
    fn a_call_that_runs_comes_back_with_what_the_tool_produced() {
        let mut round =
            Round::new(Verdict::AllowOnce).offering(Fixed::new("read").answering("fn main() {}"));

        let (results, went) = round.round(&[call("a", "read")]);

        assert_eq!(texts(&results), ["fn main() {}"]);
        assert!(matches!(went, Went::On), "the turn should carry on");
    }

    #[test]
    fn a_name_no_tool_answers_to_is_reported_to_the_model_rather_than_ending_the_turn() {
        // The model invented it, so the model is the one that can fix it.
        let mut round = Round::new(Verdict::AllowOnce).offering(Fixed::new("read"));

        let (results, went) = round.round(&[call("a", "frobnicate")]);

        assert_eq!(texts(&results), ["no tool named frobnicate"]);
        assert!(results.first().is_some_and(|r| r.output.is_failed()));
        assert!(matches!(went, Went::On));
    }

    #[test]
    fn a_tool_that_fails_reports_it_to_the_model_rather_than_ending_the_turn() {
        let mut round =
            Round::new(Verdict::AllowOnce).offering(Fixed::new("read").breaking("unreadable"));

        let (results, went) = round.round(&[call("a", "read")]);

        assert_eq!(texts(&results), ["read: unreadable"]);
        assert!(matches!(went, Went::On));
    }

    #[test]
    fn a_denied_call_ends_the_turn_and_says_so_in_its_result() {
        let mut round = Round::new(Verdict::Deny)
            .offering(Fixed::new("write").risking(Sensitivity::MutatesFile));

        let (results, went) = round.round(&[call("a", "write")]);

        assert_eq!(texts(&results), [DENIED]);
        assert!(
            matches!(went, Went::Refused(ref name) if &**name == "write"),
            "the turn should name the tool that was refused"
        );
    }

    #[test]
    fn every_call_is_answered_even_after_the_turn_is_over() {
        // A call with no result is a transcript the provider refuses, so the
        // ones that never ran are answered too.
        let mut round = Round::new(Verdict::Deny)
            .offering(Fixed::new("write").risking(Sensitivity::MutatesFile));

        let (results, _) = round.round(&[call("a", "write"), call("b", "write")]);

        assert_eq!(results.len(), 2);
        assert_eq!(texts(&results), [DENIED, NOT_RUN]);
    }

    #[test]
    fn a_call_after_a_denial_is_never_put_to_the_user() {
        let mut round = Round::new(Verdict::Deny)
            .offering(Fixed::new("write").risking(Sensitivity::MutatesFile));

        round.round(&[call("a", "write"), call("b", "write")]);

        assert_eq!(round.says.asked, 1, "the user was asked about a dead turn");
    }

    #[test]
    fn a_cancelled_round_stops_the_turn_without_running_anything_more() {
        let mut round = Round::new(Verdict::AllowOnce).offering(Fixed::new("read"));
        round.cancel.request();

        let (results, went) = round.round(&[call("a", "read")]);

        assert_eq!(texts(&results), [NOT_RUN]);
        assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
    }

    #[test]
    fn a_tool_that_noticed_the_cancellation_itself_stops_the_turn() {
        // A long-running tool checks the flag mid-work and returns. That is not
        // a failure to report to the model — the user stopped the turn.
        let mut round = Round::new(Verdict::AllowOnce).offering(Fixed::new("bash").cancelling());

        let (_, went) = round.round(&[call("a", "bash")]);

        assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
    }

    #[test]
    fn every_call_reports_that_it_finished() {
        // The renderer redraws the line it drew when the call was requested, so
        // a call with no finish stays on screen as if it were still running.
        let mut round = Round::new(Verdict::AllowOnce).offering(Fixed::new("read"));

        round.round(&[call("a", "read"), call("b", "read")]);

        let finished: Vec<String> = round
            .seen
            .try_iter()
            .filter_map(|event| match event {
                Event::ToolFinished { call, .. } => Some(call.to_string()),
                _ => None,
            })
            .collect();

        assert_eq!(finished, ["a", "b"]);
    }
}
