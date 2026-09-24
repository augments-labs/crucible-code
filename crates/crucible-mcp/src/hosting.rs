//! Selected MCP servers, offered as tools beside the built-in ones.
//!
//! A server here is somebody else's program. It starts under its selected
//! sandbox policy, is spoken to over its own standard input and output, asked
//! once what it offers, and what
//! comes back becomes descriptors the model can call. This is where that meets
//! the roster crucible compiled in: the runner drives one live toolset, so the
//! two have to arrive as one generation or not at all.
//!
//! **Nothing starts because this module exists.** A run that selected no server
//! starts no process, opens no pipe and advertises no MCP tool; what it hands
//! the runner is the built-in generation itself, the same value that would have
//! reached it without this composition in front. `ToolSourceKind::Mcp` existing
//! offers nothing either — a tool is here because somebody named the server it
//! came from.
//!
//! **Every offered name is spelled somewhere crucible does not put its own.**
//! A server offering `read` would otherwise collide with the built-in `read`
//! and the whole generation would be refused — a run failing over a name
//! crucible chose long before anybody selected that server. So the name the
//! model calls is `mcp:server/tool`, and the only collision left is two servers
//! selected under one name, which is a selection to correct rather than a
//! surprise.
//!
//! **One catalogue per prepared lifecycle.** The runner refreshes between
//! every pass of every turn, and a round trip to each server on each of those
//! would cost more than it could tell: this crate speaks no notification, so a
//! second reading would return what the first one did. The catalogue is read at
//! preparation, and refresh republishes it against whatever the built-in roster
//! has become — which does move, because `tool_search` reveals a schema
//! mid-turn.
//!
//! **A call that was sent cannot be unsent, and that decides everything after
//! it.** An interrupt reaches into the wait — the reading side looks up between
//! short waits rather than blocking in a syscall, so escape ends a slow call at
//! the press rather than at `requestSeconds`. What it cannot do is reach the
//! server. The request has gone, the tool may be running, and from this side a
//! tool that never started, one that finished, and one whose answer was lost
//! are the same silence. So an interrupted server is finished with for the
//! run: the conversation would otherwise read the abandoned call's answer as
//! the reply to the next question.
//!
//! **`restarts` is spent on the endings where asking again is asking once.** A
//! server whose process died before crucible let go of the frame left the far
//! end untouched, and starting it again and sending the same call is one call.
//! Every other ending has a request outstanding, and no amount of budget makes
//! repeating it safe — the budget is not consulted for those. A restarted
//! server has to still offer the tool under the same name and the same schema,
//! because the descriptor the model was shown is the one it wrote its arguments
//! against.
//!
//! **A handle is dead the moment its lifecycle is.** Disposal stops every
//! server and marks it gone, so an executor still held from an earlier turn
//! refuses rather than speaking into a pipe that now belongs to nothing. The
//! next turn prepares again and mints its own after confirmed cleanup. An
//! unconfirmed stop keeps disposal failed and prevents another preparation or
//! replacement server, even after the backend consumes its process handle. A
//! refused startup has the same obligation: an optional server is skippable
//! only when its process cleanup is confirmed.
//!
//! **Every wait is awaited, and a wait given up on leaves nothing unowned.**
//! Starting a server, greeting it, reading its catalogue and calling it are
//! awaited, and a step of its sandbox that waits is given up on at the
//! server's handshake patience, or sooner where the lifecycle's cancel, or the
//! call's, is raised. Whatever awaits them can also give any of them up by
//! dropping it, as a turn racing a call against its deadline does. So what a
//! step reaches is put where the next step finds it before it waits: a process
//! being started is held as unconfirmed cleanup until its start answers, a
//! server joins its lifecycle before it is greeted, a replacement joins its
//! server before it is greeted, and an exchange is marked begun until the
//! server has answered it. A server left with an exchange unanswered, given up
//! on or ended without its answer, is asked nothing further, as an interrupted
//! one is, and is stopped by the next call to it or by disposal; one a
//! preparation given up on had started is stopped by the next preparation too.
//! Stopping a server still waits on the caller's thread, bounded by its grace
//! and its publication ceiling.

use std::ffi::OsString;
use std::fmt::{self, Write as _};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox::{
    SandboxAudit, SandboxCommand, SandboxEnablement, SandboxEnvironment, SandboxManifest,
    SandboxPolicy, SandboxRequest, SandboxService,
};
use crucible_tools::{
    Approved, Command, Sensitivity, Summary, Tool, ToolContext, ToolDescriptor, ToolEntry,
    ToolError, ToolOutput, ToolProvenance, ToolSnapshot, ToolSourceKind, Toolset, ToolsetContext,
    ToolsetError,
};
use crucible_transport::{Ambiguity, Finish, Restarts};
use crucible_types::{Ancestry, SandboxId, ToolArgs, ToolId};
use serde_json::Value;
use tokio::runtime::Handle;

use crate::hosted::refused_stop;
use crate::{Answered, Hosted, Offered, Rebuffed, Unanswered, Unstarted, Withheld};

#[cfg(test)]
mod tests;

/// What every tool taken from an MCP server is named under.
/// The wait a selection that named no number gets.
///
/// Short on purpose: it is not a guess at how long a server takes, it is how
/// long a caller who said nothing is willing to find out.
const BRIEF: Duration = Duration::from_secs(1);

const NAMESPACE: &str = "mcp";

/// Between the namespace and the server, and between the server and the tool.
///
/// Two marks rather than one so that the three parts stay legible in a name the
/// model reads back to crucible: `mcp:docs/search` says which server answered
/// without a reader having to know how many pieces to expect.
const OF: char = ':';
const WITHIN: char = '/';

/// One MCP server this run was told to host.
///
/// Inert: building one starts nothing, opens nothing and reaches nothing. It is
/// the selection, and [`Hosting`] is the only thing that acts on it.
///
/// `Debug` says which server and how many arguments, never what they say: an
/// argument is where a server is handed a token or a connection string.
pub struct Chosen {
    /// What the tools it offers are named under, which is what the user typed.
    name: Box<str>,
    program: PathBuf,
    arguments: Vec<OsString>,
    environment: SandboxEnvironment,
    /// How long it has to agree a protocol version, and each step of starting
    /// its sandbox has to answer.
    handshake: Duration,
    /// How long one request to it may take, once it has.
    request: Duration,
    /// How long it is given to go on its own before it is stopped.
    grace: Duration,
    /// Whether a run that cannot start it fails, rather than going without it.
    required: bool,
    /// How often its process may be started again after it ends.
    restarts: u32,
    /// What it is confined to, which is its own: two servers written down in
    /// one file can name two directories to run in, and a policy shared across
    /// the selection could only be right about one of them.
    policy: SandboxPolicy,
    enablement: Option<Arc<SandboxEnablement>>,
}

impl std::fmt::Debug for Chosen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chosen")
            .field("name", &self.name)
            .field("program", &self.program)
            .field("arguments", &self.arguments.len())
            .finish_non_exhaustive()
    }
}

impl Chosen {
    /// Records one selection, under the confinement it will run in.
    ///
    /// The policy is an argument rather than a default because there is no
    /// confinement a caller could be assumed to have meant: what a server may
    /// reach is the whole of what selecting one costs, and a builder step that
    /// could be left off would make the unconfined case the quiet one.
    ///
    /// The defaults here are the impatient reading of an absent number: a
    /// server that has not spoken in a second is one this run should not be
    /// waiting on, and a server nobody said was required is one the run can do
    /// without. Every selection crucible makes for itself states all four; a
    /// caller that states none gets a lifecycle that fails fast rather than a
    /// turn that hangs.
    pub fn new(
        name: impl Into<Box<str>>,
        program: impl Into<PathBuf>,
        arguments: impl IntoIterator<Item = OsString>,
        policy: SandboxPolicy,
    ) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            arguments: arguments.into_iter().collect(),
            environment: SandboxEnvironment::empty(),
            handshake: BRIEF,
            request: BRIEF,
            grace: BRIEF,
            required: false,
            restarts: 0,
            policy,
            enablement: None,
        }
    }

    /// Applies the host's shared choice to each new server lifecycle.
    #[must_use]
    pub fn following_enablement(mut self, control: Arc<SandboxEnablement>) -> Self {
        self.enablement = Some(control);
        self
    }

    /// The confinement it will actually run under.
    ///
    /// Not the policy it was recorded with: a host that turns confinement off
    /// for a run turns it off for every server the run selected, and the value
    /// a reader wants is the one a process would be started with.
    #[must_use]
    pub fn effective_policy(&self) -> SandboxPolicy {
        self.enablement.as_ref().map_or_else(
            || self.policy.clone(),
            |control| self.policy.clone().with_enabled(control.enabled()),
        )
    }

    /// The environment it is started with, which is the whole of what it gets.
    #[must_use]
    pub fn given(mut self, environment: SandboxEnvironment) -> Self {
        self.environment = environment;
        self
    }

    /// How long it has to greet, how long a request to it may take, and how
    /// long it is given to stop.
    #[must_use]
    pub const fn waiting(
        mut self,
        handshake: Duration,
        request: Duration,
        grace: Duration,
    ) -> Self {
        self.handshake = handshake;
        self.request = request;
        self.grace = grace;
        self
    }

    /// Whether the run fails when this one cannot be started.
    #[must_use]
    pub const fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// How often its process may be started again after it ends.
    ///
    /// Zero, the default, is one start and no more. It is a ceiling on the
    /// endings crucible can prove were harmless, not a retry policy: an ending
    /// with a request outstanding is refused whatever this says.
    #[must_use]
    pub const fn restarting(mut self, restarts: u32) -> Self {
        self.restarts = restarts;
        self
    }

    /// What the tools it offers are named under, which is what the user typed.
    #[must_use]
    pub const fn name(&self) -> &str {
        &self.name
    }

    /// The program that is started, which is already an absolute path.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The arguments it is started with, in the order they were written down.
    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// The environment it is started with, which is the whole of what it gets.
    #[must_use]
    pub const fn environment(&self) -> &SandboxEnvironment {
        &self.environment
    }

    /// How long it has to agree a protocol version, and each step of starting
    /// its sandbox has to answer.
    #[must_use]
    pub const fn handshake(&self) -> Duration {
        self.handshake
    }

    /// How long one request to it may take, once it has.
    #[must_use]
    pub const fn request(&self) -> Duration {
        self.request
    }

    /// How long it is given to go on its own before it is stopped.
    #[must_use]
    pub const fn grace(&self) -> Duration {
        self.grace
    }

    /// Whether a run that cannot start it fails, rather than going without it.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        self.required
    }

    /// How often its process may be started again after it ends.
    #[must_use]
    pub const fn restarts(&self) -> u32 {
        self.restarts
    }
}

/// An ordinary refusal may be optional; uncertain process cleanup never is.
enum StartFailure {
    Refused(ToolsetError),
    Unreaped(ToolsetError),
}

impl From<ToolsetError> for StartFailure {
    fn from(error: ToolsetError) -> Self {
        Self::Refused(error)
    }
}

impl StartFailure {
    fn refused(chosen: &Chosen, problem: &dyn std::fmt::Display) -> Self {
        Self::Refused(ToolsetError::Source {
            id: chosen.name.clone(),
            problem: problem.to_string().into(),
        })
    }

    /// A sandbox step given up on while it waited confirms nothing about
    /// what it began, so its server is held rather than skipped.
    ///
    /// Given up on at the handshake patience or at `interrupt`, and said to
    /// be the first where the second was not raised: the one is the far end
    /// taking too long, the other the near end no longer wanting it.
    fn given_up(chosen: &Chosen, step: &str, interrupt: &Cancel) -> Self {
        let waited = if interrupt.requested() {
            String::new()
        } else {
            format!(" after {:?}", chosen.handshake)
        };
        Self::Unreaped(ToolsetError::Source {
            id: chosen.name.clone(),
            problem: format!(
                "crucible stopped waiting{waited} while {step}, so whatever that step began \
                 is unconfirmed"
            )
            .into(),
        })
    }

    /// Ends the conversation before returning its protocol refusal.
    fn after(chosen: &Chosen, problem: &dyn std::fmt::Display, hosted: Hosted) -> Self {
        match hosted.stop(chosen.grace).finish {
            Finish::Exited(_) | Finish::Stopped => Self::refused(chosen, problem),
            Finish::Unpublished(unpublished) => Self::refused(
                chosen,
                &format!("{problem}; nothing it wrote was published: {unpublished}"),
            ),
            Finish::Unreaped(cleanup) => Self::Unreaped(ToolsetError::Source {
                id: chosen.name.clone(),
                // A refused stop's words already say that what it began is
                // unconfirmed; a failed stop's need not, and are led in by it.
                problem: if refused_stop(&cleanup) {
                    format!("{problem}; cleanup: {cleanup}")
                } else {
                    format!("{problem}; unconfirmed cleanup: {cleanup}")
                }
                .into(),
            }),
        }
    }

    /// What went wrong, whichever kind of failure it was.
    fn problem(self) -> ToolsetError {
        match self {
            Self::Refused(problem) | Self::Unreaped(problem) => problem,
        }
    }
}

/// Starts `chosen`'s process confined, and holds a conversation over it.
///
/// Free of [`Hosting`] because it is also what a restart does, and a restart
/// happens with a call in hand rather than a lifecycle: a second copy of these
/// steps is a second place for the handshake patience, the confinement or the
/// catalogue bound to drift.
///
/// Each step the sandbox takes is awaited, and given up on where `interrupt`
/// is raised while it waits or where it has waited the handshake patience: a
/// step that never answers ends there whether or not anybody cancels. A step
/// given up on was dropped, and with it whatever it held, as the session
/// behind a later step is when this returns; the sandbox contract does not say
/// what dropping one of these steps leaves behind, so the server is held as
/// unconfirmed cleanup and is never passed over.
async fn launch(
    chosen: &Chosen,
    starting: &Starting,
    ancestry: Ancestry,
    audit: &SandboxAudit,
    interrupt: &Cancel,
) -> Result<Hosted, StartFailure> {
    let refused = |problem: &dyn std::fmt::Display| StartFailure::refused(chosen, problem);
    let given_up = |step| move || StartFailure::given_up(chosen, step, interrupt);
    // Each step's own patience, counted from when it is asked.
    let bounded = || interrupt.child_until(Instant::now().checked_add(chosen.handshake));

    // A call identity of its own rather than the run's: what the audit is
    // about here is the server, and every tool it later offers is one call
    // inside this one process.
    let request = SandboxRequest::new(
        SandboxId::new(),
        ancestry,
        ToolId::new(format!("{NAMESPACE}{OF}{}", chosen.name)),
        chosen.effective_policy(),
        SandboxManifest::empty(),
    )
    .with_audit(audit.clone())
    .map_err(|error| refused(&error))?;
    let mut session = bounded()
        .race(starting.sandbox.prepare(request))
        .await
        .ok_or_else(given_up("its sandbox was being prepared"))?
        .map_err(|e| refused(&e))?;
    bounded()
        .race(session.materialize())
        .await
        .ok_or_else(given_up("its sandbox was being materialized"))?
        .map_err(|e| refused(&e))?;
    let command = SandboxCommand::new(
        chosen.program.clone(),
        chosen.arguments.iter().cloned(),
        chosen.environment.clone(),
    )
    .map_err(|e| refused(&e))?
    // Crucible keeps the writing end: the whole protocol is a conversation,
    // and a server whose input crucible let go could be greeted but never
    // asked anything.
    .spoken_to();
    let process = bounded()
        .race(session.start(command))
        .await
        .ok_or_else(given_up("its process was being started"))?
        .map_err(|e| refused(&e))?;

    let withheld = Withheld::given(&chosen.environment);
    Hosted::withholding(process, chosen.handshake, withheld, &starting.runtime).map_err(|error| {
        let problem = ToolsetError::Source {
            id: chosen.name.clone(),
            problem: error.to_string().into(),
        };
        match error {
            Unstarted::Unreaped { .. } => StartFailure::Unreaped(problem),
            Unstarted::Unspeakable | Unstarted::Unheard => StartFailure::Refused(problem),
        }
    })
}

/// Agrees a version with a hosted server, and reads what it offers.
///
/// Both exchanges are given up on where `interrupt` is raised while they wait.
async fn introduce(
    chosen: &Chosen,
    hosted: &mut Hosted,
    interrupt: &Cancel,
) -> Result<Vec<Offered>, Rebuffed> {
    let greeting = hosted.greet_async(Some(interrupt)).await?;
    // Under the other number from here on: the greeting was a peer reading
    // from a table, and everything after it is a peer doing work.
    hosted.patient_for(chosen.request);
    hosted.catalogue_async(&greeting, Some(interrupt)).await
}

/// The built-in roster, and whatever the selected servers offered.
pub struct Hosting {
    /// The roster crucible compiled in, as the one thing this composes with.
    ///
    /// A toolset rather than the concrete roster, because what this adds to is
    /// only ever asked the questions the trait names: a generation, a
    /// registered entry, and the four lifecycle steps. Taking the concrete type
    /// would tie one protocol client to whatever else that type grows.
    builtin: Arc<dyn Toolset>,
    chosen: Vec<Arc<Chosen>>,
    starting: Starting,
    /// The servers one prepared lifecycle started, held across the waits that
    /// start and stop them, so a preparation and a disposal never overlap.
    lifecycle: tokio::sync::Mutex<Lifecycle>,
    /// What those servers offered, which the questions that cannot wait read.
    published: Mutex<Published>,
}

/// What a server is started by and spoken to on, the same for every server of
/// one hosting and every restart of one server.
#[derive(Clone)]
struct Starting {
    /// What starts it confined.
    sandbox: Arc<dyn SandboxService>,
    /// Where its streams are read and written, by tasks its conversation
    /// holds.
    runtime: Handle,
}

/// The servers of one prepared lifecycle.
#[derive(Default)]
struct Lifecycle {
    /// One startup whose process handle did not confirm cleanup. No subsequent
    /// preparation can append another failure or start a server.
    ///
    /// Also set while a process is being started, and cleared once starting
    /// it answers: a preparation given up on part way through starting one has
    /// confirmed nothing about it.
    unreaped_start: Option<Box<str>>,
    /// Every server started for this lifecycle, in selection order.
    ///
    /// A server is here from the moment its process has started, before it is
    /// greeted, so one a preparation was greeting when it was given up on is
    /// still reached by what comes next.
    servers: Vec<Arc<Server>>,
    /// Whether `servers` is a whole preparation's, rather than what one given
    /// up on part way had started, and it started something: a preparation
    /// that started nothing, or whose disposal has begun, is prepared again.
    whole: bool,
}

/// What one prepared lifecycle published.
#[derive(Default)]
struct Published {
    /// The entries its servers' catalogues produced, fixed at preparation.
    offered: Vec<ToolEntry>,
    /// The last merged generation, under the built-in generation it was merged
    /// from. Republishing on every pass would mint a generation an admission
    /// from the pass before could not resolve through.
    merged: Option<(Box<str>, ToolSnapshot)>,
}

/// One started server, and the conversation crucible has with it.
struct Server {
    /// What its tools are named under.
    name: Box<str>,
    /// The selection it was started from, kept because a restart starts it
    /// again from exactly the same words.
    chosen: Arc<Chosen>,
    /// What starts it and where it is spoken to, which are the same as the
    /// lifecycle used.
    starting: Starting,
    /// Whose run this process belongs to, for the audit a restart also owes.
    ancestry: Ancestry,
    /// The conversation, the budget and what was published, under one lock,
    /// held across every exchange with it.
    ///
    /// One rather than two because they are decided together: what happens to
    /// a server whose call failed is read off the budget and written back to
    /// the conversation, and a second lock would let two calls each spend the
    /// last restart. Held across the exchange because the conversation is
    /// sequential: a second call over the same pipes while the first waits
    /// would be read against the wrong question.
    live: tokio::sync::Mutex<Speaking>,
}

/// A server's conversation, and what this run published for it.
struct Speaking {
    conversation: Conversation,
    /// Every tool this run published for it, as it was offered at start-up.
    ///
    /// The whole catalogue rather than the tool a call is about, because a
    /// restart is checked against what the model can *see*: the descriptors
    /// went out together and any of them may be called next. Empty until its
    /// first catalogue has been read.
    published: Vec<Offered>,
}

/// What a server is, between one call and the next.
enum Conversation {
    /// Only a live conversation can send calls or spend restart authority.
    Active(Box<ActiveConversation>),
    /// The conversation ended and its process scope was confirmed stopped.
    Closed,
    /// The backend consumed its process handle without confirming cleanup, or
    /// a process is being started whose start has not answered yet.
    /// Repeating disposal must keep reporting that uncertainty.
    Unreaped,
}

/// The process and its audit collector have the same live ownership boundary.
struct ActiveConversation {
    hosted: Hosted,
    audit: SandboxAudit,
    /// How many more times it may be started again.
    restarts: Restarts,
    /// Whether an exchange with it has begun and not answered.
    ///
    /// Set before a greeting, a catalogue or a call is awaited and cleared
    /// once the server has answered it, so one given up on while it waited,
    /// or a call that ended without an answer, leaves this set: its question
    /// may be with the server, and its answer would be read as the reply to
    /// the next one.
    midway: bool,
}

impl Speaking {
    /// The live conversation, where there is one.
    fn active(&mut self) -> Option<&mut ActiveConversation> {
        match &mut self.conversation {
            Conversation::Active(active) => Some(active),
            Conversation::Closed | Conversation::Unreaped => None,
        }
    }

    /// Takes the live conversation out, leaving it closed; anything else is
    /// left as it was.
    fn ended(&mut self) -> Option<Box<ActiveConversation>> {
        match std::mem::replace(&mut self.conversation, Conversation::Closed) {
            Conversation::Active(active) => Some(active),
            other => {
                self.conversation = other;
                None
            }
        }
    }
}

impl Hosting {
    /// The built-in roster with `chosen` servers hosted beside it, each
    /// spoken to by tasks on `runtime`.
    ///
    /// The runtime is what each server's streams need of it: the local
    /// backend's pipes on Unix need its I/O driver, every default waiting
    /// read its timer, and the default asynchronous input its blocking
    /// threads.
    pub fn new(
        builtin: Arc<dyn Toolset>,
        sandbox: Arc<dyn SandboxService>,
        chosen: Vec<Chosen>,
        runtime: Handle,
    ) -> Self {
        Self {
            builtin,
            chosen: chosen.into_iter().map(Arc::new).collect(),
            starting: Starting { sandbox, runtime },
            lifecycle: tokio::sync::Mutex::new(Lifecycle::default()),
            published: Mutex::new(Published::default()),
        }
    }

    /// Starts one server, greets it, and turns its catalogue into entries.
    ///
    /// The server joins `lifecycle` as soon as its process has started, and
    /// leaves it again where it is refused.
    async fn host(
        &self,
        chosen: &Arc<Chosen>,
        context: &ToolsetContext,
        lifecycle: &mut Lifecycle,
    ) -> Result<Vec<ToolEntry>, StartFailure> {
        let provenance = ToolProvenance::new(
            ToolSourceKind::Mcp,
            format!("{NAMESPACE}{OF}{}", chosen.name),
            format!("MCP server {} at {}", chosen.name, chosen.program.display()),
        )
        .map_err(ToolsetError::from)?;
        let audit = context
            .sandbox_audit(ToolId::new(format!("{NAMESPACE}{OF}{}", chosen.name)))
            .map_err(|error| ToolsetError::Source {
                id: chosen.name.clone(),
                problem: error.to_string().into(),
            })?;

        // Unconfirmed until starting it answers, as the lifecycle says.
        lifecycle.unreaped_start = Some(chosen.name.clone());
        let launched = launch(
            chosen,
            &self.starting,
            context.ancestry(),
            &audit,
            context.cancel(),
        )
        .await;
        lifecycle.unreaped_start = None;
        let hosted = launched?;

        let server = Arc::new(Server {
            name: chosen.name.clone(),
            chosen: Arc::clone(chosen),
            starting: self.starting.clone(),
            ancestry: context.ancestry(),
            live: tokio::sync::Mutex::new(Speaking {
                conversation: Conversation::Active(Box::new(ActiveConversation {
                    hosted,
                    audit,
                    restarts: Restarts::ceiling(chosen.restarts),
                    midway: true,
                })),
                published: Vec::new(),
            }),
        });
        lifecycle.servers.push(Arc::clone(&server));
        let offered = {
            let mut live = server.live.lock().await;
            let offered = server.introducing(&mut live, context.cancel()).await;
            if let Ok(offered) = &offered {
                live.published.clone_from(offered);
            }
            offered
        };
        let offered = match offered {
            Ok(offered) => offered,
            Err(failure) => {
                // Stopped already, or held as the failure says: either way it
                // is the failure's to report now, not the lifecycle's.
                lifecycle.servers.pop();
                return Err(failure);
            }
        };

        let program: Box<str> = chosen.program.display().to_string().into();
        let mut entries = Vec::with_capacity(offered.len());
        for one in offered {
            let called: Box<str> =
                format!("{NAMESPACE}{OF}{}{WITHIN}{}", chosen.name, one.shown()).into();
            let descriptor =
                ToolDescriptor::new(called.clone(), one.schema().to_string(), provenance.clone())
                    .map_err(ToolsetError::from)?;
            entries.push(ToolEntry::new(
                descriptor,
                Arc::new(Calling {
                    called,
                    offered: one,
                    program: program.clone(),
                    server: Arc::clone(&server),
                }),
            ));
        }
        Ok(entries)
    }

    /// Stops every server `lifecycle` holds, keeping the ones whose cleanup
    /// was not confirmed, and hands back the first refusal.
    ///
    /// Every one of them, and the first refusal afterwards: a server that
    /// could not be reaped must not leave the ones after it running. They stay
    /// in `lifecycle` until each has been stopped, so a release given up on
    /// part way leaves the rest where the next one finds them.
    async fn release(lifecycle: &mut Lifecycle) -> Option<ToolsetError> {
        let servers = lifecycle.servers.clone();
        let mut refused = None;
        for server in &servers {
            if let Err(problem) = server.release().await {
                refused = refused.or(Some(problem));
            }
        }
        let mut kept = Vec::new();
        for server in servers {
            if server.unreaped().await {
                kept.push(server);
            }
        }
        lifecycle.servers = kept;
        refused
    }

    /// The generation this lifecycle publishes, rebuilt only when the built-in
    /// roster has moved under it.
    async fn generation(&self, context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError> {
        let builtin = Toolset::snapshot(self.builtin.as_ref(), context).await?;
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if published.offered.is_empty() {
            return Ok(builtin);
        }

        // Keyed on the built-in generation's own label because that is the one
        // public spelling of its identity. Reused rather than rebuilt so that a
        // call admitted in the pass before still resolves: a fresh generation
        // every pass would refuse every one of them.
        let from = builtin.generation().context_id();
        if let Some((built, merged)) = published.merged.as_ref()
            && built.as_ref() == from
        {
            return Ok(merged.clone());
        }

        let merged = ToolSnapshot::new(
            builtin
                .entries()
                .iter()
                .cloned()
                .chain(published.offered.iter().cloned()),
        )?;
        published.merged = Some((from.into(), merged.clone()));
        Ok(merged)
    }
}

impl fmt::Debug for Hosting {
    /// The selection and how much of it is live, which is the whole of what a
    /// reader can act on. The roster and the sandbox service behind it are
    /// somebody else's values and say nothing here that they do not say better
    /// where they are held. How many servers are started is left out while a
    /// preparation or a disposal holds them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let published = self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("Hosting")
            .field(
                "chosen",
                &self
                    .chosen
                    .iter()
                    .map(|one| one.name.as_ref())
                    .collect::<Vec<_>>(),
            )
            .field(
                "started",
                &self
                    .lifecycle
                    .try_lock()
                    .ok()
                    .map(|lifecycle| lifecycle.servers.len()),
            )
            .field("offered", &published.offered.len())
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Stops the conversation and the process behind it, once.
    ///
    /// Taking it ends the conversation. Cleanup uncertainty stays explicit,
    /// because a consumed process handle is not proof its scope has ended.
    async fn release(&self) -> Result<(), ToolsetError> {
        let mut live = self.live.lock().await;
        self.released(&mut live)
    }

    /// [`Self::release`], for a caller already holding the conversation.
    fn released(&self, live: &mut Speaking) -> Result<(), ToolsetError> {
        match live.ended() {
            Some(active) => {
                let finished = self.reaped(&mut live.conversation, active.hosted);
                drop(active.audit);
                finished
            }
            None => match live.conversation {
                Conversation::Unreaped => Err(self.unconfirmed()),
                Conversation::Closed | Conversation::Active(_) => Ok(()),
            },
        }
    }

    /// Records the outcome while the caller holds the conversation lock.
    ///
    /// The stop waits on the caller's thread, up to the grace and past that
    /// up to the publication ceiling of a process that has ended, as
    /// [`Hosted::stop`] does.
    fn reaped(&self, live: &mut Conversation, hosted: Hosted) -> Result<(), ToolsetError> {
        match hosted.stop(self.chosen.grace).finish {
            Finish::Exited(_) | Finish::Stopped => Ok(()),
            // Its scope ended and was reaped, so nothing keeps it from being
            // started again. What was lost is what it wrote, and that is said.
            Finish::Unpublished(problem) => Err(ToolsetError::Source {
                id: self.name.clone(),
                problem: format!("nothing it wrote was published: {problem}").into(),
            }),
            Finish::Unreaped(problem) => {
                *live = Conversation::Unreaped;
                Err(ToolsetError::Source {
                    id: self.name.clone(),
                    problem: problem.to_string().into(),
                })
            }
        }
    }

    /// Retains a bounded failure category, not an opaque backend diagnostic.
    fn unconfirmed(&self) -> ToolsetError {
        unconfirmed(&self.name)
    }

    async fn unreaped(&self) -> bool {
        matches!(self.live.lock().await.conversation, Conversation::Unreaped)
    }

    /// Greets the server `live` holds and reads what it offers.
    ///
    /// The conversation is marked mid-exchange until both have answered. One
    /// that is refused is ended here, and the conversation left as its ending
    /// says: closed where its process was confirmed stopped, unreaped where
    /// not.
    async fn introducing(
        &self,
        live: &mut Speaking,
        interrupt: &Cancel,
    ) -> Result<Vec<Offered>, StartFailure> {
        let Some(active) = live.active() else {
            return Err(StartFailure::refused(
                &self.chosen,
                &"the server lifecycle has ended",
            ));
        };
        active.midway = true;
        let problem = match introduce(&self.chosen, &mut active.hosted, interrupt).await {
            Ok(offered) => {
                active.midway = false;
                return Ok(offered);
            }
            Err(problem) => problem,
        };
        let Some(active) = live.ended() else {
            return Err(StartFailure::refused(&self.chosen, &problem));
        };
        let failure = StartFailure::after(&self.chosen, &problem, active.hosted);
        if matches!(failure, StartFailure::Unreaped(_)) {
            live.conversation = Conversation::Unreaped;
        }
        Err(failure)
    }

    /// Starts this server again, if the ending permits it and the budget has
    /// one left, and hands back a conversation that still offers everything
    /// this run published for it.
    ///
    /// The budget is asked before anything is started, and it is asked with the
    /// ending's own certainty rather than a decision made here: a request that
    /// was outstanding is refused by [`Restarts::again`] whatever the ceiling
    /// says, so the rule that a half-done call is never repeated is written
    /// once, beside every other program crucible supervises.
    ///
    /// The old conversation is stopped first. A process that has to be started
    /// again is one crucible has already lost track of, and leaving it running
    /// beside its replacement would leave a server nothing will ever reap.
    ///
    /// Given up on part way, it leaves the server where disposal reaches it:
    /// held as unconfirmed while its replacement's process is being started,
    /// and holding the replacement, marked mid-exchange, while it is greeted.
    async fn restart(&self, after: Ambiguity, interrupt: &Cancel) -> Result<(), Box<str>> {
        let mut live = self.live.lock().await;
        let Some(active) = live.active() else {
            return Err("the server lifecycle has ended".into());
        };
        let permitted = active
            .restarts
            .again(after)
            .map_err(|no| no.said_of("the server"))?;
        let Some(previous) = live.ended() else {
            return Err("the server lifecycle has ended".into());
        };
        let ActiveConversation {
            hosted,
            audit,
            restarts,
            ..
        } = *previous;
        self.reaped(&mut live.conversation, hosted)
            .map_err(|error| error.to_string())?;

        // Unconfirmed until starting its replacement answers.
        live.conversation = Conversation::Unreaped;
        let hosted = launch(
            &self.chosen,
            &self.starting,
            self.ancestry,
            &audit,
            interrupt,
        )
        .await
        .map_err(|failure| {
            if let StartFailure::Refused(_) = failure {
                live.conversation = Conversation::Closed;
            }
            format!(
                "restart {} did not start it: {}",
                permitted.nth(),
                failure.problem()
            )
            .into_boxed_str()
        })?;
        live.conversation = Conversation::Active(Box::new(ActiveConversation {
            hosted,
            audit,
            restarts,
            midway: true,
        }));
        let offered = self
            .introducing(&mut live, interrupt)
            .await
            .map_err(|failure| {
                format!(
                    "restart {} did not start it: {}",
                    permitted.nth(),
                    failure.problem()
                )
                .into_boxed_str()
            })?;

        // The descriptors the model wrote its arguments against are the ones
        // this run published, and a server is free to come back offering
        // something else under the same names. Sending arguments checked
        // against an old schema to the new tool would be crucible vouching for
        // a promise nobody made, so a catalogue that moved ends the server
        // instead.
        //
        // All of them rather than the one this call is about: the model holds
        // every descriptor this run published and may call any of them next, so
        // a server that came back with the tool in hand intact and its
        // neighbour reshaped would leave the rest of the roster describing
        // something that is no longer there.
        let moved = live
            .published
            .iter()
            .find(|then| {
                !offered
                    .iter()
                    .any(|now| now.name() == then.name() && now.schema() == then.schema())
            })
            .map(|moved| moved.shown().to_owned());
        if let Some(moved) = moved {
            let cleanup = self.released(&mut live);
            return Err(format!(
                "it came back without {moved}, or offering it under a different schema, so the \
                 tools this run published no longer describe it{}",
                cleanup
                    .err()
                    .map_or_else(String::new, |error| format!("; {error}"))
            )
            .into_boxed_str());
        }
        Ok(())
    }
}

/// A bounded retained failure identifies its server without keeping backend text.
fn unconfirmed(name: &str) -> ToolsetError {
    ToolsetError::Source {
        id: name.into(),
        problem: "prior server cleanup remains unconfirmed".into(),
    }
}

impl Toolset for Hosting {
    fn prepare<'a>(
        &'a self,
        context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            Toolset::prepare(self.builtin.as_ref(), context).await?;
            let mut lifecycle = self.lifecycle.lock().await;
            if let Some(name) = &lifecycle.unreaped_start {
                return Err(unconfirmed(name));
            }
            for server in &lifecycle.servers {
                if server.unreaped().await {
                    return Err(server.unconfirmed());
                }
            }
            if lifecycle.whole {
                return Ok(());
            }
            // What a preparation given up on part way had started is stopped
            // before anything is started again: nothing else would reach it.
            if let Some(refused) = Self::release(&mut lifecycle).await {
                return Err(refused);
            }

            let mut offered = Vec::new();
            for chosen in &self.chosen {
                match self.host(chosen, context, &mut lifecycle).await {
                    Ok(entries) => offered.extend(entries),
                    // A server nobody said was required is one this run can do
                    // without: its tools are simply not offered, and the turn goes
                    // ahead with the rest. Saying `required` is what turns a
                    // machine that is missing a program into a refused run.
                    Err(StartFailure::Refused(_)) if !chosen.required => {}
                    Err(failure) => {
                        let problem = match failure {
                            StartFailure::Refused(problem) => problem,
                            StartFailure::Unreaped(problem) => {
                                lifecycle.unreaped_start = Some(chosen.name.clone());
                                problem
                            }
                        };
                        // Stop partial preparation immediately. Retain uncertain
                        // cleanup so disposal can report it alongside this cause.
                        drop(Self::release(&mut lifecycle).await);
                        return Err(problem);
                    }
                }
            }

            // A preparation that started nothing is tried again by the next,
            // as one that selected nothing costs nothing to repeat.
            lifecycle.whole = !lifecycle.servers.is_empty();
            let mut published = self
                .published
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            published.offered = offered;
            published.merged = None;
            Ok(())
        })
    }

    fn snapshot<'a>(
        &'a self,
        context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(self.generation(context))
    }

    fn refresh<'a>(
        &'a self,
        context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(self.generation(context))
    }

    fn registered(&self, name: &str) -> Option<ToolEntry> {
        Toolset::registered(self.builtin.as_ref(), name).or_else(|| {
            let published = self
                .published
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            published
                .offered
                .iter()
                .find(|entry| entry.descriptor().name() == name)
                .cloned()
        })
    }

    fn dispose<'a>(
        &'a self,
        context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            // The servers are released, and the lifecycle given back, before
            // the built-in tools are disposed of. Releasing a server waits for
            // a call still speaking to it to answer, and then stops it on this
            // thread, one server after another: up to its grace, past that up
            // to its publication ceiling where it has ended, and then, where it
            // has not finished by then, for the whole of its stop, with each
            // look at its status able to conclude its ending. A preparation
            // waits behind it; `snapshot`, `refresh`, `registered` and `Debug`
            // read what was published, which is withdrawn first, and do not.
            let refused = {
                let mut lifecycle = self.lifecycle.lock().await;
                {
                    let mut published = self
                        .published
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    published.offered.clear();
                    published.merged = None;
                }
                // Before anything waits, with what was published: a disposal
                // given up on part way has withdrawn this lifecycle's tools,
                // so the next preparation starts again rather than standing
                // on it.
                lifecycle.whole = false;
                let unreaped = lifecycle.unreaped_start.as_deref().map(unconfirmed);
                let released = Self::release(&mut lifecycle).await;
                unreaped.or(released)
            };
            Toolset::dispose(self.builtin.as_ref(), context).await?;
            refused.map_or(Ok(()), Err)
        })
    }
}

/// One tool a server offered, as something the model can call.
struct Calling {
    /// The name the model calls, which is not the name the server knows.
    called: Box<str>,
    /// What the server calls it, and the shape it takes.
    offered: Offered,
    /// The program the server is, for the question the user is asked.
    program: Box<str>,
    server: Arc<Server>,
}

impl Tool for Calling {
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
        arguments(&self.called, args).map(drop)
    }

    /// Running somebody else's program, because that is what it is. Nothing
    /// here can say what a call does: the schema is the server's own text and
    /// the code behind it was never read, so the honest classification is the
    /// one that says a program is about to act.
    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::SpawnsProcess {
            command: Command::Understood {
                sent: format!("{} ({})", self.called, self.program).into(),
                parts: Box::new([self.called.clone()]),
            },
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new(self.called.clone())
    }

    fn run<'a>(
        &'a self,
        approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let arguments = arguments(&self.called, approved.args())?;
            // Asked before anything is sent, because refusing here costs the far
            // end nothing at all: no process is disturbed and no budget is spent.
            // Once the frame has gone the answer is the one below, which is more
            // expensive and less certain.
            if context.cancel().requested() {
                return Err(ToolError::Cancelled(self.called.clone()));
            }

            let problem = match self.attempt(&arguments, context).await {
                Ok(answered) => return Ok(said(&answered)),
                // A handle that outlived its lifecycle, or a server an earlier
                // exchange was given up on with. Nothing is running for this
                // call: the turn that read this catalogue is over, or the server
                // was finished with, and starting it again here would give it a
                // process no disposal will ever reach or a question it may
                // already be answering.
                Err(Refusal::Gone) => return Err(self.stale()),
                Err(Refusal::Unanswered(problem)) => problem,
            };
            // The server answered, and what it answered with is this crate's
            // complaint rather than the conversation's. Nothing is stopped and
            // nothing is spent.
            if problem.settled() {
                return Err(self.broke(&problem));
            }

            // Everything from here is a conversation that cannot carry another
            // call. Whether the server may have acted on the one it was given is
            // what decides both the budget and the words, and only a frame crucible
            // never let go of can answer no.
            let after = if problem.outstanding() {
                Ambiguity::Unsettled
            } else {
                Ambiguity::Settled
            };
            if let Err(refused) = self.server.restart(after, context.cancel()).await {
                drop(self.server.release().await);
                return Err(if problem.interrupted() {
                    // Reported as the interruption it was. The restart was refused
                    // because the call is outstanding, which is that same sentence
                    // twice and is not news to whoever pressed the key.
                    ToolError::Cancelled(self.called.clone())
                } else {
                    self.gone(&problem, &refused)
                });
            }

            // One retry, and the same reading of its failure: a server that has to
            // be started again for every call is one this run will not get an
            // answer out of, and the budget is what stops that being discovered a
            // call at a time forever.
            match self.attempt(&arguments, context).await {
                Ok(answered) => Ok(said(&answered)),
                Err(Refusal::Gone) => Err(self.stale()),
                Err(Refusal::Unanswered(again)) => {
                    if !again.settled() {
                        drop(self.server.release().await);
                    }
                    Err(if again.interrupted() {
                        ToolError::Cancelled(self.called.clone())
                    } else {
                        self.broke(&again)
                    })
                }
            }
        })
    }
}

impl Calling {
    /// Sends the call, with the interrupt that can end the waiting early.
    ///
    /// A wait somebody ends by pressing escape ends at the press rather than at
    /// the request patience. The lock is held while the call waits, because
    /// the conversation carries one call at a time, and given back before
    /// anything else, because what happens next may be a restart and that
    /// takes the same lock.
    ///
    /// Given up on while it waits, or ended without an answer, it leaves the
    /// conversation marked mid-exchange, and the next attempt finishes with
    /// the server rather than asking it anything; a restart replaces the
    /// conversation, mark and all.
    async fn attempt(
        &self,
        arguments: &Value,
        context: &ToolContext<'_>,
    ) -> Result<Answered, Refusal> {
        let mut live = self.server.live.lock().await;
        let Some(active) = live.active() else {
            // The lifecycle that read this catalogue has ended, or a call
            // before this one left the conversation somewhere it could not come
            // back from.
            return Err(Refusal::Gone);
        };
        if active.midway {
            // An exchange before this one was given up on while it waited, so
            // the server may be answering it still. It is finished with, as
            // an interrupted one is: its answer would be read as this call's.
            drop(self.server.released(&mut live));
            return Err(Refusal::Gone);
        }
        active.midway = true;
        let answered = active
            .hosted
            .call_async(&self.offered, arguments, Some(context.cancel()))
            .await;
        // Only an answer settles the exchange. One that did not come back is
        // still with the server, whatever takes this lock next: a call queued
        // behind this one finishes with the server rather than asking it, as
        // this call's restart or release would have.
        if answered.as_ref().map_or_else(Unanswered::settled, |_| true) {
            active.midway = false;
        }
        answered.map_err(Refusal::Unanswered)
    }

    /// A handle whose lifecycle has ended.
    fn stale(&self) -> ToolError {
        ToolError::StaleGeneration {
            tool: self.called.clone(),
        }
    }

    /// A call that produced no result, in words the model can read.
    fn broke(&self, problem: &Unanswered) -> ToolError {
        ToolError::Io {
            tool: self.called.clone(),
            problem: format!("the MCP server {} could not answer", self.server.name).into(),
            source: io::Error::other(problem.to_string()),
        }
    }

    /// A server this run has finished with, and why it will not be asked again.
    fn gone(&self, problem: &Unanswered, refused: &str) -> ToolError {
        ToolError::Io {
            tool: self.called.clone(),
            problem: format!(
                "the MCP server {} could not answer and will not be asked again: {refused}",
                self.server.name
            )
            .into(),
            source: io::Error::other(problem.to_string()),
        }
    }
}

/// Why a call produced no result, including the one ending that is not the
/// server's doing.
enum Refusal {
    /// There was no conversation to speak over.
    Gone,

    /// There was, and this is what it said.
    Unanswered(Unanswered),
}

/// What a model is shown for one answered call.
///
/// A result that lost something says so. The mark inside the text says where
/// the cut was; this says how much went, which is the part a reader of what
/// survived has no way to work out.
fn said(answered: &Answered) -> ToolOutput {
    let mut said = answered.text().to_owned();
    if answered.omitted() > 0 {
        let omitted = answered.omitted();
        let _ = write!(
            said,
            "\n[…crucible left out {omitted} bytes of this result…]"
        );
    }
    if answered.failed() {
        ToolOutput::failed(said)
    } else {
        ToolOutput::ok(said)
    }
}

/// The arguments a call carried, as an MCP server takes them.
///
/// Read here rather than passed through, because what a server is sent has to
/// be one object: a provider that streamed a list or a bare string would
/// otherwise reach the server as a message the protocol has no shape for.
fn arguments(called: &str, args: &ToolArgs) -> Result<Value, ToolError> {
    let text = args.as_str().trim();

    // Nothing at all is the commonest call an MCP server takes — a tool with no
    // argument — and a provider writes that as an empty string as readily as an
    // empty object. Refusing it would refuse the call rather than correct it.
    if text.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }

    let value: Value = serde_json::from_str(text)
        .map_err(|problem| wrong(called, format!("the arguments are not JSON: {problem}")))?;
    if !value.is_object() {
        return Err(wrong(
            called,
            "the arguments must be an object, which is the only shape an MCP tool takes",
        ));
    }
    Ok(value)
}

/// A call this executor will not send, in words the model can act on.
fn wrong(called: &str, problem: impl Into<Box<str>>) -> ToolError {
    ToolError::Arguments {
        tool: called.into(),
        problem: problem.into(),
    }
}
