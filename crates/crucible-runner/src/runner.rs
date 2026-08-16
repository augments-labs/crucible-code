//! The turn loop.
//!
//! Ask the model, run what it asked for, tell it what happened, ask again —
//! until it yields, the user stops it, or something goes wrong.
//!
//! Progress leaves through events, because the thread that draws is not this
//! one. The outcome leaves through the return value, because the caller is
//! what decides whether the session continues.

use crucible_core::{
    Ask, Cancel, Delta, DeltaStream, Effort, Event, Message, Mode, Permission, Post, Provider,
    ProviderError, ProviderLimit, Request, Spend, StopReason, ToolCall, Transcript, TurnError,
    TurnId,
};

use crucible_session::Session;

use crate::tools::Tools;

mod answer;
mod work;

use answer::Answer;
use work::{Went, Work};

const MAX_PROVIDER_RESPONSES_PER_TURN: usize = 32;
const MAX_TURN_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOOL_CALLS_PER_TURN: usize = 128;
const MAX_TOOL_OUTPUT_BYTES_PER_TURN: usize = 4 * 1024 * 1024;

/// Provider-controlled work retained during one turn.
#[derive(Default)]
struct TurnBounds {
    responses: usize,
    retained: usize,
    calls: usize,
    tool_output: usize,
}

impl TurnBounds {
    fn before_response(&self, provider: &'static str) -> Result<(), ProviderError> {
        if self.responses == MAX_PROVIDER_RESPONSES_PER_TURN {
            return Err(ProviderError::Limit {
                provider,
                limit: ProviderLimit::ProviderResponses,
                maximum: MAX_PROVIDER_RESPONSES_PER_TURN,
            });
        }
        Ok(())
    }

    fn heard(&mut self, answer: &Answer) {
        self.responses += 1;
        self.retained = self.retained.saturating_add(answer.retained());
    }

    fn accept_calls(&mut self, provider: &'static str, calls: usize) -> Result<(), ProviderError> {
        if calls > MAX_TOOL_CALLS_PER_TURN.saturating_sub(self.calls) {
            return Err(ProviderError::Limit {
                provider,
                limit: ProviderLimit::TurnToolCalls,
                maximum: MAX_TOOL_CALLS_PER_TURN,
            });
        }
        self.calls += calls;
        Ok(())
    }
}

/// Which model to ask, and how.
#[derive(Debug, Clone)]
pub struct Model {
    /// The model's name, as the provider spells it.
    pub name: Box<str>,
    /// Ceiling on one response.
    pub max_tokens: u32,
    /// The system prompt, if the session has one.
    pub system: Option<Box<str>>,
    /// How hard to think, where somebody said. `None` leaves it to the vendor.
    pub effort: Option<Effort>,
}

/// Drives turns to completion.
///
/// Holds what outlives a turn: the provider, the tools, the session's
/// permission memory, the transcript, and the log it is written to.
#[derive(Debug)]
pub struct Runner {
    provider: Box<dyn Provider>,
    tools: Tools,
    model: Model,
    permission: Permission,
    transcript: Transcript,
    session: Session,
    turn: TurnId,
}

impl Runner {
    /// A session that has not said anything yet.
    #[must_use]
    pub fn new(provider: Box<dyn Provider>, tools: Tools, model: Model, session: Session) -> Self {
        Self {
            provider,
            tools,
            model,
            permission: Permission::new(),
            transcript: Transcript::new(),
            session,
            turn: TurnId::FIRST,
        }
    }

    /// Takes the engine configuration described, rather than the default one.
    ///
    /// Built by the wiring and handed over whole, so this crate never learns
    /// that a rule has a syntax or that a mode has a spelling. A session
    /// without this call is one where nothing was configured, which is the
    /// engine asking about every change and every command.
    #[must_use]
    pub fn permitting(mut self, permission: Permission) -> Self {
        self.permission = permission;
        self
    }

    /// Picks up a transcript that already happened — what `--continue`
    /// replays.
    ///
    /// The turn count comes with it. Numbering the first continued turn `1`
    /// would tell the user this is a new session, which is exactly what
    /// they asked it not to be.
    #[must_use]
    pub fn resuming(mut self, transcript: Transcript) -> Self {
        self.turn = Self::counting(&transcript);
        self.transcript = transcript;
        self
    }

    /// Puts this runner on a different session, and hands back the one it was
    /// recording to.
    ///
    /// What `/resume` runs, and the reason it hands the old session back rather
    /// than dropping it: closing one properly means consuming it — see
    /// [`Session::finish`] — and the first write that failed is worth saying
    /// before the session it failed in is out of sight.
    ///
    /// Everything about the session that was answered is answered again. The
    /// transcript, the log and the turn count come from the session picked up;
    /// what was allowed for the rest of the *last* session is forgotten, since
    /// that scope was the thing just left behind. The mode is not an answer of
    /// that kind — it is where this process is being run, it is on screen at
    /// all times, and a session that quietly moved it would be the one place
    /// the row under the box could be wrong.
    pub fn pick_up(&mut self, session: Session, transcript: Transcript) -> Session {
        self.permission.forget();
        self.turn = Self::counting(&transcript);
        self.transcript = transcript;

        std::mem::replace(&mut self.session, session)
    }

    /// The transcript so far.
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Where the session is being recorded.
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The permission mode this session is in, which the prompt shows at all
    /// times.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.permission.mode()
    }

    /// Steps that mode on to the next of the ring, and says which it is now.
    ///
    /// Reachable only between turns, because a turn owns the runner while it
    /// runs. That is not a rule this crate enforces so much as one the loop's
    /// shape already made true, and it is what leaves the engine needing no
    /// lock: nothing is deciding a call while the mode is being changed.
    pub fn cycle(&mut self) -> Mode {
        self.permission.cycle()
    }

    /// Puts the mode where the user named, rather than stepping to it.
    ///
    /// Reachable at the same moment [`Runner::cycle`] is and for the same
    /// reason.
    pub fn switch(&mut self, mode: Mode) {
        self.permission.switch(mode);
    }

    /// The model this session is asking, as the provider spells it.
    ///
    /// Empty where nothing has chosen one. That is a session that can do
    /// everything except take a turn, and the caller is what refuses the turn —
    /// this crate is handed a name and does not decide which names are real.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model.name
    }

    /// The vendor this session is writing to, by the name it is asked for on
    /// the command line and written down under.
    ///
    /// Read off the provider rather than remembered beside it, so that
    /// [`Runner::serve`] cannot leave the two disagreeing: what a status row
    /// says is then the vendor the next turn actually reaches.
    #[must_use]
    pub fn serving(&self) -> &'static str {
        self.provider.name()
    }

    /// Asks a different model from the next turn on.
    ///
    /// The provider is not changed by this: which vendor is being written to was
    /// settled by which credential the wiring resolved, and a name that vendor
    /// does not serve comes back as its own refusal rather than as a silent
    /// redirection to one that does. [`Runner::serve`] is what changes it, and
    /// it costs a credential to call.
    ///
    /// Reachable between turns, where [`Runner::switch`] is and for the same
    /// reason: a turn owns the runner while it runs.
    pub fn ask(&mut self, model: &str) {
        self.model.name = model.into();
    }

    /// Stands the session under different instructions from the next turn on.
    ///
    /// The caller writes them, because what a turn is asked under is a fact
    /// about the harness rather than about the loop. This exists because part
    /// of what they say is about the session itself — which model is answering,
    /// and how hard it was asked to think — and both of those change while a
    /// session runs. Written once at startup they would go on describing the
    /// session the first turn was taken in.
    ///
    /// Reachable between turns, where [`Runner::ask`] is and for the same
    /// reason: a turn owns the runner while it runs.
    pub fn telling(&mut self, system: &str) {
        self.model.system = Some(system.into());
    }

    /// Writes to a different vendor from the next turn on.
    ///
    /// What a key given mid-session is for. Until there was one, the provider a
    /// run resolved at startup was the provider it died with, so a machine that
    /// had never logged in spent its whole first session refusing every turn —
    /// the key it was just given being read by a run that had already finished
    /// deciding.
    ///
    /// The transcript is kept across the swap, and that is deliberate rather
    /// than incidental: what was said is what the user said, and a vendor is who
    /// it gets sent to. What does not carry is anything the old vendor knows
    /// about the old messages, which is nothing this program was ever told.
    ///
    /// Reachable between turns, where [`Runner::ask`] is and for the same
    /// reason: a turn owns the runner while it runs.
    pub fn serve(&mut self, provider: Box<dyn Provider>) {
        self.provider = provider;
    }

    /// How hard this session is asking the model to think.
    ///
    /// `None` where nothing has said, which is not the middle rung: it is the
    /// field left off the request altogether, and what a vendor does with a
    /// request that does not carry one is the vendor's own default per model.
    #[must_use]
    pub const fn effort(&self) -> Option<Effort> {
        self.model.effort
    }

    /// Asks for a different rung from the next turn on.
    ///
    /// There is no way back to `None` from here, and that is the honest shape:
    /// a rung asked for is a rung the caller can see on the screen, and a
    /// session cannot un-see it by being handed the vendor's default again — a
    /// default this program is never told the name of.
    ///
    /// Reachable between turns, where [`Runner::ask`] is and for the same
    /// reason: a turn owns the runner while it runs.
    pub const fn think(&mut self, effort: Effort) {
        self.model.effort = Some(effort);
    }

    /// Hands the session out, for the caller that is finished driving turns.
    ///
    /// The loop ends owning the runner, and closing a session properly means
    /// consuming it — see [`Session::finish`].
    #[must_use]
    pub fn into_session(self) -> Session {
        self.session
    }

    /// Appends a message to the transcript.
    ///
    /// The only way either the transcript or the log is written. Two calls
    /// that could be made separately would eventually be made separately, and
    /// a log that is missing one message is a session that cannot be continued.
    fn record(&mut self, message: Message) {
        self.session.append(&message);
        self.transcript.push(message);
    }

    /// Takes one turn: the prompt, and the exchange until the model yields.
    ///
    /// `cancel` arrives cleared: [`Cancel::reset`] is the caller's to call, on
    /// the thread that reads the keyboard, before the thread this runs on
    /// exists. Clearing it here would clear a key pressed in between, so a
    /// flag found raised belongs to this turn and stops it — before the prompt
    /// is recorded and before a request goes out, whatever a given provider
    /// makes of being handed a cancel that is already up.
    ///
    /// # Errors
    ///
    /// [`TurnError`] when the provider failed or the user refused a tool. A
    /// tool that ran and did not like what it found is not an error — that
    /// goes back to the model as a result it can work around.
    pub fn turn(
        &mut self,
        prompt: &str,
        ask: &mut dyn Ask,
        events: &dyn Post,
        cancel: &Cancel,
    ) -> Result<StopReason, TurnError> {
        // The number this turn would have, worked out before it is known
        // whether the turn gets to take it. One expression rather than two, so
        // that the turn which runs and the turn which is stopped on the way in
        // cannot come to disagree about what the next one is called.
        let turn = if self.transcript.is_empty() {
            self.turn
        } else {
            self.turn.next()
        };

        if cancel.requested() {
            return Ok(Self::stopped(turn, events));
        }

        self.turn = turn;
        events.post(Event::TurnStarted { turn: self.turn });
        self.record(Message::User(prompt.into()));

        // Posted from here rather than from either place the exchange ends, so
        // that a turn cannot acquire a second way to finish without one. The
        // reason is what tells a truncated answer from a complete one, and it
        // has to reach the thread that draws — a return value never does.
        let stop = self.exchange(ask, events, cancel)?;
        events.post(Event::TurnFinished {
            turn: self.turn,
            stop,
        });

        Ok(stop)
    }

    /// Ends a turn the user stopped before it began, and says so twice.
    ///
    /// Nothing is recorded, and `turn` is not kept. The prompt was never sent,
    /// so the transcript has no half of an exchange to explain and the model is
    /// never told a question that was withdrawn a moment after it was asked —
    /// which is what recording it would come to, since every request afterwards
    /// carries it. The count follows the transcript, so a turn that adds
    /// nothing to it leaves the number free: this announces the number it would
    /// have had, and the next prompt is that turn, taken for real.
    ///
    /// Both events go out all the same, because the screen is the other record:
    /// a start with no finish leaves the turn looking as though it is still
    /// running, and a finish with no start is a shape nothing else here
    /// produces.
    fn stopped(turn: TurnId, events: &dyn Post) -> StopReason {
        let stop = StopReason::Cancelled;

        events.post(Event::TurnStarted { turn });
        events.post(Event::TurnFinished { turn, stop });

        stop
    }

    /// Passes of asking and running, until something ends the turn.
    ///
    /// A failure returns instead, and the caller posts nothing: the failure is
    /// its own event, and a turn with two endings on screen has one too many.
    fn exchange(
        &mut self,
        ask: &mut dyn Ask,
        events: &dyn Post,
        cancel: &Cancel,
    ) -> Result<StopReason, TurnError> {
        self.exchange_with_tool_output_limit(ask, events, cancel, MAX_TOOL_OUTPUT_BYTES_PER_TURN)
    }

    /// Completes a turn under an explicit tool-result byte ceiling.
    fn exchange_with_tool_output_limit(
        &mut self,
        ask: &mut dyn Ask,
        events: &dyn Post,
        cancel: &Cancel,
        tool_output_maximum: usize,
    ) -> Result<StopReason, TurnError> {
        let mut bounds = TurnBounds::default();

        // The turn's own running total, not a bound: nothing here refuses a
        // response for having cost too much, and the number exists to be read
        // rather than to be checked against anything.
        let mut spent = Spend::NONE;

        loop {
            bounds.before_response(self.provider.name())?;
            let (answer, said) = self.listen(events, cancel, &bounds, &mut spent)?;
            bounds.heard(&answer);
            let (text, calls) = answer.finish();

            if let Some(stop) = Self::over(said, &calls) {
                // Calls the model did not finish asking for go no further. A
                // call is written to the transcript only once it has a result,
                // and these will never get one.
                //
                // The reason is written down with them. It is what the session
                // log carries into a replay and what the providers send back to
                // the model, and both of those outlive the notice the user read
                // while it happened.
                self.record(Message::Agent {
                    text,
                    calls: Vec::new(),
                    stop: Some(stop),
                });
                return Ok(stop);
            }

            bounds.accept_calls(self.provider.name(), calls.len())?;

            for call in &calls {
                events.post(Event::ToolRequested { call: call.clone() });
            }

            // Recorded before they run, because running them is what changes
            // the tree: a turn that ends part way through a tool pass would
            // otherwise leave a log whose last word is the prompt, and a
            // continued session that reads files it has already edited. A log
            // ending on a call nothing answered is the shape the replay already
            // drops on the way back in. The calls are cloned because the pass
            // needs them too — one pass's worth, which is what the turn holds
            // either way and does not grow with the transcript.
            self.record(Message::Agent {
                text,
                calls: calls.clone(),
                stop: Some(said),
            });

            let (results, went, output_bytes) = Work {
                tools: &self.tools,
                permission: &mut self.permission,
                ask,
                events,
                cancel,
            }
            .pass(&calls, bounds.tool_output, tool_output_maximum);

            bounds.tool_output = bounds.tool_output.saturating_add(output_bytes);

            self.record(Message::ToolResults(results));

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

    /// Whether the turn is over, and why — or `None` to run the calls.
    ///
    /// Every variant is named rather than caught by a rest pattern: a reason
    /// added to [`StopReason`] has to stop the build here, where the decision
    /// is whether a turn goes on, rather than be waved through as an ending.
    fn over(said: StopReason, calls: &[ToolCall]) -> Option<StopReason> {
        match said {
            StopReason::WantsTools if !calls.is_empty() => None,
            // Waiting on nothing is yielding, whatever the provider called it.
            // Believing it instead would re-send an unchanged transcript and
            // ask the same question until the user noticed.
            StopReason::WantsTools => Some(StopReason::Yielded),
            StopReason::Yielded
            | StopReason::OutOfTokens
            | StopReason::Filtered
            | StopReason::Paused
            | StopReason::Cancelled
            | StopReason::Unknown => Some(said),
        }
    }

    /// One request, read to the end, and the reason it ended.
    ///
    /// An answer that breaks off part way is recorded before the failure
    /// leaves: those deltas were posted as they arrived, so the user has
    /// already read them, and a transcript that does not hold them is one the
    /// user and the model disagree about — the next prompt would follow the
    /// last one with nothing in between. Calls the model never finished asking
    /// for go no further, the same as when it stops early. What is recorded
    /// says the answer never reached an ending, which is what keeps the next
    /// request and a later replay from reading it as one that did.
    ///
    /// A stream that ends without saying why is that same failure: the socket
    /// went quiet, and quiet is what a finished response and a truncated one
    /// have in common.
    fn listen(
        &mut self,
        events: &dyn Post,
        cancel: &Cancel,
        bounds: &TurnBounds,
        spent: &mut Spend,
    ) -> Result<(Answer, StopReason), TurnError> {
        let mut stream = self.provider.stream(self.request(), cancel)?;
        let mut answer = Answer::within(
            self.provider.name(),
            bounds.retained,
            MAX_TURN_RESPONSE_BYTES,
        );

        let heard = Self::hear(stream.as_mut(), &mut answer, events, spent)
            .and_then(|()| answer.reached().map_err(TurnError::from));

        match heard {
            Ok(said) => Ok((answer, said)),
            Err(problem) => {
                let stop = answer.stop();
                let (text, _) = answer.finish();
                self.record(Message::Agent {
                    text,
                    calls: Vec::new(),
                    stop,
                });

                Err(problem)
            }
        }
    }

    /// Reads deltas into `answer` until the stream ends.
    fn hear(
        stream: &mut dyn DeltaStream,
        answer: &mut Answer,
        events: &dyn Post,
        spent: &mut Spend,
    ) -> Result<(), TurnError> {
        // What the turn had spent before this response opened. Each reading a
        // provider sends is this response's total rather than an increment, so
        // it is added to that fixed number and not to the last reading — which
        // is also what makes a provider that sends one final figure and one
        // that counts up as it goes come out the same.
        let before = *spent;

        while let Some(delta) = stream.next() {
            match delta? {
                Delta::Text(text) => {
                    answer.say(&text)?;
                    events.post(Event::Delta { text });
                }
                Delta::ToolStarted { id, name } => answer.calling(id, name)?,
                Delta::ToolArgs(fragment) => answer.arguments(&fragment)?,
                Delta::Spent(said) => {
                    *spent = before.and(said);
                    events.post(Event::Spent { spend: *spent });
                }
                Delta::Stopped(stop) => answer.stopped(stop)?,
            }
        }

        Ok(())
    }

    /// Where a transcript that already happened leaves the count.
    ///
    /// The turn a continued session is *on*, not the one after it: the loop
    /// steps the count on its way into a turn, and numbering the first
    /// continued turn `1` would tell the user this is a new session, which is
    /// exactly what they asked it not to be.
    fn counting(transcript: &Transcript) -> TurnId {
        (1..transcript.turns()).fold(TurnId::FIRST, |turn, _| turn.next())
    }

    /// What to send this pass.
    fn request(&self) -> Request<'_> {
        Request {
            model: &self.model.name,
            transcript: &self.transcript,
            tools: self.tools.schemas(),
            max_tokens: self.model.max_tokens,
            system: self.model.system.as_deref(),
            effort: self.model.effort,
        }
    }
}

#[cfg(test)]
mod tests;
