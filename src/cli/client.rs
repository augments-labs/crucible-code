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
//! of it.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;

use tokio::sync::{mpsc, oneshot};

use crucible_app::Conversation;
use crucible_app::catalog_fetch::Source;
use crucible_app::client::agents::{self as door, AgentsError, Shelf, Watching};
use crucible_app::client::{
    Answered, Confirmation, Desk, Front, Performed, Shown, account, asked, asking, interrupt, keep,
    perform,
};
use crucible_app::content_use::Warned;
use crucible_app::launching::{Launcher, Steps};
use crucible_app::providers::Providers;
use crucible_client_api::{
    Capabilities, Capability, Command, Correlation, Decision, ErrorCode, InstallProgress, Name,
    Outcome, Pending, Refusal, Request, Said, Theme,
};
use crucible_runtime::{BoxFuture, Cancel};

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

    /// What gives the download up, which then commits nothing.
    pub(crate) const fn cancel(&self) -> &Cancel {
        self.cancel
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
            };
            door(&mut front, watching)
        });
        let mut carrying = Carrying {
            from,
            held: None,
            told: told_,
            cancel: cancel_,
            over: over_,
        };
        let stood = stood(overseer, &mut carrying);
        if stood.is_err() {
            cancel_.request();
        }
        // Nobody is left to answer: a question still out, or put from here
        // on, is answered by nobody.
        drop(carrying);
        let carried = worker
            .join()
            .unwrap_or_else(|panicked| std::panic::resume_unwind(panicked));
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
        | AgentsError::Unlisted(_)
        | AgentsError::NoInstance(_)
        | AgentsError::Unsigned(_)
        | AgentsError::NoMethod { .. }
        | AgentsError::Keyless(_)
        | AgentsError::Typed => ErrorCode::InvalidArgument,
        AgentsError::Config(_)
        | AgentsError::NotInstalled(_)
        | AgentsError::NoPrevious(_)
        | AgentsError::InUse { .. }
        | AgentsError::Unsupported(_)
        | AgentsError::Program(_)
        | AgentsError::Kept { .. }
        | AgentsError::TooLarge
        | AgentsError::Store(_)
        | AgentsError::Download(_)
        | AgentsError::Unpacked(_)
        | AgentsError::Stub(_)
        | AgentsError::FileListUnfetched(_)
        | AgentsError::FileListUnread(_)
        | AgentsError::Remember(_)
        | AgentsError::Unadded { .. }
        | AgentsError::Smoke { .. }
        | AgentsError::Unclouded { .. }
        | AgentsError::Unlocated(_)
        | AgentsError::Conversation(_)
        | AgentsError::Unnamed(_)
        | AgentsError::Unstored { .. }
        | AgentsError::Credentials(_)
        | AgentsError::Unprofiled(_)
        | AgentsError::Unlaunched(_)
        | AgentsError::HoldUnkept(_)
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
