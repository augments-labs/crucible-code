//! Selected MCP servers, offered as tools beside the built-in ones.
//!
//! A server here is somebody else's program. It is started confined, spoken to
//! over its own standard input and output, asked once what it offers, and what
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
//! it.** An interrupt reaches into the wait — the reading side is a poll loop,
//! not a blocked syscall, so escape ends a slow call at the press rather than
//! at `requestSeconds`. What it cannot do is reach the server. The request has
//! gone, the tool may be running, and from this side a tool that never started,
//! one that finished, and one whose answer was lost are the same silence. So an
//! interrupted server is finished with for the run: the conversation would
//! otherwise read the abandoned call's answer as the reply to the next
//! question.
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
//! next turn prepares again and mints its own.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crucible_core::{
    Ancestry, Approved, Cancel, Command, Finish, SandboxCommand, SandboxEnvironment, SandboxId,
    SandboxManifest, SandboxPolicy, SandboxRequest, SandboxService, Sensitivity, Summary, Tool,
    ToolArgs, ToolContext, ToolDescriptor, ToolEntry, ToolError, ToolId, ToolOutput,
    ToolProvenance, ToolSnapshot, ToolSourceKind, Toolset, ToolsetContext, ToolsetError,
};
use crucible_extension::{Ambiguity, Restarts};
use crucible_mcp::{Answered, Hosted, Offered, Unanswered};
use crucible_runner::Tools;
use serde_json::Value;

pub(crate) mod selecting;
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
pub(crate) struct Chosen {
    /// What the tools it offers are named under, which is what the user typed.
    name: Box<str>,
    program: PathBuf,
    arguments: Vec<OsString>,
    environment: SandboxEnvironment,
    /// How long it has to agree a protocol version.
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
    pub(crate) fn new(
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
        }
    }

    /// The environment it is started with, which is the whole of what it gets.
    pub(crate) fn given(mut self, environment: SandboxEnvironment) -> Self {
        self.environment = environment;
        self
    }

    /// How long it has to greet, how long a request to it may take, and how
    /// long it is given to stop.
    pub(crate) const fn waiting(
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
    pub(crate) const fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// How often its process may be started again after it ends.
    ///
    /// Zero, the default, is one start and no more. It is a ceiling on the
    /// endings crucible can prove were harmless, not a retry policy: an ending
    /// with a request outstanding is refused whatever this says.
    pub(crate) const fn restarting(mut self, restarts: u32) -> Self {
        self.restarts = restarts;
        self
    }
}

/// One server that is running, and the catalogue it answered with.
struct Started {
    /// The conversation crucible holds with it.
    hosted: Hosted,
    /// What it said it offers, in the order it said it.
    offered: Vec<Offered>,
}

/// Starts `chosen` confined, agrees a version, and reads what it offers.
///
/// Free of [`Hosting`] because it is also what a restart does, and a restart
/// happens with a call in hand rather than a lifecycle: a second copy of these
/// steps is a second place for the handshake patience, the confinement or the
/// catalogue bound to drift.
fn start(
    chosen: &Chosen,
    sandbox: &dyn SandboxService,
    ancestry: Ancestry,
    interrupt: Option<&Cancel>,
) -> Result<Started, Box<str>> {
    let refused = |problem: &dyn std::fmt::Display| -> Box<str> { problem.to_string().into() };

    // A call identity of its own rather than the run's: what the audit is
    // about here is the server, and every tool it later offers is one call
    // inside this one process.
    let request = SandboxRequest::new(
        SandboxId::new(),
        ancestry,
        ToolId::new(format!("{NAMESPACE}{OF}{}", chosen.name)),
        chosen.policy.clone(),
        SandboxManifest::empty(),
    );
    let mut session = sandbox.prepare(request).map_err(|e| refused(&e))?;
    session.materialize().map_err(|e| refused(&e))?;
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
    let process = session.start(command).map_err(|e| refused(&e))?;

    let mut hosted = Hosted::over(process, chosen.handshake).map_err(|e| refused(&e))?;
    let greeting = hosted.greet(interrupt).map_err(|e| refused(&e))?;
    // Under the other number from here on: the greeting was a peer reading
    // from a table, and everything after it is a peer doing work.
    hosted.patient_for(chosen.request);
    let offered = hosted
        .catalogue(&greeting, interrupt)
        .map_err(|e| refused(&e))?;
    Ok(Started { hosted, offered })
}

/// The built-in roster, and whatever the selected servers offered.
pub(crate) struct Hosting {
    builtin: Tools,
    chosen: Vec<Arc<Chosen>>,
    sandbox: Arc<dyn SandboxService>,
    live: Mutex<Live>,
}

/// What one prepared lifecycle is holding.
#[derive(Default)]
struct Live {
    /// Every server started for this lifecycle, in selection order.
    servers: Vec<Arc<Server>>,
    /// The entries their catalogues produced, fixed at preparation.
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
    /// What starts it, which is the same service the lifecycle used.
    sandbox: Arc<dyn SandboxService>,
    /// Whose run this process belongs to, for the audit a restart also owes.
    ancestry: Ancestry,
    /// Every tool this run published for it, as it was offered at start-up.
    ///
    /// The whole catalogue rather than the tool a call is about, because a
    /// restart is checked against what the model can *see*: the descriptors
    /// went out together and any of them may be called next.
    published: Vec<Offered>,
    /// The conversation and the budget, under one lock.
    ///
    /// One rather than two because they are decided together: what happens to
    /// a server whose call failed is read off the budget and written back to
    /// the conversation, and a second lock would let two calls each spend the
    /// last restart.
    live: Mutex<Conversation>,
}

/// What a server is, between one call and the next.
struct Conversation {
    /// Taken at disposal, because stopping consumes it, and taken when a call
    /// leaves the two ends disagreeing about what was asked.
    ///
    /// This is the whole of a handle's validity: a handle that finds nothing
    /// here is one whose server has ended, and there is no second flag beside
    /// it to disagree with.
    hosted: Option<Hosted>,
    /// How many more times it may be started again.
    restarts: Restarts,
}

impl Hosting {
    /// The built-in roster with `chosen` servers hosted beside it.
    pub(crate) fn new(
        builtin: Tools,
        sandbox: Arc<dyn SandboxService>,
        chosen: Vec<Chosen>,
    ) -> Self {
        Self {
            builtin,
            chosen: chosen.into_iter().map(Arc::new).collect(),
            sandbox,
            live: Mutex::new(Live::default()),
        }
    }

    /// Starts one server, greets it, and turns its catalogue into entries.
    fn host(
        &self,
        chosen: &Arc<Chosen>,
        context: &ToolsetContext,
    ) -> Result<(Arc<Server>, Vec<ToolEntry>), ToolsetError> {
        let Started { hosted, offered } = start(
            chosen,
            self.sandbox.as_ref(),
            context.ancestry(),
            Some(context.cancel()),
        )
        .map_err(|problem| ToolsetError::Source {
            id: chosen.name.clone(),
            problem,
        })?;

        let provenance = ToolProvenance::new(
            ToolSourceKind::Mcp,
            format!("{NAMESPACE}{OF}{}", chosen.name),
            format!("MCP server {} at {}", chosen.name, chosen.program.display()),
        )?;
        let program: Box<str> = chosen.program.display().to_string().into();
        let server = Arc::new(Server {
            name: chosen.name.clone(),
            chosen: Arc::clone(chosen),
            sandbox: Arc::clone(&self.sandbox),
            ancestry: context.ancestry(),
            published: offered.clone(),
            live: Mutex::new(Conversation {
                hosted: Some(hosted),
                restarts: Restarts::ceiling(chosen.restarts),
            }),
        });

        let mut entries = Vec::with_capacity(offered.len());
        for one in offered {
            let called: Box<str> =
                format!("{NAMESPACE}{OF}{}{WITHIN}{}", chosen.name, one.name()).into();
            let descriptor =
                ToolDescriptor::new(called.clone(), one.schema().to_string(), provenance.clone())?;
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
        Ok((server, entries))
    }

    /// The generation this lifecycle publishes, rebuilt only when the built-in
    /// roster has moved under it.
    fn generation(&self) -> Result<ToolSnapshot, ToolsetError> {
        let builtin = self.builtin.snapshot()?;
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if live.offered.is_empty() {
            return Ok(builtin);
        }

        // Keyed on the built-in generation's own label because that is the one
        // public spelling of its identity. Reused rather than rebuilt so that a
        // call admitted in the pass before still resolves: a fresh generation
        // every pass would refuse every one of them.
        let from = builtin.generation().context_id();
        if let Some((built, merged)) = live.merged.as_ref()
            && built.as_ref() == from
        {
            return Ok(merged.clone());
        }

        let merged = ToolSnapshot::new(
            builtin
                .entries()
                .iter()
                .cloned()
                .chain(live.offered.iter().cloned()),
        )?;
        live.merged = Some((from.into(), merged.clone()));
        Ok(merged)
    }
}

impl Server {
    /// Stops the conversation and the process behind it, once.
    ///
    /// Taking it is what ends it, so a second call reaches a server that is
    /// already gone and does nothing to it.
    fn release(&self) -> Result<(), ToolsetError> {
        let taken = self
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .hosted
            .take();
        self.reaped(taken)
    }

    /// The same, for a conversation already taken out from under the lock.
    fn reaped(&self, taken: Option<Hosted>) -> Result<(), ToolsetError> {
        let Some(hosted) = taken else {
            return Ok(());
        };

        match hosted.stop(self.chosen.grace).finish {
            Finish::Exited(_) | Finish::Stopped => Ok(()),
            Finish::Unreaped(problem) => Err(ToolsetError::Source {
                id: self.name.clone(),
                problem: problem.to_string().into(),
            }),
        }
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
    fn restart(&self, after: Ambiguity, interrupt: Option<&Cancel>) -> Result<(), Box<str>> {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        let permitted = live.restarts.again(after).map_err(|no| no.to_string())?;
        drop(self.reaped(live.hosted.take()));

        let Started { hosted, offered } = start(
            &self.chosen,
            self.sandbox.as_ref(),
            self.ancestry,
            interrupt,
        )
        .map_err(|problem| {
            format!("restart {} did not start it: {problem}", permitted.nth()).into_boxed_str()
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
        let moved = self.published.iter().find(|then| {
            !offered
                .iter()
                .any(|now| now.name() == then.name() && now.schema() == then.schema())
        });
        if let Some(moved) = moved {
            drop(self.reaped(Some(hosted)));
            return Err(format!(
                "it came back without {}, or offering it under a different schema, so the \
                 tools this run published no longer describe it",
                moved.name()
            )
            .into_boxed_str());
        }

        live.hosted = Some(hosted);
        Ok(())
    }
}

impl Toolset for Hosting {
    fn prepare(&self, context: &ToolsetContext) -> Result<(), ToolsetError> {
        Toolset::prepare(&self.builtin, context)?;
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if !live.servers.is_empty() {
            return Ok(());
        }

        let mut servers = Vec::with_capacity(self.chosen.len());
        let mut offered = Vec::new();
        for chosen in &self.chosen {
            match self.host(chosen, context) {
                Ok((server, entries)) => {
                    servers.push(server);
                    offered.extend(entries);
                }
                // A server nobody said was required is one this run can do
                // without: its tools are simply not offered, and the turn goes
                // ahead with the rest. Saying `required` is what turns a
                // machine that is missing a program into a refused run.
                Err(_) if !chosen.required => {}
                Err(problem) => {
                    // Whatever did start belongs to a lifecycle that will now
                    // never exist, so it is stopped here: disposal reaches only
                    // what preparation recorded, and this record is discarded.
                    for started in &servers {
                        drop(started.release());
                    }
                    return Err(problem);
                }
            }
        }

        live.servers = servers;
        live.offered = offered;
        live.merged = None;
        Ok(())
    }

    fn snapshot(&self, _context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError> {
        self.generation()
    }

    fn refresh(&self, _context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError> {
        self.generation()
    }

    fn dispose(&self, context: &ToolsetContext) -> Result<(), ToolsetError> {
        let servers = {
            let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
            live.offered.clear();
            live.merged = None;
            std::mem::take(&mut live.servers)
        };

        // Every one of them, and the first refusal afterwards: a server that
        // could not be reaped must not leave the ones after it running.
        let mut refused = None;
        for server in &servers {
            if let Err(problem) = server.release() {
                refused = refused.or(Some(problem));
            }
        }
        Toolset::dispose(&self.builtin, context)?;
        refused.map_or(Ok(()), Err)
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

    fn run(&self, approved: Approved, context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let arguments = arguments(&self.called, approved.args())?;
        // Asked before anything is sent, because refusing here costs the far
        // end nothing at all: no process is disturbed and no budget is spent.
        // Once the frame has gone the answer is the one below, which is more
        // expensive and less certain.
        if context.cancel().requested() {
            return Err(ToolError::Cancelled(self.called.clone()));
        }

        let problem = match self.attempt(&arguments, context) {
            Ok(answered) => return Ok(said(&answered)),
            // A handle that outlived its lifecycle. Nothing broke and nothing
            // is running: the turn that read this catalogue is over, and
            // starting the server again here would give a finished run a
            // process no disposal will ever reach.
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
        if let Err(refused) = self.server.restart(after, Some(context.cancel())) {
            drop(self.server.release());
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
        self.attempt(&arguments, context)
            .map(|answered| said(&answered))
            .map_err(|again| match again {
                Refusal::Gone => self.stale(),
                Refusal::Unanswered(again) => {
                    if !again.settled() {
                        drop(self.server.release());
                    }
                    if again.interrupted() {
                        ToolError::Cancelled(self.called.clone())
                    } else {
                        self.broke(&again)
                    }
                }
            })
    }
}

impl Calling {
    /// Sends the call, with the interrupt that can end the waiting early.
    ///
    /// A wait somebody ends by pressing escape ends at the press rather than at
    /// the request patience. The lock is taken and released inside, because
    /// what happens next may be a restart and that takes the same lock.
    fn attempt(&self, arguments: &Value, context: &ToolContext<'_>) -> Result<Answered, Refusal> {
        let mut live = self
            .server
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(hosted) = live.hosted.as_mut() else {
            // The lifecycle that read this catalogue has ended, or a call
            // before this one left the conversation somewhere it could not come
            // back from.
            return Err(Refusal::Gone);
        };
        hosted
            .call(&self.offered, arguments, Some(context.cancel()))
            .map_err(Refusal::Unanswered)
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
