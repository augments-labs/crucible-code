//! The runner, behind the contract every backend implements.
//!
//! [`Native`] borrows a runner for as long as it is used and answers the
//! [`Backend`] operations with it: a prompt is one of the runner's turns, an
//! interrupt is the turn's own cancel, and a permission question the engine
//! asks becomes a [`Pending`] that a [`Decision`] naming it settles. Nothing is
//! sent anywhere new and nothing is granted here: an allowed decision goes back
//! to the permission engine as the verdict a person would have given, and the
//! engine builds the proof a tool runs under where it builds it today.
//!
//! What the runner reports is translated by hand, one match over its events,
//! into the contract's bounded values, and a kind the runner adds is a kind
//! this match has to decide about before it compiles. A turn's ending and its
//! usage are posted once, from what the turn answered: [`Event::Finished`] or
//! [`Event::Failed`] last, and before it one [`Event::Usage`] holding what that
//! turn moved the session's totals by, never the totals themselves.
//!
//! A permission question carries the line or address exactly as the call
//! sends it, beside the spelling a rule is about, and says when a command
//! would be left running after the turn. One the table refuses as cut short is
//! not put, as a client's is not: a yes to the start of a line is a yes to a
//! line nobody read, so the call is refused as it is where nobody answers.
//!
//! Confinement is claimed from the sandbox's own claim and from nothing else,
//! so [`Confinement::HardEnforced`] is claimed only where the sandbox says it
//! enforces.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crucible_execution::{
    About, Backend, Capabilities, Capability, Confined, Confinement, Cost, Currency, Decision,
    Description, Detail, Error, Event, Failure, Lasting, Lookup, NAME_BYTES, Observation, Outcome,
    Pending, PendingRef, Pendings, Scope, SessionRef, Sink, Stale, Text, TurnRequest, Unsupported,
    Usage,
};
use crucible_runner::{Event as Reported, EventEnvelope, Post, Runner, SessionCost, Totals};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_sandbox::SandboxCapability;
use crucible_tools::{Argument, Ask, Looking, Remember, Sensitivity, Verdict};
use crucible_types::{CostAmount, StopReason, ToolCall, TurnId};
use tokio::sync::oneshot;

/// The driver name the runner is described under.
pub const DRIVER: &str = "native";

/// What is answered where nobody will: no, and only this once.
const DENIED: (Verdict, Remember) = (Verdict::Deny, Remember::Never);

/// Femtocurrency in one millionth of a unit.
const FEMTO_PER_MILLIONTH: u128 = 1_000_000_000;

/// The runner, answering as a [`Backend`].
pub struct Native<'r> {
    /// The runner, while no turn holds it.
    runner: Mutex<Option<&'r mut Runner>>,
    /// The session the runner records into, as the contract names it.
    session: SessionRef,
    /// What the running turn is waiting on.
    pendings: Mutex<Pendings>,
    /// Where each open question's decision is handed to the turn waiting on
    /// it.
    waiting: Mutex<Vec<(PendingRef, oneshot::Sender<Decision>)>>,
    /// The running turn's stop.
    cancel: Cancel,
    /// Lines added to the running turn; nothing adds any through the contract.
    steer: Steer,
    /// Things told to the running turn; nothing tells any through the contract.
    aside: Aside,
    /// What it claims.
    capabilities: Capabilities,
}

impl std::fmt::Debug for Native<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Native")
            .field("session", &self.session)
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl<'r> Native<'r> {
    /// `runner`, recording into the session named `session`, confined as
    /// `sandbox` claims where a sandbox confines its tools.
    ///
    /// Nothing once this process has numbered every instance it can.
    #[must_use]
    pub fn new(
        runner: &'r mut Runner,
        session: SessionRef,
        sandbox: Option<SandboxCapability>,
    ) -> Option<Self> {
        Some(Self {
            runner: Mutex::new(Some(runner)),
            session,
            pendings: Mutex::new(Pendings::new()?),
            waiting: Mutex::new(Vec::new()),
            cancel: Cancel::new(),
            steer: Steer::new(),
            aside: Aside::new(),
            capabilities: claims(sandbox),
        })
    }

    /// The pending table, whoever last held it.
    fn pendings(&self) -> MutexGuard<'_, Pendings> {
        self.pendings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The decisions being waited on, whoever last held them.
    fn waiting(&self) -> MutexGuard<'_, Vec<(PendingRef, oneshot::Sender<Decision>)>> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens `pending` in the running turn, to be decided through `answer`.
    fn raise(&self, pending: Pending, answer: oneshot::Sender<Decision>) -> Option<PendingRef> {
        let mut pendings = self.pendings();
        let reference = pendings.raise(pending).ok()?;
        self.waiting().push((reference.clone(), answer));
        Some(reference)
    }

    /// The running turn has ended: nothing it raised is open any more.
    fn ended(&self) {
        let mut pendings = self.pendings();
        pendings.end();
        self.waiting().clear();
    }
}

/// What the runner claims behind the contract, confined as `sandbox` says.
fn claims(sandbox: Option<SandboxCapability>) -> Capabilities {
    let claimed =
        Capability::ALL
            .into_iter()
            .fold(Capabilities::unknown(), |claims, capability| {
                let supported = match capability {
                    Capability::Cancel
                    | Capability::PermissionOnce
                    | Capability::PermissionSession
                    | Capability::Usage => true,
                    // The runner does these, through the conversation that holds
                    // it; none of them is reached through this adapter.
                    Capability::Steer
                    | Capability::Resume
                    | Capability::Fork
                    | Capability::Rewind
                    | Capability::Compaction
                    | Capability::ModelSwitch
                    | Capability::Questions
                    | Capability::ContextOccupancy
                    | Capability::AccountStatus
                    | Capability::Logout
                    | Capability::ProfileIsolation => false,
                };
                claims.declared(capability, supported)
            });
    match sandbox {
        Some(claim) => claimed.confined(Confined::new(confinement(claim), Scope::Instance)),
        None => claimed,
    }
}

/// The sandbox's claim, in the contract's words.
#[must_use]
pub const fn confinement(claim: SandboxCapability) -> Confinement {
    match claim {
        SandboxCapability::Enforced => Confinement::HardEnforced,
        SandboxCapability::Observed => Confinement::Observed,
        SandboxCapability::Unsupported => Confinement::Unsupported,
    }
}

/// Refuses `capability` unless the runner claims it.
fn gate(capabilities: &Capabilities, capability: Capability) -> Result<(), Error> {
    if capabilities.supports(capability) {
        Ok(())
    } else {
        Err(Unsupported(capability).into())
    }
}

impl Backend for Native<'_> {
    fn describe(&self) -> Description {
        Description::new(DRIVER, DRIVER, self.capabilities).reporting(env!("CARGO_PKG_VERSION"))
    }

    fn open(&self, session: Option<SessionRef>) -> BoxFuture<'_, Result<SessionRef, Error>> {
        Box::pin(async move {
            if session.is_some() {
                gate(&self.capabilities, Capability::Resume)?;
            }
            Ok(self.session.clone())
        })
    }

    /// Runs one of the runner's turns. The runner numbers its own turns, so
    /// the turn [`Event::Started`] names is the runner's and not the
    /// request's.
    fn prompt<'a>(
        &'a self,
        turn: TurnRequest,
        events: &'a dyn Sink,
    ) -> BoxFuture<'a, Result<Outcome, Error>> {
        Box::pin(async move {
            let busy = || Failure::new("a turn is already running on this backend");
            let mut held = Held::take(&self.runner).ok_or_else(busy)?;
            let runner = held.runner().ok_or_else(busy)?;
            // An interrupt asks the running turn to stop; one from before this
            // turn began asked nothing of it.
            self.cancel.reset();
            let before = runner.totals();
            let translating = Translating::new(self, events);
            let run = runner.starting(&translating, &self.cancel, &self.steer, &self.aside);
            let mut asking = Asking {
                native: self,
                events,
            };
            let turned = runner
                .turn(turn.prompt(), Box::new([]), &mut asking, &run)
                .await;
            self.ended();
            if let Some(usage) = translating.usage(before) {
                events.post(Event::Usage(Observation::live(usage, Scope::Session)));
            }
            match turned {
                Ok(turned) => {
                    let outcome = outcome(turned.stop());
                    events.post(Event::Finished(outcome));
                    Ok(outcome)
                }
                Err(error) => {
                    let failure = Failure::new(&error.to_string());
                    events.post(Event::Failed(failure.clone()));
                    Err(failure.into())
                }
            }
        })
    }

    fn decide(&self, pending: &PendingRef, decision: Decision) -> Result<(), Error> {
        match decision {
            Decision::Allow(Lasting::Once) => gate(&self.capabilities, Capability::PermissionOnce)?,
            Decision::Allow(Lasting::Session) => {
                gate(&self.capabilities, Capability::PermissionSession)?;
            }
            Decision::Answer(_) => gate(&self.capabilities, Capability::Questions)?,
            Decision::Deny => {}
        }
        let answer = {
            let mut pendings = self.pendings();
            pendings.settle(pending, &decision)?;
            let mut waiting = self.waiting();
            let at = waiting
                .iter()
                .position(|(open, _)| open == pending)
                .ok_or(Stale::Settled)?;
            waiting.swap_remove(at).1
        };
        // The turn stopped waiting after the table settled it: nothing is
        // left to tell, and the turn has already refused the call.
        answer.send(decision).map_err(|_| Stale::Settled)?;
        Ok(())
    }

    fn interrupt(&self) -> Result<(), Error> {
        gate(&self.capabilities, Capability::Cancel)?;
        self.cancel.request();
        // A question still open would hold the turn short of the boundary it
        // stops at, so each is answered no, and settled in the table as no.
        let mut pendings = self.pendings();
        for (reference, answer) in self.waiting().drain(..) {
            drop(pendings.settle(&reference, &Decision::Deny));
            drop(answer.send(Decision::Deny));
        }
        Ok(())
    }

    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        // A turn runs inside its own prompt, and the borrow of the runner
        // ends with it: once nothing is awaiting a prompt, nothing started
        // here is running.
        Box::pin(async { Ok(()) })
    }
}

/// The runner, taken out for one turn and put back however the turn ends.
struct Held<'s, 'r> {
    /// Where it goes back to.
    slot: &'s Mutex<Option<&'r mut Runner>>,
    /// The runner, for as long as the turn holds it.
    runner: Option<&'r mut Runner>,
}

impl<'s, 'r> Held<'s, 'r> {
    /// The runner, unless a turn already holds it.
    fn take(slot: &'s Mutex<Option<&'r mut Runner>>) -> Option<Self> {
        let runner = slot.lock().unwrap_or_else(PoisonError::into_inner).take()?;
        Some(Self {
            slot,
            runner: Some(runner),
        })
    }

    /// The runner held: always one, since `take` made it with one and only
    /// `drop` takes it out.
    fn runner(&mut self) -> Option<&mut Runner> {
        self.runner.as_deref_mut()
    }
}

impl Drop for Held<'_, '_> {
    fn drop(&mut self) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        *slot = self.runner.take();
    }
}

/// How the turn ended, in the contract's words.
const fn outcome(stop: Option<StopReason>) -> Outcome {
    match stop {
        Some(StopReason::Yielded) => Outcome::Answered,
        Some(StopReason::Cancelled) => Outcome::Cancelled,
        // A guardrail refused, or the model stopped short of an answer.
        None
        | Some(
            StopReason::WantsTools
            | StopReason::OutOfTokens
            | StopReason::WindowExceeded
            | StopReason::Filtered
            | StopReason::Paused
            | StopReason::Unknown,
        ) => Outcome::CutShort,
    }
}

/// The permission engine's asking, put through the contract.
struct Asking<'n, 'r> {
    /// The backend whose table the question is opened in.
    native: &'n Native<'r>,
    /// Where the question is posted.
    events: &'n dyn Sink,
}

impl Ask for Asking<'_, '_> {
    fn ask<'a>(
        &'a mut self,
        call: &'a ToolCall,
        sensitivity: &'a Sensitivity,
    ) -> BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async move {
            let pending = Pending::Permission {
                tool: Text::clipped(&call.name, NAME_BYTES),
                summary: Text::new(&sensitivity.to_string()),
                detail: detail(call, sensitivity),
            };
            let (answer, decided) = oneshot::channel();
            let Some(reference) = self.native.raise(pending.clone(), answer) else {
                return DENIED;
            };
            self.events.post(Event::Pending(reference, pending));
            match decided.await {
                Ok(Decision::Allow(Lasting::Once)) => (Verdict::Allow, Remember::Never),
                Ok(Decision::Allow(Lasting::Session)) => (Verdict::Allow, Remember::Session),
                Ok(Decision::Deny | Decision::Answer(_)) | Err(_) => DENIED,
            }
        })
    }
}

/// What kind of thing the call would do, with the command line or address
/// exactly as the call sends it, where it sends one, and whether a command
/// would outlive the turn. The summary is the
/// spelling a rule is about, which for a line is its commands without the
/// operators between them and for a search is its host without the query: a
/// question showing only that would be asking about something nobody sent.
fn detail(call: &ToolCall, sensitivity: &Sensitivity) -> Detail {
    match sensitivity {
        Sensitivity::SpawnsProcess { command } => Detail::Command {
            sent: Text::new(command.sent()),
            left_running: crucible_builtins::backgrounded(&call.args),
        },
        Sensitivity::ReachesNetwork { host } => Detail::Network {
            sent: Text::new(host.sent()),
        },
        Sensitivity::ReadOnly { .. } => Detail::Reads,
        Sensitivity::ReadsOutside { .. } => Detail::ReadsOutside,
        Sensitivity::MutatesFile { .. } => Detail::MutatesFile,
    }
}

/// What the runner reports, posted in the contract's words.
struct Translating<'n, 'r> {
    /// The backend whose table a turn's start opens.
    native: &'n Native<'r>,
    /// Where it is posted.
    events: &'n dyn Sink,
    /// The turn's number, once it has started.
    turn: Mutex<Option<TurnId>>,
    /// The session's totals as the turn last moved them.
    used: Mutex<Option<Totals>>,
}

impl<'n, 'r> Translating<'n, 'r> {
    /// Posting to `events` for `native`.
    const fn new(native: &'n Native<'r>, events: &'n dyn Sink) -> Self {
        Self {
            native,
            events,
            turn: Mutex::new(None),
            used: Mutex::new(None),
        }
    }

    /// What the turn moved the session's totals by since `before`, where it
    /// started and moved them.
    fn usage(&self, before: Totals) -> Option<Usage> {
        let turn = (*self.turn.lock().unwrap_or_else(PoisonError::into_inner))?;
        let after = (*self.used.lock().unwrap_or_else(PoisonError::into_inner))?;
        let moved = Usage::of(turn)
            .input(after.input().saturating_sub(before.input()))
            .output(after.output().saturating_sub(before.output()));
        // The runner sums a figure no response reported as nothing, so a
        // cache figure that did not move is one nothing is known of.
        let moved = match after.cache_read().saturating_sub(before.cache_read()) {
            0 => moved,
            read => moved.cache_read(read),
        };
        let moved = match after.cache_write().saturating_sub(before.cache_write()) {
            0 => moved,
            written => moved.cache_write(written),
        };
        Some(match spent(before.cost(), after.cost()) {
            Some(cost) => moved.cost(cost),
            None => moved,
        })
    }

    /// The contract's word for `event`, where it has one.
    fn translated(&self, event: &Reported) -> Option<Event> {
        match event {
            Reported::TurnStarted { turn } => {
                self.native
                    .pendings()
                    .begin(self.native.session.clone(), *turn);
                *self.turn.lock().unwrap_or_else(PoisonError::into_inner) = Some(*turn);
                Some(Event::Started { turn: *turn })
            }
            Reported::Delta { text } => Some(Event::Delta(Text::new(text))),
            Reported::ToolRequested {
                call,
                summary,
                looking,
                ..
            } => Some(Event::ToolActivity {
                call: Text::clipped(call.id.as_str(), NAME_BYTES),
                tool: Text::clipped(&call.name, NAME_BYTES),
                summary: Text::new(summary.as_str()),
                about: match summary.argument() {
                    Argument::Path => About::Path,
                    Argument::Address => About::Address,
                    Argument::Command => About::Command,
                    Argument::Other => About::Other,
                },
                looked: looking.map(|looking| match looking {
                    Looking::Pattern => Lookup::Pattern,
                    Looking::File => Lookup::File,
                    Looking::Directory => Lookup::Directory,
                    Looking::Command => Lookup::Command,
                    Looking::WebSearch => Lookup::WebSearch,
                    Looking::WebPage => Lookup::WebPage,
                }),
            }),
            Reported::ToolFinished { call, output, .. } => Some(Event::ToolReport {
                call: Text::clipped(call.as_str(), NAME_BYTES),
                failed: output.is_failed(),
                report: Text::new(output.text()),
                diff: output.diff().cloned(),
            }),
            Reported::Used { totals } => {
                *self.used.lock().unwrap_or_else(PoisonError::into_inner) = Some(*totals);
                None
            }
            // The ending is posted once, by the prompt, from what the turn
            // answered: the runner's own word for it would be a second one.
            // The rest is the runner's own bookkeeping, and what a terminal
            // draws from it: the contract has no word for any of it, and a
            // consumer of the contract has asked for none of it. Another
            // agent's report is never one this runner makes.
            Reported::TurnFinished { .. }
            | Reported::AgentCalled { .. }
            | Reported::AgentAnswered { .. }
            | Reported::Reported { .. }
            | Reported::Failed { .. }
            | Reported::PromptCache { .. }
            | Reported::Sandbox { .. }
            | Reported::Wrote { .. }
            | Reported::Retrying
            | Reported::FastRefused { .. }
            | Reported::Carried { .. }
            | Reported::Compacting { .. }
            | Reported::Compacted { .. }
            | Reported::AgentCompacted { .. }
            | Reported::Spent { .. }
            | Reported::PlanLimits { .. }
            | Reported::Aged { .. }
            | Reported::Unread { .. }
            | Reported::Steered { .. } => None,
        }
    }
}

impl Post for Translating<'_, '_> {
    fn post(&self, reported: EventEnvelope) {
        if let Some(event) = self.translated(reported.event()) {
            self.events.post(event);
        }
    }
}

/// What a turn cost, where the session was priced before it and after it in
/// one currency.
fn spent(before: SessionCost, after: SessionCost) -> Option<Cost> {
    let after = priced(after)?;
    let before = match before {
        SessionCost::Unspent => CostAmount::new(after.currency(), after.unit(), 0),
        SessionCost::Priced(amount) => amount,
        SessionCost::AtLeast(_) | SessionCost::NotPriced => return None,
    };
    if before.currency() != after.currency() {
        return None;
    }
    let femto = after.femtocurrency().checked_sub(before.femtocurrency())?;
    let millionths = u64::try_from(femto / FEMTO_PER_MILLIONTH).ok()?;
    Some(Cost::new(
        millionths,
        Currency::new(after.currency().as_str())?,
    ))
}

/// The session's sum, where every response was priced.
const fn priced(cost: SessionCost) -> Option<CostAmount> {
    match cost {
        SessionCost::Priced(amount) => Some(amount),
        SessionCost::Unspent | SessionCost::AtLeast(_) | SessionCost::NotPriced => None,
    }
}

#[cfg(test)]
mod tests;
