//! The Agent Client Protocol, spoken as its client to any agent that speaks it.
//!
//! [`Acp`] is a [`Dialect`] for one agent, described by its [`Profile`]: the
//! command it is found as, the protocol versions it may answer with, the check
//! its own description of itself must pass and the method it signs in with.
//! A run's first turn initializes the agent and reads what it says of itself
//! once, into an [`Initialized`]; nothing else is sent to an agent that answers
//! with a version or a description its profile does not admit. The session is
//! then opened, or resumed where the conversation has one, and the prompt goes
//! out once the agent has named it. Later turns of the run send the prompt
//! alone.
//!
//! This client offers the agent no file system and no terminal, so the one
//! request it answers is a permission. Any other request asks for authority
//! the agent was never given, and the turn ends incomplete rather than leave
//! it unanswered. A permission is answered with one of the options the agent
//! offered for it, and every question still open when the turn is stopped is
//! answered cancelled, as the protocol asks.
//!
//! Account steps are spoken over the same wire: each initializes the agent,
//! then asks whether a session can be opened, signs in, or signs out, and a
//! sign-out the agent does not offer is reported unverified rather than
//! assumed.

mod permission;
mod session;
mod wire;

use std::path::PathBuf;

use crucible_execution::{
    Authentication, Decision, Detail, Failure, Outcome, Pending, Text, TurnRequest,
};
use crucible_types::TurnId;
use crucible_transport::FRAME_BYTES;
use serde_json::{Value, json};

use crate::acp::permission::Permissions;
use crate::acp::session::Updates;
use crate::acp::wire::{Frame, Refused};
use crate::{Account, Act, Dialect, Ran, Reading, Request, Shape, Told};

/// The protocol version this client speaks.
const PROTOCOL: u64 = 1;

/// The code an agent refuses with when it must be signed in first.
const SIGN_IN_REQUIRED: i64 = -32000;

/// The most bytes a name the agent gives keeps: its own name, its version, a
/// sign-in method or a session.
const NAME_BYTES: usize = 256;

/// The most sign-in methods an agent's description keeps.
const METHODS: usize = 16;

/// The most options one permission question keeps.
const OPTIONS: usize = 16;

/// One agent that speaks the protocol, as this client runs it.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    /// The command it is looked up as.
    pub program: &'static str,
    /// The protocol versions its answer to initializing may carry.
    pub versions: &'static [u64],
    /// Whether the agent, as it described itself, is one this profile runs.
    pub admits: fn(&Initialized) -> Result<(), Failure>,
    /// The sign-in method it signs in with, where it has one.
    pub sign_in: Option<&'static str>,
}

/// What an agent said of itself when it was initialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Initialized {
    /// The protocol version it answered with.
    protocol: u64,
    /// Its name.
    name: Option<Text>,
    /// Its version.
    version: Option<Text>,
    /// Whether it can load a session it kept.
    loads: bool,
    /// Whether it can resume a session it kept without saying it again.
    resumes: bool,
    /// Whether it can sign out.
    signs_out: bool,
    /// The sign-in methods it offers, by id.
    methods: Vec<Text>,
}

impl Initialized {
    /// What the result of initializing says.
    fn read(result: &Value) -> Self {
        let named = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .map(|name| Text::clipped(name, NAME_BYTES))
        };
        let capabilities = result.get("agentCapabilities");
        let offered = |path: &[&str]| {
            path.iter()
                .fold(capabilities, |value, key| value?.get(key))
                .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
        };
        Self {
            protocol: result
                .get("protocolVersion")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            name: named(result.pointer("/agentInfo/name")),
            version: named(result.pointer("/agentInfo/version")),
            loads: offered(&["loadSession"]),
            resumes: offered(&["sessionCapabilities", "resume"]),
            signs_out: offered(&["auth", "logout"]),
            methods: result
                .get("authMethods")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|method| named(method.get("id")))
                .take(METHODS)
                .collect(),
        }
    }

    /// The protocol version it answered with.
    #[must_use]
    pub const fn protocol(&self) -> u64 {
        self.protocol
    }

    /// Its name, where it gave one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(Text::as_str)
    }

    /// Its version, where it gave one.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_ref().map(Text::as_str)
    }

    /// Whether it can load a session it kept.
    #[must_use]
    pub const fn loads(&self) -> bool {
        self.loads
    }

    /// Whether it can resume a session it kept.
    #[must_use]
    pub const fn resumes(&self) -> bool {
        self.resumes
    }

    /// Whether it can sign out.
    #[must_use]
    pub const fn signs_out(&self) -> bool {
        self.signs_out
    }

    /// Whether it offers the sign-in method `id`.
    #[must_use]
    pub fn offers(&self, id: &str) -> bool {
        self.methods.iter().any(|method| method.as_str() == id)
    }
}

/// A request this client sent and is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Awaited {
    /// Initializing, for a turn or for an account step.
    Initialize,
    /// Opening a session: fresh, or one kept.
    Open {
        /// Whether it is one kept.
        kept: bool,
    },
    /// The running turn's prompt.
    Prompt,
    /// Opening a session to learn whether one can be.
    Status,
    /// Signing in.
    SignIn,
    /// Signing out.
    SignOut,
}

/// One agent's wire, read and written as the protocol's client.
#[derive(Debug)]
pub struct Acp {
    /// The agent.
    profile: Profile,
    /// The directory sessions are opened in.
    workspace: Option<PathBuf>,
    /// The session to resume, where there is one.
    resume: Option<String>,
    /// The id the next request is sent under.
    next: u64,
    /// The request sent and not yet answered, with its id.
    awaited: Option<(u64, Awaited)>,
    /// The open session.
    session: Option<String>,
    /// The running turn.
    turn: TurnId,
    /// Its prompt, held until the session it goes to is open.
    held: Option<String>,
    /// Whether the running turn has been asked to stop.
    stopping: bool,
    /// The permission questions the running turn has open.
    permissions: Permissions,
    /// The reader of the open session's updates.
    updates: Updates,
}

impl Acp {
    /// The client of the agent `profile` describes.
    #[must_use]
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            workspace: None,
            resume: None,
            next: 0,
            awaited: None,
            session: None,
            turn: TurnId::FIRST,
            held: None,
            stopping: false,
            permissions: Permissions::default(),
            updates: Updates::fresh(),
        }
    }

    /// The request `method` with `params`, sent under the next id and waited
    /// on as `awaited`.
    fn send(&mut self, method: &str, params: &Value, awaited: Awaited) -> String {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.awaited = Some((id, awaited));
        wire::request(id, method, params)
    }

    /// The request that initializes the agent.
    fn initialize(&mut self) -> String {
        let params = json!({
            "protocolVersion": PROTOCOL,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false,
            },
            "clientInfo": {"name": "crucible", "version": env!("CARGO_PKG_VERSION")},
        });
        self.send("initialize", &params, Awaited::Initialize)
    }

    /// What the agent's answer to initializing it says, where its profile
    /// admits it.
    fn admitted(&self, result: &Value) -> Result<Initialized, Failure> {
        let initialized = Initialized::read(result);
        if !self.profile.versions.contains(&initialized.protocol) {
            return Err(Failure::new(
                "The agent answered with a protocol version this client does not speak.",
            ));
        }
        (self.profile.admits)(&initialized)?;
        Ok(initialized)
    }

    /// The directory a session is opened in, as the wire names it.
    fn cwd(&self) -> Result<String, Failure> {
        self.workspace
            .as_ref()
            .map(|workspace| workspace.display().to_string())
            .ok_or_else(|| Failure::new("There is no directory to open the agent's session in."))
    }

    /// The request that opens the session the conversation has, or a new one.
    fn open_session(&mut self, initialized: &Initialized) -> Result<String, Failure> {
        let cwd = self.cwd()?;
        let Some(kept) = self.resume.clone() else {
            let params = json!({"cwd": cwd, "mcpServers": []});
            return Ok(self.send("session/new", &params, Awaited::Open { kept: false }));
        };
        let params = json!({"sessionId": kept, "cwd": cwd, "mcpServers": []});
        let method = if initialized.resumes {
            "session/resume"
        } else if initialized.loads {
            "session/load"
        } else {
            return Err(Failure::new("The agent cannot resume a session it kept."));
        };
        Ok(self.send(method, &params, Awaited::Open { kept: true }))
    }

    /// The request that sends `prompt` to the open session.
    fn prompt(&mut self, prompt: &str) -> Result<String, Failure> {
        let session = self.session.clone().unwrap_or_default();
        let params = json!({"sessionId": session, "prompt": [{"type": "text", "text": prompt}]});
        let frame = self.send("session/prompt", &params, Awaited::Prompt);
        if frame.len() >= FRAME_BYTES {
            return Err(Failure::new("The prompt is too long for this agent's wire."));
        }
        Ok(frame)
    }

    /// The request awaited under `id`, which an answer settles.
    fn settle(&mut self, id: &Value) -> Result<Awaited, Failure> {
        match self.awaited {
            Some((awaited, what)) if id.as_u64() == Some(awaited) => {
                self.awaited = None;
                Ok(what)
            }
            _ => Err(Failure::new(
                "The agent answered a request this client was not waiting on.",
            )),
        }
    }

    /// What the answer `outcome` to the request `awaited` means for the turn.
    fn responded(
        &mut self,
        awaited: Awaited,
        outcome: Result<Value, Refused>,
    ) -> Result<Vec<Act>, Failure> {
        let result = match outcome {
            Ok(result) => result,
            Err(refused) => return Ok(self.ended(Act::Fail(refusal(&refused)))),
        };
        match awaited {
            Awaited::Initialize => {
                let opened = self
                    .admitted(&result)
                    .and_then(|initialized| self.open_session(&initialized));
                Ok(vec![opened.map_or_else(Act::Fail, Act::Send)])
            }
            Awaited::Open { kept } => {
                let named = if kept {
                    self.resume.clone()
                } else {
                    result
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .filter(|named| named.len() <= NAME_BYTES)
                        .map(str::to_owned)
                };
                let Some(named) = named else {
                    return Ok(vec![Act::Fail(Failure::new(
                        "The agent opened a session without a name it can be resumed by.",
                    ))]);
                };
                self.session = Some(named.clone());
                self.updates = if kept { Updates::resumed() } else { Updates::fresh() };
                let held = self.held.take().unwrap_or_default();
                Ok(vec![
                    Act::Session(named),
                    self.prompt(&held).map_or_else(Act::Fail, Act::Send),
                ])
            }
            Awaited::Prompt => {
                let outcome = match result.get("stopReason").and_then(Value::as_str) {
                    Some("end_turn") => Outcome::Answered,
                    Some("cancelled") => Outcome::Cancelled,
                    Some("max_tokens" | "max_turn_requests" | "refusal") => Outcome::CutShort,
                    _ => return Err(Failure::new("The agent ended its turn for no reason it named.")),
                };
                Ok(self.ended(Act::End(outcome)))
            }
            Awaited::Status | Awaited::SignIn | Awaited::SignOut => Err(Failure::new(
                "The agent answered an account step during a turn.",
            )),
        }
    }

    /// `end`, after the usage of the turn it ends.
    fn ended(&mut self, end: Act) -> Vec<Act> {
        self.updates
            .settled(self.turn)
            .into_iter()
            .chain([end])
            .collect()
    }

    /// The question the permission request `id` asks.
    fn asked(&mut self, id: &Value, params: &Value) -> Act {
        let native = id.to_string();
        let options = params
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                let id = option.get("optionId")?.as_str()?;
                let kind = option.get("kind")?.as_str()?;
                Some((id.into(), kind.into()))
            })
            .take(OPTIONS)
            .collect();
        self.permissions.offered(&native, options);
        let call = params.get("toolCall").unwrap_or(&Value::Null);
        let kind = call.get("kind").and_then(Value::as_str).unwrap_or("other");
        let title = call.get("title").and_then(Value::as_str).unwrap_or_default();
        let raw = |key: &str| {
            call.get("rawInput")
                .and_then(|input| input.get(key))
                .and_then(Value::as_str)
                .unwrap_or(title)
        };
        let detail = match kind {
            "read" | "search" => Detail::Reads,
            "edit" | "delete" | "move" => Detail::MutatesFile,
            "fetch" => Detail::Network {
                sent: Text::new(raw("url")),
            },
            // A command, and whatever the agent did not name, is asked as
            // what it says it would run, the widest thing it could be.
            _ => Detail::Command {
                sent: Text::new(raw("command")),
                left_running: false,
            },
        };
        Act::Ask {
            native,
            pending: Pending::Permission {
                tool: Text::new(kind),
                summary: Text::new(title),
                detail,
            },
        }
    }

    /// What the answer `outcome` to the account step `awaited` means.
    fn stepped(
        &mut self,
        step: Account,
        awaited: Awaited,
        outcome: Result<Value, Refused>,
    ) -> Told {
        let done = |reading| Told::Done(reading);
        match (awaited, outcome) {
            (Awaited::Initialize, Ok(result)) => {
                if step == Account::Version {
                    return done(Initialized::read(&result).version.map_or_else(
                        || Reading::Unreadable(Failure::new("The agent did not say its version.")),
                        Reading::Version,
                    ));
                }
                let initialized = match self.admitted(&result) {
                    Ok(initialized) => initialized,
                    Err(failure) => return done(Reading::Unreadable(failure)),
                };
                self.after_initializing(step, &initialized)
            }
            (Awaited::Status, Ok(_)) | (Awaited::SignIn, Ok(_)) => {
                done(Reading::Signed(Authentication::SignedIn))
            }
            (Awaited::Status, Err(refused)) if refused.code == SIGN_IN_REQUIRED => {
                done(Reading::Signed(Authentication::SignedOut))
            }
            (Awaited::SignOut, Ok(_)) => done(Reading::Signed(Authentication::SignedOut)),
            (_, Err(refused)) => done(Reading::Unreadable(refusal(&refused))),
            (Awaited::Open { .. } | Awaited::Prompt, Ok(_)) => done(Reading::Unreadable(
                Failure::new("The agent answered a turn's request during an account step."),
            )),
        }
    }

    /// What account step `step` sends once the agent is initialized as
    /// `initialized`.
    fn after_initializing(&mut self, step: Account, initialized: &Initialized) -> Told {
        let unreadable = |reason| Told::Done(Reading::Unreadable(Failure::new(reason)));
        match step {
            Account::Version => unreadable("The agent's version was asked twice."),
            Account::Status => match self.cwd() {
                Ok(cwd) => Told::Send(self.send(
                    "session/new",
                    &json!({"cwd": cwd, "mcpServers": []}),
                    Awaited::Status,
                )),
                Err(failure) => Told::Done(Reading::Unreadable(failure)),
            },
            Account::SignIn => match self.profile.sign_in {
                Some(method) if initialized.offers(method) => Told::Send(self.send(
                    "authenticate",
                    &json!({"methodId": method}),
                    Awaited::SignIn,
                )),
                _ => unreadable("The agent offers no sign-in this client uses."),
            },
            Account::SignOut if initialized.signs_out => {
                Told::Send(self.send("logout", &json!({}), Awaited::SignOut))
            }
            Account::SignOut => {
                unreadable("The agent offers no sign-out, so signing out is unverified.")
            }
        }
    }
}

/// What the agent's refusal `refused` says, for the user.
fn refusal(refused: &Refused) -> Failure {
    if refused.code == SIGN_IN_REQUIRED {
        return Failure::new("The agent is not signed in. Sign in with /login, then try again.");
    }
    Failure::new(&format!("The agent refused: {}", refused.message.as_str()))
}

impl Dialect for Acp {
    fn program(&self) -> &'static str {
        self.profile.program
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        self.workspace = shape.workspace.map(PathBuf::from);
        self.resume = shape.resume.map(str::to_owned);
        Request {
            arguments: Vec::new(),
            environment: Vec::new(),
        }
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        // A question a turn left open was answered or withdrawn when it
        // ended, so each turn's table begins empty and holds at most what
        // the host's own bound lets one turn raise.
        self.permissions = Permissions::default();
        self.stopping = false;
        self.turn = turn.turn();
        if self.session.is_some() {
            return Ok(vec![self.prompt(turn.prompt())?]);
        }
        if self.awaited.is_some() {
            return Err(Failure::new("The agent's session is still being opened."));
        }
        self.held = Some(turn.prompt().to_owned());
        Ok(vec![self.initialize()])
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        match wire::parse(frame)? {
            Frame::Response { id, outcome } => {
                let awaited = self.settle(&id)?;
                self.responded(awaited, outcome)
            }
            Frame::Request { id, method, params } if method == "session/request_permission" => {
                Ok(vec![self.asked(&id, &params)])
            }
            Frame::Request { .. } => Err(Failure::new(
                "The agent asked for something this client does not offer it.",
            )),
            Frame::Notification { method, params } if method == "session/update" => {
                let ours = self.session.is_some()
                    && params.get("sessionId").and_then(Value::as_str) == self.session.as_deref();
                let replaying = matches!(self.awaited, Some((_, Awaited::Open { .. })));
                Ok(match params.get("update") {
                    Some(update) if ours && !replaying => self.updates.heard(update),
                    _ => Vec::new(),
                })
            }
            Frame::Notification { .. } => Ok(Vec::new()),
        }
    }

    fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        if self.stopping {
            return self.permissions.withdraw(native);
        }
        self.permissions.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.stopping = true;
        let session = self.session.as_deref()?;
        Some(wire::notification("session/cancel", &json!({"sessionId": session})))
    }

    fn account(&mut self, _step: Account, shape: Shape<'_>) -> Option<Request> {
        Some(self.request(shape))
    }

    fn read(&self, _step: Account, _ran: &Ran) -> Reading {
        Reading::Unreadable(Failure::new("This agent's account steps are spoken."))
    }

    fn spoken(&mut self, _step: Account) -> Option<Vec<String>> {
        self.next = 0;
        Some(vec![self.initialize()])
    }

    fn told(&mut self, step: Account, frame: &str) -> Result<Told, Failure> {
        match wire::parse(frame)? {
            Frame::Response { id, outcome } => {
                let awaited = self.settle(&id)?;
                Ok(self.stepped(step, awaited, outcome))
            }
            Frame::Request { .. } => Err(Failure::new(
                "The agent asked for something during an account step.",
            )),
            Frame::Notification { .. } => Ok(Told::Nothing),
        }
    }
}

#[cfg(test)]
mod tests;
