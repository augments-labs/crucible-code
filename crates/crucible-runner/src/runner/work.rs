//! Running what the model asked for.
//!
//! One rule shapes this file: **every call the transcript records has a result
//! recorded with it.** A provider refuses a transcript containing a request
//! with no answer, so a turn that stops half way through a pass — because the
//! user cancelled, or said no — still writes a result for each remaining call
//! saying why there is nothing in it.

use crucible_core::{
    Approved, Ask, Cancel, Event, Permission, Post, Settled, StopReason, ToolCall, ToolError,
    ToolOutput, ToolResult,
};

use crate::tools::Tools;

/// What a call is answered with when the turn ended before it could run.
const NOT_RUN: &str = "not run: the turn ended first";

/// What a call is answered with when the user said no.
const DENIED: &str = "the user did not allow this";

/// What a call is answered with when its output would cross the turn boundary.
const OUTPUT_LIMIT: &str = "not run: the turn output limit was reached";

/// What a call is answered with when standing policy forbids it — a rule, or
/// the engine keeping its own configuration out of reach. Phrased for the
/// model, which is what reads it: it says the wall is standing rather than
/// momentary, so the answer is to do something else and not to rephrase this.
const FORBIDDEN: &str = "permission policy does not allow this; asking again will not change it";

/// What one pass of calls decided about the turn.
pub(crate) enum Went {
    /// Every call ran. Ask the model again.
    On,
    /// The user cancelled part way through.
    Stopped(StopReason),
    /// The user refused this tool.
    Refused(Box<str>),
    /// A tool result would have crossed the retained-output boundary.
    OutputLimit,
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

/// Everything a pass of calls needs, gathered so the runner reads as one thing.
pub(crate) struct Work<'a> {
    /// What may be called.
    pub(crate) tools: &'a Tools,
    /// The session's memory of what was already allowed.
    pub(crate) permission: &'a mut Permission,
    /// How to put a call to the user.
    pub(crate) ask: &'a mut dyn Ask,
    /// Where progress is reported.
    pub(crate) events: &'a dyn Post,
    /// Whether the user has asked everything to stop.
    pub(crate) cancel: &'a Cancel,
}

impl Work<'_> {
    /// Runs `calls` in order, and answers every one of them.
    pub(crate) fn pass(
        &mut self,
        calls: &[ToolCall],
        held: usize,
        maximum: usize,
    ) -> (Vec<ToolResult>, Went, usize) {
        let mut results = Vec::with_capacity(calls.len());
        let mut went = Went::On;
        let mut produced = 0_usize;

        for (index, call) in calls.iter().enumerate() {
            let mut output = match went {
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
                Went::OutputLimit => ToolOutput::failed(""),
            };

            // Leave enough room to answer every later call even when this one
            // fills the budget. The provider requires a result for every call
            // already recorded, so dropping the tail is not a valid bound.
            let later = calls.len().saturating_sub(index + 1);
            let reserved = later.saturating_mul(NOT_RUN.len());
            let room = maximum
                .saturating_sub(held)
                .saturating_sub(produced)
                .saturating_sub(reserved);
            if output.text().len() > room {
                output = ToolOutput::failed(if OUTPUT_LIMIT.len() <= room {
                    OUTPUT_LIMIT
                } else {
                    ""
                });
                went = Went::OutputLimit;
            }
            produced = produced.saturating_add(output.text().len());

            // Cloned because both halves need it: the renderer shows what the
            // tool produced, and the transcript sends it to the model. It is
            // one tool's output, so it does not grow with the transcript.
            self.events.post(Event::ToolFinished {
                call: call.id.clone(),
                output: output.clone(),
            });

            results.push(ToolResult {
                id: call.id.clone(),
                output,
            });
        }

        (results, went, produced)
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
        match self.permission.decide(call, &sensitivity, self.ask) {
            Settled::Approved(approved) => self.run(approved),
            // Standing policy, which the model can read and work around. It
            // costs nothing to hit twice, so the turn carries on.
            Settled::Forbidden => Ran::Output(ToolOutput::failed(FORBIDDEN)),
            // A person, about this moment. The turn ends, because a model that
            // is told no and left running will ask the same thing in a shape
            // the rules happen not to cover.
            Settled::Refused => Ran::Refused,
        }
    }

    /// Runs the tool the approval names.
    ///
    /// Looked up from the approval rather than kept from before the verdict.
    /// The handle above answered how dangerous the call was; the one that runs
    /// comes out of the same value as the arguments and the proof, so a verdict
    /// reached about one tool cannot arrive at another with that tool's
    /// arguments beside it — which is the guarantee the whole mechanism is for,
    /// and it should not rest on two lines staying next to each other.
    fn run(&self, approved: Approved) -> Ran {
        let Some(tool) = self.tools.find(approved.tool()) else {
            // A name that reached a verdict is a name a lookup already
            // answered to, so this is the arm nothing takes. Answered rather
            // than asserted: the model can read a result, and a session is
            // worth more than a proof about a branch.
            return Ran::Output(failure(&ToolError::Unknown(approved.tool().into())));
        };

        match tool.run(approved) {
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
    use std::sync::mpsc::{Receiver, Sender, channel};

    use crucible_core::{ToolArgs, ToolId, Verdict};

    use super::*;
    use crate::fake::{Fixed, Says, changing};

    /// One pass, with everything it needed set up around it.
    struct Proof {
        tools: Tools,
        permission: Permission,
        says: Says,
        cancel: Cancel,
        events: Sender<Event>,
        seen: Receiver<Event>,
    }

    impl Proof {
        fn new(verdict: Verdict) -> Self {
            Self::asking(Says::new(verdict))
        }

        fn asking(says: Says) -> Self {
            let (events, seen) = channel();

            Self {
                tools: Tools::new(),
                permission: Permission::new(),
                says,
                cancel: Cancel::new(),
                events,
                seen,
            }
        }

        fn offering(mut self, tool: Fixed) -> Self {
            self.tools.add(Box::new(tool));
            self
        }

        fn pass(&mut self, calls: &[ToolCall]) -> (Vec<ToolResult>, Went) {
            let (results, went, _) = self.within(calls, 0, usize::MAX);
            (results, went)
        }

        fn within(
            &mut self,
            calls: &[ToolCall],
            held: usize,
            maximum: usize,
        ) -> (Vec<ToolResult>, Went, usize) {
            Work {
                tools: &self.tools,
                permission: &mut self.permission,
                ask: &mut self.says,
                events: &self.events,
                cancel: &self.cancel,
            }
            .pass(calls, held, maximum)
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
        let mut proof =
            Proof::new(Verdict::Allow).offering(Fixed::new("read").answering("fn main() {}"));

        let (results, went) = proof.pass(&[call("a", "read")]);

        assert_eq!(texts(&results), ["fn main() {}"]);
        assert!(matches!(went, Went::On), "the turn should carry on");
    }

    #[test]
    fn the_tool_that_runs_is_the_one_the_verdict_was_reached_about() {
        // The name is dispatched on out of the approval, beside the arguments
        // and the proof. Two tools answering differently are how a pass can
        // say which of them ran.
        let mut proof = Proof::new(Verdict::Allow)
            .offering(Fixed::new("read").answering("what read produced"))
            .offering(Fixed::new("grep").answering("what grep produced"));

        let (results, went) = proof.pass(&[call("a", "grep")]);

        assert_eq!(texts(&results), ["what grep produced"]);
        assert!(matches!(went, Went::On));
    }

    #[test]
    fn a_name_no_tool_answers_to_is_reported_to_the_model_rather_than_ending_the_turn() {
        // The model invented it, so the model is the one that can fix it.
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read"));

        let (results, went) = proof.pass(&[call("a", "frobnicate")]);

        assert_eq!(texts(&results), ["no tool named frobnicate"]);
        assert!(results.first().is_some_and(|r| r.output.is_failed()));
        assert!(matches!(went, Went::On));
    }

    #[test]
    fn a_tool_that_fails_reports_it_to_the_model_rather_than_ending_the_turn() {
        let mut proof =
            Proof::new(Verdict::Allow).offering(Fixed::new("read").breaking("unreadable"));

        let (results, went) = proof.pass(&[call("a", "read")]);

        assert_eq!(texts(&results), ["read: unreadable"]);
        assert!(matches!(went, Went::On));
    }

    #[test]
    fn a_denied_call_ends_the_turn_and_says_so_in_its_result() {
        let mut proof = Proof::new(Verdict::Deny).offering(Fixed::new("write").risking(changing()));

        let (results, went) = proof.pass(&[call("a", "write")]);

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
        let mut proof = Proof::new(Verdict::Deny).offering(Fixed::new("write").risking(changing()));

        let (results, _) = proof.pass(&[call("a", "write"), call("b", "write")]);

        assert_eq!(results.len(), 2);
        assert_eq!(texts(&results), [DENIED, NOT_RUN]);
    }

    #[test]
    fn a_call_allowed_for_the_session_is_not_put_to_the_user_again() {
        // One permission engine covers the pass, so the second call finds what
        // the first was allowed. A fresh engine per call would ask twice and
        // make `always` mean `once`.
        let mut proof = Proof::asking(Says::for_the_session())
            .offering(Fixed::new("write").risking(changing()));

        proof.pass(&[call("a", "write"), call("b", "write")]);

        assert_eq!(proof.says.asked, 1);
    }

    #[test]
    fn a_call_after_a_denial_is_never_put_to_the_user() {
        let mut proof = Proof::new(Verdict::Deny).offering(Fixed::new("write").risking(changing()));

        proof.pass(&[call("a", "write"), call("b", "write")]);

        assert_eq!(proof.says.asked, 1, "the user was asked about a dead turn");
    }

    #[test]
    fn a_cancelled_pass_stops_the_turn_without_running_anything_more() {
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read"));
        proof.cancel.request();

        let (results, went) = proof.pass(&[call("a", "read")]);

        assert_eq!(texts(&results), [NOT_RUN]);
        assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
    }

    #[test]
    fn a_tool_that_noticed_the_cancellation_itself_stops_the_turn() {
        // A long-running tool checks the flag mid-work and returns. That is not
        // a failure to report to the model — the user stopped the turn.
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("bash").cancelling());

        let (_, went) = proof.pass(&[call("a", "bash")]);

        assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
    }

    #[test]
    fn every_call_reports_that_it_finished() {
        // The renderer redraws the line it drew when the call was requested, so
        // a call with no finish stays on screen as if it were still running.
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read"));

        proof.pass(&[call("a", "read"), call("b", "read")]);

        let finished: Vec<String> = proof
            .seen
            .try_iter()
            .filter_map(|event| match event {
                Event::ToolFinished { call, .. } => Some(call.to_string()),
                Event::TurnStarted { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::TurnFinished { .. }
                | Event::Failed { .. } => None,
            })
            .collect();

        assert_eq!(finished, ["a", "b"]);
    }

    #[test]
    fn an_output_limit_still_answers_every_recorded_call() {
        let oversized = "x".repeat(OUTPUT_LIMIT.len() + 1);
        let maximum = OUTPUT_LIMIT.len() + NOT_RUN.len();
        let mut proof =
            Proof::new(Verdict::Allow).offering(Fixed::new("read").answering(&oversized));

        let (results, went, produced) =
            proof.within(&[call("a", "read"), call("b", "read")], 0, maximum);

        assert_eq!(results.len(), 2);
        assert_eq!(texts(&results), [OUTPUT_LIMIT, ""]);
        assert!(matches!(went, Went::OutputLimit));
        assert!(produced <= maximum);
    }
}
