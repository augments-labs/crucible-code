//! The account operations on an installed agent, as a client asks for them.
//!
//! Each asks which way to go before anything changes, through the [`Front`]
//! it is handed, and a client that did not say it answers account questions is
//! asked nothing: the operation is [`Account::Undecided`] and the files are as
//! they were. So is one where the client cancelled, went back, or answered
//! nothing.
//!
//! Once a way is chosen, the operation takes its instance's credential scope
//! through a [`Coordinator`] and keeps each stage in the scope's journal before
//! taking the step it names, so a crash leaves the instance needing a check
//! rather than half-changed without a record. A disconnect or a removal writes
//! the configuration last, through [`remember`], and the journal's record is
//! finished only after that write. Where the journal cannot be kept after the
//! run was closed, the client is told the operation failed: the run stays
//! stopped, nothing is written, and the record left open is found by the next
//! operation on the scope. A sign-out writes first, and is told apart below.
//!
//! What came of it is the application's [`Account`], read by a terminal whole,
//! and translated for a client by [`Account::outcome`], a match written out
//! here.
//!
//! | Asked | Chosen | Outcome |
//! | --- | --- | --- |
//! | disconnect | disconnect | [`LogoutOutcome::Disconnected`] |
//! | disconnect, its run not confirmed stopped | disconnect | [`LogoutOutcome::NewWorkDisabledWorkNotConfirmedStopped`] |
//! | disconnect, the file not written | disconnect | [`LogoutOutcome::StoppedForThisRunOnly`] |
//! | remove, a provider reached through it | nothing | [`LogoutOutcome::StillReferenced`] |
//! | remove | remove | [`LogoutOutcome::IntegrationRemoved`] |
//! | remove, the file not written | remove | [`ErrorCode::Failed`] |
//! | remove, an instance the user file does not name | nothing | [`ErrorCode::InvalidArgument`] |
//! | sign out | sign out | [`LogoutOutcome::AgentSignedOut`] |
//! | sign out, not shown signed out afterwards | sign out | [`LogoutOutcome::SignOutUnverified`] |
//! | sign out, the file not written | sign out | [`LogoutOutcome::StoppedForThisRunOnly`] |
//! | sign out | disconnect instead | as disconnect |
//! | any | cancel, back, or nobody | [`LogoutOutcome::Undecided`] |
//! | connect, signed in | use it, then connect | [`LoginOutcome::AgentConnected`] |
//! | connect, signed in | use it, then connect; the file not written | [`LoginOutcome::ConnectUnsaved`] |
//! | connect | the agent's route accepted; the yes not written | [`LoginOutcome::ConnectUnsaved`] |
//! | connect | cancel, back, or nobody | [`LoginOutcome::Undecided`] |
//! | connect, an instance not named | nothing | [`ErrorCode::InvalidArgument`] |
//!
//! A removal whose write fails after its run was stopped is told only that it
//! failed: the settings are still there, so the next run starts the agent as
//! before, and nothing is lasting about the stop to report.
//!
//! A sign-out takes every instance sharing the credential scope out of use
//! before the agent's own program is run: their runs in this conversation are
//! stopped, and `enabled: false` is written for each in one update of the
//! user file. Only once that write has landed is the agent's logout run, and
//! then a status step after it; where the write fails, no step is run. It is
//! reported signed out only where both steps read signed out, and otherwise as
//! unverified. Once the write has landed, a journal that cannot be kept is
//! reported as unverified too, since the instances are already out of use; a
//! stage the journal refused is not taken.
//!
//! A connect starts nothing of the agent's before its content-use route has a
//! yes, and then reads only whether the agent is signed in. Signed in, the
//! client is asked to use that account and then to confirm the route; not
//! signed in, or not read either way, it is told so, with the command that
//! signs the agent in where it has one, and may check again, [`CHECKS`] times
//! in all. Nothing is written before the route is confirmed: then the
//! provider's `access` names the instance and the instance is enabled, in one
//! update of the user file, under the scope's journal.

use std::path::Path;

use crucible_client_api::{
    Command, ErrorCode, LoginOutcome, LogoutOutcome, Outcome, Problem, Refusal, Request, Text,
};
use crucible_config::{ConfigError, ExternalAgent};
use crucible_execution::Authentication;
use crucible_execution::{Admission, Ending, Failure};
use crucible_external_harnesses::{
    Account as Step, Contended, Coordinator, Ended, Journal, Reading, Stage, Work,
};

use super::deciding::{Confirmation, Front, confirmed};
use super::performing::{Desk, Performed};
use crate::Conversation;
use crate::remember::{self, RememberError};

/// What came of an account operation, in the application's own values.
#[derive(Debug)]
pub enum Account {
    /// The instance takes no new work, which is written down.
    Disconnected {
        /// Whether the work it had running in this conversation is known to
        /// have stopped, which it is where there was none.
        stopped: bool,
    },
    /// The instance takes no new work in this conversation, and that could
    /// not be written down.
    ThisRunOnly(RememberError),
    /// The instance's settings are gone from the user file, and its run in
    /// this conversation was stopped first.
    Removed,
    /// Nothing was removed, because the provider named here is still reached
    /// through the instance.
    StillReferenced(String),
    /// The instance's run was stopped, and its settings could not be taken out
    /// of the file.
    Unremoved(RememberError),
    /// Every instance sharing the credential scope takes no new work, which
    /// is written down, and the agent's own program read signed out both by
    /// its logout and by a status step after it.
    SignedOut,
    /// Every instance sharing the credential scope takes no new work, which
    /// is written down, and the agent's sign-out could not be confirmed, for
    /// this reason. A client is told the sentence alone, since the reason may
    /// be what the agent's program printed.
    SignOutUnverified(Failure),
    /// Nobody chose a way to go on, so nothing changed.
    Undecided,
    /// The provider named here is reached through the instance, which takes
    /// new work, and that is written down.
    Connected,
    /// The agent read signed in, and the route through it could not be
    /// written down, so nothing changed.
    ConnectUnsaved(RememberError),
    /// The yes to the agent's content-use route could not be written down,
    /// so nothing of the agent's was started and nothing changed.
    HoldUnkept(RememberError),
    /// Nobody went on with a connect, so nothing changed.
    Unconnected,
    /// It did not begin, or could not be kept as it went.
    Contended(Contended),
}

impl Account {
    /// What happened, as a client is told it.
    #[must_use]
    pub fn outcome(&self) -> Outcome {
        Outcome::Logout(match self {
            Self::Disconnected { stopped: true } => LogoutOutcome::Disconnected {
                said: Text::cut(DISCONNECTED),
            },
            Self::Disconnected { stopped: false } => {
                LogoutOutcome::NewWorkDisabledWorkNotConfirmedStopped {
                    said: Text::cut(NOT_CONFIRMED_STOPPED),
                }
            }
            Self::ThisRunOnly(problem) => LogoutOutcome::StoppedForThisRunOnly {
                said: Text::cut(THIS_RUN_ONLY),
                problem: Problem::failed(problem),
            },
            Self::Removed => LogoutOutcome::IntegrationRemoved {
                said: Text::cut(REMOVED),
            },
            Self::StillReferenced(provider) => LogoutOutcome::StillReferenced {
                said: Text::cut(&format!("{provider} {STILL_REFERENCED}")),
            },
            Self::SignedOut => LogoutOutcome::AgentSignedOut {
                said: Text::cut(Ended::SignedOut.says()),
            },
            Self::SignOutUnverified(failure) => LogoutOutcome::SignOutUnverified {
                said: Text::cut(Ended::SignOutUnverified(failure.clone()).says()),
            },
            Self::Undecided => LogoutOutcome::Undecided,
            Self::Connected => {
                return Outcome::Login(LoginOutcome::AgentConnected {
                    said: Text::cut(CONNECTED),
                });
            }
            Self::ConnectUnsaved(problem) => {
                return Outcome::Login(LoginOutcome::ConnectUnsaved {
                    said: Text::cut(CONNECT_UNSAVED),
                    problem: Problem::failed(problem),
                });
            }
            Self::HoldUnkept(problem) => {
                return Outcome::Login(LoginOutcome::ConnectUnsaved {
                    said: Text::cut(HOLD_UNKEPT),
                    problem: Problem::failed(problem),
                });
            }
            Self::Unconnected => return Outcome::Login(LoginOutcome::Undecided),
            Self::Contended(Contended::Busy) => {
                return Outcome::Refused(ErrorCode::Busy.into());
            }
            Self::Unremoved(_)
            | Self::Contended(
                Contended::Unresolved(_)
                | Contended::Refused(_)
                | Contended::Order
                | Contended::Journal(_),
            ) => return Outcome::Refused(ErrorCode::Failed.into()),
        })
    }
}

/// What a person is told once an instance is disconnected. Whether the agent
/// is still signed in is not known here, so nothing is said of it but that
/// Crucible did not change it.
pub const DISCONNECTED: &str = "Disconnected from Crucible. External credentials were not changed.";

/// What a person is told where the run could not be confirmed stopped.
pub const NOT_CONFIRMED_STOPPED: &str = "New work disabled; existing work not confirmed stopped.";

/// What a person is told where the disconnect could not be written down.
pub const THIS_RUN_ONLY: &str = "Stopped for this run only; disconnect could not be saved.";

/// The choice that goes on with a disconnect, by its name.
pub const DISCONNECT: &str = "Disconnect from Crucible";

/// What a person is told once an instance's settings are removed.
pub const REMOVED: &str =
    "Integration settings removed. External credentials and conversation history were not deleted.";

/// What a person is told after the provider's name where a provider is still
/// reached through the instance.
pub const STILL_REFERENCED: &str =
    "is still reached through this agent; choose another way to reach it first.";

/// The choice that goes on with a removal, by its name.
pub const REMOVE: &str = "Remove integration settings";

/// The choice that goes on with a sign-out, by its name.
pub const SIGN_OUT: &str = "Sign out of the agent";

/// The choice a sign-out offers instead, by its name.
pub const DISCONNECT_INSTEAD: &str = "Disconnect from Crucible instead";

/// What a person is told once a provider is reached through an agent.
pub const CONNECTED: &str = "Connected. The provider is reached through this agent.";

/// What a person is told where the agent is signed in and the route through
/// it could not be written down.
pub const CONNECT_UNSAVED: &str =
    "The agent signed in, but Crucible could not activate the integration.";

/// What a person is told where the yes to an agent's route could not be
/// written down.
pub const HOLD_UNKEPT: &str =
    "Crucible could not write down the yes to this agent's route, so nothing was changed.";

/// The choice that goes on with the account an agent is signed in with.
pub const USE_ACCOUNT: &str = "Use this account";

/// The choice that reads an agent's sign-in again.
pub const CHECK_AGAIN: &str = "Check again";

/// The choice that writes a connect down.
pub const CONNECT: &str = "Connect";

/// How many times a connect reads whether the agent is signed in, the first
/// time and each check again together, before it is given up as undecided.
pub const CHECKS: usize = 5;

/// Carries out `request` where it is an account operation, asking `front`
/// which way to go before anything changes.
///
/// Any other command is refused as [`ErrorCode::Busy`] and nothing is done:
/// [`perform`](super::perform) and the other doors answer them. An instance
/// the configuration does not name, or names as already disabled, is refused
/// as [`ErrorCode::InvalidArgument`], before anything is asked.
pub async fn account(
    conversation: &mut Conversation,
    request: &Request,
    desk: &Desk<'_>,
    front: &mut dyn Front,
) -> Performed {
    match request.command() {
        Command::Disconnect { instance } => {
            let Some(agent) = desk
                .switching
                .settings
                .external_agents()
                .into_iter()
                .find(|agent| agent.name() == instance.as_str() && agent.enabled())
            else {
                return Performed::Refused(ErrorCode::InvalidArgument.into());
            };
            let confirmation = disconnecting(&agent, conversation.instance());
            match confirmed(request.capabilities(), front, &confirmation).await {
                Some(0) => Performed::Account(disconnect(conversation, desk, &agent).await),
                _ => Performed::Account(Account::Undecided),
            }
        }
        Command::RemoveIntegration { instance } => {
            let Some(agent) = desk
                .switching
                .settings
                .external_agents()
                .into_iter()
                .find(|agent| agent.name() == instance.as_str())
            else {
                return Performed::Refused(ErrorCode::InvalidArgument.into());
            };
            if let Some(provider) = desk.switching.settings.referencing(agent.name()) {
                return Performed::Account(Account::StillReferenced(provider));
            }
            // Only the user file is written, so an instance some other file
            // names is refused here, before its run is stopped for nothing.
            match remember::removable(desk.switching.choosing, agent.name()) {
                Ok(()) => {}
                Err(RememberError::Unusable(ConfigError::StillReferenced { provider, .. })) => {
                    return Performed::Account(Account::StillReferenced(provider.into()));
                }
                Err(RememberError::Unusable(_)) => {
                    return Performed::Refused(ErrorCode::InvalidArgument.into());
                }
                Err(problem) => return Performed::Account(Account::Unremoved(problem)),
            }
            let confirmation = removing(&agent, conversation.instance());
            match confirmed(request.capabilities(), front, &confirmation).await {
                Some(0) => Performed::Account(remove(conversation, desk, &agent).await),
                _ => Performed::Account(Account::Undecided),
            }
        }
        Command::SignOut { instance } => {
            let agents = desk.switching.settings.external_agents();
            let Some(agent) = agents
                .iter()
                .find(|agent| agent.name() == instance.as_str())
                .cloned()
            else {
                return Performed::Refused(ErrorCode::InvalidArgument.into());
            };
            let sharing: Vec<ExternalAgent> = agents
                .into_iter()
                .filter(|other| {
                    other.driver() == agent.driver() && other.profile() == agent.profile()
                })
                .collect();
            let confirmation = signing_out(&agent, &sharing, conversation.instance());
            match confirmed(request.capabilities(), front, &confirmation).await {
                Some(0) => Performed::Account(sign_out(conversation, desk, &agent, &sharing).await),
                Some(1) => Performed::Account(disconnect(conversation, desk, &agent).await),
                _ => Performed::Account(Account::Undecided),
            }
        }
        Command::Connect { provider, instance } => {
            let Some(served) = crate::providers::offered(desk.switching.providers)
                .find(|served| served.name == provider.as_str())
            else {
                return Performed::Refused(ErrorCode::UnknownProvider.into());
            };
            // Which agent to ask about, where none is named, is a question
            // this door does not put yet.
            let Some(agent) = instance.as_ref().and_then(|instance| {
                desk.switching
                    .settings
                    .external_agents()
                    .into_iter()
                    .find(|agent| agent.name() == instance.as_str())
            }) else {
                return Performed::Refused(ErrorCode::InvalidArgument.into());
            };
            let Some(row) = crate::backends::REGISTRY.row(agent.driver()) else {
                return Performed::Refused(ErrorCode::InvalidArgument.into());
            };
            let asked = Connecting {
                provider: served.name,
                agent: &agent,
                row,
            };
            Performed::Account(connect(conversation, request, desk, front, &asked).await)
        }
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
        | Command::Exit => Performed::Refused(Refusal::from(ErrorCode::Busy)),
    }
}

/// The question a disconnect of `agent` puts, where the conversation is on
/// the instance `on`.
fn disconnecting(agent: &ExternalAgent, on: Option<&str>) -> Confirmation {
    let name = agent.name();
    let working = if on == Some(name) {
        "Active work: this conversation runs on it, and its run will be stopped."
    } else {
        "Active work: none in this conversation."
    };
    Confirmation {
        asks: format!("Disconnect {name} from Crucible?"),
        facts: vec![
            format!("Agent: {name}, run as {}.", agent.executable()),
            match agent.profile() {
                Some(profile) => format!("Profile: {profile}."),
                None => "Profile: the agent's default.".to_owned(),
            },
            "Account: not known to Crucible.".to_owned(),
            working.to_owned(),
            "Sharing: other programs using this profile keep their sign-in; Crucible cannot \
             tell which share it."
                .to_owned(),
        ],
        choices: vec![
            (
                DISCONNECT.to_owned(),
                "Crucible stops using this agent; its sign-in is left as it is.".to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// The question a removal of `agent` puts, where the conversation is on the
/// instance `on`.
fn removing(agent: &ExternalAgent, on: Option<&str>) -> Confirmation {
    let name = agent.name();
    let working = if on == Some(name) {
        "Active work: this conversation runs on it, and its run will be stopped."
    } else {
        "Active work: none in this conversation."
    };
    Confirmation {
        asks: format!("Remove the settings for {name} from Crucible?"),
        facts: vec![
            format!("Agent: {name}, run as {}.", agent.executable()),
            match agent.profile() {
                Some(profile) => format!("Profile: {profile}."),
                None => "Profile: the agent's default.".to_owned(),
            },
            "Kept: the agent's sign-in, its installed program and conversation history.".to_owned(),
            working.to_owned(),
        ],
        choices: vec![
            (
                REMOVE.to_owned(),
                "Crucible forgets this agent; adding it again needs its settings again.".to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// The question a sign-out of `agent` puts, every instance in `sharing` on
/// its credential scope, where the conversation is on the instance `on`.
fn signing_out(agent: &ExternalAgent, sharing: &[ExternalAgent], on: Option<&str>) -> Confirmation {
    let name = agent.name();
    let working = if sharing.iter().any(|one| on == Some(one.name())) {
        "Active work: this conversation runs on this profile, and its run will be stopped."
    } else {
        "Active work: none in this conversation."
    };
    let names: Vec<&str> = sharing.iter().map(ExternalAgent::name).collect();
    Confirmation {
        asks: format!("Sign {name} out of its agent account?"),
        facts: vec![
            format!("Agent: {name}, run as {}.", agent.executable()),
            match agent.profile() {
                Some(profile) => format!("Profile: {profile}."),
                None => "Profile: the agent's default.".to_owned(),
            },
            "Account: not known to Crucible.".to_owned(),
            working.to_owned(),
            format!(
                "Sharing: {} use this profile, and each is disconnected first.",
                names.join(", ")
            ),
            "Outside Crucible: the agent's own program on this profile is signed out too."
                .to_owned(),
        ],
        choices: vec![
            (
                SIGN_OUT.to_owned(),
                "Crucible stops using every instance on this profile, then runs the agent's own \
                 sign-out."
                    .to_owned(),
            ),
            (
                DISCONNECT_INSTEAD.to_owned(),
                "Crucible stops using this instance; its sign-in is left as it is.".to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// Takes every instance in `sharing` out of use from Crucible, durably, and
/// only then signs `agent`'s credential scope out through its own program.
async fn sign_out(
    conversation: &mut Conversation,
    desk: &Desk<'_>,
    agent: &ExternalAgent,
    sharing: &[ExternalAgent],
) -> Account {
    let mut coordinator = match coordinating(desk.switching.choosing, agent) {
        Ok(coordinator) => coordinator,
        Err(contended) => return Account::Contended(contended),
    };
    let mut slot = match coordinator.begin(Work::SignOut, agent.profile(), 0) {
        Ok(slot) => slot,
        Err(contended) => return Account::Contended(contended),
    };
    if let Err(contended) = slot.advance(Stage::Closed) {
        return Account::Contended(contended);
    }
    // Whether each run stopped is not what a sign-out reports: the agent's
    // sign-in is what it changes, and every instance on it is disabled first.
    for one in sharing {
        let _ = conversation.disconnect(one.name()).await;
    }
    if let Err(contended) = slot.advance(Stage::Quiesced) {
        return Account::Contended(contended);
    }
    let names: Vec<&str> = sharing.iter().map(ExternalAgent::name).collect();
    if let Err(problem) = remember::disabling_every(desk.switching.choosing, &names) {
        let _ = slot.finish(Ending::Failed);
        return Account::ThisRunOnly(problem);
    }
    // The instances are out of use from here on, so a journal that cannot be
    // kept no longer refuses the request: it leaves the sign-out unverified.
    let unkept = || Account::SignOutUnverified(Failure::new("the journal could not be kept"));
    if slot.advance(Stage::Mutating).is_err() {
        return unkept();
    }
    let unstepped = || Reading::Unreadable(Failure::new("this run takes no agent steps"));
    let logout = match conversation.steps() {
        Some(steps) => steps.account(agent, Step::SignOut).await,
        None => unstepped(),
    };
    if slot.advance(Stage::Verifying).is_err() {
        return unkept();
    }
    let status = match conversation.steps() {
        Some(steps) => steps.account(agent, Step::Status).await,
        None => unstepped(),
    };
    let (ending, account) = match Ended::signed_out(&logout, &status) {
        Ended::SignedOut => (Ending::Succeeded, Account::SignedOut),
        Ended::SignOutUnverified(failure) | Ended::Unverified(failure) => {
            (Ending::Unknown, Account::SignOutUnverified(failure))
        }
        Ended::SignedIn | Ended::Disconnected(_) => (
            Ending::Unknown,
            Account::SignOutUnverified(Failure::new("the sign-out was not read")),
        ),
    };
    match slot.finish(ending) {
        Ok(_) => account,
        Err(_) => unkept(),
    }
}

/// A connect asked for: the provider to reach, the instance to reach it
/// through, and the row of that instance's agent.
struct Connecting<'a> {
    provider: &'static str,
    agent: &'a ExternalAgent,
    row: &'static crate::backends::Row,
}

/// Reaches the provider asked for through its instance once the agent reads
/// signed in and the client has confirmed the route, asking `front` each way
/// to go on.
async fn connect(
    conversation: &mut Conversation,
    request: &Request,
    desk: &Desk<'_>,
    front: &mut dyn Front,
    asked: &Connecting<'_>,
) -> Account {
    let Connecting {
        provider,
        agent,
        row,
    } = *asked;
    let capabilities = request.capabilities();
    // Nothing of the agent's is started before its route has a yes, and a yes
    // is written down before anything is.
    if let Some(consent) = conversation.consent()
        && let Some(warned) = consent.asks(row.route).copied()
    {
        if !super::deciding::warned(capabilities, front, &warned).await {
            return Account::Unconnected;
        }
        if let Err(problem) = consent.accept(&warned) {
            return Account::HoldUnkept(problem);
        }
    }
    let unstepped = || Reading::Unreadable(Failure::new("this run takes no agent steps"));
    let keyed = desk.switching.logins.read().has_key(provider);
    for _ in 0..CHECKS {
        let status = match conversation.steps() {
            Some(steps) => steps.account(agent, Step::Status).await,
            None => unstepped(),
        };
        let question = match &status {
            Reading::Signed(Authentication::SignedIn) => signed_in(agent),
            Reading::Signed(Authentication::SignedOut) => signed_out(agent, row),
            Reading::Signed(
                Authentication::Unknown | Authentication::Expired | Authentication::Unverifiable,
            )
            | Reading::Version(_)
            | Reading::Unreadable(_) => unverified(agent),
        };
        let signed = matches!(status, Reading::Signed(Authentication::SignedIn));
        match confirmed(capabilities, front, &question).await {
            Some(0) if signed => {
                match confirmed(capabilities, front, &routing(agent, provider, keyed)).await {
                    Some(0) => return commit(desk, provider, agent),
                    // Going back is one level up: the sign-in is read again.
                    Some(_) => {}
                    None => return Account::Unconnected,
                }
            }
            // Checking again.
            Some(0) => {}
            _ => return Account::Unconnected,
        }
    }
    Account::Unconnected
}

/// Writes down that `provider` is reached through `agent`, and that `agent`
/// takes new work, under its scope's journal.
fn commit(desk: &Desk<'_>, provider: &str, agent: &ExternalAgent) -> Account {
    let mut coordinator = match coordinating(desk.switching.choosing, agent) {
        Ok(coordinator) => coordinator,
        Err(contended) => return Account::Contended(contended),
    };
    let mut slot = match coordinator.begin(Work::Connect, agent.profile(), 0) {
        Ok(slot) => slot,
        Err(contended) => return Account::Contended(contended),
    };
    if let Err(contended) = slot.advance(Stage::Committing) {
        return Account::Contended(contended);
    }
    if let Err(problem) = remember::connecting(desk.switching.choosing, provider, agent.name()) {
        let _ = slot.finish(Ending::Failed);
        return Account::ConnectUnsaved(problem);
    }
    // The route is written and in force whether or not the journal takes its
    // end; an open record is replaced by the next connect of the scope.
    let _ = slot.finish(Ending::Succeeded);
    Account::Connected
}

/// What the person is asked where `agent` reads signed in.
fn signed_in(agent: &ExternalAgent) -> Confirmation {
    let name = agent.name();
    Confirmation {
        asks: format!("Use the account {name} is signed in with?"),
        facts: vec![
            format!("Agent: {name}, run as {}.", agent.executable()),
            profiled(agent),
            "Account: signed in; the agent does not tell Crucible which.".to_owned(),
        ],
        choices: vec![
            (
                USE_ACCOUNT.to_owned(),
                "Go on to confirm the route through this agent.".to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// What the person is told where `agent`, of the row `row`, reads signed out.
fn signed_out(agent: &ExternalAgent, row: &crate::backends::Row) -> Confirmation {
    let name = agent.name();
    let how = match (row.sign_in)(agent.profile().map(Path::new)) {
        Some(command) => {
            format!("Sign in: run `{command}` in a terminal, then choose Check again.")
        }
        None => "Sign in: open the agent and sign in there, then choose Check again.".to_owned(),
    };
    Confirmation {
        asks: format!("{name} is not signed in."),
        facts: vec![
            format!("Agent: {name}, run as {}.", agent.executable()),
            profiled(agent),
            how,
        ],
        choices: vec![
            (
                CHECK_AGAIN.to_owned(),
                "Read the agent's sign-in again.".to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// What the person is told where `agent`'s sign-in could not be read.
fn unverified(agent: &ExternalAgent) -> Confirmation {
    let name = agent.name();
    Confirmation {
        asks: Ended::Unverified(Failure::new("")).says().to_owned(),
        facts: vec![
            format!("Agent: {name}, run as {}.", agent.executable()),
            profiled(agent),
            "Status: the agent did not say whether it is signed in.".to_owned(),
        ],
        choices: vec![
            (
                CHECK_AGAIN.to_owned(),
                "Read the agent's sign-in again.".to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// The route a connect of `provider` through `agent` would write, `keyed`
/// where a key is stored for the provider.
fn routing(agent: &ExternalAgent, provider: &str, keyed: bool) -> Confirmation {
    let name = agent.name();
    let mut facts = vec![
        format!("Provider: {provider} is reached through {name} from now on."),
        format!("Agent: {name}, run as {}.", agent.executable()),
    ];
    if keyed {
        facts.push(format!(
            "Provider {provider} switches from a stored API key to this agent; the key is kept."
        ));
    }
    Confirmation {
        asks: format!("Connect {provider} through {name}?"),
        facts,
        choices: vec![
            (
                CONNECT.to_owned(),
                "Crucible writes the route down and uses this agent for the provider.".to_owned(),
            ),
            ("Go back".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// The profile line of a question about `agent`.
fn profiled(agent: &ExternalAgent) -> String {
    match agent.profile() {
        Some(profile) => format!("Profile: {profile}."),
        None => "Profile: the agent's default.".to_owned(),
    }
}

/// Takes `agent`'s settings out of the user file, once its run in this
/// conversation is stopped and new work on it refused.
async fn remove(
    conversation: &mut Conversation,
    desk: &Desk<'_>,
    agent: &ExternalAgent,
) -> Account {
    let mut coordinator = match coordinating(desk.switching.choosing, agent) {
        Ok(coordinator) => coordinator,
        Err(contended) => return Account::Contended(contended),
    };
    let mut slot = match coordinator.begin(Work::Remove, agent.profile(), 0) {
        Ok(slot) => slot,
        Err(contended) => return Account::Contended(contended),
    };
    if let Err(contended) = slot.advance(Stage::Closed) {
        return Account::Contended(contended);
    }
    // Whether the run stopped is not what a removal reports: once the settings
    // are gone, nothing can start it again.
    let _ = conversation.disconnect(agent.name()).await;
    for stage in [Stage::Quiesced, Stage::Committing] {
        if let Err(contended) = slot.advance(stage) {
            return Account::Contended(contended);
        }
    }
    match remember::removing(desk.switching.choosing, agent.name()) {
        Ok(()) => match slot.finish(Ending::Succeeded) {
            Ok(_) => Account::Removed,
            Err(contended) => Account::Contended(contended),
        },
        Err(problem) => {
            let _ = slot.finish(Ending::Failed);
            match problem {
                RememberError::Unusable(ConfigError::StillReferenced { provider, .. }) => {
                    Account::StillReferenced(provider.into())
                }
                problem => Account::Unremoved(problem),
            }
        }
    }
}

/// Takes `agent` out of use from Crucible: refused new work, its run in this
/// conversation stopped, and `enabled: false` written down.
async fn disconnect(
    conversation: &mut Conversation,
    desk: &Desk<'_>,
    agent: &ExternalAgent,
) -> Account {
    let mut coordinator = match coordinating(desk.switching.choosing, agent) {
        Ok(coordinator) => coordinator,
        Err(contended) => return Account::Contended(contended),
    };
    // No generation of the instance's settings is kept yet, so every record
    // says the first.
    let mut slot = match coordinator.begin(Work::Disconnect, agent.profile(), 0) {
        Ok(slot) => slot,
        Err(contended) => return Account::Contended(contended),
    };
    if let Err(contended) = slot.advance(Stage::Closed) {
        return Account::Contended(contended);
    }
    let stopped = conversation.disconnect(agent.name()).await.unwrap_or(true);
    for stage in [Stage::Quiesced, Stage::Committing] {
        if let Err(contended) = slot.advance(stage) {
            return Account::Contended(contended);
        }
    }
    match remember::disabling(desk.switching.choosing, agent.name()) {
        Ok(()) => match slot.finish(Ending::Succeeded) {
            Ok(_) => Account::Disconnected { stopped },
            Err(contended) => Account::Contended(contended),
        },
        Err(problem) => {
            // The record is closed as failed where it can be, and otherwise
            // left open for the next operation to find; either way the file
            // still enables the instance.
            let _ = slot.finish(Ending::Failed);
            Account::ThisRunOnly(problem)
        }
    }
}

/// The coordinator of `agent`'s credential scope, its journal kept beside the
/// user file `choosing`.
fn coordinating(choosing: &Path, agent: &ExternalAgent) -> Result<Coordinator, Contended> {
    let directory = choosing
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join("accounts");
    let scope = format!(
        "{}:{}",
        agent.driver().name(),
        agent.profile().unwrap_or("default")
    );
    let journal = Journal::of(directory, &scope)?;
    let admission = if agent.enabled() {
        Admission::Enabled
    } else {
        Admission::Disabled
    };
    Ok(Coordinator::restore(journal, agent.name(), admission)?)
}
