//! `crucible agents`: the agent catalog's commands, from a command line.
//!
//! `search`, `install`, `add`, `update`, `rollback`, `remove`, `verify`,
//! `sign-in` and `try` are each a request to [`crucible_app::client::agents::run`], the
//! door every front end asks, sent with no conversation: the door is lent
//! [`super::client::shelf`] with nothing to start an agent with. The door asks
//! before anything is fetched or changed, and the question is asked here, on
//! this process's own terminal: written to standard error and answered on
//! standard input, by a choice's number alone on a line that ends within
//! [`auth::MAX_ANSWER`] bytes, anything else being no. Where either is not a
//! terminal nothing is asked, so nothing changes; an update the user's own
//! file lets the door take without asking says that it was taken so, and a
//! move says which instances it moved and why it was not smoke-tested, where
//! it was not. A sign-in that finds no key kept for its method asks for one at
//! the prompt `crucible auth login` hides a key at, and hands it to the door to
//! keep. A sign-in the agent runs as a command, with no real terminal, needs
//! standard input, standard output and standard error all at one: the door is
//! lent a launcher, under the sandbox the user's own file says, and a
//! [`Console`], which writes each line the command says to standard error and
//! sends the command each line typed. A sign-in the catalog records as needing
//! a real terminal needs the same three streams on a terminal, and is lent the
//! same [`Console`] as its terminal: the command runs there unconfined, after
//! a confirmation made for that run, with its own process group in the
//! foreground. Anywhere else nothing is lent and the door refuses it, saying a
//! terminal is needed. How far a download has come
//! goes to standard error, on one line
//! rewritten in place where standard error is a terminal and as one line where
//! it is not; the answer goes to standard output and a refusal to standard
//! error, written [`failing`]. What the catalog says is hostile, so whatever
//! of it is written is escaped first.
//!
//! `search` and `try` are lent a launcher and its steps, under the sandbox the
//! user's own file says, as a sign-in run as a command is: the listing then
//! names, beside each agent, the user's own program of it found on `PATH`, and
//! the user's own Antigravity after them with what it sees of its sign-in. A
//! search that cannot be lent them lists as before, naming none. `try` asks
//! the door for its agent's own program alone, as a listing would name it
//! wherever its row would fall, and sends the door that move, which asks on
//! this terminal, as an install does, before the program is started once,
//! confined with the network closed, and writes the instance that runs it
//! once it answers as an agent; where the door finds none, it is refused and
//! nothing is started.
//!
//! `crucible agents unpack-check`, which the help leaves out, is the agent
//! catalog's intake asking how a download, or a package tree from its lock
//! and tarballs, unpacks by the rules an install holds it to; the file list
//! it records goes to standard output as
//! [`crucible_app::installing::unpack_check`] gives it, and a refusal to
//! standard error, written [`failing`].
//!
//! `crucible agents probe`, which the help leaves out as well, is the same
//! intake starting the agent once in a sandbox with no network and asking it to
//! initialize; what [`crucible_app::probing::probe`] records goes to standard
//! output, and a refusal to standard error, written [`failing`].

use std::fmt::Write as _;
use std::future::Future;
use std::io::{self, BufRead, BufReader, IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crucible_app::catalog_fetch::trust_in;
use crucible_app::client::agents::{
    self as door, AgentsError, HandoverFailed, How, Listed, Outcome, OwnAntigravity, OwnProgram,
    SaidLine, Shelf, SignInChannel, SignInTerminal, Typed, Typing,
};
use crucible_app::client::{Confirmation, Front, Shown};
use crucible_app::installing;
use crucible_app::probing::{self, Mode};
use crucible_app::services::{Services, serving};
use crucible_client_api::{
    Capabilities, Capability, Command, Correlation, Decision, ErrorCode, InstallProgress, Name,
    Pending, Refusal, Request, Said, Text,
};
use crucible_config::Home;
use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox::SandboxService;
use crucible_tui::Held;
use crucible_types::AgentKey;
use crucible_types::shown::escaped;
use tokio::runtime::Handle;
use tokio::sync::{Mutex, mpsc};

use super::AgentsAction;
use super::auth::{self, Unstored};
use super::client::Suspend;
use super::ending::Told;
use super::failure::failing;
use super::visible;

#[cfg(test)]
mod tests;

/// Runs `crucible agents` as `action` asks, and stops.
pub(super) fn acted(action: &AgentsAction) -> ExitCode {
    match action {
        AgentsAction::UnpackCheck {
            entry,
            artifact,
            out,
        } => unpack_check(entry, artifact, out),
        AgentsAction::Probe {
            root,
            entry,
            unconfined,
        } => probe(root, entry, *unconfined),
        _ => served(action),
    }
}

/// Carries out `action` at the agents door, on the application's runtime and
/// for crucible's home as this process's environment names it, and writes
/// what came of it.
fn served(action: &AgentsAction) -> ExitCode {
    let home = match Home::find(&|name| std::env::var_os(name)) {
        Ok(home) => home,
        Err(homeless) => {
            let _ = io::stderr().write_all(failing(&homeless.to_string()).as_bytes());
            return ExitCode::FAILURE;
        }
    };
    let said = |text: &str| {
        let _ = io::stderr().write_all(text.as_bytes());
    };
    let terminal = io::stderr().is_terminal();
    let answer = askable(io::stdin().is_terminal(), terminal).then_some(answer as fn() -> String);
    let streams = (
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        terminal,
    );
    let (answer, stopped) = serving(|services| match services.runtime().handle() {
        Ok(runtime) => Desk {
            home: &home,
            shelf: super::client::shelf(&home, None, None),
            runtime: &runtime,
            said: &said,
            terminal,
            answer,
            console: console_where(streams, services),
            services: Some(services),
        }
        .carried(action, &mut typed),
        Err(unstarted) => Answer::Refused(caused(&unstarted)),
    });
    let mut exit = match &answer {
        Answer::Done(text) => {
            let _ = io::stdout().write_all(visible(text).as_bytes());
            ExitCode::SUCCESS
        }
        Answer::Differs(text) => {
            let _ = io::stdout().write_all(visible(text).as_bytes());
            ExitCode::FAILURE
        }
        Answer::Refused(problem) => {
            let _ = io::stderr().write_all(failing(problem).as_bytes());
            ExitCode::FAILURE
        }
        Answer::Stopped(_) => ExitCode::FAILURE,
    };
    if let Err(unfinished) = stopped {
        let _ = io::stderr().write_all(failing(&caused(&unfinished)).as_bytes());
        exit = ExitCode::FAILURE;
    }
    // Obeyed once everything else is said and the runtime is shut down.
    match answer {
        Answer::Stopped(told) => told.obeyed(),
        Answer::Done(_) | Answer::Differs(_) | Answer::Refused(_) => exit,
    }
}

/// Whether an install's question is asked: where standard input, which
/// answers it, and standard error, where it is written, are both terminals.
fn askable(stdin: bool, stderr: bool) -> bool {
    stdin && stderr
}

/// Whether a key is asked for at the prompt that hides it: where standard
/// input, output and error are each a terminal.
fn promptable(stdin: bool, stdout: bool, stderr: bool) -> bool {
    stdin && stdout && stderr
}

/// What a [`Desk`] is lent to read lines at a console: `services`, where
/// standard input, output and error, in that order, are each a terminal, and
/// otherwise nothing.
fn console_where<T>((stdin, stdout, stderr): (bool, bool, bool), services: T) -> Option<T> {
    promptable(stdin, stdout, stderr).then_some(services)
}

/// Why a key cannot be asked for here, where standard input, output or error
/// is not a terminal.
const UNPROMPTED: &str = "the key is asked for at a prompt that hides it, which needs standard \
                          input, output and error on a terminal, and one of them is not; run \
                          `crucible agents sign-in` on a terminal, without redirecting it";

/// The key for the instance `instance`, typed at a prompt on this terminal
/// that hides it, or `None` where it was left without one. A key is never
/// read from an argument or a pipe.
fn typed(instance: &str) -> Result<Option<Typed>, Unstored> {
    prompted(
        promptable(
            io::stdin().is_terminal(),
            io::stdout().is_terminal(),
            io::stderr().is_terminal(),
        ),
        instance,
    )
}

/// The key for `instance`, asked for at the hidden prompt where it is
/// `promptable`, and refused without a word asked where it is not.
fn prompted(promptable: bool, instance: &str) -> Result<Option<Typed>, Unstored> {
    if !promptable {
        return Err(Unstored::Refused(UNPROMPTED.to_owned()));
    }
    Ok(auth::hidden(instance)?.map(Typed::new))
}

/// What one command came to, for the caller to write.
#[derive(Debug)]
enum Answer {
    /// What it did, for standard output; it ends 0.
    Done(String),
    /// What it found, for standard output, where something differs from what
    /// was installed; it ends 1.
    Differs(String),
    /// Why it did not do what was asked, for standard error; it ends 1.
    Refused(String),
    /// The process was told to stop while the key's prompt stood, and the
    /// prompt has been put away.
    Stopped(Told),
}

/// What one command is carried out with.
struct Desk<'a> {
    /// Crucible's home.
    home: &'a Home,
    /// What the agents door is lent.
    shelf: Shelf<'a>,
    /// The runtime the door is waited for on.
    runtime: &'a Handle,
    /// Where what goes to standard error is written, as it is given.
    said: &'a (dyn Fn(&str) + Sync),
    /// Whether standard error is a terminal, so that a download's progress
    /// is rewritten in place.
    terminal: bool,
    /// Where a question's answer is read from, where it can be asked at all.
    answer: Option<fn() -> String>,
    /// What a sign-in the agent runs as a command is started with, where this
    /// process has a terminal at standard input and both outputs to run it on.
    /// With none, the door is lent nothing and refuses one.
    console: Option<&'a Services>,
    /// What a launcher and its steps are lent from, so that a listing finds
    /// the user's own programs and a try starts one; with none, a listing
    /// names none and a try is refused.
    services: Option<&'a Services>,
}

impl Desk<'_> {
    /// Carries out `action`, asking `key` for a key a sign-in needs and finds
    /// none kept for.
    fn carried(
        &self,
        action: &AgentsAction,
        key: &mut dyn FnMut(&str) -> Result<Option<Typed>, Unstored>,
    ) -> Answer {
        let named = |agent: &str, command: fn(Name) -> Command| match Name::new(agent) {
            Ok(agent) => self.answered(self.ask(command(agent)), None),
            Err(refusal) => Answer::Refused(refusal.to_string()),
        };
        match action {
            AgentsAction::Search { text } => self.answered(self.listed(), text.as_deref()),
            AgentsAction::Install { agent, version } => {
                self.installed(agent, version.as_deref(), How::Installed)
            }
            AgentsAction::Add { agent, version } => {
                self.installed(agent, version.as_deref(), How::Added)
            }
            AgentsAction::Update { agent } => named(agent, |agent| Command::UpdateAgent { agent }),
            AgentsAction::Rollback { agent } => {
                named(agent, |agent| Command::RollbackAgent { agent })
            }
            AgentsAction::Remove { agent } => named(agent, |agent| Command::RemoveAgent { agent }),
            AgentsAction::Verify { agent } => named(agent, |agent| Command::VerifyAgent { agent }),
            AgentsAction::SignIn { instance, method } => {
                self.signed_in(instance, method.as_deref(), key)
            }
            AgentsAction::Try { agent } => self.tried(agent),
            // The catalog's intake runs these, and they ask the door nothing.
            AgentsAction::UnpackCheck { .. } | AgentsAction::Probe { .. } => Answer::Refused(
                "the catalog's intake runs this command, which asks the agents door nothing"
                    .to_owned(),
            ),
        }
    }

    /// Installs or adds `agent` at `version`, or at the newest release the
    /// catalog offers this platform where no version is named.
    fn installed(&self, agent: &str, version: Option<&str>, how: How) -> Answer {
        let version = match version {
            Some(version) => version.to_owned(),
            None => match self.newest(agent) {
                Ok(version) => version,
                Err(refused) => return refused,
            },
        };
        let (agent, version) = match (Name::new(agent), Name::new(&version)) {
            (Ok(agent), Ok(version)) => (agent, version),
            (Err(refusal), _) | (_, Err(refusal)) => return Answer::Refused(refusal.to_string()),
        };
        let command = if how == How::Added {
            Command::AddAgent { agent, version }
        } else {
            Command::InstallAgent { agent, version }
        };
        self.answered(self.ask(command), None)
    }

    /// The newest release of `agent` the catalog offers this platform, as the
    /// door reads it once it has refreshed the catalog.
    fn newest(&self, agent: &str) -> Result<String, Answer> {
        let unrefreshed = match self.ask(Command::AgentCatalog) {
            Ok(Outcome::Catalog { unrefreshed, .. }) => unrefreshed,
            other => return Err(self.answered(other, None)),
        };
        let Ok(key) = AgentKey::new(agent) else {
            return Err(Answer::Refused(
                Refusal::from(ErrorCode::InvalidArgument).to_string(),
            ));
        };
        if let Some(offer) = trust_in(self.home.path())
            .offers(Some(&key))
            .into_iter()
            .next()
        {
            return Ok(offer.pin.version.into());
        }
        let mut refused = caused(&AgentsError::Unlisted(key));
        if let Some(why) = unrefreshed {
            let _ = write!(
                refused,
                "; the catalog could not be refreshed first: {}",
                caused(&why)
            );
        }
        Err(Answer::Refused(refused))
    }

    /// Signs the configured `instance` in by `method`, or by the one method
    /// its agent offers where none is named, asking `key` for a key where the
    /// method takes one and none is kept.
    fn signed_in(
        &self,
        instance: &str,
        method: Option<&str>,
        key: &mut dyn FnMut(&str) -> Result<Option<Typed>, Unstored>,
    ) -> Answer {
        let method = match method {
            Some(method) => method.to_owned(),
            None => match door::sign_in_methods(self.home, instance) {
                Err(refused) => return Answer::Refused(caused(&refused)),
                Ok(methods) => match methods.as_slice() {
                    [one] => one.to_string(),
                    [] => {
                        return Answer::Refused(format!(
                            "{instance}'s agent offers no way to sign in"
                        ));
                    }
                    several => {
                        return Answer::Refused(format!(
                            "{instance} signs in by several methods: {}; name one with --method",
                            several.join(", ")
                        ));
                    }
                },
            },
        };
        let command = match (Name::new(instance), Name::new(&method)) {
            (Ok(instance), Ok(method)) => Command::SignInAgent { instance, method },
            (Err(refusal), _) | (_, Err(refusal)) => return Answer::Refused(refusal.to_string()),
        };
        if let Some(services) = self.console {
            match door::runs_a_command(self.home, instance, &method) {
                Ok(true) => return self.answered(self.at_console(services, command), None),
                Ok(false) => {}
                Err(refused) => return Answer::Refused(caused(&refused)),
            }
        }
        match self.ask(command.clone()) {
            Err(AgentsError::Unstored { .. }) => {}
            asked => return self.answered(asked, None),
        }
        match key(instance) {
            Ok(Some(typed)) => {
                if let Err(refused) = door::keep_key(self.home, instance, &method, typed) {
                    return Answer::Refused(caused(&refused));
                }
            }
            Ok(None) => return Answer::Refused("nothing was typed, so no key was kept".to_owned()),
            Err(Unstored::Refused(problem)) => return Answer::Refused(problem),
            Err(Unstored::Ended(told)) => return Answer::Stopped(told),
        }
        self.answered(self.ask(command), None)
    }

    /// The catalog's listing, with the user's own programs found on the
    /// `PATH` a launch here looks on where a launcher can be lent; where none
    /// can, the listing names none of them.
    fn listed(&self) -> Result<Outcome, AgentsError> {
        let lent = self.services.and_then(|services| {
            door::lending_here(self.home, services, self.runtime, |launcher, steps| {
                let lending = super::client::shelf(self.home, Some(launcher), Some(steps));
                self.asked(
                    &install_request(Command::AgentCatalog),
                    &lending,
                    None,
                    None,
                )
            })
            .ok()
        });
        lent.unwrap_or_else(|| self.ask(Command::AgentCatalog))
    }

    /// Tries the user's own program of `agent`, the one the door finds for
    /// that agent here, with a launcher and its steps lent to start it: the
    /// door asks on this terminal before it is started.
    fn tried(&self, agent: &str) -> Answer {
        let Ok(name) = Name::new(agent) else {
            return Answer::Refused(format!("{} is not an agent's name", escaped(agent)));
        };
        let Some(services) = self.services else {
            return Answer::Refused(UNSTARTED.to_owned());
        };
        let tried = door::lending_here(self.home, services, self.runtime, |launcher, steps| {
            let lending = super::client::shelf(self.home, Some(launcher), Some(steps));
            let lending = &lending;
            // Found and tried in the one wait, so that what is tried is what
            // the door found.
            let waited = self.waited(None, None, |front, watching| {
                Box::pin(async move {
                    let program = match taken(agent, door::own_program(lending, agent).await?) {
                        Ok(program) => program,
                        Err(refused) => return Ok(Err(refused)),
                    };
                    let program = Text::cut(&program.to_string_lossy());
                    if program.truncated() {
                        return Ok(Err(unfound(agent)));
                    }
                    let request = install_request(Command::TryOwnAgent {
                        agent: name,
                        program,
                    });
                    door::run(&request, lending, front, watching).await.map(Ok)
                })
            });
            match waited {
                Ok(Err(refused)) => Answer::Refused(refused),
                Ok(Ok(outcome)) => self.answered(Ok(outcome), None),
                Err(refused) => self.answered(Err(refused), None),
            }
        });
        tried.unwrap_or_else(|refused| Answer::Refused(caused(&refused)))
    }

    /// `command` carried out at the agents door, asked on this terminal, with
    /// how far a download has come said as it comes.
    fn ask(&self, command: Command) -> Result<Outcome, AgentsError> {
        self.asked(&install_request(command), &self.shelf, None, None)
    }

    /// `command`, a sign-in the agent runs as a command, carried out with the
    /// door lent this terminal to carry it to and a launcher to start it with;
    /// and with the agent's route asked here, since it will be started.
    fn at_console(&self, services: &Services, command: Command) -> Result<Outcome, AgentsError> {
        let request = Request::new(
            Capabilities::none()
                .with(Capability::Account)
                .with(Capability::Install)
                .with(Capability::Permissions),
            Correlation::new(1),
            command,
        );
        let console = Console::new(self.said);
        door::lending_here(self.home, services, self.runtime, |launcher, steps| {
            let lending = super::client::shelf(self.home, Some(launcher), Some(steps));
            self.asked(&request, &lending, Some(&console), Some(&console))
        })?
    }

    /// `request` carried out at the agents door lent `lending`, `sign_in` and
    /// `terminal`.
    fn asked(
        &self,
        request: &Request,
        lending: &Shelf<'_>,
        sign_in: Option<&dyn SignInChannel>,
        handover: Option<&dyn SignInTerminal>,
    ) -> Result<Outcome, AgentsError> {
        self.waited(sign_in, handover, |front, watching| {
            Box::pin(door::run(request, lending, front, watching))
        })
    }

    /// What `work` comes to, waited for on the application's runtime with
    /// this terminal to ask on, lent `sign_in` and `handover`, and with how
    /// far a download has come said as it comes.
    fn waited<'l, T>(
        &'l self,
        sign_in: Option<&'l dyn SignInChannel>,
        handover: Option<&'l dyn SignInTerminal>,
        work: impl for<'w> FnOnce(&'w mut (dyn Front + 'l), door::Watching<'w>) -> Waited<'w, T>,
    ) -> T {
        let (said, terminal) = (self.said, self.terminal);
        let mut front = Asked {
            said,
            answer: self.answer,
        };
        // Nothing here cancels: a download that is interrupted is interrupted
        // with the process and commits nothing, and a command run to sign in
        // has its input closed by that, and is ended only where the sandbox
        // ties it to this process.
        let cancel = Cancel::new();
        let mut shown: Option<Progress> = None;
        let outcome = {
            let mut progress = |told: InstallProgress| {
                let progress = shown
                    .get_or_insert_with(|| Progress::new(escaped(told.agent().as_str()), terminal));
                said(&progress.heard(told.done(), told.total()));
            };
            let watching = door::Watching {
                progress: &mut progress,
                cancel: &cancel,
                link: &|link| said(&linked(link.as_str(), link.incomplete())),
                sign_in,
                terminal: handover,
            };
            self.runtime.block_on(work(&mut front, watching))
        };
        if let Some(progress) = &shown {
            said(progress.ended());
        }
        outcome
    }

    /// What `outcome` is written as, listing only the agents whose name holds
    /// `matching` where a listing does.
    fn answered(&self, outcome: Result<Outcome, AgentsError>, matching: Option<&str>) -> Answer {
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(refused) => return Answer::Refused(caused(&refused)),
        };
        match outcome {
            Outcome::Refused(refusal) => Answer::Refused(refusal.to_string()),
            Outcome::Catalog {
                agents,
                more,
                unrefreshed,
                registry,
                antigravity,
            } => {
                if let Some(why) = unrefreshed {
                    (self.said)(&failing(&format!(
                        "the catalog could not be refreshed, so this is what was known before: {}",
                        caused(&why)
                    )));
                }
                if let Some(line) = registry {
                    (self.said)(&failing(line.as_ref()));
                }
                Answer::Done(listing(&agents, antigravity.as_ref(), more, matching))
            }
            Outcome::Undecided if self.answer.is_some() => {
                Answer::Refused("nothing was confirmed, so nothing was changed".to_owned())
            }
            Outcome::Undecided => Answer::Refused(
                "nothing was changed: this command asks its question on a terminal, and standard \
                 input or standard error is not one"
                    .to_owned(),
            ),
            Outcome::Moved {
                agent,
                how,
                version,
                instances,
                left,
                downloaded,
                standing,
                probe: _,
                unsmoked,
                unpruned,
            } => {
                let program = self.at(&agent, &version);
                let at = placed(program.as_deref());
                let version = escaped(&version);
                let mut said = match how {
                    How::Installed => {
                        installed(agent.as_str(), &version, program.as_deref(), downloaded)
                    }
                    How::Added => match instances.last() {
                        Some(added) => format!(
                            "Added {} {version}{at} as the instance {}.\n",
                            agent.as_str(),
                            escaped(added)
                        ),
                        None => format!("Added {} {version}{at}.\n", agent.as_str()),
                    },
                    How::Updated => format!("Updated {} to {version}{at}.\n", agent.as_str()),
                    How::RolledBack => {
                        format!("Rolled {} back to {version}{at}.\n", agent.as_str())
                    }
                };
                if standing {
                    said.push_str("Taken without asking, under agents.update = \"reviewed\".\n");
                }
                // An add's sentence names the instance it wrote.
                if how != How::Added && !instances.is_empty() {
                    let _ = writeln!(said, "Moved onto it: {}.", joined(&instances));
                }
                if let Some(why) = unsmoked {
                    let _ = writeln!(said, "Not smoke-tested: {}.", escaped(&why));
                }
                if !left.is_empty() {
                    let _ = writeln!(
                        said,
                        "Left as they were, since each named another program by the time it was \
                         written: {}.",
                        joined(&left)
                    );
                }
                if let Some(unpruned) = unpruned {
                    let _ = writeln!(
                        said,
                        "The versions no instance runs were not pruned: {}.",
                        caused(&*unpruned)
                    );
                }
                Answer::Done(said)
            }
            Outcome::Current { agent, version } => {
                let program = self.at(&agent, &version);
                Answer::Done(installed(
                    agent.as_str(),
                    &escaped(&version),
                    program.as_deref(),
                    false,
                ))
            }
            Outcome::Removed { agent, versions } => Answer::Done(format!(
                "Removed {} {}.\n",
                agent.as_str(),
                joined(&versions)
            )),
            Outcome::SignedIn {
                instance, method, ..
            } => Answer::Done(format!(
                "Signed {} in by {}.\n",
                escaped(&instance),
                escaped(&method)
            )),
            Outcome::Tried {
                agent,
                instance,
                program,
            } => Answer::Done(format!(
                "Your own {agent} at {} answered as an agent, and the instance {} runs it.\n",
                escaped(&program.display().to_string()),
                escaped(&instance)
            )),
            Outcome::Kept { instance, method } => Answer::Done(format!(
                "Kept the key for {} to sign in by {}; its agent is given it the next time it \
                 starts.\n",
                escaped(&instance),
                escaped(&method)
            )),
            Outcome::Verified { agent, versions } => {
                let mut said = String::new();
                let mut differs = false;
                for (version, mismatches) in &versions {
                    let version = escaped(version);
                    if mismatches.is_empty() {
                        let _ =
                            writeln!(said, "{} {version} is as it was installed.", agent.as_str());
                    } else {
                        differs = true;
                        let named: Vec<String> = mismatches
                            .iter()
                            .take(NAMED)
                            .map(|mismatch| escaped(&mismatch.path))
                            .collect();
                        let _ = write!(
                            said,
                            "{} {version} differs from what was installed at {}",
                            agent.as_str(),
                            named.join(", ")
                        );
                        if let Some(more) =
                            mismatches.len().checked_sub(NAMED).filter(|more| *more > 0)
                        {
                            let _ = write!(said, " and {more} more");
                        }
                        said.push_str(".\n");
                    }
                }
                if differs {
                    Answer::Differs(said)
                } else {
                    Answer::Done(said)
                }
            }
        }
    }

    /// Where the install store keeps the program `agent` is started as at
    /// `version`, as the trust state offers that version now, or nothing where
    /// it no longer offers it.
    fn at(&self, agent: &AgentKey, version: &str) -> Option<PathBuf> {
        let offer = trust_in(self.home.path())
            .offers(Some(agent))
            .into_iter()
            .find(|offer| *offer.pin.version == *version)?;
        Some(
            kept(
                &self.home.path().join("programs"),
                agent,
                version,
                offer.pin.lock_revision,
            )
            .join(&*offer.pin.target.cmd),
        )
    }
}

/// The directory the install store in `programs` keeps `version` of `agent`
/// in, named as the store names it: the version, and `+` and its package
/// tree's `revision` where it has one.
fn kept(programs: &Path, agent: &AgentKey, version: &str, revision: Option<u64>) -> PathBuf {
    let named = match revision {
        Some(revision) => format!("{version}+{revision}"),
        None => version.to_owned(),
    };
    programs.join(agent.as_str()).join(named)
}

/// What an install of `agent` at `version`, whose program is `at`, is
/// answered with: that it was installed and how to use it where it was
/// `downloaded`, and that it already was where the release was already there.
fn installed(agent: &str, version: &str, at: Option<&Path>, downloaded: bool) -> String {
    let at = placed(at);
    if downloaded {
        format!(
            "Installed {agent} {version}{at}. /login offers it under Use an installed coding \
             agent.\n"
        )
    } else {
        format!("{agent} {version} is already installed{at}.\n")
    }
}

/// ` at ` and the program `at`, or nothing where that is not known.
fn placed(at: Option<&Path>) -> String {
    at.map(|at| format!(" at {}", escaped(&at.display().to_string())))
        .unwrap_or_default()
}

/// `values`, each escaped so that none starts a line of its own, between
/// commas.
fn joined(values: &[Box<str>]) -> String {
    values
        .iter()
        .map(|value| escaped(value))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `command` as this command line sends it: claiming install alone.
fn install_request(command: Command) -> Request {
    Request::new(
        Capabilities::none().with(Capability::Install),
        Correlation::new(1),
        command,
    )
}

/// The program a try of `agent` takes, as the door `found` it, or why
/// nothing was tried: beside a configured instance, the door's own refusal.
fn taken(agent: &str, found: OwnProgram) -> Result<PathBuf, String> {
    let unlisted = || {
        format!(
            "the agent catalog lists no agent named {}, so nothing was tried",
            escaped(agent)
        )
    };
    match found {
        OwnProgram::Found(program) => Ok(program),
        OwnProgram::Beside { program, instance } => Err(AgentKey::new(agent).map_or_else(
            |_| unlisted(),
            |key| caused(&door::refused_beside(key, program, &instance)),
        )),
        OwnProgram::Unfound => Err(unfound(agent)),
        OwnProgram::Unlisted => Err(unlisted()),
    }
}

/// Why a try of `agent` is refused where no program of the user's own is
/// found for it.
fn unfound(agent: &str) -> String {
    format!(
        "no program of your own for {} was found on PATH outside crucible's install store, so \
         nothing was tried",
        escaped(agent)
    )
}

/// The work [`Desk::waited`] waits for, borrowing what it is lent for `'w`.
type Waited<'w, T> = Pin<Box<dyn Future<Output = T> + 'w>>;

/// Why a try is refused where nothing can be lent to start a program with.
const UNSTARTED: &str = "trying your own program starts it, and this command cannot start one \
                         here, so nothing was tried";

/// The most paths a `verify` names of one version before it counts the rest.
const NAMED: usize = 8;

/// The lines a listing of `agents` is written as, with the user's own
/// Antigravity, `antigravity`, after them where one was found, keeping only
/// those whose name holds `matching`, ignoring case, where it is given.
fn listing(
    agents: &[Listed],
    antigravity: Option<&OwnAntigravity>,
    more: bool,
    matching: Option<&str>,
) -> String {
    let wanted = matching.map(str::to_lowercase);
    let mut said = String::new();
    for listed in agents.iter().filter(|listed| {
        wanted
            .as_deref()
            .is_none_or(|wanted| listed.agent.as_str().to_lowercase().contains(wanted))
    }) {
        let offered = if listed.offered.is_empty() {
            "offers nothing for this platform".to_owned()
        } else {
            format!("offers {}", joined(&listed.offered))
        };
        let installed = match &listed.installed {
            Ok(versions) if versions.is_empty() => "not installed".to_owned(),
            Ok(versions) => format!("installed {}", joined(versions)),
            Err(why) => format!(
                "its directory in the store could not be read: {}",
                escaped(why)
            ),
        };
        let _ = write!(said, "{}: {offered}; {installed}", listed.agent.as_str());
        if let Some(own) = &listed.own {
            let _ = write!(
                said,
                "; your own at {} (crucible agents try {})",
                escaped(&own.display().to_string()),
                listed.agent.as_str()
            );
        }
        said.push('\n');
    }
    if let Some(own) = antigravity.filter(|own| {
        wanted
            .as_deref()
            .is_none_or(|wanted| own.agent.as_str().contains(wanted))
    }) {
        let _ = writeln!(
            said,
            "{}: your own at {} (crucible agents try {}). {}",
            own.agent.as_str(),
            escaped(&own.program.display().to_string()),
            own.agent.as_str(),
            own.note
        );
    }
    if said.is_empty() {
        said = match matching {
            Some(text) => format!(
                "No agent the catalog lists has {} in its name.\n",
                escaped(text)
            ),
            None => "The catalog lists no agent.\n".to_owned(),
        };
    }
    if more {
        said.push_str("The catalog lists more agents than these.\n");
    }
    said
}

/// What is written of a sign-in link the agent printed, where it printed
/// more than is kept where `incomplete`.
fn linked(link: &str, incomplete: bool) -> String {
    let cut = if incomplete {
        " (the agent printed more of it than crucible keeps)"
    } else {
        ""
    };
    visible(&format!(
        "Open this link to sign in{cut}: {}\n",
        escaped(link)
    ))
}

/// This process's own terminal, asked a question the agents door puts.
struct Asked<'a> {
    /// Where the question is written.
    said: &'a (dyn Fn(&str) + Sync),
    /// Where the answer is read from, where there is a terminal to ask on.
    answer: Option<fn() -> String>,
}

impl Front for Asked<'_> {
    /// Writes an install's question or the agent's route's whole and reads
    /// which way it goes on; nobody answers anything else, nor anything where
    /// there is no terminal.
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            let answer = self.answer?;
            let confirmation = match shown {
                Shown::Account(confirmation) | Shown::Install(confirmation) => confirmation,
                Shown::Warning(warned) => {
                    (self.said)(&visible(&format!(
                        "{}\n  {}\n  {}\n  {}\nSign in anyway? [y/N] ",
                        escaped(&warned.shown),
                        escaped(&warned.warning.sentence),
                        escaped(&warned.warning.cited()),
                        escaped(&warned.warning.link),
                    )));
                    let answered = tokio::task::spawn_blocking(answer).await.ok()?;
                    let id = pending.id();
                    let yes = matches!(answered.trim().to_ascii_lowercase().as_str(), "y" | "yes");
                    return Some(if yes {
                        Decision::Accepted { id }
                    } else {
                        Decision::Declined { id }
                    });
                }
                _ => return None,
            };
            let (Pending::Account { choices, .. } | Pending::Install { choices, .. }) = pending
            else {
                return None;
            };
            (self.said)(&visible(&question(confirmation)));
            // Read off the runtime's threads, since a person takes their time.
            let answered = tokio::task::spawn_blocking(answer).await.ok()?;
            let id = pending.id();
            Some(match chosen(&answered, choices.len()) {
                // Named as the pending action names it, which is what it is
                // held against.
                Some(at) => Decision::Picked {
                    id,
                    choice: Said::new(choices.get(at)?.name.as_str()).ok()?,
                },
                None => Decision::Declined { id },
            })
        })
    }

    fn refused(&mut self, refusal: Refusal) {
        (self.said)(&failing(&refusal.to_string()));
    }

    /// The question is written whole, never the cut words of the pending
    /// action.
    fn draws_whole(&self) -> bool {
        true
    }
}

/// The longest line typed to a sign-in command, in bytes. A longer one is not
/// cut into something other than what was typed: the command's input closes.
const TYPED_LINE: usize = 64 * 1024;

/// How many lines typed ahead of a sign-in command wait for it.
const TYPED_AHEAD: usize = 16;

/// This process's own terminal, lent to a sign-in the agent runs as a command.
///
/// The door is driven on the one thread that writes here, between its waits, so
/// each line is written whole and nothing draws beside it; the only other thread
/// is the one that reads standard input and writes nothing. It sets no terminal
/// mode, so a line is echoed and edited as the terminal does by itself. A line
/// typed here cannot be told to be a secret: what it was asked for is the
/// command's to know, and it sends it as any other.
struct Console<'a> {
    /// Where each line is written.
    said: &'a (dyn Fn(&str) + Sync),
    /// The lines typed, from when the first is first asked for, so that what
    /// the door itself asks on this terminal is read before anything else is.
    typed: OnceLock<Mutex<mpsc::Receiver<String>>>,
    /// This process's terminal, lent to a command that needs a real one.
    ///
    /// Nothing else here sets a terminal mode: `crucible agents sign-in` runs
    /// on the terminal it was started on, so the modes to give back are none.
    /// What the handover still does is the part that matters here — the
    /// child's process group is made the foreground one, and the parent takes
    /// it back — which is what keeps Ctrl+C with the child.
    terminal: std::sync::Mutex<Suspend>,
}

impl<'a> Console<'a> {
    /// A console writing to `said` and reading standard input.
    fn new(said: &'a (dyn Fn(&str) + Sync)) -> Self {
        Self {
            said,
            typed: OnceLock::new(),
            terminal: std::sync::Mutex::new(Suspend::holding(Held::default(), || {})),
        }
    }
}

impl SignInChannel for Console<'_> {
    fn said(&self, line: SaidLine) {
        (self.said)(&written_line(line.as_str(), line.incomplete()));
    }

    fn line(&self) -> BoxFuture<'_, Option<Typing>> {
        Box::pin(async move {
            let typed = self
                .typed
                .get_or_init(|| Mutex::new(typed_lines(BufReader::new(io::stdin()))));
            typed.lock().await.recv().await.map(Typing::Line)
        })
    }
}

impl SignInTerminal for Console<'_> {
    fn handed(&self, pid: u32) -> Result<(), HandoverFailed> {
        let suspend = self
            .terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        suspend
            .handed(pid)
            .map_err(|why| HandoverFailed::new(why.to_string()))
    }

    fn resumed(&self) {
        let suspend = self
            .terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = suspend.resumed();
    }
}

/// What leads every line a sign-in command wrote. The line is escaped after it,
/// so what the command writes cannot end this line and begin one without it,
/// and nothing it writes reads as crucible's own.
const CARRIED: &str = "sign-in command | ";

/// A line a sign-in command wrote, as it is written here.
fn written_line(line: &str, incomplete: bool) -> String {
    let cut = if incomplete {
        " (the command printed more of this than crucible keeps)"
    } else {
        ""
    };
    visible(&format!("{CARRIED}{}{cut}\n", escaped(line)))
}

/// Each line `input` gives, without its line ending, each read to at most
/// [`TYPED_LINE`] bytes, on a thread of its own, since a person takes their time
/// and nothing can interrupt the read. No more are given after a longer one, or
/// after its end. The thread ends with them, or is left waiting on the terminal
/// until this process ends, which follows a sign-in at once.
fn typed_lines(input: impl BufRead + Send + 'static) -> mpsc::Receiver<String> {
    let (sent, typed) = mpsc::channel(TYPED_AHEAD);
    let reading = move || {
        let mut input = input;
        loop {
            let mut line = Vec::new();
            let limit = u64::try_from(TYPED_LINE).unwrap_or(u64::MAX);
            let Ok(read) = (&mut input).take(limit).read_until(b'\n', &mut line) else {
                return;
            };
            if read == 0 || (read == TYPED_LINE && line.last() != Some(&b'\n')) {
                return;
            }
            let line = String::from_utf8_lossy(&line);
            let line = line.strip_suffix('\n').unwrap_or(&line);
            let line = line.strip_suffix('\r').unwrap_or(line);
            if sent.blocking_send(line.to_owned()).is_err() {
                return;
            }
        }
    };
    // With no thread to read on, the command is told its input has ended.
    let _ = std::thread::Builder::new()
        .name("agents-sign-in-input".to_owned())
        .spawn(reading);
    typed
}

/// `confirmation` as it is asked: what it asks, each fact on a line of its
/// own, and its choices numbered, each piece escaped so none stands for a
/// line it is not.
fn question(confirmation: &Confirmation) -> String {
    let mut asked = format!("{}\n", escaped(&confirmation.asks));
    for fact in &confirmation.facts {
        let _ = writeln!(asked, "  {}", escaped(fact));
    }
    for (at, (name, says)) in confirmation.choices.iter().enumerate() {
        let _ = writeln!(
            asked,
            "  {}. {}: {}",
            at.saturating_add(1),
            escaped(name),
            escaped(says)
        );
    }
    let _ = write!(
        asked,
        "Which one? [1-{}, Enter for none] ",
        confirmation.choices.len()
    );
    asked
}

/// The answer to a question, as [`answer_in`] reads it from standard input.
fn answer() -> String {
    answer_in(&mut io::stdin().lock())
}

/// The line `input` gives, without its newline, read to at most
/// [`auth::MAX_ANSWER`] bytes; nothing, which is no, where it gave no newline
/// within them, having been cut or having ended without one. A carriage return
/// before the newline, as a console ends a line with, is not part of it.
fn answer_in(input: &mut impl BufRead) -> String {
    let mut line = String::new();
    if input.take(auth::MAX_ANSWER).read_line(&mut line).is_err() {
        return String::new();
    }
    match line.strip_suffix('\n') {
        Some(ended) => ended.strip_suffix('\r').unwrap_or(ended).to_owned(),
        None => String::new(),
    }
}

/// The choice of `count` that `answered` numbers, counting from one: only
/// digits, the first of them not `0`, naming one of them.
fn chosen(answered: &str, count: usize) -> Option<usize> {
    if answered.starts_with('0') || !answered.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    answered
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
        .filter(|at| *at < count)
}

/// Runs `crucible agents unpack-check` over the artifact the catalog spells
/// in `entry` and its download at `artifact`, or the lock at `entry` and the
/// directory of its tarballs at `artifact`, unpacking into `out`, and stops.
pub(super) fn unpack_check(entry: &Path, artifact: &Path, out: &Path) -> ExitCode {
    written(checked(entry, artifact, out))
}

/// Runs `crucible agents probe` over the release unpacked into `root` and the
/// entry at `entry`, and stops.
pub(super) fn probe(root: &Path, entry: &Path, unconfined: bool) -> ExitCode {
    written(
        moded(unconfined, &|name| std::env::var_os(name))
            .and_then(|mode| probed(root, entry, mode, &probing::local, probing::BOUND)),
    )
}

/// The mode `--unconfined` asks for as `lookup` reads the environment, or the
/// refusal for standard error.
fn moded(
    unconfined: bool,
    lookup: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Mode, String> {
    Mode::of(unconfined, lookup).map_err(|refused| failing(&caused(&refused)))
}

/// Writes `answer` to standard output where it is an answer and to standard
/// error where it is a refusal.
fn written(answer: Result<String, String>) -> ExitCode {
    match answer {
        Ok(text) => {
            let _ = io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        Err(refused) => {
            let _ = io::stderr().write_all(refused.as_bytes());
            ExitCode::FAILURE
        }
    }
}

/// What `crucible agents probe` writes: the JSON for standard output, or the
/// refusal for standard error, saying what refused it and why as each cause
/// beneath it gives it. The agent is given `within` to answer, and the
/// sandbox is the one `sandbox` makes on the application's runtime.
fn probed(
    root: &Path,
    entry: &Path,
    mode: Mode,
    sandbox: &dyn Fn(Handle) -> Arc<dyn SandboxService>,
    within: Duration,
) -> Result<String, String> {
    probing::probe(root, entry, mode, sandbox, within).map_err(|refused| failing(&caused(&refused)))
}

/// What `crucible agents unpack-check` writes: the file list for standard
/// output, or the refusal for standard error, saying what refused it and why
/// as each cause beneath it gives it.
fn checked(entry: &Path, artifact: &Path, out: &Path) -> Result<String, String> {
    installing::unpack_check(entry, artifact, out).map_err(|refused| failing(&caused(&refused)))
}

/// The most causes beneath a refusal [`caused`] says.
const CAUSES: usize = 8;

/// `refused` and the causes beneath it, each after a colon; a cause whose
/// words are already said is not said again.
fn caused(refused: &dyn std::error::Error) -> String {
    let mut said = refused.to_string();
    let mut cause = refused.source();
    for _ in 0..CAUSES {
        let Some(next) = cause else {
            break;
        };
        let words = next.to_string();
        if !said.contains(&words) {
            said.push_str(": ");
            said.push_str(&words);
        }
        cause = next.source();
    }
    said
}

/// What has been said about a download's progress.
struct Progress {
    /// The agent's name, escaped.
    agent: String,
    terminal: bool,
    /// The last whole percent said, once anything has been.
    said: Option<u64>,
}

impl Progress {
    fn new(agent: String, terminal: bool) -> Self {
        Self {
            agent,
            terminal,
            said: None,
        }
    }

    /// What to write now that `held` of `of` bytes have arrived: on a
    /// terminal the percent come, each time it changes, and elsewhere one line
    /// opening the download.
    fn heard(&mut self, held: u64, of: u64) -> String {
        let percent = held
            .saturating_mul(100)
            .checked_div(of)
            .unwrap_or(100)
            .min(100);
        let opening = format!("Downloading {} ({} MB)", self.agent, of / 1_000_000);
        let text = if self.terminal && self.said != Some(percent) {
            format!("\r{opening}: {percent:>3}%")
        } else if !self.terminal && self.said.is_none() {
            format!("{opening}…\n")
        } else {
            String::new()
        };
        self.said = Some(percent);
        text
    }

    /// What ends what was said: the line a terminal was rewriting, where one
    /// was.
    fn ended(&self) -> &'static str {
        if self.terminal && self.said.is_some() {
            "\n"
        } else {
            ""
        }
    }
}
