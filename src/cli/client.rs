//! The terminal as one front end of the application.
//!
//! What this loop asks of a conversation it asks the way any front end does:
//! as a [`Request`] of the client contract — the protocol version, what this
//! front end can answer, an identity of its own, and one command — carried
//! out by [`crucible_app::client`]. A consumer with no terminal comes through
//! the same doors with the same values, so there is nothing the keyboard can
//! do to a conversation that it cannot.
//!
//! What comes back is the application's own answer, and the command that
//! asked draws it. The sentences and rows stay on this side: a screen is made
//! from the whole value, never from the copy the contract cuts to its
//! ceilings, and nothing drawn is ever serialized.
//!
//! An account operation stops to ask which way to go on, and the terminal is
//! asked as any front end is: [`Answering`] puts each question through the
//! application's [`Front`], and the drawing thread, which polls the operation
//! itself, takes it off a channel, draws it and sends the answer back. The
//! panel is drawn from the question whole, as the host holds it.
//!
//! A terminal has somebody at it and a screen, so it claims every capability,
//! but in a connect, which claims every one but [`Capability::Install`]: a
//! client that claims it can be offered a move on an agent in place of the
//! connect, and only the sender that carries the move out may claim it. That
//! is `/login`'s agent connect, which names its claim through
//! [`Terms::operate_claiming`] beside the carrying out; every other connect,
//! `/logout`'s among them, is asked for with the claim the command decides.
//!
//! A move is carried out at the agents door by [`Terms::carry`], on a thread
//! of its own, since a download goes on while the drawing thread draws how
//! far it has got. The door says that, and the link a sign-in waits on, into
//! a place that keeps only the latest of each, and puts its questions on the
//! same channel an account operation does; only the drawing thread draws any
//! of it. A front end that can draw a sign-in run as a command is lent a channel
//! too: what the command says goes into [`Heard`], which keeps its own ceiling
//! of lines, and what is typed goes back over a short queue, the door's thread
//! only ever reporting. A sign-in that needs a real terminal is lent this
//! process's terminal instead, through [`Handover`]: the drawing thread stops
//! while the child has it and paints every row again once it is back. Where
//! output is redirected nothing is lent, and the door refuses that method
//! before it runs it.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::Poll;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crucible_app::Conversation;
use crucible_app::catalog_fetch::{RegistrySource, Source};
use crucible_app::client::agents::{
    self as door, AgentsError, HandoverFailed, SaidLine, Shelf, SignInChannel, SignInTerminal,
    Typing, Watching,
};
use crucible_app::client::{
    Answered, Confirmation, Desk, Front, Performed, Questions, Shown, account, asked, asking,
    interrupt, keep, perform,
};
use crucible_app::content_use::Warned;
use crucible_app::launching::{Launcher, Steps};
use crucible_app::providers::Providers;
use crucible_client_api::{
    Capabilities, Capability, Command, Correlation, Decision, ErrorCode, InstallProgress, Name,
    Outcome, Pending, Refusal, Request, Said, Theme,
};
use crucible_runtime::{BoxFuture, Cancel};
#[cfg(unix)]
pub(crate) use crucible_tui::Suspend;

/// A stand-in for the terminal handover where there is no `Suspend` to make
/// one from: no platform without a controlling terminal and process groups
/// runs a sign-in at a real one, so nothing is ever handed over.
#[cfg(not(unix))]
#[derive(Debug)]
pub(crate) struct Suspend;

// The stand-in keeps `Suspend`'s call shape on a platform with no terminal to
// hand over, so no call site needs a `cfg`, and it holds no state to read. A
// `resumed` that cannot fail is still written as the `Result` its Unix twin
// returns, so the caller's `let _ =` is the same on every platform.
#[allow(clippy::unused_self, clippy::unnecessary_wraps)]
#[cfg(not(unix))]
impl Suspend {
    pub(crate) fn holding(_: crucible_tui::Held, _: impl Fn() + Send + Sync + 'static) -> Self {
        Self
    }

    pub(crate) fn handed(&self, _: u32) -> Result<(), HandoverFailed> {
        Err(HandoverFailed::new(
            "this platform has no terminal to hand over",
        ))
    }

    pub(crate) fn resumed(&self) -> Result<(), HandoverFailed> {
        Ok(())
    }
}

use super::converse::Terms;

/// What is told as requests, answers, pending actions and decisions cross
/// between this front end and the application.
///
/// Nothing that ships listens: a terminal draws what it is answered and keeps
/// no record of it, so [`Client::new`] holds no witness and every crossing
/// costs one branch. It is here so that what crosses can be held beside what a
/// client with no terminal was answered by the code that ships, with nothing
/// compiled in or out to let it.
pub(crate) trait Witness: std::fmt::Debug + Send + Sync {
    /// `request` was answered `outcome`, and `conversation` stands as it does
    /// once it had been.
    fn answered(&self, request: &Request, conversation: &Conversation, outcome: Outcome);

    /// `request`, which needs no conversation, was answered `outcome`.
    fn apart(&self, request: &Request, outcome: Outcome);

    /// A turn stopped on `pending`, and the terminal was put it.
    fn put(&self, pending: &Pending);

    /// What the terminal said about the action it was put.
    fn decided(&self, decision: &Decision);
}

/// What the terminal claims in a request asking for `command`, where its
/// sender names no claim of its own: every capability, but in a connect,
/// every one but [`Capability::Install`].
///
/// A client that claims install can be offered a move on an agent in place of
/// a connect, and only a sender that carries the move out claims it, by
/// naming its claim through [`Client::claiming`]. Every connect asked for
/// here, whichever command sends it, claims the same. Every command is named,
/// so a new one decides what it claims.
const fn claimed(command: &Command) -> Capabilities {
    match command {
        Command::Connect { .. } => Capabilities::every().without(Capability::Install),
        Command::Prompt(_)
        | Command::Compact
        | Command::Cancel
        | Command::Decide(_)
        | Command::Answer { .. }
        | Command::Clear
        | Command::Resume(_)
        | Command::SelectModel { .. }
        | Command::SetEffort(_)
        | Command::SetSpeed(_)
        | Command::SetMode(_)
        | Command::CycleMode
        | Command::Login { .. }
        | Command::Logout { .. }
        | Command::Disconnect { .. }
        | Command::RemoveIntegration { .. }
        | Command::SignOut { .. }
        | Command::InspectCache
        | Command::CleanCache
        | Command::Sandbox { .. }
        | Command::Theme(_)
        | Command::Setting { .. }
        | Command::Help
        | Command::ReleaseNotes { .. }
        | Command::Context
        | Command::Usage
        | Command::AskLimits
        | Command::AgentCatalog
        | Command::InstallAgent { .. }
        | Command::AddAgent { .. }
        | Command::UpdateAgent { .. }
        | Command::RollbackAgent { .. }
        | Command::RemoveAgent { .. }
        | Command::VerifyAgent { .. }
        | Command::SignInAgent { .. }
        | Command::Exit => Capabilities::every(),
    }
}

/// The requests this process has made, numbered as they are made.
///
/// Cloned to the worker a turn runs on: both sides number from one count, so
/// an identity names one request for as long as the process runs.
#[derive(Debug, Clone, Default)]
pub(crate) struct Client {
    sent: Arc<AtomicU64>,
    /// Who is told what crosses, where anybody is.
    witness: Option<Arc<dyn Witness>>,
}

impl Client {
    /// A client that has asked nothing yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The next request, asking for `command` and claiming what [`claimed`]
    /// says the terminal claims for it.
    pub(crate) fn asking(&self, command: Command) -> Request {
        let claims = claimed(&command);
        self.claiming(command, claims)
    }

    /// The next request, asking for `command` and claiming `claims`: what a
    /// sender that carries out what the claim lets it be offered names.
    pub(crate) fn claiming(&self, command: Command, claims: Capabilities) -> Request {
        let number = self.sent.fetch_add(1, Ordering::Relaxed).saturating_add(1);

        Request::new(claims, Correlation::new(number), command)
    }

    /// Asks the turn `cancel` belongs to to stop.
    ///
    /// What the application answers is that it heard, and the turn says the
    /// rest in its own outcome, so there is nothing here to draw.
    pub(crate) fn interrupt(&self, cancel: &Cancel) {
        let request = self.asking(Command::Cancel);
        let heard = interrupt(&request, cancel);

        if let Some(witness) = &self.witness {
            witness.apart(&request, heard);
        }
    }

    /// Tells whoever is listening what `request` was answered with. The
    /// outcome is made only where somebody is.
    pub(crate) fn answered(
        &self,
        request: &Request,
        conversation: &Conversation,
        outcome: impl FnOnce() -> Outcome,
    ) {
        if let Some(witness) = &self.witness {
            witness.answered(request, conversation, outcome());
        }
    }

    /// Tells whoever is listening that the terminal was put `pending`.
    pub(crate) fn put(&self, pending: &Pending) {
        if let Some(witness) = &self.witness {
            witness.put(pending);
        }
    }

    /// Tells whoever is listening what the terminal said about a pending
    /// action.
    pub(crate) fn decided(&self, decision: &Decision) {
        if let Some(witness) = &self.witness {
            witness.decided(decision);
        }
    }
}

impl Terms {
    /// Asks the application for `command`, against the providers in force as
    /// it is asked, and hands back what came of it for the caller to draw.
    ///
    /// Waited for here, on the runtime the turns run on: the drawing thread
    /// asks, and is never inside that runtime, so it can wait on it.
    pub(crate) fn perform(&self, conversation: &mut Conversation, command: Command) -> Performed {
        let request = self.client.asking(command);
        self.performed(conversation, &request)
    }

    /// Carries out `request` through the door that answers it whole, waiting
    /// for it, and tells whoever is listening what came of it.
    fn performed(&self, conversation: &mut Conversation, request: &Request) -> Performed {
        let (providers, settings) = (self.providers.snapshot(), self.reading());
        let desk = self.desk(&providers, &settings);
        let performed = self.runtime.block_on(perform(conversation, request, &desk));

        self.client
            .answered(request, conversation, || performed.outcome());

        performed
    }
}

/// A question an account operation stopped on, whole, as the host holds it.
#[derive(Debug, Clone)]
pub(crate) enum Question {
    /// Which way the operation goes on.
    Account(Confirmation),
    /// The route an installed agent would be started on, whose vendor uses
    /// what is sent.
    Warning(Warned),
}

/// What the drawing thread answered a [`Question`] with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reply {
    /// The choice at this place in the question's list.
    Took(usize),
    /// Yes to the route.
    Yes,
    /// No to it.
    No,
}

/// A question on its way to the drawing thread, and where its answer goes:
/// nothing, where it was not answered.
struct Put {
    question: Question,
    reply: oneshot::Sender<Option<Reply>>,
}

/// What [`Terms::operate`] is waiting for next.
enum Step {
    /// The operation is over.
    Done(Performed),
    /// It stopped on a question.
    Asked(Put),
}

/// The front an account operation is asked through: each question goes to
/// the thread polling the operation, which draws it and answers.
struct Answering {
    to: mpsc::Sender<Put>,
    client: Client,
}

impl Front for Answering {
    /// Hands the question to the drawing thread and awaits its answer. Only
    /// an account operation's questions, an install's, and the route either
    /// would start an agent on are put here; anything else is answered by
    /// nobody.
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            let question = match shown {
                // An install is asked the way an account operation is: which
                // way it goes on, from choices the question names.
                Shown::Account(confirmation) | Shown::Install(confirmation) => {
                    Question::Account(confirmation.clone())
                }
                Shown::Warning(warned) => Question::Warning(warned.clone()),
                Shown::Call { .. } | Shown::Questions(_) | Shown::External(_) => return None,
            };

            self.client.put(pending);

            let (reply, hear) = oneshot::channel();
            self.to.send(Put { question, reply }).await.ok()?;
            let id = pending.id();
            let decision = match (hear.await.ok()??, pending) {
                // Named as the pending action names it, which is what it is
                // held against.
                (
                    Reply::Took(at),
                    Pending::Account { choices, .. } | Pending::Install { choices, .. },
                ) => Decision::Picked {
                    id,
                    choice: Said::new(choices.get(at)?.name.as_str()).ok()?,
                },
                (Reply::Took(_), _) => return None,
                (Reply::Yes, _) => Decision::Accepted { id },
                (Reply::No, _) => Decision::Declined { id },
            };

            self.client.decided(&decision);

            Some(decision)
        })
    }

    /// Nothing to say: every decision made above names the action it was put
    /// and answers it as it asked.
    fn refused(&mut self, _: Refusal) {}

    /// The panel is drawn from the question as the host holds it, never from
    /// the cut words a pending action carries.
    fn draws_whole(&self) -> bool {
        true
    }
}

impl Terms {
    /// Asks the application for the account operation `command`, answering
    /// each question it stops on with `answer`, here on the drawing thread,
    /// and hands back what came of it for the caller to draw.
    ///
    /// The operation is polled here, between questions, on the runtime the
    /// turns run on, and stops being polled while a question is drawn: what
    /// it does, it does while nobody is being asked anything.
    ///
    /// # Errors
    ///
    /// Whatever `answer` fails with; the operation is dropped where it stood.
    pub(crate) fn operate<E>(
        &self,
        conversation: &mut Conversation,
        command: Command,
        answer: impl FnMut(&Question) -> Result<Option<Reply>, E>,
    ) -> Result<Performed, E> {
        let claims = claimed(&command);
        self.operate_claiming(conversation, command, claims, answer)
    }

    /// [`Terms::operate`], with the request claiming `claims`: what a sender
    /// that carries out what the claim lets it be offered names.
    ///
    /// # Errors
    ///
    /// As [`Terms::operate`].
    pub(crate) fn operate_claiming<E>(
        &self,
        conversation: &mut Conversation,
        command: Command,
        claims: Capabilities,
        mut answer: impl FnMut(&Question) -> Result<Option<Reply>, E>,
    ) -> Result<Performed, E> {
        let request = self.client.claiming(command, claims);
        let (providers, settings) = (self.providers.snapshot(), self.reading());
        let desk = self.desk(&providers, &settings);
        // One question is out at a time: the operation awaits each answer.
        let (to, mut from) = mpsc::channel(1);
        let mut front = Answering {
            to,
            client: self.client.clone(),
        };

        let performed = {
            let mut operation = std::pin::pin!(account(conversation, &request, &desk, &mut front));
            loop {
                let step = self.runtime.block_on(std::future::poll_fn(|context| {
                    if let Poll::Ready(performed) = operation.as_mut().poll(context) {
                        return Poll::Ready(Step::Done(performed));
                    }
                    // The front that asks holds the only sender and outlives
                    // this loop, so the channel closing never ends the wait:
                    // the operation does.
                    match from.poll_recv(context) {
                        Poll::Ready(Some(put)) => Poll::Ready(Step::Asked(put)),
                        Poll::Ready(None) | Poll::Pending => Poll::Pending,
                    }
                }));
                match step {
                    Step::Done(performed) => break performed,
                    Step::Asked(put) => {
                        let replied = answer(&put.question)?;
                        // An operation that stopped waiting has nobody to tell.
                        let _ = put.reply.send(replied);
                    }
                }
            }
        };

        self.client
            .answered(&request, conversation, || performed.outcome());

        Ok(performed)
    }
}

/// What the agents door has said while it carries a move out, as the drawing
/// thread draws it: only the latest of each, so nothing kept grows with what
/// it does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Watched {
    /// How far a download has got, where one has begun.
    pub(crate) progress: Option<InstallProgress>,
    /// The sign-in link the agent printed last, within the door's ceiling,
    /// and whether that ceiling cut it.
    pub(crate) link: Option<(String, bool)>,
}

/// How many lines of what a sign-in command says are kept: more than a window
/// shows, and a ceiling of the panel's own beside the door's on what it carries.
pub(crate) const HEARD_LIMIT: usize = 256;

/// How many typed lines wait for the door: a person cannot get far ahead of a
/// command that reads, and one that does not read is told to them at the panel.
const TYPED_LINES: usize = 16;

/// What a sign-in command has said, as the door carried it: its latest
/// [`HEARD_LIMIT`] lines, the oldest going first, and how many went. It is the
/// only place they are kept: the drawing thread reads them from here to draw.
#[derive(Debug, Default)]
pub(crate) struct Heard {
    lines: VecDeque<SaidLine>,
    told: u64,
}

impl Heard {
    /// Takes one more line, letting the oldest go where that makes too many.
    pub(crate) fn take(&mut self, line: SaidLine) {
        if self.lines.len() >= HEARD_LIMIT {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
        self.told = self.told.saturating_add(1);
    }

    /// The lines kept, oldest first.
    pub(crate) fn lines(&self) -> impl Iterator<Item = &SaidLine> {
        self.lines.iter()
    }

    /// How many lines have been said in all, which changes whenever one is.
    pub(crate) const fn told(&self) -> u64 {
        self.told
    }

    /// How many of them are no longer kept.
    pub(crate) fn dropped(&self) -> u64 {
        self.told
            .saturating_sub(u64::try_from(self.lines.len()).unwrap_or(u64::MAX))
    }
}

/// The channel a sign-in run as a command is lent. It only reports: the door's
/// thread calls it, and only the drawing thread draws.
struct Lent {
    heard: Arc<Mutex<Heard>>,
    typed: tokio::sync::Mutex<mpsc::Receiver<Typing>>,
    listening: Arc<AtomicBool>,
    handover: Arc<Handover>,
}

impl SignInChannel for Lent {
    fn said(&self, line: SaidLine) {
        self.heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(line);
    }

    /// Being asked for a line is what says the command is running. Dropped
    /// before it answers, it loses no line: taking the lock and receiving both
    /// give nothing up when they are dropped.
    fn line(&self) -> BoxFuture<'_, Option<Typing>> {
        Box::pin(async move {
            self.listening.store(true, Ordering::Release);
            self.typed.lock().await.recv().await
        })
    }
}

impl SignInTerminal for Lent {
    fn handed(&self, pid: u32) -> Result<(), HandoverFailed> {
        self.handover.handed(pid)
    }

    fn resumed(&self) {
        self.handover.resumed();
    }
}

/// How long the door waits for the drawing thread to say it has left its read
/// loop before it takes the terminal as one that cannot be given.
///
/// The drawing thread looks at a watching panel once a beat, so this is many
/// beats: a wait that long means it is not coming back to the loop at all, and
/// a child handed a terminal nobody will stop reading is worse than a refusal.
const LEFT_BOUND: Duration = Duration::from_secs(2);

/// The terminal a sign-in at a real one is lent, and the flags the drawing
/// thread reads while it is out on loan.
///
/// The door's thread calls this; only the terminal sequences themselves leave
/// here, and the drawing thread is what puts a frame on the screen again when
/// [`SignInTerminal::resumed`] says it is back. A handover is not made until
/// the drawing thread has acknowledged that it left the loop that reads the
/// terminal, so the leave sequences cannot race a read or a frame.
struct Handover {
    /// The terminal modes, where this session has a terminal to give.
    suspend: Mutex<Option<Suspend>>,
    /// Whether a child has the terminal, so the panel waits rather than draws
    /// over it.
    handed: Arc<AtomicBool>,
    /// Whether the terminal has come back and the next frame must paint every
    /// row.
    repaint: AtomicBool,
    /// Whether the drawing thread has said it left its read loop for the
    /// handover being made now.
    left: Mutex<bool>,
    /// Ring when it does.
    left_changed: Condvar,
}

impl Handover {
    fn new(suspend: Option<Suspend>) -> Self {
        Self {
            suspend: Mutex::new(suspend),
            handed: Arc::new(AtomicBool::new(false)),
            repaint: AtomicBool::new(false),
            left: Mutex::new(false),
            left_changed: Condvar::new(),
        }
    }

    /// Whether the child holds the terminal right now, or is waiting for it.
    fn suspended(&self) -> bool {
        self.handed.load(Ordering::Acquire)
    }

    /// Says the drawing thread has left its read loop: the door may write the
    /// leave sequences and give the child the terminal.
    fn left(&self) {
        *self.left.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.left_changed.notify_all();
    }

    /// Waits, bounded, until the drawing thread has left its read loop.
    fn wait_left(&self) -> Result<(), HandoverFailed> {
        let left = self.left.lock().unwrap_or_else(PoisonError::into_inner);
        let (left, waited) = self
            .left_changed
            .wait_timeout_while(left, LEFT_BOUND, |left| !*left)
            .unwrap_or_else(PoisonError::into_inner);
        if waited.timed_out() && !*left {
            return Err(HandoverFailed::new(
                "the drawing thread did not leave the terminal in time",
            ));
        }
        Ok(())
    }

    /// Takes the request to repaint every row, if the terminal has come back
    /// since it was last taken.
    fn take_repaint(&self) -> bool {
        self.repaint.swap(false, Ordering::AcqRel)
    }
}

impl SignInTerminal for Handover {
    fn handed(&self, pid: u32) -> Result<(), HandoverFailed> {
        // Set first: the drawing thread must not start a frame between this
        // and the child taking the screen, and it must leave the loop that
        // reads the terminal before the leave sequences are written.
        *self.left.lock().unwrap_or_else(PoisonError::into_inner) = false;
        self.handed.store(true, Ordering::Release);
        self.wait_left()?;
        let suspend = self.suspend.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(suspend) = suspend.as_ref() {
            suspend
                .handed(pid)
                .map_err(|why| HandoverFailed::new(why.to_string()))?;
        }
        Ok(())
    }

    fn resumed(&self) {
        let suspend = self.suspend.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(suspend) = suspend.as_ref() {
            let _ = suspend.resumed();
        }
        self.handed.store(false, Ordering::Release);
        self.repaint.store(true, Ordering::Release);
        *self.left.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }
}

/// A move being carried out at the agents door, as the drawing thread stands
/// over it between the questions it puts.
pub(crate) struct Carrying<'a> {
    /// Where the door's questions arrive.
    from: mpsc::Receiver<Put>,
    /// A question that arrived while the drawing thread was standing, and is
    /// answered once it stops.
    held: Option<Put>,
    /// What the door has said so far.
    told: &'a Mutex<Watched>,
    /// What gives a download up.
    cancel: &'a Cancel,
    /// Raised once the door has answered, however it did.
    over: &'a AtomicBool,
    /// What a sign-in command has said.
    heard: Arc<Mutex<Heard>>,
    /// Where what is typed for it goes; dropped, its input closes.
    typing: mpsc::Sender<Typing>,
    /// Raised once the door asks for a line, which it does with the command
    /// running.
    listening: Arc<AtomicBool>,
    /// The terminal, where this session has one to lend a sign-in.
    handover: Arc<Handover>,
}

impl Carrying<'_> {
    /// Whether the door is still at work with nothing to ask: what a panel
    /// standing over it stands while.
    pub(crate) fn working(&mut self) -> bool {
        if self.held.is_none() {
            self.held = self.from.try_recv().ok();
        }
        self.held.is_none() && !self.over.load(Ordering::Acquire)
    }

    /// What the door has said so far.
    pub(crate) fn watched(&self) -> Watched {
        self.told
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What gives the download up, which then commits nothing, or stops a
    /// sign-in command.
    pub(crate) const fn cancel(&self) -> &Cancel {
        self.cancel
    }

    /// Whether a sign-in command is running, wanting what is typed for it.
    pub(crate) fn listening(&self) -> bool {
        self.listening.load(Ordering::Acquire)
    }

    /// Whether a sign-in at a real terminal has this terminal right now, so a
    /// panel must wait rather than draw over the child.
    pub(crate) fn handed(&self) -> bool {
        self.handover.suspended()
    }

    /// Whether the terminal has come back since the last frame, which is what
    /// asks for every row to be painted again.
    pub(crate) fn take_repaint(&self) -> bool {
        self.handover.take_repaint()
    }

    /// Says the drawing thread has left the loop that reads the terminal, so
    /// the door may write the leave sequences and give the child the screen.
    pub(crate) fn left(&self) {
        self.handover.left();
    }

    /// What a sign-in command has said.
    pub(crate) fn heard(&self) -> Arc<Mutex<Heard>> {
        Arc::clone(&self.heard)
    }

    /// Where what is typed for a sign-in command goes.
    pub(crate) fn typing(&self) -> mpsc::Sender<Typing> {
        self.typing.clone()
    }
}

#[cfg(test)]
impl<'a> Carrying<'a> {
    /// A move with nothing said and no question out, for a case that stands a
    /// panel over it and decides for itself when a child is handed the
    /// terminal.
    pub(crate) fn for_test(
        told: &'a Mutex<Watched>,
        cancel: &'a Cancel,
        over: &'a AtomicBool,
    ) -> Self {
        let (typing, _typed) = mpsc::channel(TYPED_LINES);
        Self {
            from: mpsc::channel(1).1,
            held: None,
            told,
            cancel,
            over,
            heard: Arc::new(Mutex::new(Heard::default())),
            typing,
            listening: Arc::new(AtomicBool::new(false)),
            handover: Arc::new(Handover::new(None)),
        }
    }

    /// The flag a case sets to say a child has been handed the terminal.
    pub(crate) fn handed_for_test(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.handover.handed)
    }

    /// Whether the drawing thread has said it left its read loop.
    pub(crate) fn acknowledged_for_test(&self) -> bool {
        *self
            .handover
            .left
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Raises the flag it holds when dropped, so the door's thread says it is
/// over however it ends, a panic among the ways.
struct Over<'a>(&'a AtomicBool);

impl Drop for Over<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Terms {
    /// Carries the move `command` out at the agents door, lent the shelf this
    /// conversation stands on, and hands back what the door answered for the
    /// caller to draw.
    ///
    /// The door runs on a thread of its own, on the runtime the turns run on,
    /// so a download goes on while this, the drawing thread, stands `stand`
    /// over it: `overseer` stands what [`Carrying::watched`] says for as
    /// long as [`Carrying::working`] holds, and cancels through
    /// [`Carrying::cancel`]. Each question the door stops on it answers here,
    /// as [`Terms::operate`] answers an account operation's.
    ///
    /// # Errors
    ///
    /// Whatever `overseer` fails with. The download is cancelled, any
    /// question out is left unanswered, and the door is waited for before
    /// this returns.
    pub(crate) fn carry<O: Overseer>(
        &self,
        conversation: &Conversation,
        command: Command,
        overseer: &mut O,
    ) -> Result<Result<door::Outcome, AgentsError>, O::Error> {
        let request = self.client.asking(command);
        let carried = match crucible_config::Home::find(&|name| std::env::var_os(name)) {
            Err(homeless) => Ok(Err(AgentsError::Config(homeless))),
            Ok(home) => {
                let steps = conversation.steps();
                let (request, runtime, home) = (&request, &self.runtime, &home);
                overseen(&self.client, overseer, move |front, watching| {
                    let carried = |smoke: Option<&Launcher<'_>>| {
                        let lent = shelf(home, smoke, steps);
                        runtime.block_on(door::run(request, &lent, front, watching))
                    };
                    match steps {
                        Some(steps) => steps.lending(|launcher| carried(Some(launcher))),
                        None => carried(None),
                    }
                })
            }
        }?;

        if let Some(witness) = &self.client.witness {
            match &carried {
                Ok(door::Outcome::Refused(refusal)) => {
                    witness.apart(&request, Outcome::Refused(*refusal));
                }
                Err(refused) => witness.apart(&request, Outcome::Refused(coded(refused).into())),
                // The contract has no outcome for what the door did; it is
                // the door's own to say, and the terminal draws it whole.
                Ok(_) => {}
            }
        }

        Ok(carried)
    }
}

/// Runs `door`, the half of [`Terms::carry`] that is the agents door, on a
/// thread of its own, lent the front that puts each question it stops on to
/// `overseer` and the watching `overseer` stands over, and stands `overseer`
/// over it from this, the drawing thread, until it is over.
///
/// # Errors
///
/// Whatever `overseer` fails with. The download is cancelled, any question
/// out is left unanswered, and the door is waited for before this returns.
fn overseen<O: Overseer, R: Send>(
    client: &Client,
    overseer: &mut O,
    door: impl FnOnce(&mut Answering, Watching<'_>) -> R + Send,
) -> Result<R, O::Error> {
    let told = Mutex::new(Watched::default());
    let cancel = Cancel::new();
    let over = AtomicBool::new(false);
    // One question is out at a time: the door awaits each answer.
    let (to, from) = mpsc::channel(1);
    let front = Answering {
        to,
        client: client.clone(),
    };
    let heard = Arc::new(Mutex::new(Heard::default()));
    let listening = Arc::new(AtomicBool::new(false));
    let (typing, typed) = mpsc::channel(TYPED_LINES);
    let suspended = overseer.suspend();
    let lends_terminal = suspended.is_some();
    let handover = Arc::new(Handover::new(suspended));
    let lent = Lent {
        heard: Arc::clone(&heard),
        typed: tokio::sync::Mutex::new(typed),
        listening: Arc::clone(&listening),
        handover: Arc::clone(&handover),
    };
    let lends = overseer.carries_lines();
    let lent_ = &lent;
    let (told_, cancel_, over_) = (&told, &cancel, &over);
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let _over = Over(over_);
            let mut front = front;
            let mut progress = |progress: InstallProgress| {
                told_
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .progress = Some(progress);
            };
            let watching = Watching {
                progress: &mut progress,
                cancel: cancel_,
                link: &|link| {
                    let link = (link.as_str().to_owned(), link.incomplete());
                    told_.lock().unwrap_or_else(PoisonError::into_inner).link = Some(link);
                },
                sign_in: lends.then_some(lent_ as &dyn SignInChannel),
                terminal: lends_terminal.then_some(lent_ as &dyn SignInTerminal),
            };
            door(&mut front, watching)
        });
        let mut carrying = Carrying {
            from,
            held: None,
            told: told_,
            cancel: cancel_,
            over: over_,
            heard,
            typing,
            listening,
            handover,
        };
        let stood = stood(overseer, &mut carrying);
        let repaint = stood.is_ok() && carrying.take_repaint();
        if stood.is_err() {
            cancel_.request();
        }
        // Nobody is left to answer: a question still out, or put from here
        // on, is answered by nobody.
        drop(carrying);
        let carried = worker
            .join()
            .unwrap_or_else(|panicked| std::panic::resume_unwind(panicked));
        if repaint {
            overseer.repaint()?;
        }
        stood.map(|()| carried)
    })
}

/// Who stands over a move [`Terms::carry`] carries out: it answers each
/// question the door stops on, and stands a panel over the door between them.
/// It is on the drawing thread, and nothing else draws.
pub(crate) trait Overseer {
    /// What it fails with, which gives the move up.
    type Error;

    /// Answers `question`, the way a question an account operation stops on
    /// is answered: nothing where it had no room to stand.
    ///
    /// # Errors
    ///
    /// Whatever drawing or reading the answer failed with.
    fn answer(&mut self, question: &Question) -> Result<Option<Reply>, Self::Error>;

    /// Stands a panel over the door `carrying` watches, for as long as
    /// [`Carrying::working`] holds or less.
    ///
    /// # Errors
    ///
    /// Whatever drawing the panel or reading a key failed with.
    fn stand(&mut self, carrying: &mut Carrying<'_>) -> Result<(), Self::Error>;

    /// Whether it can stand a panel over a sign-in run as a command, showing
    /// what the command says and taking what is typed for it. Where it cannot,
    /// the door is lent no channel and refuses that method before running it.
    fn carries_lines(&self) -> bool {
        false
    }

    /// This session's terminal modes, where it has a terminal to give a sign-in
    /// command that needs a real one. Where it does not, the door is lent no
    /// handover and refuses that method before running it.
    fn suspend(&self) -> Option<Suspend> {
        None
    }

    /// Paints every row again after a child had the terminal.
    ///
    /// # Errors
    ///
    /// Whatever drawing the frame failed with.
    fn repaint(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Stands `overseer` over the door `carrying` watches until it is over, and
/// has it answer each question the door stops on.
fn stood<O: Overseer>(overseer: &mut O, carrying: &mut Carrying<'_>) -> Result<(), O::Error> {
    loop {
        if carrying.held.is_none() {
            carrying.held = carrying.from.try_recv().ok();
        }
        if let Some(put) = carrying.held.take() {
            let replied = overseer.answer(&put.question)?;
            // A door that stopped waiting has nobody to tell.
            let _ = put.reply.send(replied);
            continue;
        }
        if carrying.over.load(Ordering::Acquire) {
            return Ok(());
        }
        overseer.stand(carrying)?;
    }
}

/// The contract's code for `refused`, a refusal at the agents door.
///
/// Written out by hand, every variant named, so a new one decides its code.
/// [`ErrorCode::Busy`] is a run's alone: a sign-in that would start the agent
/// beside a live run, or one not confirmed stopped, is refused until the run
/// has ended, and nothing else here is about now.
pub(crate) const fn coded(refused: &AgentsError) -> ErrorCode {
    match refused {
        AgentsError::Running(_) | AgentsError::Unstopped(_) => ErrorCode::Busy,
        // What the request named is not one there is, or not one this can do.
        AgentsError::Unoffered { .. }
        | AgentsError::Unregistered { .. }
        | AgentsError::Unlisted(_)
        | AgentsError::NoInstance(_)
        | AgentsError::Unsigned(_)
        | AgentsError::NoMethod { .. }
        | AgentsError::Keyless(_)
        | AgentsError::Unlent(_)
        | AgentsError::Typed => ErrorCode::InvalidArgument,
        AgentsError::Config(_)
        | AgentsError::UnreadInstall { .. }
        | AgentsError::UnreadUpdate { .. }
        | AgentsError::Unreadable { .. }
        | AgentsError::NotInstalled(_)
        | AgentsError::NoPrevious(_)
        | AgentsError::InUse { .. }
        | AgentsError::Unsupported(_)
        | AgentsError::Program(_)
        | AgentsError::Kept { .. }
        | AgentsError::TooLarge
        | AgentsError::TooManyFiles { .. }
        | AgentsError::Store(_)
        | AgentsError::Download(_)
        | AgentsError::Unpacked(_)
        | AgentsError::Probe { .. }
        | AgentsError::Stub(_)
        | AgentsError::FileListUnfetched(_)
        | AgentsError::FileListUnread(_)
        | AgentsError::Remember(_)
        | AgentsError::Unadded { .. }
        | AgentsError::Smoke { .. }
        | AgentsError::Script { .. }
        | AgentsError::Unclouded { .. }
        | AgentsError::Unlocated(_)
        | AgentsError::Conversation(_)
        | AgentsError::Unnamed(_)
        | AgentsError::Unstored { .. }
        | AgentsError::Credentials(_)
        | AgentsError::Unprofiled(_)
        | AgentsError::Unlaunched(_)
        | AgentsError::HoldUnkept(_)
        | AgentsError::Piped(_)
        | AgentsError::Unconfined(_)
        | AgentsError::Unverified(_) => ErrorCode::Failed,
    }
}

/// A plan being asked how much of its limits is used, on the runtime, while
/// the drawing thread goes on drawing and reading keys.
///
/// It ends by [`Terms::asked`], which takes the answer back, or by
/// [`Terms::abandon`], which gives the question up and answers the request
/// with what is known. A panel closed with the question out ends it one of
/// those two ways. Merely dropped, the question is stopped and nothing more:
/// its request is never answered. However it ends, it counts toward the
/// minute, so the plan is not asked again until a minute has passed.
pub(crate) struct Out {
    request: Request,
    provider: &'static str,
    answer: tokio::task::JoinHandle<Answered>,
}

impl Out {
    /// The registry name of the provider whose plan is being asked.
    pub(crate) const fn provider(&self) -> &'static str {
        self.provider
    }

    /// Whether the plan has answered, or the question has otherwise ended.
    pub(crate) fn ended(&self) -> bool {
        self.answer.is_finished()
    }
}

impl Drop for Out {
    fn drop(&mut self) {
        self.answer.abort();
    }
}

impl Terms {
    /// Asks the plan of the provider `conversation` is asking how much of its
    /// limits is used, where the application asks it at all, and hands back
    /// the question still out. Nothing is waited for here.
    pub(crate) fn ask_limits(&self, conversation: &mut Conversation) -> Option<Out> {
        let request = self.client.asking(Command::AskLimits);
        let Ok(Some(question)) = asking(conversation, &request, std::time::Instant::now()) else {
            // Nothing was sent; the request is answered with what is known,
            // through the door that answers it whole. Whatever kept it from
            // being put still holds there, so that door sends nothing either.
            self.performed(conversation, &request);
            return None;
        };
        Some(Out {
            provider: question.provider(),
            answer: self.runtime.spawn(question.answered()),
            request,
        })
    }

    /// Takes what the plan `out` asked answered back to `conversation`, and
    /// hands back the usage as it now stands for the caller to draw. Waits for
    /// the answer where it has not come yet, so a caller that must not wait
    /// asks [`Out::ended`] first.
    pub(crate) fn asked(&self, conversation: &mut Conversation, mut out: Out) -> Performed {
        let performed = match self.runtime.block_on(&mut out.answer) {
            Ok(answered) => asked(conversation, answered),
            // The question panicked or was abandoned: nothing was learned. It
            // was put moments ago, so the door that answers it whole sends
            // nothing and answers with what is known.
            Err(_) => return self.performed(conversation, &out.request),
        };
        self.client
            .answered(&out.request, conversation, || performed.outcome());
        performed
    }

    /// Gives up the question `out` before its answer came: the request is
    /// closed and answered with what is known. The question still counts
    /// toward the minute, so the next opening within it asks nothing.
    pub(crate) fn abandon(&self, conversation: &mut Conversation, out: Out) {
        out.answer.abort();
        // The question counts as put moments ago, so the door that answers it
        // whole sends nothing.
        self.performed(conversation, &out.request);
        drop(out);
    }
}

impl Terms {
    /// Asks for the command `asking` makes of `provider`, a name out of the
    /// registry. One the contract would not carry is refused here, as it would
    /// be on arriving.
    pub(crate) fn perform_naming(
        &self,
        conversation: &mut Conversation,
        provider: &str,
        asking: impl FnOnce(Name) -> Command,
    ) -> Performed {
        match Name::new(provider) {
            Ok(provider) => self.perform(conversation, asking(provider)),
            Err(refusal) => Performed::Refused(refusal),
        }
    }

    /// Asks for what is written down about this machine and is about no
    /// conversation, which is what lets it be asked while a turn has it.
    pub(crate) fn keep(&self, command: Command) -> Performed {
        let request = self.client.asking(command);
        let (providers, settings) = (self.providers.snapshot(), self.reading());
        let kept = keep(&request, &self.desk(&providers, &settings));

        if let Some(witness) = &self.client.witness {
            witness.apart(&request, kept.outcome());
        }

        kept
    }

    /// Settles the questions `command` answers in `questions`, the handle
    /// startup supplied. Needs no conversation, which is what lets the dock
    /// answer while a turn has it: the running turn delivers what settles.
    pub(crate) fn answer(&self, command: Command, questions: &Questions) -> Outcome {
        let request = self.client.asking(command);
        let answered = crucible_app::client::answer(&request, questions);

        if let Some(witness) = &self.client.witness {
            witness.apart(&request, answered.clone());
        }

        answered
    }

    /// Asks the turn `cancel` belongs to to stop. Needs no conversation, which
    /// is what lets a key ask it while a turn has the conversation.
    pub(crate) fn interrupt(&self, cancel: &Cancel) {
        self.client.interrupt(cancel);
    }

    /// What the host lends a command, from the providers in force and the
    /// settings as [`Terms::reading`] reads them.
    fn desk<'a>(
        &'a self,
        providers: &'a Providers,
        settings: &'a crucible_config::Settings,
    ) -> Desk<'a> {
        Desk {
            switching: self.switching(providers, settings),
            sessions: &self.sessions,
            workspace: &self.workspace,
            reads: |named| crucible_tui::syntax::colours(named).is_some(),
            environment: |name| std::env::var(name).ok(),
            notes: super::converse::command::notes::answered,
        }
    }
}

/// What the agents door is lent by every front end that asks it, for crucible's
/// `home`: the catalog where it is published, under the keys compiled into
/// this build and through the hosts it is served from, with the proxy settings
/// this process was started with, and `smoke` and `steps`, the launcher and
/// the steps of the conversation asking, where there is one. Both are lent
/// together: a sign-in that starts the agent is refused beside a launcher with
/// no steps, since the runs beside it would not be known.
pub(crate) fn shelf<'a>(
    home: &'a crucible_config::Home,
    smoke: Option<&'a Launcher<'a>>,
    steps: Option<&'a Steps>,
) -> Shelf<'a> {
    Shelf {
        catalog: Source::published(&|name| std::env::var(name).ok()),
        registry: Some(RegistrySource::published(&|name| std::env::var(name).ok())),
        home,
        smoke,
        steps,
    }
}

/// What asking for `theme` to be written down came to: nothing where it was,
/// and otherwise the line saying why this session is the only one drawn so.
pub(crate) fn unwritten(terms: &Terms, theme: Result<Theme, Refusal>) -> Option<String> {
    let kept = match theme {
        Ok(theme) => terms.keep(Command::Theme(theme)),
        Err(refusal) => Performed::Refused(refusal),
    };

    match kept {
        Performed::Theme(Ok(())) => None,
        Performed::Theme(Err(problem)) => Some(format!("! {problem}")),
        other => Some(astray(&other)),
    }
}

/// The one line for an answer the command that asked has no row for.
///
/// A refusal is the only one a terminal can meet: a name too long for the
/// contract, typed after `/model`. Every other arm answers a different
/// command, and is named rather than dropped so that a request routed wrongly
/// is a line on the screen and not a silence.
pub(crate) fn astray(performed: &Performed) -> String {
    match performed {
        Performed::Refused(refusal) => format!("! {refusal}"),
        other => format!("! the application answered {}", other.outcome().kind()),
    }
}

#[cfg(test)]
pub(crate) mod tests;
