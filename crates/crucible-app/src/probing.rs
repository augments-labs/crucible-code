//! What the agent catalog's intake records of an agent by starting it once.
//!
//! [`probe`] starts the program an entry lists, from the directory its release
//! was unpacked into, through [`crate::launching::probing`]: the sandbox
//! required, the network closed and that directory read-only. It sends the one
//! `initialize` a client sends and keeps what comes back as JSON the
//! intake can compare from run to run: whether the sandbox closed the network,
//! the `agentInfo` the agent gave, each sign-in method with its kind and, for a
//! terminal method, the command, arguments and environment it names and
//! whether it says it needs a real terminal, the capabilities, and that the
//! conformance cases were not run, since every one of them needs a turn and a
//! turn needs an account.
//!
//! Nothing is signed in to and no credential is read; the program is started
//! with an environment that holds only what a program needs to run at all.
//! Everything kept is bounded before it is kept, and the output says when any
//! of it was cut. The agent's reply is hostile text: it is parsed here, once,
//! and nothing of it is passed on but what is written below.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crucible_execution::{Failure, Text, TurnRequest};
use crucible_external_harnesses::acp::Acp;
use crucible_external_harnesses::catalog::{CatalogError, Entry, INDEX_BYTES};
use crucible_external_harnesses::{Account, Act, Dialect, Ran, Reading, Request, Shape, Told};
use crucible_sandbox::SandboxService;
use crucible_sandbox_local::LocalSandbox;
use serde_json::{Map, Value, json};
use tokio::runtime::Handle;

use crate::installing::{Unchecked, bounded};
use crate::launching::{self, Probe, Sandbox, Unprobeable};
use crate::runtime::Unstarted;
use crate::services::{Unfinished, serving};

/// How long the agent is given to answer `initialize`.
pub const BOUND: Duration = Duration::from_secs(30);

/// The most bytes any one text of the agent's answer keeps.
const TEXT_BYTES: usize = 256;

/// The most sign-in methods, arguments to one, and variables of one, kept.
const ITEMS: usize = 16;

/// The most bytes of the agent's capabilities kept, as JSON.
const CAPABILITY_BYTES: usize = 4096;

/// The most bytes of what a method carries under `_meta` kept, as JSON.
const META_BYTES: usize = 1024;

/// The most lines the agent may say before it answers.
const LINES: usize = 64;

/// Why the probe has no JSON to give.
#[derive(Debug, thiserror::Error)]
pub enum Unprobed {
    /// The entry could not be read within its bound.
    #[error(transparent)]
    Read(#[from] Unchecked),
    /// The entry is not one as the catalog spells it.
    #[error("{} is not an entry as the catalog spells one", .path.display())]
    Entry {
        /// The file.
        path: PathBuf,
        /// Which field is wrong.
        #[source]
        source: CatalogError,
    },
    /// Starting the program without the sandbox was asked for on a process
    /// that does not say it is on a disposable runner.
    #[error(
        "--unconfined starts the agent without a sandbox, so it is refused unless the \
         environment variable CI is true, which says this is a disposable runner"
    )]
    NotDisposable,
    /// The program could not be started.
    #[error(transparent)]
    Start(#[from] Unprobeable),
    /// The runtime the program is spoken to on would not start.
    #[error(transparent)]
    Unstarted(#[from] Unstarted),
    /// The program did not answer `initialize`.
    #[error("the agent did not answer initialize: {0}")]
    Unanswered(Box<str>),
    /// The program refused `initialize`.
    #[error("the agent refused initialize: {0}")]
    Refused(Box<str>),
    /// The services the probe ran on had not finished when their bounds ran
    /// out.
    #[error(transparent)]
    Unfinished(#[from] Unfinished),
}

/// How the program is held in: by the sandbox, or, on a runner to be thrown
/// away, not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    /// Whether the program may be started where the sandbox cannot close the
    /// network.
    unconfined: bool,
}

impl Mode {
    /// The sandbox is required.
    pub const CONFINED: Self = Self { unconfined: false };

    /// The mode `unconfined` asks for, as the environment `lookup` reads
    /// allows: starting the program without the sandbox is allowed only where
    /// `CI` is `true`, which a process running on a disposable runner says.
    ///
    /// # Errors
    ///
    /// [`Unprobed::NotDisposable`] where it is asked for and `CI` is not
    /// `true`.
    pub fn of(
        unconfined: bool,
        lookup: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<Self, Unprobed> {
        if unconfined && lookup("CI").as_deref() != Some(std::ffi::OsStr::new("true")) {
            return Err(Unprobed::NotDisposable);
        }
        Ok(Self { unconfined })
    }
}

/// The sandbox the application's commands run in, on `runtime`.
#[must_use]
pub fn local(runtime: Handle) -> Arc<dyn SandboxService> {
    Arc::new(LocalSandbox::new().watching_on(runtime))
}

/// Starts the program the entry at `entry` lists, found under the directory
/// `root` its release was unpacked into, in the sandbox `sandbox` makes on
/// the application's runtime, sends it `initialize`, and gives what it
/// answered as JSON ending in a line feed. The agent is given `within` to
/// answer.
///
/// Where that sandbox cannot close the network nothing is started, unless
/// `mode` is the unconfined one, which only a disposable runner is given, and
/// then the output says the sandbox was unavailable and why.
///
/// # Errors
///
/// [`Unprobed`] saying why there is nothing to record.
pub fn probe(
    root: &Path,
    entry: &Path,
    mode: Mode,
    sandbox: &dyn Fn(Handle) -> Arc<dyn SandboxService>,
    within: Duration,
) -> Result<String, Unprobed> {
    let ceiling = u64::try_from(INDEX_BYTES).unwrap_or(u64::MAX);
    let text = bounded(entry, ceiling)?;
    let described = Entry::unsigned(&text).map_err(|source| Unprobed::Entry {
        path: entry.to_owned(),
        source,
    })?;
    let (recorded, unfinished) = serving(|services| {
        let runtime = services.runtime().handle()?;
        runtime.block_on(initialized(
            sandbox(runtime.clone()),
            runtime.clone(),
            root,
            &described,
            mode.unconfined,
            within,
        ))
    });
    let recorded = recorded?;
    unfinished?;
    Ok(recorded)
}

/// Starts the program and records its answer, on the runtime that is polling
/// this.
// Each is an input of its own to the probe, and a value gathering them would
// be built at its one caller and taken apart here.
#[allow(clippy::too_many_arguments)]
async fn initialized(
    service: Arc<dyn SandboxService>,
    runtime: Handle,
    root: &Path,
    entry: &Entry,
    unconfined: bool,
    within: Duration,
) -> Result<String, Unprobed> {
    let Probe {
        host,
        dialect,
        sandbox,
    } = launching::probing(service, runtime, root, entry, unconfined).await?;
    let mut hearing = Hearing::new(dialect);
    let frames = hearing
        .spoken(Account::Version)
        .ok_or_else(|| Unprobed::Unanswered("this agent speaks no initialize".into()))?;
    let reading = host
        .speak(&mut hearing, Account::Version, &frames, &|_| {}, within)
        .await;
    let Some(said) = hearing.answered else {
        let why = match reading {
            Reading::Unreadable(failure) => failure.reason().as_str().to_owned(),
            Reading::Version(_) | Reading::Signed(_) => "it said nothing".to_owned(),
        };
        return Err(Unprobed::Unanswered(why.into()));
    };
    let recorded = recorded(&sandbox, &said)?;
    let mut written = serde_json::to_string_pretty(&recorded).map_err(|_| {
        Unprobed::Unanswered("what it answered could not be written as JSON".into())
    })?;
    written.push('\n');
    Ok(written)
}

/// The agent's wire, listening for the one answer the probe waits on and
/// keeping it whole, which the wire's own reading of it would not.
struct Hearing {
    /// The wire, which says what is sent.
    acp: Acp,
    /// How many lines the agent has said.
    lines: usize,
    /// The answer to `initialize`, once it has come.
    answered: Option<Map<String, Value>>,
}

impl Hearing {
    const fn new(acp: Acp) -> Self {
        Self {
            acp,
            lines: 0,
            answered: None,
        }
    }
}

impl Dialect for Hearing {
    fn program(&self) -> &str {
        self.acp.program()
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        self.acp.request(shape)
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        self.acp.open(turn)
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        self.acp.heard(frame)
    }

    fn answer(
        &mut self,
        native: &str,
        decision: &crucible_execution::Decision,
    ) -> Result<String, Failure> {
        self.acp.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.acp.cancel()
    }

    fn account(&mut self, step: Account, shape: Shape<'_>) -> Option<Request> {
        self.acp.account(step, shape)
    }

    fn read(&self, step: Account, ran: &Ran) -> Reading {
        self.acp.read(step, ran)
    }

    fn spoken(&mut self, step: Account) -> Option<Vec<String>> {
        self.acp.spoken(step)
    }

    fn told(&mut self, _step: Account, frame: &str) -> Result<Told, Failure> {
        self.lines = self.lines.saturating_add(1);
        if self.lines > LINES {
            return Err(Failure::new(
                "the agent said more than is kept before it answered",
            ));
        }
        let Ok(Value::Object(said)) = serde_json::from_str::<Value>(frame) else {
            return Err(Failure::new("the agent said something that is not JSON"));
        };
        let answers = said.get("id") == Some(&json!(0))
            && (said.contains_key("result") || said.contains_key("error"));
        if answers {
            self.answered = Some(said);
            return Ok(Told::Done(Reading::Version(Text::new("initialized"))));
        }
        if said.contains_key("method") && said.contains_key("id") {
            return Err(Failure::new(
                "the agent asked for something before it was initialized",
            ));
        }
        Ok(Told::Nothing)
    }
}

/// What is kept of an agent's answer, and whether any of it was cut.
#[derive(Default)]
struct Kept {
    /// Whether anything was left out.
    cut: bool,
}

impl Kept {
    /// `value` as text cut to [`TEXT_BYTES`] bytes on a character boundary;
    /// nothing where it is not text.
    fn text(&mut self, value: Option<&Value>) -> Value {
        match value.and_then(Value::as_str) {
            Some(text) => Value::String(self.clipped(text)),
            None => Value::Null,
        }
    }

    /// `text` cut to [`TEXT_BYTES`] bytes on a character boundary.
    fn clipped(&mut self, text: &str) -> String {
        let end = text.floor_char_boundary(TEXT_BYTES);
        self.cut |= end < text.len();
        text.get(..end).unwrap_or_default().to_owned()
    }

    /// The first [`ITEMS`] of `items`.
    fn items<T>(&mut self, items: impl IntoIterator<Item = T>) -> Vec<T> {
        let mut kept: Vec<T> = items.into_iter().take(ITEMS + 1).collect();
        if kept.len() > ITEMS {
            self.cut = true;
            kept.truncate(ITEMS);
        }
        kept
    }

    /// `value` as it is where it is at most `ceiling` bytes of JSON, else
    /// nothing.
    fn json(&mut self, value: Option<&Value>, ceiling: usize) -> Value {
        let Some(value) = value else {
            return Value::Null;
        };
        if value.to_string().len() > ceiling {
            self.cut = true;
            return Value::Null;
        }
        value.clone()
    }
}

/// What the intake records of the answer `said` to `initialize`, from an
/// agent held in as `sandbox` says.
fn recorded(sandbox: &Sandbox, said: &Map<String, Value>) -> Result<Value, Unprobed> {
    let mut kept = Kept::default();
    if let Some(error) = said.get("error") {
        let message = kept.text(error.get("message"));
        return Err(Unprobed::Refused(
            message.as_str().unwrap_or("with no message").into(),
        ));
    }
    let result = said.get("result");
    let methods: Vec<Value> = kept
        .items(
            result
                .and_then(|result| result.get("authMethods"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .into_iter()
        .map(|method| method_of(&mut kept, method))
        .collect();
    let mut recorded = Map::new();
    recorded.insert(
        "sandbox".into(),
        match sandbox {
            Sandbox::Closed => json!("closed"),
            Sandbox::Unavailable(why) => json!(format!("unavailable: {}", kept.clipped(why))),
        },
    );
    recorded.insert(
        "agentInfo".into(),
        json!({
            "name": kept.text(result.and_then(|result| result.pointer("/agentInfo/name"))),
            "version": kept.text(result.and_then(|result| result.pointer("/agentInfo/version"))),
        }),
    );
    recorded.insert("authMethods".into(), Value::Array(methods));
    recorded.insert(
        "capabilities".into(),
        kept.json(
            result.and_then(|result| result.get("agentCapabilities")),
            CAPABILITY_BYTES,
        ),
    );
    recorded.insert("conformance".into(), json!("not run"));
    if kept.cut {
        recorded.insert("incomplete".into(), json!(true));
    }
    Ok(Value::Object(recorded))
}

/// One sign-in method as the intake records it: its id, its name and its kind,
/// which is `agent` where the agent names none, and for a terminal method the
/// command, arguments and environment it names, and whether it needs a real
/// terminal, which is `unknown` unless the method says.
fn method_of(kept: &mut Kept, method: &Value) -> Value {
    let kind = match method.get("type").and_then(Value::as_str) {
        None | Some("agent") => "agent",
        Some("env_var") => "env_var",
        Some("terminal") => "terminal",
        Some(_) => "other",
    };
    let mut recorded = Map::new();
    recorded.insert("id".into(), kept.text(method.get("id")));
    recorded.insert("name".into(), kept.text(method.get("name")));
    recorded.insert("kind".into(), json!(kind));
    if kind == "terminal" {
        recorded.insert("command".into(), kept.text(method.get("command")));
        let args: Vec<Value> = kept
            .items(
                method
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .into_iter()
            .map(|arg| kept.text(Some(arg)))
            .collect();
        recorded.insert("args".into(), Value::Array(args));
        let env: Map<String, Value> = kept
            .items(
                method
                    .get("env")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten(),
            )
            .into_iter()
            .map(|(name, value)| (kept.clipped(name), kept.text(Some(value))))
            .collect();
        recorded.insert("env".into(), Value::Object(env));
        let meta = method.get("_meta");
        let needs = meta
            .and_then(|meta| meta.get("tty"))
            .and_then(Value::as_bool);
        recorded.insert(
            "tty".into(),
            needs.map_or_else(|| json!("unknown"), Value::Bool),
        );
        if meta.is_some() {
            recorded.insert("meta".into(), kept.json(meta, META_BYTES));
        }
    }
    Value::Object(recorded)
}

#[cfg(test)]
mod tests;
