//! Anthropic's Claude Code, driven over its own stream protocol in a
//! configuration directory of its own.
//!
//! [`Claude`] starts the installed program once for a run, printing frames
//! and reading them, in its `default` permission mode unless it was made for
//! a conversation's mode through the [`Modes`] table, with its permission
//! prompts sent to this side as questions, and with the model chosen for it
//! where one was, else its own default. Each turn is a user
//! frame on the same input, and ends at the `result` frame that answers it; a
//! run for a session the program kept starts by resuming it. The program is
//! started with only the environment its profile builds, so no key or cloud
//! account the shell holds reaches it, and whether it is signed in is read
//! from its own account commands in that profile.
//!
//! The turn's figures are read from its `result` frame: the tokens are that
//! turn's, the input summed from its uncached, cache-read and cache-written
//! parts, and the cost is the session's running total, so a turn reports the
//! difference from the turn before. A fresh session has cost nothing before
//! its first turn; a resumed one has cost an amount this run never heard, so
//! its first turn reports no cost and the turns after it do. A turn that
//! reports its tokens and no cost leaves the total unknown in the same way.
//!
//! Each turn opens with a frame saying how the program runs it, and a turn
//! it runs on a key its profile was not signed in with, in a permission mode
//! that asks less than the one it was started in, or as a release nobody
//! reviewed, is stopped as soon as that frame is read.
//!
//! Once a turn is told to stop, every question it asked is answered no,
//! whatever was decided: the program was told to stop, and nobody is left to
//! watch what a yes would run.
//!
//! Started without its bare mode, the program runs the hooks and connects the
//! servers a project's own settings name before it is asked anything.

mod account;
mod control;
mod manifest;
mod modes;
mod profile;
mod wire;

pub use account::sign_in;
pub use manifest::Manifest;
pub use modes::{Mode, Modes, Permission};

pub use crate::antigravity::Ended;

use std::path::PathBuf;

use crucible_execution::{
    Cost, Decision, Event, Failure, Observation, Outcome, Scope, Text, TurnRequest, Usage,
};
use crucible_transport::FRAME_BYTES;
use crucible_types::TurnId;
use serde_json::json;

use crate::{Account, Act, Dialect, Ran, Reading, Request, Shape, Told};
use control::Questions;
use wire::{Assistant, Ended as Result_, Frame, Init, Subtype};

/// The command it is looked up as.
const PROGRAM: &str = "claude";

/// The id of the one request this side makes: stopping the turn.
const INTERRUPT: &str = "crucible-interrupt";

/// Where the agent may say its key comes from when it sends requests as the
/// account its profile is signed in to; any other source, such as the
/// environment a project's settings set or a helper command, is refused.
const KEYS_SIGNED_IN: [&str; 2] = ["none", "/login managed key"];

/// The most calls that change a file a turn remembers until they report.
const EDITS: usize = 64;

/// What the session is known to have cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spent {
    /// This much, or nothing yet.
    Known(Option<Cost>),
    /// An amount nobody said.
    Unknown,
}

/// Claude Code's wire.
#[derive(Debug)]
pub struct Claude {
    /// The permission mode it is started in.
    permission: Permission,
    /// The directory the conversation works in, where it was named.
    workspace: Option<PathBuf>,
    /// The running turn.
    turn: Option<TurnId>,
    /// Whether the running turn was asked to stop.
    stopping: bool,
    /// Whether the running turn heard that the account is not signed in.
    signed_out: bool,
    /// The questions the running turn has open.
    questions: Questions,
    /// The running turn's calls that change a file, each its id and the file,
    /// until they report.
    edits: Vec<(Box<str>, Text)>,
    /// What the session had cost when the running turn began.
    spent: Spent,
}

impl Default for Claude {
    fn default() -> Self {
        Self::with(Permission::Default)
    }
}

impl Claude {
    /// The wire for a conversation in `mode`.
    #[must_use]
    pub fn in_mode(mode: Mode) -> Self {
        Self::with(Modes::permission(mode))
    }

    /// The wire started in `permission`.
    fn with(permission: Permission) -> Self {
        Self {
            permission,
            workspace: None,
            turn: None,
            stopping: false,
            signed_out: false,
            questions: Questions::default(),
            edits: Vec::new(),
            spent: Spent::Known(None),
        }
    }

    /// The session `init` opens, where the program runs it as it was
    /// started to.
    ///
    /// # Errors
    ///
    /// A [`Failure`] naming what differs, where it sends requests with a key
    /// the profile was not signed in with, runs in a mode that asks less
    /// than it was started in, or is a release nobody reviewed.
    fn opened(&self, init: Init) -> Result<String, Failure> {
        if !KEYS_SIGNED_IN.contains(&init.key.as_str()) {
            return Err(Failure::new(
                "The agent is using a key from its settings, a project's or a helper \
                 command, not the account its profile is signed in to, so the turn was \
                 stopped. Remove that key, then try again.",
            ));
        }
        if !Permission::named(&init.mode).is_some_and(|mode| self.permission.admits(mode)) {
            return Err(Failure::new(&format!(
                "The agent is running in its {} permission mode, not the {} mode it was \
                 started in, so the turn was stopped. Check the project's Claude Code \
                 settings, then try again.",
                init.mode,
                self.permission.name()
            )));
        }
        Manifest::REVIEWED.reviewed(&init.version)?;
        Ok(init.session)
    }

    /// The acts a message from the model comes to.
    fn said(&mut self, said: Assistant) -> Vec<Act> {
        self.signed_out |= said.signed_out;
        let mut acts = Vec::new();
        if said.main && !said.signed_out {
            acts.extend(
                said.text
                    .iter()
                    .map(|text| Act::Say(Event::Delta(Text::new(text)))),
            );
        }
        for (call, tool, input) in said.calls {
            let field = |name: &str| input.get(name).and_then(serde_json::Value::as_str);
            let edited = match tool.as_str() {
                "Edit" | "Write" | "MultiEdit" => field("file_path"),
                "NotebookEdit" => field("notebook_path"),
                _ => None,
            };
            if let Some(path) = edited
                && self.edits.len() < EDITS
            {
                self.edits.push((call.into(), Text::new(path)));
            }
            let summary = [
                "command",
                "file_path",
                "notebook_path",
                "path",
                "pattern",
                "url",
            ]
            .into_iter()
            .chain(["query", "description"])
            .find_map(field)
            .unwrap_or_default();
            acts.push(Act::Say(Event::ToolActivity {
                tool: Text::new(&tool),
                summary: Text::new(summary),
            }));
        }
        acts
    }

    /// The acts a `result` frame comes to: the turn's figures, then its end.
    fn ended(&mut self, ended: &Result_) -> Vec<Act> {
        let mut acts = Vec::from_iter(self.usage(ended).map(Act::Say));
        acts.push(match ended.subtype {
            Subtype::Success if !ended.failed => Act::End(Outcome::Answered),
            Subtype::DuringExecution if self.stopping => Act::End(Outcome::Cancelled),
            Subtype::Limit => Act::End(Outcome::CutShort),
            Subtype::Success | Subtype::DuringExecution if self.signed_out => Act::Fail(
                Failure::new("The agent is not signed in. Sign in with /login, then try again."),
            ),
            Subtype::Success => Act::Fail(Failure::new(&format!(
                "The agent ended the turn with an error: {}",
                ended.said.as_deref().unwrap_or("it gave no reason")
            ))),
            Subtype::DuringExecution => Act::Fail(Failure::new(
                "The agent stopped the turn with an error while it ran.",
            )),
        });
        self.turn = None;
        acts
    }

    /// The turn's figures, where the frame gives any.
    fn usage(&mut self, ended: &Result_) -> Option<Event> {
        let turn = self.turn?;
        let [uncached, read, written, output] = ended.tokens;
        let before = self.spent;
        // A turn that reported its tokens and not what the session cost
        // leaves the total unknown, so the next turn that does cannot tell
        // its own share. One that reported nothing, as a stopped turn may,
        // leaves it where it was.
        if let Some(now) = ended.cost {
            self.spent = Spent::Known(Some(now));
        } else if ended.tokens.iter().any(Option::is_some) {
            self.spent = Spent::Unknown;
        }
        let spent = ended.cost.and_then(|now| match before {
            Spent::Known(None) => Some(now),
            Spent::Known(Some(before)) if before.currency() == now.currency() => now
                .millionths()
                .checked_sub(before.millionths())
                .map(|millionths| Cost::new(millionths, now.currency())),
            Spent::Known(Some(_)) | Spent::Unknown => None,
        });
        let input = [uncached, read, written]
            .into_iter()
            .try_fold(0_u64, |sum, part| sum.checked_add(part?));
        let mut usage = Usage::of(turn);
        let mut any = false;
        for (figure, with) in [
            (input, Usage::input as fn(Usage, u64) -> Usage),
            (read, Usage::cache_read),
            (written, Usage::cache_write),
            (output, Usage::output),
        ] {
            if let Some(figure) = figure {
                usage = with(usage, figure);
                any = true;
            }
        }
        if let Some(spent) = spent {
            usage = usage.cost(spent);
            any = true;
        }
        any.then(|| Event::Usage(Observation::live(usage, Scope::Session)))
    }

    /// The acts the tool results `reported` come to: a changed file for each
    /// call that changes one and did not fail.
    fn reported(&mut self, reported: Vec<(String, bool)>) -> Vec<Act> {
        let mut acts = Vec::new();
        for (call, failed) in reported {
            if let Some(place) = self.edits.iter().position(|(edit, _)| **edit == *call) {
                let (_, path) = self.edits.remove(place);
                if !failed {
                    acts.push(Act::Say(Event::FileChanged { path }));
                }
            }
        }
        acts
    }

    /// The request a step or a run starts with, given `arguments`.
    fn request(arguments: Vec<String>, shape: Shape<'_>) -> Request {
        Request {
            arguments,
            environment: profile::environment(std::env::vars_os(), shape.profile),
        }
    }
}

impl Dialect for Claude {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        self.workspace = shape.workspace.map(Into::into);
        self.spent = if shape.resume.is_some() {
            Spent::Unknown
        } else {
            Spent::Known(None)
        };
        let mut arguments = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompt-tool=stdio",
            "--permission-mode",
            self.permission.name(),
        ]
        .map(str::to_owned)
        .to_vec();
        if let Some(model) = shape.model {
            arguments.extend(["--model".to_owned(), model.to_owned()]);
        }
        if let Some(resume) = shape.resume {
            arguments.extend(["--resume".to_owned(), resume.to_owned()]);
        }
        Self::request(arguments, shape)
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        let frame = json!({
            "type": "user",
            "session_id": "",
            "message": {"role": "user", "content": [{"type": "text", "text": turn.prompt()}]},
            "parent_tool_use_id": null,
        })
        .to_string();
        if frame.len() >= FRAME_BYTES {
            return Err(Failure::new(
                "The prompt is too long for this agent's wire.",
            ));
        }
        self.turn = Some(turn.turn());
        self.stopping = false;
        self.signed_out = false;
        self.questions.clear();
        self.edits.clear();
        Ok(vec![frame])
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        Ok(match wire::frame(frame)? {
            Frame::Session(init) => vec![Act::Session(self.opened(init)?)],
            Frame::Assistant(said) => self.said(said),
            Frame::Reported(reported) => self.reported(reported),
            Frame::Result(ended) => self.ended(&ended),
            Frame::Ask(ask) => {
                let native = ask.request.clone();
                let pending = self.questions.asked(ask, self.workspace.as_deref())?;
                vec![Act::Ask { native, pending }]
            }
            Frame::Asked(request) => vec![Act::Send(control::refusal(&request))],
            Frame::Answered(request) if request == INTERRUPT => Vec::new(),
            Frame::Answered(_) => {
                return Err(Failure::new(
                    "The agent answered a request this side never made.",
                ));
            }
            // A question withdrawn stays open here: the host still settles
            // it, and an answer to a withdrawn question is one the agent
            // passes over.
            Frame::Withdrawn(_) | Frame::Nothing => Vec::new(),
        })
    }

    fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        // A turn told to stop allows nothing more: a yes the user gave as
        // the stop went out would run what nobody is left to watch.
        let decision = if self.stopping {
            &Decision::Deny
        } else {
            decision
        };
        self.questions.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.stopping = true;
        Some(
            json!({
                "type": "control_request",
                "request_id": INTERRUPT,
                "request": {"subtype": "interrupt"},
            })
            .to_string(),
        )
    }

    fn account(&mut self, step: Account, shape: Shape<'_>) -> Option<Request> {
        if step == Account::Models {
            return Some(Self::request(
                account::STREAM.map(str::to_owned).to_vec(),
                shape,
            ));
        }
        let arguments = account::arguments(step)?;
        Some(Self::request(
            arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            shape,
        ))
    }

    fn read(&self, step: Account, ran: &Ran) -> Reading {
        account::read(step, ran)
    }

    fn spoken(&mut self, step: Account) -> Option<Vec<String>> {
        (step == Account::Models).then(|| vec![account::initialize()])
    }

    fn told(&mut self, step: Account, frame: &str) -> Result<Told, Failure> {
        match step {
            Account::Models => account::told(frame),
            Account::Version | Account::Status | Account::SignIn | Account::SignOut => {
                Err(Failure::new("This agent speaks no such account step."))
            }
        }
    }
}

#[cfg(test)]
mod tests;
