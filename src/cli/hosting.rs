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
    Approved, Command, Finish, SandboxCommand, SandboxEnvironment, SandboxId, SandboxManifest,
    SandboxPolicy, SandboxRequest, SandboxService, Sensitivity, Summary, Tool, ToolArgs,
    ToolContext, ToolDescriptor, ToolEntry, ToolError, ToolId, ToolOutput, ToolProvenance,
    ToolSnapshot, ToolSourceKind, Toolset, ToolsetContext, ToolsetError,
};
use crucible_mcp::{Hosted, Offered};
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
}

/// The built-in roster, and whatever the selected servers offered.
pub(crate) struct Hosting {
    builtin: Tools,
    chosen: Vec<Chosen>,
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
    /// How long it is given to go on its own at disposal.
    grace: Duration,
    /// Taken at disposal, because stopping consumes the conversation.
    ///
    /// This is the whole of a handle's validity: a handle that finds nothing
    /// here is one whose lifecycle has ended, and there is no second flag
    /// beside it to disagree with.
    talking: Mutex<Option<Hosted>>,
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
            chosen,
            sandbox,
            live: Mutex::new(Live::default()),
        }
    }

    /// Starts one server, greets it, and turns its catalogue into entries.
    fn host(
        &self,
        chosen: &Chosen,
        context: &ToolsetContext,
    ) -> Result<(Arc<Server>, Vec<ToolEntry>), ToolsetError> {
        let refused = |problem: &dyn std::fmt::Display| ToolsetError::Source {
            id: chosen.name.clone(),
            problem: problem.to_string().into(),
        };

        // A call identity of its own rather than the run's: what the audit is
        // about here is the server, and every tool it later offers is one call
        // inside this one process.
        let request = SandboxRequest::new(
            SandboxId::new(),
            context.ancestry(),
            ToolId::new(format!("{NAMESPACE}{OF}{}", chosen.name)),
            chosen.policy.clone(),
            SandboxManifest::empty(),
        );
        let mut session = self.sandbox.prepare(request).map_err(|e| refused(&e))?;
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
        let greeting = hosted.greet().map_err(|e| refused(&e))?;
        // Under the other number from here on: the greeting was a peer reading
        // from a table, and everything after it is a peer doing work.
        hosted.patient_for(chosen.request);
        let offered = hosted.catalogue(&greeting).map_err(|e| refused(&e))?;

        let provenance = ToolProvenance::new(
            ToolSourceKind::Mcp,
            format!("{NAMESPACE}{OF}{}", chosen.name),
            format!("MCP server {} at {}", chosen.name, chosen.program.display()),
        )?;
        let program: Box<str> = chosen.program.display().to_string().into();
        let server = Arc::new(Server {
            name: chosen.name.clone(),
            grace: chosen.grace,
            talking: Mutex::new(Some(hosted)),
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
            .talking
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(hosted) = taken else {
            return Ok(());
        };

        match hosted.stop(self.grace).finish {
            Finish::Exited(_) | Finish::Stopped => Ok(()),
            Finish::Unreaped(problem) => Err(ToolsetError::Source {
                id: self.name.clone(),
                problem: problem.to_string().into(),
            }),
        }
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
        // Asked before anything is sent, because that is where this can still
        // answer. Once a call is on its way the wait belongs to the request
        // deadline the record set: the conversation is a blocking read of one
        // pipe, and an interrupt cannot reach into it.
        if context.cancel().requested() {
            return Err(ToolError::Cancelled(self.called.clone()));
        }
        let mut talking = self
            .server
            .talking
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(hosted) = talking.as_mut() else {
            // The lifecycle that read this catalogue has ended, and a handle
            // outlives it whenever a turn was abandoned mid-call.
            return Err(ToolError::StaleGeneration {
                tool: self.called.clone(),
            });
        };

        let answered = hosted
            .call(&self.offered, &arguments)
            .map_err(|problem| ToolError::Io {
                tool: self.called.clone(),
                problem: format!("the MCP server {} could not answer", self.server.name).into(),
                source: io::Error::other(problem.to_string()),
            })?;

        let mut said = answered.text().to_owned();
        // A result that lost something says so. The mark inside the text says
        // where the cut was; this says how much went, which is the part a
        // reader of what survived has no way to work out.
        if answered.omitted() > 0 {
            let omitted = answered.omitted();
            let _ = write!(
                said,
                "\n[…crucible left out {omitted} bytes of this result…]"
            );
        }

        Ok(if answered.failed() {
            ToolOutput::failed(said)
        } else {
            ToolOutput::ok(said)
        })
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
