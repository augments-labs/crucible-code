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

use std::path::Path;

use crucible_client_api::{
    Command, ErrorCode, LogoutOutcome, Outcome, Problem, Refusal, Request, Text,
};
use crucible_config::{ConfigError, ExternalAgent};
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
