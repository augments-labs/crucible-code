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

use crucible_core::{
    Ask, Compacting, Event, Message, ProviderError, StopReason, ToolsetContext, TurnError,
};

use crate::context::RunContext;

use super::{After, Counting, Listening, Runner, TurnBounds, Went, Work};

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
    /// [`StopReason`].
    pub(super) fn drive(&mut self, counting: &mut Counting) -> Result<StopReason, TurnError> {
        let run = self.run;
        let events = run.reporting();
        let cancel = run.cancel();
        let steer = run.steer();
        let aside = run.aside();
        let tool_output_maximum = run.policy().bounds.tool_output_bytes;

        let mut bounds = TurnBounds::default();
        let mut fruitless = 0;

        loop {
            // A line typed while the turn ran is worked in here, between one
            // pass and the next: recorded as a prompt the same way the turn's
            // own first one was, so the request below carries it and the agent
            // adjusts course rather than finishing a plan the reader moved past.
            // Checked at the top so a burst typed in a pass arrives together,
            // and so it cannot land while a tool call is out.
            for line in steer.take() {
                events.post(Event::Steered { line: line.clone() });
                self.runner.record(Message::said(line));
                events.post(Event::Carried {
                    left: self.runner.load.left(counting.window, counting.reserve),
                });
            }

            // And what happened while it ran, in the same place for the same
            // reason: a command the agent was told not to poll for has exited,
            // and the pass that follows is the first one that can do anything
            // about it. No `Steered` goes with it — the reader did not type it,
            // and an event saying they did would put a sentence in the panel
            // that nobody wrote. The line above it is already on their screen.
            for note in aside.take() {
                self.runner.record(Message::said(note));
                events.post(Event::Carried {
                    left: self.runner.load.left(counting.window, counting.reserve),
                });
            }

            // Read once per pass: `tool_search` can reveal a schema mid-turn.
            // The exact set measured here is handed to the request below, so an
            // estimate cannot count one set and send another.
            let tools = if self.first_tools {
                self.first_tools = false;
                self.runner.toolset.snapshot(self.toolsets)?
            } else {
                self.runner.toolset.refresh(self.toolsets)?
            };
            self.runner.tools = tools.clone();
            let advertised = tools.advertised();

            // Recording is what measures the transcript, and it happens on the
            // runner rather than here; reading it back at the top of each pass
            // is what makes the check below see the results of the last one.
            counting.load = self.runner.load;
            counting
                .load
                .requesting(self.runner.spec.instructions(), &advertised);

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
                match self.runner.made_room(
                    Compacting::Full,
                    run,
                    &mut fruitless,
                    &mut counting.spent,
                )? {
                    // Re-enter the boundary check against the reduced load.
                    // A prune that helped but did not help enough may still need
                    // the complete-active-pass recap before any request is safe.
                    After::Carry => continue,
                    After::Stuck => return Err(TurnError::NoRoom),
                    After::Stopped => return Ok(StopReason::Cancelled),
                }
            }

            // The other half of the reactive rail. One vendor says the request
            // did not fit inside a response it went on to stream; the others
            // refuse it outright, and the remedy is the same either way.
            // Compaction replaced `self.load`; refresh the request estimate
            // before sending rather than carrying the pre-compaction count into
            // the response that calibrates it.
            counting.load = self.runner.load;
            counting
                .load
                .requesting(self.runner.spec.instructions(), &advertised);

            let heard = match self.runner.listen(
                &bounds,
                Listening {
                    run,
                    advertised: &advertised,
                    counting,
                },
            ) {
                Err(TurnError::Provider(ProviderError::WindowExceeded { provider }))
                    if run.policy().compaction.automatic =>
                {
                    match self.runner.made_room(
                        Compacting::Refused,
                        run,
                        &mut fruitless,
                        &mut counting.spent,
                    )? {
                        After::Carry => continue,
                        After::Stopped => return Ok(StopReason::Cancelled),
                        After::Stuck => {
                            return Err(TurnError::Provider(ProviderError::WindowExceeded {
                                provider,
                            }));
                        }
                    }
                }
                heard => heard?,
            };
            let (answer, said) = heard;

            // And what the response reported goes the other way: the counts a
            // provider sends are read here and belong to the session, as does a
            // window it proved larger than anybody had written down.
            self.runner.load = counting.load;
            self.runner.spec.model.window = counting.window;

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
                    self.runner.record(Message::Agent {
                        text,
                        calls: Vec::new(),
                        stop: Some(said),
                    });
                }
                if !run.policy().compaction.automatic {
                    return Ok(said);
                }
                match self.runner.made_room(
                    Compacting::Refused,
                    run,
                    &mut fruitless,
                    &mut counting.spent,
                )? {
                    After::Carry => continue,
                    After::Stuck => return Err(TurnError::NoRoom),
                    After::Stopped => return Ok(StopReason::Cancelled),
                }
            }
            bounds.heard(&answer);
            let (text, calls) = answer.finish();

            if let Some(stop) = Runner::over(said, &calls) {
                // Calls the model did not finish asking for go no further. A
                // call is written to the transcript only once it has a result,
                // and these will never get one.
                //
                // The reason is written down with them. It is what the session
                // log carries into a replay and what the providers send back to
                // the model, and both of those outlive the notice the user read
                // while it happened.
                self.runner.record(Message::Agent {
                    text,
                    calls: Vec::new(),
                    stop: Some(stop),
                });
                return Ok(stop);
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
            self.runner.record(Message::Agent {
                text,
                calls: calls.clone(),
                stop: Some(said),
            });

            let (results, went, output_bytes) = Work {
                tools: &tools,
                permission: &mut self.runner.permission,
                ask: &mut *self.ask,
                events,
                cancel,
            }
            .pass(&calls, bounds.tool_output, tool_output_maximum);

            bounds.tool_output = bounds.tool_output.saturating_add(output_bytes);

            self.runner.record(Message::ToolResults(results));
            events.post(Event::Carried {
                left: self.runner.load.left(counting.window, counting.reserve),
            });

            match went {
                Went::On => {}
                Went::Stopped(stop) => return Ok(stop),
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
