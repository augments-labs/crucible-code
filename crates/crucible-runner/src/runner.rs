//! The turn loop.
//!
//! Ask the model, run what it asked for, tell it what happened, ask again —
//! until it yields, the user stops it, or something goes wrong.
//!
//! A response that failed before it said a word is asked for once more rather
//! than counted as the thing that went wrong. The socket a provider closed
//! while the tools ran is the usual reason, and it is safe to ask again for
//! exactly the reason it is worth doing: nothing arrived, so nothing has been
//! drawn that a second answer could contradict.
//!
//! Progress leaves through events, because the thread that draws is not this
//! one. The outcome leaves through the return value, because the caller is
//! what decides whether the session continues.

use std::thread;
use std::time::Duration;

use crucible_core::{
    Ask, Cancel, Compacting, Delta, DeltaStream, Effort, Event, Message, Mode, Permission, Post,
    Provider, ProviderError, Request, Spend, StopReason, Summary, ToolCall, ToolSchema, Transcript,
    TurnError, TurnId,
};

use crucible_session::Session;

use crate::tools::Tools;

mod answer;
mod compaction;
mod load;
mod work;

use answer::Answer;
use load::{Counting, Load};
use work::{Went, Work};

/// The most provider-controlled response data one turn retains, in bytes.
///
/// A bound on memory rather than on how long a turn may run: it exists against
/// a provider that will not stop talking, and it is what keeps the peak-memory
/// budget true. The counts that used to sit beside it — responses, tool calls —
/// bounded the wrong thing. A turn is long because there is work in it, and
/// what actually runs out is the model's window, which is now measured and
/// answered by making room rather than by ending the turn.
const MAX_TURN_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// And the most tool-result text, for the same reason.
const MAX_TOOL_OUTPUT_BYTES_PER_TURN: usize = 4 * 1024 * 1024;

/// How many compactions one turn may run without getting anywhere.
///
/// A compaction that frees nothing and is asked for again is the one way this
/// loop can spin without the transcript growing, so it is the one thing still
/// counted. Two, because the first may legitimately free little on a session
/// that is mostly one enormous turn, and a third has proved the point.
const COMPACTIONS_WITHOUT_PROGRESS: u8 = 2;

/// How many more times one response may be asked for after it failed.
///
/// Small on purpose. What this recovers is the moment rather than the request —
/// a connection the provider closed while the tools ran, a service busy for a
/// second — and a failure that outlives two goes is one the user is better off
/// being told about than waited through.
const RETRIES: u8 = 2;

/// How long to wait before the first of them, doubling for the next.
///
/// Short, because the failure this recovers is usually a socket that was
/// already gone rather than a service asking to be left alone — and because a
/// user watching a row that says `retrying` is watching this number.
const FIRST_PAUSE: Duration = Duration::from_millis(250);

/// How long a pause holds before it looks at the cancel again.
const CANCEL_SLICE: Duration = Duration::from_millis(25);

/// Provider-controlled work retained during one turn.
#[derive(Default)]
struct TurnBounds {
    retained: usize,
    tool_output: usize,
}

impl TurnBounds {
    fn heard(&mut self, answer: &Answer) {
        self.retained = self.retained.saturating_add(answer.retained());
    }
}

/// Which model to ask, and how.
#[derive(Debug, Clone)]
pub struct Model {
    /// The model's name, as the provider spells it.
    pub name: Box<str>,
    /// Ceiling on one response.
    pub max_tokens: u32,
    /// How much this model accepts at once, in tokens, where anybody knows.
    ///
    /// `None` is not a large window — it is no answer, and a session runs
    /// without a proactive bound rather than against a number this loop made
    /// up. The wiring above resolves it; this crate is handed the result.
    pub window: Option<u32>,
    /// The system prompt, if the session has one.
    pub system: Option<Box<str>>,
    /// How hard to think, where somebody said. `None` leaves it to the vendor.
    pub effort: Option<Effort>,
}

/// What a session does when the window fills.
///
/// Handed over whole by the wiring, so this crate never learns that any of it
/// has a spelling in a file. The default is a session that compacts when it has
/// to and is bounded by nothing else, which is the answer for somebody who has
/// never heard of any of this.
#[derive(Debug, Clone, Copy)]
pub struct Compaction {
    /// Whether a full window is answered by making room rather than by failing.
    pub automatic: bool,
    /// Room to leave for the next exchange, in tokens, where somebody said.
    pub reserve: Option<u64>,
    /// How many turns are kept word for word after the recap.
    pub keep: usize,
    /// The most one turn may produce before it is stopped, in tokens.
    pub spend_ceiling: Option<u64>,
    /// How large a session must be before picking it up asks about it.
    ///
    /// Carried here rather than read where it is used, so the wiring resolves
    /// every compaction answer in one place. This loop never asks anybody
    /// anything about it — a turn already running has nobody to ask.
    pub ask_on_resume: Option<u64>,
}

impl Default for Compaction {
    fn default() -> Self {
        Self {
            automatic: true,
            reserve: None,
            // Enough to hold what the turn is in the middle of and the one
            // before it, which is what "carry on from here" needs.
            keep: 2,
            spend_ceiling: None,
            ask_on_resume: None,
        }
    }
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
    compacting: Compaction,
    load: Load,
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
            compacting: Compaction::default(),
            load: Load::default(),
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

    /// Takes the compaction settings described, rather than the default ones.
    ///
    /// Handed over whole for the reason [`Runner::permitting`] is: this crate
    /// never learns that any of it has a spelling in a file.
    #[must_use]
    pub fn compacting(mut self, compacting: Compaction) -> Self {
        self.compacting = compacting;
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
        self.recount();
        self
    }

    /// Measures a transcript this runner did not build a message at a time.
    ///
    /// Walked once, where a session is picked up, rather than on any path a
    /// turn takes: what it costs is proportional to the transcript, and the one
    /// moment that is affordable is the one where the transcript was just read
    /// off a disk.
    fn recount(&mut self) {
        self.load.replaced();
        for message in self.transcript.messages() {
            self.load.recorded(message);
        }
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
        self.recount();

        std::mem::replace(&mut self.session, session)
    }

    /// The transcript so far.
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// What the next request would carry, in tokens.
    ///
    /// An estimate for the stretch nothing has reported on yet, and what the
    /// provider said for everything before it. Read by the wiring to decide
    /// whether a session picked up is worth asking about — the loop itself
    /// never asks, because a turn already running has nobody to ask.
    #[must_use]
    pub fn carrying(&self) -> u64 {
        self.load.tokens()
    }

    /// How much of the model's window is left, where a window is known.
    ///
    /// The same reading the row above the box shows during a turn, asked for
    /// between turns — where no event is arriving to carry it.
    #[must_use]
    pub fn left(&self) -> Option<u8> {
        self.load.left(self.model.window)
    }

    /// What this session was told to do when the window fills.
    #[must_use]
    pub const fn compaction(&self) -> Compaction {
        self.compacting
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
        self.load.recorded(&message);
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

        // The turn's own running totals. A bound only where somebody asked for
        // one: a turn that runs long because there is work in it is not a turn
        // to stop, and what a runaway one actually consumes is this.
        let mut counting = Counting {
            spent: Spend::NONE,
            load: self.load,
            window: self.model.window,
        };

        let reserve = load::reserve(
            self.model.max_tokens,
            self.model.window,
            self.compacting.reserve,
        );
        let mut fruitless = 0;

        loop {
            // Recording is what measures the transcript, and it happens on the
            // runner rather than here; reading it back at the top of each pass
            // is what makes the check below see the results of the last one.
            counting.load = self.load;

            if let Some(ceiling) = self.compacting.spend_ceiling
                && counting.spent.tokens() >= ceiling
            {
                return Err(TurnError::Spent { ceiling });
            }

            // Before the request rather than after the answer, because here the
            // transcript *is* what the next request would carry — the results
            // of the last pass are already in it. Checked at the top of the
            // loop, so it cannot run while a tool call is out, and the turn
            // carries on afterwards rather than ending.
            if self.compacting.automatic
                && counting.load.full(counting.window, reserve)
                && !self.made_room(Compacting::Full, events, cancel, &mut fruitless)?
            {
                return Err(TurnError::NoRoom);
            }

            // The other half of the reactive rail. One vendor says the request
            // did not fit inside a response it went on to stream; the others
            // refuse it outright, and the remedy is the same either way.
            let heard = match self.listen(events, cancel, &bounds, &mut counting) {
                Err(TurnError::Provider(ProviderError::WindowExceeded { provider }))
                    if self.compacting.automatic =>
                {
                    if self.made_room(Compacting::Refused, events, cancel, &mut fruitless)? {
                        continue;
                    }
                    return Err(TurnError::Provider(ProviderError::WindowExceeded {
                        provider,
                    }));
                }
                heard => heard?,
            };
            let (answer, said) = heard;

            // And what the response reported goes the other way: the counts a
            // provider sends are read here and belong to the session.
            self.load = counting.load;

            // The provider read the request and could not fit it. Making room
            // and asking the same question again is the whole remedy, and it is
            // the reason this reason is not folded in with the ceiling that
            // cuts an answer short.
            if said == StopReason::WindowExceeded {
                if !self.compacting.automatic {
                    return Ok(said);
                }
                if !self.made_room(Compacting::Refused, events, cancel, &mut fruitless)? {
                    return Err(TurnError::NoRoom);
                }
                continue;
            }
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

            for call in &calls {
                // A name no tool answers to is a call `Work` refuses a moment
                // later, and it has nothing to say about itself first.
                let summary = self
                    .tools
                    .find(&call.name)
                    .map_or_else(|| Summary::new(""), |tool| tool.summary(&call.args));

                events.post(Event::ToolRequested {
                    call: call.clone(),
                    summary,
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

    /// Makes room, and says whether it got anywhere.
    ///
    /// `false` where two goes in a row freed nothing, which is the one way this
    /// loop could spin without the transcript growing: everything else it does
    /// either adds to the transcript or ends the turn. The caller decides what
    /// to say about it, because the two rails reached here for different
    /// reasons and owe the reader different sentences.
    ///
    /// # Errors
    ///
    /// [`TurnError`] where the request for the recap itself failed.
    fn made_room(
        &mut self,
        why: Compacting,
        events: &dyn Post,
        cancel: &Cancel,
        fruitless: &mut u8,
    ) -> Result<bool, TurnError> {
        match self.compact(why, events, cancel)? {
            Some(compacted) if compacted.after < compacted.before => *fruitless = 0,
            _ => *fruitless += 1,
        }

        Ok(*fruitless < COMPACTIONS_WITHOUT_PROGRESS)
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
            | StopReason::WindowExceeded
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
    ///
    /// A failure that reached none of that is asked again instead, up to
    /// [`RETRIES`] times. The one it exists for is a connection the provider
    /// closed while the tools ran — the turn's own pauses are exactly where a
    /// pooled connection goes stale, so the request that fails is the one after
    /// a tool pass rather than the first, and the discussion stops part way
    /// through. Both halves of the condition carry weight: only a failure
    /// [`ProviderError::transient`] calls a moment rather than a request, and
    /// only a response that said nothing. Deltas are posted as they arrive, so
    /// re-asking after one would put an answer on screen twice and leave the
    /// transcript holding the half that was taken back.
    fn listen(
        &mut self,
        events: &dyn Post,
        cancel: &Cancel,
        bounds: &TurnBounds,
        counting: &mut Counting,
    ) -> Result<(Answer, StopReason), TurnError> {
        let mut left = RETRIES;
        let mut pause = FIRST_PAUSE;

        loop {
            let mut answer = Answer::within(
                self.provider.name(),
                bounds.retained,
                MAX_TURN_RESPONSE_BYTES,
            );

            let problem = match self.hearing(&mut answer, events, cancel, counting) {
                Ok(said) => return Ok((answer, said)),
                Err(problem) => problem,
            };

            if left > 0 && Self::again(&problem, &answer) {
                left -= 1;
                events.post(Event::Retrying);

                // A pause the user sat through and then had to interrupt would
                // be this program keeping them waiting rather than the provider.
                if Self::pausing(pause, cancel) {
                    pause = pause.saturating_mul(2);
                    continue;
                }
            }

            let stop = answer.stop();
            let (text, _) = answer.finish();
            self.record(Message::Agent {
                text,
                calls: Vec::new(),
                stop,
            });

            return Err(problem);
        }
    }

    /// One request, read to the end, recording nothing either way.
    ///
    /// Separate from [`Self::listen`] because what a failed response leaves in
    /// the transcript depends on whether it is going to be asked again, and that
    /// question is asked once rather than at each place the reading can fail.
    fn hearing(
        &self,
        answer: &mut Answer,
        events: &dyn Post,
        cancel: &Cancel,
        counting: &mut Counting,
    ) -> Result<StopReason, TurnError> {
        // Read once per pass rather than once per turn: `tool_search` reveals a
        // name while the turn is running, and what it revealed has to be in the
        // very next request or the model cannot call what it just looked up.
        let advertised = self.tools.advertised();
        let mut stream = self.provider.stream(self.request(&advertised), cancel)?;

        Self::hear(stream.as_mut(), answer, events, counting)
            .and_then(|()| answer.reached().map_err(TurnError::from))
    }

    /// Whether this failure, on this much of an answer, is worth asking again.
    ///
    /// The stop is checked beside the bytes because a response can fail after
    /// one and hold nothing: what a stop reason has already told the turn is as
    /// much a thing not to say twice as a sentence the user has read.
    fn again(problem: &TurnError, answer: &Answer) -> bool {
        matches!(problem, TurnError::Provider(failure) if failure.transient())
            && answer.retained() == 0
            && answer.stop().is_none()
    }

    /// Waits out a pause, and says whether it ran to the end.
    ///
    /// In slices, because the thread this runs on is the one holding the turn:
    /// a user who presses Esc during a pause is answered at the next slice
    /// rather than when the provider would have been asked again.
    fn pausing(pause: Duration, cancel: &Cancel) -> bool {
        let mut left = pause;

        while !left.is_zero() {
            if cancel.requested() {
                return false;
            }

            let slice = left.min(CANCEL_SLICE);
            thread::sleep(slice);
            left -= slice;
        }

        !cancel.requested()
    }

    /// Reads deltas into `answer` until the stream ends.
    fn hear(
        stream: &mut dyn DeltaStream,
        answer: &mut Answer,
        events: &dyn Post,
        counting: &mut Counting,
    ) -> Result<(), TurnError> {
        // What the turn had spent before this response opened. Each reading a
        // provider sends is this response's total rather than an increment, so
        // it is added to that fixed number and not to the last reading — which
        // is also what makes a provider that sends one final figure and one
        // that counts up as it goes come out the same.
        let before = counting.spent;

        while let Some(delta) = stream.next() {
            match delta? {
                Delta::Text(text) => {
                    answer.say(&text)?;
                    events.post(Event::Delta { text });
                }
                Delta::ToolStarted { id, name } => answer.calling(id, name)?,
                Delta::ToolArgs(fragment) => answer.arguments(&fragment)?,
                Delta::Spent(said) => {
                    counting.spent = before.and(said);
                    counting.load.spent(said);
                    events.post(Event::Spent {
                        spend: counting.spent,
                    });
                }
                // Not added to the spend beside it, and not accumulated at
                // all. What a request carried is a level rather than a total —
                // the transcript goes whole to the provider every time, so each
                // reading supersedes the last instead of extending it, and a
                // running sum of them would describe a session nobody had.
                Delta::Carried(carried) => {
                    counting.load.carried(carried);
                    events.post(Event::Carried {
                        left: counting.load.left(counting.window),
                    });
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
    ///
    /// The advertised schemas are handed in rather than read here, because they
    /// are built per pass — a tool the model looked up mid-turn belongs in the
    /// next request — and a `Request` borrows for as long as the caller holds
    /// them.
    fn request<'a>(&'a self, advertised: &'a [ToolSchema]) -> Request<'a> {
        Request {
            model: &self.model.name,
            transcript: &self.transcript,
            tools: advertised,
            max_tokens: self.model.max_tokens,
            system: self.model.system.as_deref(),
            effort: self.model.effort,
        }
    }
}

#[cfg(test)]
mod tests;
