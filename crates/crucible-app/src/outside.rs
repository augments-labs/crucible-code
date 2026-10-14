//! A conversation's turns on an installed agent, told the way the runner tells
//! its own.
//!
//! An installed agent reports in the execution contract's events, and a front
//! end reads runner events. `Outside` runs one turn on the agent's backend
//! and hands on each event it reports as the runner event a native turn posts
//! for the same thing, through the turn's own [`RunContext`], so the terminal
//! and a headless client read one stream whichever backend ran the turn.
//! Nothing the agent reports is an instruction here: a tool it used is a
//! [`RunEvent::Reported`], something it did, and the leave it asks for goes to
//! the turn's [`Asker`] and back to the agent as the contract's decision,
//! never as anything the permission engine reads.
//!
//! **One tally.** The figures an agent reports fold into the conversation's
//! one `Tally`, and each change is posted whole as `Used`, so what a front
//! end holds mid-turn and what is read between turns are the same figures.
//! Tokens add up turn by turn. A cost reported in dollars adds to a priced
//! sum; one in another currency, a sum too large to state, or a turn that
//! answered without a cost leaves the session not priced for good. A turn
//! that ended after it began with no cost reported, stopped, cut short,
//! failed, or left by its caller, leaves the sum so far as a floor, since it
//! may have spent and did not say. A turn begins when the agent's host says
//! it has, before anything is written to the agent, so a turn whose request
//! may never have left still counts as one that may have spent: the figures
//! never read lower than what was spent. A launch refused before that
//! changes nothing, and no plan limit is ever posted, since no agent reports
//! one.
//!
//! **One transcript.** What a person sees of the conversation, their prompt
//! and the agent's answer, is kept beside the backend and written to the
//! session as a native turn's are, the answer bounded at the transcript's
//! ceiling for one message and recorded as cut where it was. Nothing of the
//! agent's own context is kept: that is the agent's.
//!
//! **A spent backend is replaced.** A turn that failed, one whose effects are
//! uncertain and one whose caller stopped waiting leave a backend that takes
//! no further turn: it is closed, and the next turn launches another through
//! the [`Relaunch`] the conversation holds, resuming the agent's own session
//! by the name the agent last gave it. A change of mode does the same, since
//! the agent is told its mode only at launch, and so it never changes
//! mid-turn. A mode the agent has no launch for is never taken: neither by a
//! change nor by a conversation moved onto the route in it.
//!
//! **Launched by its first turn.** A conversation moved onto the route with
//! no run in hand launches one at its first turn, the same way a spent run is
//! replaced, so a run that started on the route has asked nothing of the agent
//! before somebody asks it something.

use std::collections::VecDeque;
use std::future::poll_fn;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use crucible_execution::{
    Backend, Cost, Decision, Error, Event, Failure, Outcome, PENDINGS, Pending, PendingRef, Sink,
    Text, TurnRequest, Usage,
};
use crucible_models::ProviderError;
use crucible_runner::{
    Event as RunEvent, ReportedTokens, Reporter, RunContext, RunResult, SessionCost, Totals,
    TurnError, Turned,
};
use crucible_runtime::BoxFuture;
use crucible_session::{ExternalRef, RouteBinding, Session, Uncertain};
use crucible_tools::Mode;
use crucible_types::{
    Attachment, CONTINUATION_BYTES, CostAmount, Message, PricingCurrency, PricingUnit, Spend,
    StopReason, Transcript, TurnId,
};

use crate::client::Asker;

/// What launches an installed agent again, for a conversation whose backend is
/// spent, cleared, or wanted under another mode.
///
/// Supplied by whoever moved the conversation onto the agent's route, so that
/// the route's launch, the hold on its vendor and its permission modes are
/// read in one place.
pub trait Relaunch: Send + Sync {
    /// Whether the agent can be launched under `mode`, or the sentence saying
    /// why not, decided before anything is started.
    ///
    /// # Errors
    ///
    /// [`Failure`] saying why the agent has no launch under `mode`.
    fn admits(&self, mode: Mode) -> Result<(), Failure>;

    /// A new run of the agent under `mode`, resuming the agent's own session
    /// `resume` names, or starting one where it is `None`.
    fn launch<'a>(
        &'a self,
        mode: Mode,
        resume: Option<&'a ExternalRef>,
    ) -> BoxFuture<'a, Result<Box<dyn Backend>, Failure>>;
}

/// The installed agent a conversation moves onto: its first run where one
/// was launched already, what launches the next, and the route the session is
/// bound to.
pub struct Installed {
    /// The agent's run, launched under the conversation's mode; `None` leaves
    /// the first turn to launch it.
    pub backend: Option<Box<dyn Backend>>,
    /// What launches another.
    pub relaunch: Box<dyn Relaunch>,
    /// The route the session is bound to.
    pub route: RouteBinding,
}

impl std::fmt::Debug for Installed {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Installed")
            .field("route", &self.route)
            .finish_non_exhaustive()
    }
}

/// Refuses anything attached to a turn on an installed agent, which no agent
/// is handed.
///
/// # Errors
///
/// [`TurnError::Refused`] where something is attached.
pub(crate) fn unattached(attached: &[Attachment]) -> Result<(), TurnError> {
    if attached.is_empty() {
        Ok(())
    } else {
        Err(TurnError::Refused(
            "an attachment to an installed agent's turn".into(),
        ))
    }
}

/// The one currency an agent's cost is counted in.
const DOLLARS: PricingCurrency = PricingCurrency::new("USD");

/// Femtocurrency in one millionth of a currency.
const FEMTO: u128 = 1_000_000_000;

/// The sentence a failure is ended with where what the agent was doing is not
/// known to have happened or not.
const UNKNOWN_EFFECTS: &str = " What the agent was doing may or may not have happened.";

/// What an installed agent has reported using, over a whole session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tally {
    /// The tokens reported.
    tokens: ReportedTokens,
    /// What it came to.
    cost: SessionCost,
    /// When the first turn began.
    started: Option<Instant>,
    /// The turn under way, and whether it reported a cost.
    open: Option<bool>,
}

impl Tally {
    /// Nothing reported yet.
    pub(crate) const fn new() -> Self {
        Self {
            tokens: ReportedTokens {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            cost: SessionCost::Unspent,
            started: None,
            open: None,
        }
    }

    /// The session's figures, as a front end reads them.
    pub(crate) fn totals(&self) -> Totals {
        self.started.map_or_else(Totals::new, |started| {
            Totals::reported(self.tokens, self.cost, started)
        })
    }

    /// A turn begins. One its caller left open ends first, as a turn that
    /// said nothing of its cost. Whether the figures changed.
    pub(crate) fn begin(&mut self) -> bool {
        let changed = self.end(false);
        self.started.get_or_insert_with(Instant::now);
        self.open = Some(false);
        changed
    }

    /// The turn under way reported `usage`.
    pub(crate) fn used(&mut self, usage: &Usage) {
        let add = |sum: u64, more: Option<u64>| sum.saturating_add(more.unwrap_or(0));
        let tokens = &mut self.tokens;
        tokens.input = add(tokens.input, usage.input_tokens());
        tokens.output = add(tokens.output, usage.output_tokens());
        tokens.cache_read = add(tokens.cache_read, usage.cache_read_tokens());
        tokens.cache_write = add(tokens.cache_write, usage.cache_write_tokens());
        if let Some(spent) = usage.spent() {
            self.cost = priced(self.cost, spent);
            if let Some(open) = &mut self.open {
                *open = true;
            }
        }
    }

    /// The turn under way ended, `answered` or not. Whether the figures
    /// changed.
    pub(crate) fn end(&mut self, answered: bool) -> bool {
        let Some(priced) = self.open.take() else {
            return false;
        };
        if priced {
            return false;
        }
        let before = self.cost;
        self.cost = match (answered, self.cost) {
            (true, _) | (false, SessionCost::Unspent) => SessionCost::NotPriced,
            (false, SessionCost::Priced(sum)) => SessionCost::AtLeast(sum),
            (false, other @ (SessionCost::AtLeast(_) | SessionCost::NotPriced)) => other,
        };
        before != self.cost
    }
}

/// `cost` with `spent` added: a sum in dollars stays a sum, or a floor where
/// it was one, and anything that cannot be stated is not priced for good.
fn priced(cost: SessionCost, spent: Cost) -> SessionCost {
    let amount = (spent.currency().as_str() == DOLLARS.as_str()).then(|| {
        CostAmount::new(
            DOLLARS,
            PricingUnit::Mixed,
            u128::from(spent.millionths()).saturating_mul(FEMTO),
        )
    });
    let summed = |sum: CostAmount, amount: CostAmount| {
        sum.checked_add(amount).ok().filter(|sum| stated(*sum))
    };
    match (cost, amount) {
        (SessionCost::NotPriced, _) | (_, None) => SessionCost::NotPriced,
        (SessionCost::Unspent, Some(amount)) => Some(amount)
            .filter(|amount| stated(*amount))
            .map_or(SessionCost::NotPriced, SessionCost::Priced),
        (SessionCost::Priced(sum), Some(amount)) => {
            summed(sum, amount).map_or(SessionCost::NotPriced, SessionCost::Priced)
        }
        (SessionCost::AtLeast(sum), Some(amount)) => {
            summed(sum, amount).map_or(SessionCost::NotPriced, SessionCost::AtLeast)
        }
    }
}

/// Whether `amount` can be stated in millionths, which is how a client is
/// told it.
fn stated(amount: CostAmount) -> bool {
    u64::try_from(amount.femtocurrency() / FEMTO).is_ok()
}

/// A conversation on an installed agent's route: the agent's backend, what
/// launches it again, and what the conversation keeps of it.
pub(crate) struct Outside {
    /// The agent's run, where one is held.
    backend: Option<Box<dyn Backend>>,
    /// What launches another.
    relaunch: Box<dyn Relaunch>,
    /// The route the session is bound to.
    route: RouteBinding,
    /// The driver, as the agent registry names it.
    driver: &'static str,
    /// What a person sees of the conversation.
    transcript: Transcript,
    /// What the agent reported using.
    tally: Tally,
    /// The permission mode the agent is launched in.
    mode: Mode,
    /// The agent's own name for its session, as last given.
    named: Option<ExternalRef>,
    /// The next turn.
    turn: TurnId,
    /// Whether the run held takes no further turn: its last turn's caller
    /// stopped waiting for it, or the mode changed since it was launched.
    spent: bool,
}

impl std::fmt::Debug for Outside {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Outside")
            .field("route", &self.route)
            .field("mode", &self.mode)
            .field("tally", &self.tally)
            .finish_non_exhaustive()
    }
}

impl Outside {
    /// A conversation on the agent `installed`, its run launched under
    /// `mode` now or by the first turn, the session's agent named `named` and `transcript` what was
    /// seen of it so far.
    pub(crate) fn new(
        installed: Installed,
        transcript: Transcript,
        mode: Mode,
        named: Option<ExternalRef>,
    ) -> Self {
        let Installed {
            backend,
            relaunch,
            route,
        } = installed;
        let driver = crucible_config::Driver::ALL
            .iter()
            .map(|driver| driver.name())
            .find(|name| *name == route.driver())
            .unwrap_or("agent");
        let turn = transcript
            .messages()
            .iter()
            .filter(|message| matches!(message, Message::User { .. }))
            .fold(TurnId::FIRST, |turn, _| turn.next());
        Self {
            backend,
            relaunch,
            route,
            driver,
            transcript,
            tally: Tally::new(),
            mode,
            named,
            turn,
            spent: false,
        }
    }

    /// The driver, as the agent registry names it.
    pub(crate) const fn driver(&self) -> &'static str {
        self.driver
    }

    /// What a person sees of the conversation.
    pub(crate) const fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The session's figures.
    pub(crate) fn totals(&self) -> Totals {
        self.tally.totals()
    }

    /// The permission mode the agent is launched in.
    pub(crate) const fn mode(&self) -> Mode {
        self.mode
    }

    /// The route the session is bound to.
    pub(crate) const fn route(&self) -> &RouteBinding {
        &self.route
    }

    /// Launches the agent under `mode` from the next turn, where it can be;
    /// the mode in force either way.
    pub(crate) fn switch(&mut self, mode: Mode) -> Mode {
        if mode != self.mode && self.relaunch.admits(mode).is_ok() {
            self.spent = true;
            self.mode = mode;
        }
        self.mode
    }

    /// The next mode the agent can be launched in after the one in force,
    /// launched from the next turn.
    pub(crate) fn cycle(&mut self) -> Mode {
        let mut next = self.mode.next();
        while next != self.mode && self.relaunch.admits(next).is_err() {
            next = next.next();
        }
        self.switch(next)
    }

    /// Starts again on `transcript`, the agent's own session `named`: the run
    /// held is closed and the next turn launches one resuming it, or a new one
    /// where `named` is `None`.
    pub(crate) async fn restart(&mut self, transcript: Transcript, named: Option<ExternalRef>) {
        self.retire().await;
        self.transcript = transcript;
        self.named = named;
    }

    /// Closes the run held, if any; the next turn launches another.
    ///
    /// A run that could not be confirmed stopped is dropped all the same: its
    /// host keeps the agent's place taken, so nothing is launched over it.
    pub(crate) async fn retire(&mut self) {
        if let Some(backend) = self.backend.take() {
            let _closed = backend.close().await;
        }
        self.spent = false;
    }

    /// Takes one turn of `prompt` on the agent, recording into `session`.
    ///
    /// # Errors
    ///
    /// [`TurnError::Provider`] where the agent could not be launched or the
    /// turn failed, its text saying where what the agent was doing may or may
    /// not have happened.
    pub(crate) async fn turn(
        &mut self,
        prompt: &str,
        asker: &mut dyn Asker,
        run: &RunContext<'_>,
        session: &Session,
    ) -> Result<Turned, TurnError> {
        if self.spent {
            self.retire().await;
        }
        if self.backend.is_none() {
            let launched = self.relaunch.launch(self.mode, self.named.as_ref()).await;
            self.backend = Some(
                launched
                    .map_err(|failure| transport(self.driver, failure.reason().as_str().into()))?,
            );
        }
        let Some(backend) = self.backend.as_deref() else {
            return Err(transport(self.driver, "the agent was not launched".into()));
        };
        let turn = self.turn;
        self.turn = turn.next();
        // Until the turn's end is heard: a caller that stops waiting leaves
        // the run mid-turn.
        self.spent = true;

        let telling = Telling {
            reporter: run.reporting(),
            turn,
            backend,
            session,
            instance: self.route.instance(),
            told: Mutex::new(Told {
                tally: &mut self.tally,
                transcript: &mut self.transcript,
                named: &mut self.named,
                prompt,
                answer: String::new(),
                cut: false,
                spent: 0,
                began: false,
                failed: None,
                uncertain: None,
                pending: VecDeque::new(),
                waker: None,
            }),
        };
        let mut prompting = backend.prompt(TurnRequest::new(turn, prompt), &telling);
        let raced = run
            .cancel()
            .race(drive(&mut prompting, &telling, Some(asker)))
            .await;
        let ended = if let Some(ended) = raced {
            ended
        } else {
            // Asked to stop: the agent is asked its own way, and the turn ends
            // as the agent ends it, asking nobody anything more.
            let _rung = backend.interrupt();
            drive(&mut prompting, &telling, None).await
        };
        drop(prompting);

        let told = telling
            .told
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        let stop = match &ended {
            Ok(outcome) if !told.cut => Some(stopped(*outcome)),
            Ok(_) | Err(_) => None,
        };
        let uncertain = told.uncertain;
        if told.began {
            if !told.answer.is_empty() {
                let answer = Message::Agent {
                    text: told.answer.into(),
                    calls: Vec::new(),
                    stop,
                    continuation: None,
                };
                session.append(&answer);
                let _pushed = told.transcript.push(answer);
            }
            if let Some(under_way) = &uncertain {
                let _kept =
                    session.uncertain(&Uncertain::new(under_way.as_str(), under_way.incomplete()));
            }
            // A turn whose end was not heard ends here, as one that did not
            // say what it cost.
            told.tally.end(false);
        }
        let failed = told.failed;
        let spent = Spend::new(told.spent);

        let spent_run = ended.is_err() || uncertain.is_some();
        self.spent = false;
        if spent_run {
            self.retire().await;
        }
        match ended {
            Ok(Outcome::Cancelled) if uncertain.is_some() => Ok(Turned::Uncertain(
                RunResult::reported(run, StopReason::Cancelled, spent),
            )),
            Ok(outcome) => Ok(Turned::Ran(RunResult::reported(
                run,
                stopped(outcome),
                spent,
            ))),
            Err(error) => {
                let mut problem = failed.map_or_else(
                    || error.to_string(),
                    |failure| failure.reason().as_str().to_owned(),
                );
                if uncertain.is_some() {
                    problem.push_str(UNKNOWN_EFFECTS);
                }
                Err(transport(self.driver, problem.into()))
            }
        }
    }
}

/// How the runner says a turn that ended so stopped.
const fn stopped(outcome: Outcome) -> StopReason {
    match outcome {
        Outcome::Answered => StopReason::Yielded,
        Outcome::Cancelled => StopReason::Cancelled,
        Outcome::CutShort => StopReason::Unknown,
    }
}

/// A failure of the agent `driver`, as a provider's failure is told.
fn transport(driver: &'static str, problem: Box<str>) -> TurnError {
    TurnError::Provider(ProviderError::Transport {
        provider: driver,
        problem,
    })
}

/// Runs `prompting` to its end, putting each leave the agent asks to `asker`
/// while it runs and answering the agent, or refusing each where there is no
/// asker.
async fn drive(
    prompting: &mut BoxFuture<'_, Result<Outcome, Error>>,
    telling: &Telling<'_>,
    mut asker: Option<&mut dyn Asker>,
) -> Result<Outcome, Error> {
    loop {
        let next = poll_fn(|cx| {
            if let Poll::Ready(ended) = prompting.as_mut().poll(cx) {
                return Poll::Ready(Err(ended));
            }
            telling.asked(cx).map(Ok)
        })
        .await;
        let (reference, pending) = match next {
            Ok(put) => put,
            Err(ended) => return ended,
        };
        let Some(asker) = asker.as_deref_mut() else {
            refuse(telling.backend, &reference);
            continue;
        };
        let decided = {
            let mut asking = asker.pending(&pending);
            poll_fn(|cx| {
                if let Poll::Ready(decision) = asking.as_mut().poll(cx) {
                    return Poll::Ready(Ok(decision));
                }
                prompting.as_mut().poll(cx).map(Err)
            })
            .await
        };
        match decided {
            Ok(decision) => {
                // A decision the agent cannot take is a no, so that nothing it
                // asked is left to run on a word that did not reach it.
                if let Err(Error::Unsupported(_) | Error::Failed(_)) =
                    telling.backend.decide(&reference, decision)
                {
                    refuse(telling.backend, &reference);
                }
            }
            Err(ended) => return ended,
        }
    }
}

/// Refuses `reference`, which nobody will answer otherwise.
fn refuse(backend: &dyn Backend, reference: &PendingRef) {
    let _refused = backend.decide(reference, Decision::Deny);
}

/// The events of one turn, told as the runner tells its own.
struct Telling<'t> {
    /// Where runner events go.
    reporter: Reporter<'t>,
    /// The turn.
    turn: TurnId,
    /// The agent, for leave refused while too many are open.
    backend: &'t dyn Backend,
    /// What the turn is recorded into.
    session: &'t Session,
    /// The instance the session is bound to.
    instance: &'t str,
    /// What the turn has told so far.
    told: Mutex<Told<'t>>,
}

/// What a turn has told so far.
struct Told<'t> {
    /// The session's figures.
    tally: &'t mut Tally,
    /// What a person sees of the conversation.
    transcript: &'t mut Transcript,
    /// The agent's own name for its session.
    named: &'t mut Option<ExternalRef>,
    /// What was asked.
    prompt: &'t str,
    /// The answer so far, bounded.
    answer: String,
    /// Whether the answer was cut.
    cut: bool,
    /// The output tokens the turn reported.
    spent: u64,
    /// Whether the turn began.
    began: bool,
    /// Why it failed, where it did.
    failed: Option<Failure>,
    /// What was under way when it was lost, where it was.
    uncertain: Option<Text>,
    /// Leave asked for and not yet put.
    pending: VecDeque<(PendingRef, Pending)>,
    /// What to wake when leave is asked for.
    waker: Option<Waker>,
}

impl<'t> Telling<'t> {
    fn told(&self) -> MutexGuard<'_, Told<'t>> {
        self.told.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next leave asked for, or a wake once there is one.
    fn asked(&self, cx: &Context<'_>) -> Poll<(PendingRef, Pending)> {
        let mut told = self.told();
        told.pending.pop_front().map_or_else(
            || {
                told.waker = Some(cx.waker().clone());
                Poll::Pending
            },
            Poll::Ready,
        )
    }
}

impl Told<'_> {
    /// `text` added to the answer, within the transcript's ceiling for one
    /// message.
    fn keep(&mut self, text: &Text) {
        let room = CONTINUATION_BYTES.saturating_sub(self.answer.len());
        let mut end = text.as_str().len().min(room);
        while !text.as_str().is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        self.answer
            .push_str(text.as_str().get(..end).unwrap_or_default());
        self.cut |= text.incomplete() || end < text.as_str().len();
    }
}

impl Sink for Telling<'_> {
    fn post(&self, event: Event) {
        let mut told = self.told();
        let posted = match event {
            Event::Opened(named) => {
                if let Ok(named) = ExternalRef::new(self.instance, named.as_str()) {
                    let _kept = self.session.external(&named);
                    *told.named = Some(named);
                }
                None
            }
            Event::Started { .. } => {
                told.began = true;
                let changed = told.tally.begin();
                let asked = Message::User {
                    text: told.prompt.into(),
                    attachments: Box::new([]),
                };
                self.session.append(&asked);
                let _pushed = told.transcript.push(asked);
                drop(told);
                if changed {
                    self.used();
                }
                Some(RunEvent::TurnStarted { turn: self.turn })
            }
            Event::Delta(text) => {
                told.keep(&text);
                Some(RunEvent::Delta {
                    text: text.as_str().into(),
                })
            }
            Event::ToolActivity { tool, summary } => Some(RunEvent::Reported {
                tool: tool.as_str().into(),
                summary: summary.as_str().into(),
            }),
            Event::ToolReport { tool, report } => Some(RunEvent::Reported {
                tool: tool.as_str().into(),
                summary: report.as_str().into(),
            }),
            Event::FileChanged { path } => Some(RunEvent::Reported {
                tool: "file".into(),
                summary: path.as_str().into(),
            }),
            Event::Pending(reference, pending) => {
                if told.pending.len() >= PENDINGS {
                    drop(told);
                    refuse(self.backend, &reference);
                } else {
                    told.pending.push_back((reference, pending));
                    if let Some(waker) = told.waker.take() {
                        waker.wake();
                    }
                }
                None
            }
            Event::Usage(observed) => {
                told.tally.used(observed.value());
                told.spent = told
                    .spent
                    .saturating_add(observed.value().output_tokens().unwrap_or(0));
                Some(RunEvent::Used {
                    totals: told.tally.totals(),
                })
            }
            Event::Finished(outcome) => {
                let changed = told.tally.end(outcome == Outcome::Answered);
                drop(told);
                if changed {
                    self.used();
                }
                Some(RunEvent::TurnFinished {
                    turn: self.turn,
                    stop: stopped(outcome),
                })
            }
            Event::Failed(failure) => {
                let changed = told.tally.end(false);
                told.failed = Some(failure);
                drop(told);
                changed.then(|| self.used_event())
            }
            Event::Uncertain(effects) => {
                told.uncertain = Some(effects.under_way().clone());
                None
            }
            // Always followed by the failure that ends the turn, which is
            // what is told.
            Event::Background(_) | Event::Process(_) => None,
        };
        if let Some(posted) = posted {
            self.reporter.post(posted);
        }
    }
}

impl Telling<'_> {
    /// Posts the session's figures whole.
    fn used(&self) {
        self.reporter.post(self.used_event());
    }

    /// The session's figures, as the event posting them whole.
    fn used_event(&self) -> RunEvent {
        RunEvent::Used {
            totals: self.told().tally.totals(),
        }
    }
}

#[cfg(test)]
mod tests;
