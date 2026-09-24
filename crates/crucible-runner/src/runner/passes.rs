//! The loop itself: ask, run what was asked for, ask again.
//!
//! Lifted out of the runner so that the two things it was doing at once stop
//! sharing a name. A [`Runner`] is what a session holds: the provider it talks
//! to, the transcript it is building, what the user has already allowed, the
//! log it is writing. The passes are what one run *does* with those, for as
//! long as one turn lasts, and they need two things the runner does not own —
//! which run this is, and where to ask the user.
//!
//! So those two sit beside a borrow of the session, on a value that lives
//! exactly that long. The borrow is what says it: an [`AgentLoop`] holds the
//! runner rather than being one, cannot outlive the run it was given, and
//! nothing that survives the turn can be reached through it afterwards. How
//! much room is left is not among them: it is measured per pass and travels
//! through [`AgentLoop::drive`], because a window learned from a response is
//! a different window and a figure kept on the loop would outlive the answer
//! that corrected it.
//!
//! Nothing here decides anything the runner did not already decide. This is
//! where the loop lives now, not a second opinion about how a turn should go.

use crucible_agents::{GuardrailError, Rejection};
use crucible_core::{
    Ask, Compacting, Message, ProviderContinuation, ProviderError, RunId, Spend, StopReason,
    ToolCall, ToolsetContext,
};
use crucible_runtime::Bridge;

use crate::context::RunContext;
use crate::outcome::{RunResult, Turned};

use super::{After, Counting, Judged, Listening, Runner, TurnBounds, Went, Work};

use crate::{Event, TurnError};
/// How one run's passes ended.
///
/// The loop's own word for it, because the value a caller gets back carries a
/// figure the loop does not hold: what the turn spent lives on the totals
/// [`Runner::exchange`] owns, and threading it back through every one of the
/// ways out of here would mean writing it at each of them. This says which
/// ending was reached; the caller turns that into the [`Turned`] it hands out.
#[derive(Debug)]
pub(super) enum Ending {
    /// The model's answer was accepted, and this is how it ended.
    Stopped(StopReason),

    /// An output check refused the final candidate.
    Rejected {
        /// Which check refused, and what it said about why.
        rejection: Rejection,
        /// How the model's own answer ended, which is a separate fact from
        /// whether the answer was accepted.
        stop: StopReason,
    },

    /// An output check ran and could not reach a decision.
    Undecided {
        /// What the check said about why not.
        problem: GuardrailError,
        /// How the model's own answer ended.
        stop: StopReason,
    },
}

impl Ending {
    /// The same ending, as the value a caller of a turn is handed.
    pub(super) fn turned(self, run: RunId, spent: Spend) -> Turned {
        match self {
            Self::Stopped(stop) => Turned::Ran(RunResult::new(run, stop, spent)),
            Self::Rejected { rejection, stop } => Turned::Rejected {
                rejection,
                stop: Some(stop),
            },
            Self::Undecided { problem, stop } => Turned::Undecided {
                problem,
                stop: Some(stop),
            },
        }
    }
}

/// One run's worth of passes over one runner.
pub(super) struct AgentLoop<'a> {
    /// The session this run is being taken against.
    runner: &'a mut Runner,
    /// Which run this is, and everything it was told about how to take it.
    run: &'a RunContext<'a>,
    /// How to put a call to the user. Not the runner's, because who is being
    /// asked is a property of the run and not of the session it belongs to.
    ask: &'a mut dyn Ask,
    /// The narrow lifecycle context the live toolset was prepared under.
    toolsets: &'a ToolsetContext,
    /// Whether the first immutable generation is still to be captured.
    first_tools: bool,
}

impl<'a> AgentLoop<'a> {
    /// The loop one run will take over `runner`.
    pub(super) fn new(
        runner: &'a mut Runner,
        run: &'a RunContext<'a>,
        ask: &'a mut dyn Ask,
        toolsets: &'a ToolsetContext,
    ) -> Self {
        Self {
            runner,
            run,
            ask,
            toolsets,
            first_tools: true,
        }
    }

    /// What arrived while the pass before this one was running.
    ///
    /// A line typed while the turn ran is worked in here, between one pass and
    /// the next: recorded as a prompt the same way the turn's own first one
    /// was, so the request that follows carries it and the agent adjusts course
    /// rather than finishing a plan the reader moved past. Called at the top of
    /// a pass so a burst typed in one arrives together, and so it cannot land
    /// while a tool call is out.
    ///
    /// What happened while it ran goes in the same place for the same reason: a
    /// command the agent was told not to poll for has exited, and the pass that
    /// follows is the first one that can do anything about it. No `Steered`
    /// goes with it — the reader did not type it, and an event saying they did
    /// would put a sentence in the panel that nobody wrote. The line above it
    /// is already on their screen.
    async fn interjected(&mut self, counting: &Counting) -> Result<(), TurnError> {
        let run = self.run;
        let events = run.reporting();
        for line in run.steer().take() {
            events.post(Event::Steered { line: line.clone() });
            self.runner
                .record(run.ancestry(), Message::said(line))
                .await?;
            events.post(Event::Carried {
                left: self
                    .runner
                    .state
                    .load
                    .left(counting.window, counting.reserve),
            });
        }
        for note in run.aside().take() {
            self.runner
                .record(run.ancestry(), Message::said(note))
                .await?;
            events.post(Event::Carried {
                left: self
                    .runner
                    .state
                    .load
                    .left(counting.window, counting.reserve),
            });
        }
        Ok(())
    }

    /// The last answer of a turn: judged, then written down, then the ending.
    ///
    /// Calls the model did not finish asking for go no further. A call is
    /// written to the transcript only once it has a result, and these will
    /// never get one. The reason is written down with them: it is what the
    /// session log carries into a replay and what the providers send back to
    /// the model, and both of those outlive the notice the user read while it
    /// happened.
    ///
    /// The candidate is judged before it is accepted and before any of it is
    /// written down. A refused answer leaves no trace for the next request to
    /// carry: the deltas the reader watched arrive were provisional, and this
    /// is where that stops being true for everything else.
    async fn ending(
        &mut self,
        text: Box<str>,
        continuation: Option<ProviderContinuation>,
        calls: &[ToolCall],
        stop: StopReason,
    ) -> Result<Ending, TurnError> {
        match self.runner.vouching(&text, self.run) {
            Ok(Judged::Allowed) => {}
            Ok(Judged::Rejected(rejection)) => return Ok(Ending::Rejected { rejection, stop }),
            Err(problem) => return Ok(Ending::Undecided { problem, stop }),
        }

        self.runner
            .record(
                self.run.ancestry(),
                Message::Agent {
                    continuation: if calls.is_empty() { continuation } else { None },
                    text,
                    calls: Vec::new(),
                    stop: Some(stop),
                },
            )
            .await?;
        Ok(Ending::Stopped(stop))
    }

    /// Makes room for `why` against this run's totals, and says what the
    /// turn may do next, as [`Runner::made_room`] does.
    async fn room(
        &mut self,
        why: Compacting,
        fruitless: &mut u8,
        counting: &mut Counting,
    ) -> Result<After, TurnError> {
        self.runner
            .made_room(why, self.run, fruitless, &mut counting.spent)
            .await
    }

    /// Takes passes until the turn ends, and says how it ended.
    ///
    /// The totals are the caller's, not this loop's. Every way out of here is
    /// a way a turn ends, and there are enough of them that reaching the spend
    /// through a return value would mean writing it at each one.
    ///
    /// # Errors
    ///
    /// [`TurnError`] where a request, a tool or the transcript itself failed,
    /// and for the four endings a turn reaches rather than is stopped by:
    /// [`TurnError::Spent`] and [`TurnError::ToolOutputBytes`] where a ceiling
    /// was crossed, [`TurnError::NoRoom`] where two compactions in a row freed
    /// nothing, and [`TurnError::Refused`] where the reader declined a call.
    /// None of the four is a failure, and all four end a turn the way one
    /// does, which is why they leave through here rather than through
    /// [`StopReason`]. A step that would have had to wait ends it wherever
    /// [`Runner::turn`] says one does, and leaves what that says: as
    /// [`TurnError::Unready`], or as the source's failure where it was a tool
    /// source's own step.
    ///
    /// Every step the turn crosses to that would have had to wait ends it on
    /// the refusal, even where a stop was asked for, except a call's run in a
    /// parallel wave and a background result's acceptance, which never end it
    /// on a refusal: [`Runner::turn`] says what becomes of each. The turn's
    /// cache steps and its toolset's listing and refreshing all end it so. A
    /// compaction's steps end it as [`Runner::compact`] says. The line
    /// recording the last answer, the part of an answer a full window cut
    /// short, and the results of a pass are each awaited before the ending
    /// they lead to is reached.
    pub(super) async fn drive(&mut self, counting: &mut Counting) -> Result<Ending, TurnError> {
        let run = self.run;
        let events = run.reporting();
        let cancel = run.cancel();
        let tool_output_maximum = run.policy().bounds.tool_output_bytes;

        let mut bounds = TurnBounds::default();
        let mut fruitless = 0;

        loop {
            self.runner.flush_sandbox_audits(events)?;

            self.interjected(counting).await?;

            // Read once per pass: `tool_search` can reveal a schema mid-turn.
            // The exact set measured here is handed to the request below, so an
            // estimate cannot count one set and send another.
            let tools = if self.first_tools {
                self.first_tools = false;
                self.runner.toolset.snapshot(self.toolsets)
            } else {
                self.runner.toolset.refresh(self.toolsets)
            };
            let tools = Bridge::TurnTools
                .cross(tools)
                .map_err(TurnError::from)
                .and_then(|tools| tools.map_err(TurnError::from));
            let tools =
                super::combine_sandbox_audit(tools, self.runner.flush_sandbox_audits(events))?;
            // Narrowed to what this agent declares, against the exact
            // generation the pass admitted rather than a later one. The
            // request advertises this and a call is admitted through this, so
            // the roster the model was shown and the roster it is held to
            // cannot come apart.
            let tools = self
                .runner
                .agent
                .availability()
                .narrowing(&tools)
                .map_err(TurnError::from)?;
            self.runner.state.tools = tools.clone();
            let advertised = tools.advertised();

            // Once, against this exact immutable generation and after any
            // compaction from the preceding loop iteration rewrote history.
            // Recording the fragments updates `runner.load` before it is read
            // below, so reserve and fullness see exactly what will be sent.
            self.runner.assemble_context(run.ancestry()).await?;

            // Recording is what measures the transcript, and it happens on the
            // runner rather than here; reading it back at the top of each pass
            // is what makes the check below see the results of the last one.
            counting.load = self.runner.state.load;
            counting
                .load
                .requesting(self.runner.agent.instructions(), &advertised);

            // Worked out per pass rather than once, because what it is measured
            // against can be corrected mid-turn: a window learned from a
            // response is a different window, and a reserve left behind would
            // be held against the figure that was just disproved.
            let reserve = self
                .runner
                .reserve(run.policy().compaction, counting.window);
            counting.reserve = reserve;

            if let Some(ceiling) = run.policy().bounds.spend
                && counting.spent.tokens() >= ceiling
            {
                return Err(TurnError::Spent { ceiling });
            }

            // Before the request rather than after the answer, because here the
            // transcript *is* what the next request would carry — the results
            // of the last pass are already in it. Checked at the top of the
            // loop, so it cannot run while a tool call is out, and the turn
            // carries on afterwards rather than ending.
            if run.policy().compaction.automatic && counting.load.full(counting.window, reserve) {
                // The prompt may itself have crossed the boundary, in which
                // case no preceding load event exists. State the zero the same
                // arithmetic reached before replacing it with the compaction
                // activity, so the two cannot appear to disagree.
                events.post(Event::Carried {
                    left: counting.left(),
                });
                match self
                    .room(Compacting::Full, &mut fruitless, counting)
                    .await?
                {
                    // Re-enter the boundary check against the reduced load.
                    // A prune that helped but did not help enough may still need
                    // the complete-active-pass recap before any request is safe.
                    After::Carry => continue,
                    After::Stuck => return Err(TurnError::NoRoom),
                    After::Stopped => return Ok(Ending::Stopped(StopReason::Cancelled)),
                }
            }

            // The other half of the reactive rail. One vendor says the request
            // did not fit inside a response it went on to stream; the others
            // refuse it outright, and the remedy is the same either way.
            // Compaction replaced `self.state.load`; refresh the request estimate
            // before sending rather than carrying the pre-compaction count into
            // the response that calibrates it.
            counting.load = self.runner.state.load;
            counting
                .load
                .requesting(self.runner.agent.instructions(), &advertised);

            let heard = match self
                .runner
                .listen(
                    &bounds,
                    Listening {
                        run,
                        advertised: &advertised,
                        generation: tools.generation(),
                        counting,
                    },
                )
                .await
            {
                Err(TurnError::Provider(ProviderError::WindowExceeded { provider }))
                    if run.policy().compaction.automatic =>
                {
                    match self
                        .room(Compacting::Refused, &mut fruitless, counting)
                        .await?
                    {
                        After::Carry => continue,
                        After::Stopped => return Ok(Ending::Stopped(StopReason::Cancelled)),
                        After::Stuck => {
                            return Err(TurnError::Provider(ProviderError::WindowExceeded {
                                provider,
                            }));
                        }
                    }
                }
                heard => heard?,
            };
            let (mut answer, said) = heard;

            // And what the response reported goes the other way: the counts a
            // provider sends are read here and belong to the session, as does a
            // window it proved larger than anybody had written down.
            self.runner.state.load = counting.load;
            self.runner.state.window = counting.window;

            // The provider read the request and could not fit it. Making room
            // and asking the same question again is the whole remedy, and it is
            // the reason this reason is not folded in with the ceiling that
            // cuts an answer short.
            if said == StopReason::WindowExceeded {
                // What streamed before the cut was produced and delivered, so
                // it is written down with the reason it stopped — whether the
                // loop goes on to make room or hands the stop back. A record
                // that dropped it would end the stream mid-sentence with no
                // explanation, which a turn is promised never to do.
                bounds.heard(&answer);
                let (text, _calls) = answer.finish();
                if !text.is_empty() {
                    self.runner
                        .record(
                            run.ancestry(),
                            Message::Agent {
                                continuation: None,
                                text,
                                calls: Vec::new(),
                                stop: Some(said),
                            },
                        )
                        .await?;
                }
                if !run.policy().compaction.automatic {
                    return Ok(Ending::Stopped(said));
                }
                match self
                    .room(Compacting::Refused, &mut fruitless, counting)
                    .await?
                {
                    After::Carry => continue,
                    After::Stuck => return Err(TurnError::NoRoom),
                    After::Stopped => return Ok(Ending::Stopped(StopReason::Cancelled)),
                }
            }
            bounds.heard(&answer);
            let continuation = answer.take_continuation();
            let (text, calls) = answer.finish();

            if let Some(stop) = Runner::over(said, &calls) {
                return self.ending(text, continuation, &calls, stop).await;
            }

            for call in &calls {
                // A name no tool answers to is a call `Work` refuses a moment
                // later, and it has nothing to say about itself first.
                let entry = tools.find(&call.name);
                events.post(Event::ToolRequested {
                    summary: entry.map_or_else(
                        || crucible_core::Summary::new(""),
                        |entry| entry.tool().summary(&call.args),
                    ),
                    backgroundable: entry
                        .is_some_and(|entry| entry.tool().backgroundable(&call.args)),
                    looking: entry.and_then(|entry| entry.tool().looking(&call.args)),
                    alone: calls.len() == 1,
                    call: call.clone(),
                });
            }

            // Recorded before they run, because running them is what changes
            // the tree: a turn that ends part way through a tool pass would
            // otherwise leave a log whose last word is the prompt, and a
            // continued session that reads files it has already edited. A log
            // ending on a call nothing answered is the shape the replay already
            // drops on the way back in. The calls are cloned because the pass
            // needs them too — one pass's worth, which is what the turn holds
            // either way and does not grow with the transcript.
            self.runner
                .record(
                    run.ancestry(),
                    Message::Agent {
                        continuation,
                        text,
                        calls: calls.clone(),
                        stop: Some(said),
                    },
                )
                .await?;

            let (results, went, output_bytes) = Work {
                tools: &tools,
                permission: &mut self.runner.permission,
                ask: &mut *self.ask,
                events,
                cancel,
                ancestry: run.ancestry(),
                journal: &*self.runner.store,
                audits: &self.runner.sandbox_audits,
                concurrency: run.policy().tools.maximum_concurrency(),
            }
            .pass(&calls, bounds.tool_output, tool_output_maximum)
            .await;

            bounds.tool_output = bounds.tool_output.saturating_add(output_bytes);

            self.runner
                .record(run.ancestry(), Message::ToolResults(results))
                .await?;
            events.post(Event::Carried {
                left: self
                    .runner
                    .state
                    .load
                    .left(counting.window, counting.reserve),
            });

            match went {
                Went::On => {}
                Went::Stopped(stop) => return Ok(Ending::Stopped(stop)),
                Went::Refused(name) => return Err(TurnError::Refused(name)),
                Went::OutputLimit => {
                    return Err(TurnError::ToolOutputBytes {
                        maximum: tool_output_maximum,
                    });
                }
            }
        }
    }
}
