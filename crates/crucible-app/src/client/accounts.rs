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
//! | disconnect, the file not written, the conversation on it | disconnect | [`LogoutOutcome::StoppedForThisRunOnly`] |
//! | disconnect, the file not written, the conversation not on it | disconnect | [`ErrorCode::Failed`] |
//! | remove, a provider reached through it | nothing | [`LogoutOutcome::StillReferenced`] |
//! | remove | remove | [`LogoutOutcome::IntegrationRemoved`] |
//! | remove, the file not written, the conversation on it | remove | [`LogoutOutcome::StoppedForThisRunOnly`] |
//! | remove, the file not written, the conversation not on it | remove | [`ErrorCode::Failed`] |
//! | remove, the file not readable | remove | [`ErrorCode::Failed`] |
//! | remove, an instance the user file does not name | nothing | [`ErrorCode::InvalidArgument`] |
//! | sign out | sign out | [`LogoutOutcome::AgentSignedOut`] |
//! | sign out, not shown signed out afterwards | sign out | [`LogoutOutcome::SignOutUnverified`] |
//! | sign out, the file not written, the conversation on one of the instances sharing the scope | sign out | [`LogoutOutcome::StoppedForThisRunOnly`] |
//! | sign out, the file not written, the conversation on none of them | sign out | [`ErrorCode::Failed`] |
//! | sign out | sign out, its agent's route unanswered, declined | [`LogoutOutcome::Undecided`] |
//! | sign out | sign out, its agent's route unanswered, the yes not written | [`LogoutOutcome::HoldUnkept`] |
//! | sign out, its agent's route unanswered, the agent cannot be started | sign out | [`LogoutOutcome::Undecided`], the route not asked |
//! | sign out, an agent this release has no launch for, or a catalog agent the catalog believed gives no row | sign out | [`LogoutOutcome::Undecided`], said so, the route not asked |
//! | sign out, a run of an instance sharing the scope not confirmed stopped | sign out | [`LogoutOutcome::Undecided`], the route not asked |
//! | sign out | disconnect instead | as disconnect |
//! | any | cancel, back, or nobody | [`LogoutOutcome::Undecided`] |
//! | connect, signed in, already on the instance | the agent's route accepted | [`LoginOutcome::AgentConnected`] |
//! | connect, signed in | the agent's route accepted | [`LoginOutcome::AgentConnected`], naming the instance moved onto |
//! | connect, signed in, the agent not startable | the agent's route accepted | [`LoginOutcome::AgentConnected`], naming none |
//! | connect, signed in | the agent's route accepted; the file not written | [`LoginOutcome::ConnectUnsaved`] |
//! | connect | the agent's route accepted; the yes not written | [`LoginOutcome::ConnectUnsaved`] |
//! | connect, the agent's route unanswered, the agent cannot be started | cancel, back, or nobody | [`LoginOutcome::Undecided`], the route not asked |
//! | connect | cancel, back, or nobody | [`LoginOutcome::Undecided`] |
//! | connect, an instance the file does not name | nothing | [`ErrorCode::InvalidArgument`] |
//! | connect, an agent whose provider this build does not offer | nothing | [`ErrorCode::UnknownProvider`] |
//! | connect, no instance named, no agent shipping | nothing | [`ErrorCode::InvalidArgument`] |
//! | connect, no instance named | an agent not found, then cancel | [`LoginOutcome::Undecided`] |
//! | connect, of a client claiming install, the agent not installed and its release one crucible installs | install | [`LoginOutcome::AgentChosen`], that install, nothing started, asked of its route or written |
//! | connect, no instance named, of a client claiming install | a catalog agent, then install, update or roll back | [`LoginOutcome::AgentChosen`], that move, nothing started, asked of its route or written |
//! | connect, no instance named, of a client claiming install | a catalog agent, then a sign-in method of its instance | [`LoginOutcome::AgentChosen`], that sign-in, nothing started, asked of its route or written |
//! | connect, of a client claiming install, the agent read signed out, its instance in the file | a sign-in method | [`LoginOutcome::AgentChosen`], that sign-in, after the route's yes where it was asked and the agent's status read, nothing more |
//! | connect, no instance named, of a client claiming install, the user file not readable | a catalog agent | said so, no move offered, then [`LoginOutcome::Undecided`] |
//!
//! A removal whose write fails after the conversation's run on the instance
//! was stopped is told that the instance is stopped for this run only: the
//! settings are still in the file, so the next run starts the agent as before,
//! and a client told only that it failed would not know the conversation can
//! no longer use it. Where the conversation was not on the instance, nothing
//! was stopped, and it is told only that the removal failed. A disconnect
//! whose write fails is told the same way, and a sign-out, which takes every
//! instance sharing the credential scope out of use, is told that the
//! instances are stopped for this run only where the conversation was on any
//! of them, and otherwise only that it failed.
//!
//! A sign-out runs the agent's own program, so once it is chosen it puts the
//! agent's content-use route to the client where that has no yes, before
//! anything is changed: a no, or a yes that cannot be written down, leaves
//! every instance, its run and the files as they were. An agent that would be
//! refused before any process, as the launcher decides it, is said to be so
//! first and the route is not asked: nobody agrees to a launch that cannot
//! happen. So is a sign-out while any instance sharing the scope has a run
//! that could not be confirmed stopped, since the agent's program would run
//! beside it: that is said before the route is asked, and nothing is
//! disconnected, written or started. Choosing to disconnect
//! instead starts nothing of the agent's and asks nothing.
//!
//! Instances share a credential scope where they run one agent on one profile
//! directory; two catalog instances run one agent only where they name one
//! key. A sign-out takes every instance sharing the credential scope out of use
//! before the agent's own program is run: their runs in this conversation are
//! stopped, and `enabled: false` is written for each in one update of the
//! user file, with the `access` of every provider reached through any of
//! them, so the next start asks those providers directly. A disconnect writes
//! the same for its one instance. Only once that write has landed is the agent's logout run, and
//! then a status step after it; where the write fails, no step is run. It is
//! reported signed out only where both steps read signed out, and otherwise as
//! unverified. Once the write has landed, a journal that cannot be kept is
//! reported as unverified too, since the instances are already out of use; a
//! stage the journal refused is not taken.
//!
//! A connect that names no instance first asks which shipping agent to go
//! through, then which of its configured instances where there are several,
//! and looks for the agent's program without running it: one not found is
//! said to be so, with what to install. An agent with no configured instance
//! is given one named after it, run as its usual command, written to the user
//! file only with the route.
//!
//! A client that claims [`Capability::Install`] is offered moves on an agent
//! as well, and one that does not is put exactly the questions it was put
//! before there were any. An agent not installed whose release crucible
//! installs is offered that install first. After the shipping agents come
//! the agents the catalog offers, read from the trust state beside the user's
//! configuration file as it is believed now, with nothing fetched, and
//! listed as the agents door lists them. Choosing one never connects it,
//! since no provider of this build's is reached through a catalog agent: it
//! asks instead whether to install the version the door would, update its
//! instance, roll it back to another version the store holds, or sign its
//! configured instance in by one of the agent's methods. A row says where its
//! instance runs the user's own program rather than a release crucible
//! installed, or a version the catalog revoked. Every
//! word of the catalog's is quoted before it is shown. A move chosen here, or
//! the install offered where the agent cannot be started, ends the connect as
//! [`Account::AgentChosen`] before anything is started, asked of its route or
//! written; a sign-in chosen once the agent read signed out ends it the same
//! way, after the route's yes and the status read below. The choice itself
//! changes nothing further: the client carries it out through the agents
//! door.
//!
//! A connect reaches the one provider the agent signs in to, which the
//! client does not name. For a client that did not say it answers account
//! questions it is undecided at once, before the agent is started, even where
//! the agent is signed in. Otherwise it says first, as a sign-out does, that
//! an agent the launcher would refuse cannot be started, without asking its
//! route, and starts nothing of the agent's before its content-use route has
//! a yes, and then reads only whether the agent is
//! signed in. Signed in, it is already connected, and nothing more is asked;
//! not signed in, or not read either way, the client is told so, with the
//! command that signs the agent in where it has one, and may check again,
//! [`CHECKS`] times in all. Read signed out, it offers a client claiming
//! [`Capability::Install`] a sign-in by each of the agent's methods the
//! agents door carries out, where the instance is in the user file, and says
//! otherwise that they are offered once it is. Nothing is written before the
//! agent reads signed in: then the provider's `access` names the instance,
//! the provider is the one asked, and the instance is enabled, in one update
//! of the user file, under the scope's journal, the instance's own settings
//! first where they are new.
//!
//! The conversation then moves onto the instance in a new session, bound to
//! its route, with nothing carried over, whichever provider or route it was
//! on; the session left stays listed and can be resumed. Where it cannot move,
//! it stays where it was and the next run starts on the agent. A conversation
//! already on the instance stays where it is.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crucible_client_api::{
    AgentAction, Capability, Command, ErrorCode, LoginOutcome, LogoutOutcome, Name, Outcome,
    Problem, Refusal, Request, Text,
};
use crucible_config::{ConfigError, Driver, ExternalAgent, Home, Settings};
use crucible_execution::Authentication;
use crucible_execution::{Admission, Ending, Failure};
use crucible_external_harnesses::catalog::registry::RegistryIndex;
use crucible_external_harnesses::catalog::{AuthMethod, Method, Route, Trust};
use crucible_external_harnesses::store::{
    Probe, Receipt, Source as ReleaseSource, Store, StoreError, Version,
};
use crucible_external_harnesses::{
    Account as Step, Contended, Coordinator, Discovery, Ended, Journal, Reading, Stage, Work,
};
use crucible_types::AgentKey;

use super::agents::LISTED;
use super::deciding::{Confirmation, Front, confirmed};
use super::performing::{Desk, Performed};
use crate::Conversation;
use crate::backends::{AgentRow, Started, Unbelieved};
use crate::launching::{Origin, Unlaunched, quoted};
use crate::own_program::Platform;
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
    /// Nothing of the conversation's was stopped, because what had to be
    /// written first could not be: for a removal, the file could not be read
    /// or the conversation was not on the instance; for a disconnect, the
    /// conversation was not on the instance; for a sign-out, the conversation
    /// was on none of the instances sharing the credential scope.
    Unremoved(RememberError),
    /// The instance's run in this conversation was stopped and it takes no new
    /// work, and its settings could not be taken out of the file, so they are
    /// still there for the next run.
    RemovalUnsaved(RememberError),
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
    /// The agent's own provider is reached through the instance, which takes
    /// new work, and that is written down; the conversation stayed where it
    /// was, since it was already on that instance.
    Connected,
    /// As [`Account::Connected`], and the conversation moved onto the
    /// instance named here in a new session.
    Moved(Box<str>),
    /// As [`Account::Connected`], and the conversation stayed where it was
    /// for this reason; the next run starts on the instance.
    Unmoved(String),
    /// The agent read signed in, and the route through it could not be
    /// written down, so nothing changed.
    ConnectUnsaved(RememberError),
    /// The yes to the agent's content-use route could not be written down,
    /// so nothing of the agent's was started and nothing changed.
    HoldUnkept(RememberError),
    /// As [`Account::HoldUnkept`], for a sign-out: the agent's program was
    /// not run and no instance was stopped.
    SignOutHoldUnkept(RememberError),
    /// Nobody went on with a connect, so nothing changed.
    Unconnected,
    /// A move on an agent was chosen in place of the connect, and the choice
    /// itself changed nothing. Chosen where the agent cannot be started, or
    /// from the catalog's agents, it came before anything was started, asked
    /// of its route or written; a sign-in chosen once the agent read signed
    /// out came after the route's yes, where it was asked, and the status read.
    AgentChosen(AgentAction),
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
            Self::RemovalUnsaved(problem) => LogoutOutcome::StoppedForThisRunOnly {
                said: Text::cut(REMOVAL_UNSAVED),
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
                    onto: None,
                });
            }
            Self::Moved(instance) => {
                return Outcome::Login(LoginOutcome::AgentConnected {
                    said: Text::cut(&moved_onto(instance)),
                    onto: Name::new(instance).ok(),
                });
            }
            Self::Unmoved(_) => {
                return Outcome::Login(LoginOutcome::AgentConnected {
                    said: Text::cut(UNMOVED),
                    onto: None,
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
            Self::SignOutHoldUnkept(problem) => LogoutOutcome::HoldUnkept {
                said: Text::cut(HOLD_UNKEPT),
                problem: Problem::failed(problem),
            },
            Self::Unconnected => return Outcome::Login(LoginOutcome::Undecided),
            Self::AgentChosen(action) => {
                return Outcome::Login(LoginOutcome::AgentChosen {
                    action: action.clone(),
                });
            }
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

/// What a person is told where the instance was stopped and its settings could
/// not be taken out of the file.
pub const REMOVAL_UNSAVED: &str =
    "Stopped for this run only; the settings could not be removed and are still saved.";

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

/// What a person is told once a provider is reached through an agent and the
/// conversation moved onto `instance`: the route it is on now, and that
/// nothing came with it.
#[must_use]
pub fn moved_onto(instance: &str) -> String {
    format!(
        "Connected. This conversation now runs on the agent {instance} and starts fresh, \
         in a new session; the earlier one can be resumed."
    )
}

/// What a person is told once a provider is reached through an agent and the
/// conversation could not move onto it.
pub const UNMOVED: &str = "Connected. This conversation stays where it is; \
     the next run starts on the agent.";

/// What a person is told where the agent is signed in and the route through
/// it could not be written down.
pub const CONNECT_UNSAVED: &str =
    "The agent signed in, but Crucible could not activate the integration.";

/// What a person is told where the yes to an agent's route could not be
/// written down.
pub const HOLD_UNKEPT: &str =
    "Crucible could not write down the yes to this agent's route, so nothing was changed.";

/// The choice that reads an agent's sign-in again.
pub const CHECK_AGAIN: &str = "Check again";

/// How many times a connect reads whether the agent is signed in, the first
/// time and each check again together, before it is given up as undecided.
pub const CHECKS: usize = 5;

/// Carries out `request` where it is an account operation, asking `front`
/// which way to go before anything changes.
///
/// Any other command is refused as [`ErrorCode::Busy`] and nothing is done:
/// [`perform`](super::perform) and the other doors answer them. The agent
/// catalog commands are the exception, refused as
/// [`ErrorCode::AgentsDoor`]: the agents door serves them, and no conversation
/// does. An instance the configuration does not name, or names as
/// already disabled, is refused as [`ErrorCode::InvalidArgument`], before
/// anything is asked.
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
                .filter(|other| scope(other) == scope(&agent))
                .collect();
            let confirmation = signing_out(&agent, &sharing, conversation.instance());
            match confirmed(request.capabilities(), front, &confirmation).await {
                Some(0) => {
                    if let Some(held) = still_running(conversation, request, front, &sharing).await
                    {
                        return Performed::Account(held);
                    }
                    if let Some(unasked) =
                        launch_asked(conversation, desk, request, front, &agent).await
                    {
                        return Performed::Account(unasked);
                    }
                    Performed::Account(sign_out(conversation, desk, &agent, &sharing).await)
                }
                Some(1) => Performed::Account(disconnect(conversation, desk, &agent).await),
                _ => Performed::Account(Account::Undecided),
            }
        }
        Command::Connect { instance } => {
            let Some(instance) = instance else {
                if conversation.agents().shipping().next().is_none() {
                    return Performed::Refused(ErrorCode::InvalidArgument.into());
                }
                let (agent, row, written) = match choose(conversation, request, desk, front).await {
                    Some(Picked::Through(chosen)) => chosen,
                    Some(Picked::Moving(action)) => {
                        return Performed::Account(Account::AgentChosen(action));
                    }
                    Some(Picked::Refused(agent, why)) => {
                        let capabilities = request.capabilities();
                        return Performed::Account(
                            unconnected(capabilities, front, &agent, &why).await,
                        );
                    }
                    None => return Performed::Account(Account::Unconnected),
                };
                let provider = match &row {
                    AgentRow::Built(row) => match served(desk, row) {
                        Some(provider) => Some(provider),
                        None => return Performed::Refused(ErrorCode::UnknownProvider.into()),
                    },
                    AgentRow::Catalog(_) => None,
                };
                let asked = Connecting {
                    provider,
                    agent: &agent,
                    row,
                    written,
                };
                return Performed::Account(
                    connect(conversation, request, desk, front, &asked).await,
                );
            };
            let Some(agent) = desk
                .switching
                .settings
                .external_agents()
                .into_iter()
                .find(|agent| agent.name() == instance.as_str())
            else {
                return Performed::Refused(ErrorCode::InvalidArgument.into());
            };
            let (provider, row) = match crate::backends::REGISTRY.row(agent.driver(), agent.agent())
            {
                Some(AgentRow::Built(row)) => {
                    let Some(provider) = served(desk, row) else {
                        return Performed::Refused(ErrorCode::UnknownProvider.into());
                    };
                    (Some(provider), AgentRow::Built(row))
                }
                // A catalog agent signs in to its own vendor, and no provider
                // of this build's is reached through it: the conversation is
                // only moved onto it, under the row the catalog believed gives.
                Some(AgentRow::Catalog(_)) | None if agent.driver() == Driver::Catalog => {
                    match catalog_row(conversation, &agent) {
                        Ok(row) => (None, row),
                        Err(why) => {
                            let capabilities = request.capabilities();
                            return Performed::Account(
                                unconnected(capabilities, front, &agent, &why).await,
                            );
                        }
                    }
                }
                Some(AgentRow::Catalog(_)) | None => {
                    return Performed::Refused(ErrorCode::InvalidArgument.into());
                }
            };
            let asked = Connecting {
                provider,
                agent: &agent,
                row,
                written: true,
            };
            Performed::Account(connect(conversation, request, desk, front, &asked).await)
        }
        // The agents door serves the install commands and no conversation is
        // part of it, so this door will never serve them: a client is told
        // which door does, not to come back.
        Command::AgentCatalog
        | Command::InstallAgent { .. }
        | Command::AddAgent { .. }
        | Command::UpdateAgent { .. }
        | Command::RollbackAgent { .. }
        | Command::RemoveAgent { .. }
        | Command::VerifyAgent { .. }
        | Command::SignInAgent { .. } => Performed::Refused(Refusal::from(ErrorCode::AgentsDoor)),
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
                "Crucible stops using this agent, and a provider reached through it is asked \
                 directly from the next session; its sign-in is left as it is."
                    .to_owned(),
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
                "Crucible stops using this instance, and a provider reached through it is asked \
                 directly from the next session; its sign-in is left as it is."
                    .to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// Settles, before a sign-out changes anything, that no instance in `sharing`
/// has a run that could not be confirmed stopped, since the agent's program
/// signs the whole credential scope out and would run beside it. Says so to
/// `front`, with what to do about it, and answers the account to end the
/// sign-out with where one has, so that nothing is disconnected, written or
/// started; `None` where the sign-out may go on. It is asked before the
/// agent's route, since nobody agrees to what cannot happen.
async fn still_running(
    conversation: &Conversation,
    request: &Request,
    front: &mut dyn Front,
    sharing: &[ExternalAgent],
) -> Option<Account> {
    let held = conversation.steps()?.unstopped(sharing);
    if held.is_empty() {
        return None;
    }
    let held = Confirmation {
        asks: "This agent cannot be signed out yet.".to_owned(),
        facts: vec![
            format!("Run not confirmed stopped: {}.", held.join(", ")),
            crucible_external_harnesses::Refusal::Unstopped.to_string(),
        ],
        choices: vec![("Go back".to_owned(), "Change nothing.".to_owned())],
    };
    // Whether it was seen changes nothing: the sign-out is refused either way.
    let _seen = confirmed(request.capabilities(), front, &held).await;
    Some(Account::Undecided)
}

/// Settles, before a sign-out changes anything, that the agent's own program
/// can be run for it, and puts the agent's route to `front` where it has no
/// yes. A launch the launcher would refuse, the program not installed, the
/// row withheld or no row for the agent at all, as for a catalog agent the
/// catalog believed gives none, is said to be so first, whatever state the
/// route is in, and the route is not put: nobody agrees to what cannot happen,
/// and nothing is written for a sign-out that cannot run. Answers the account
/// to end the sign-out with where the launch is so refused and the client goes
/// no further, where the route did not get its yes, or the yes could not be
/// written down, and `None` where the sign-out may go on.
async fn launch_asked(
    conversation: &Conversation,
    desk: &Desk<'_>,
    request: &Request,
    front: &mut dyn Front,
    agent: &ExternalAgent,
) -> Option<Account> {
    let capabilities = request.capabilities();
    let row = match conversation.steps() {
        Some(steps) => steps.row_of(agent),
        None => conversation
            .agents()
            .row(agent.driver(), agent.agent())
            .ok_or_else(|| Unbelieved::NoLaunch.into()),
    };
    let row = match row {
        Ok(row) => row,
        Err(Unlaunched::Unbelieved(Unbelieved::NoLaunch)) => {
            return Some(
                unrun(
                    capabilities,
                    front,
                    agent,
                    "This release has no launch for its driver, so the agent's own sign-out \
                     cannot run, and nothing is changed.",
                )
                .await,
            );
        }
        Err(why) => {
            let said =
                format!("{why} The agent's own sign-out cannot run, and nothing is changed.");
            return Some(unrun(capabilities, front, agent, &said).await);
        }
    };
    // A sign-out offers no move on the agent in place of itself.
    if found(
        conversation,
        desk,
        front,
        capabilities,
        agent,
        (&row, &shown_as(conversation, &row)),
        None,
    )
    .await
        != Some(Looked::Startable)
    {
        return Some(Account::Undecided);
    }
    let consent = conversation.consent()?;
    // A route nothing warns about has no yes to wait for and none to ask, and
    // is not one a sign-out may start the agent on.
    if consent.routes().warned(row.route()).is_none() {
        return Some(
            unrun(
                capabilities,
                front,
                agent,
                "Its route has no hold on what it sends, so the agent's own sign-out cannot \
                 be started, and nothing is changed.",
            )
            .await,
        );
    }
    let warned = consent.asks(row.route())?;
    if !super::deciding::warned(capabilities, front, &warned).await {
        return Some(Account::Undecided);
    }
    consent
        .accept(&warned)
        .err()
        .map(Account::SignOutHoldUnkept)
}

/// Tells `front` that the sign-out of `agent` cannot run for `why`, and that
/// nothing is changed, and answers the account to end it with. Whether it was
/// seen changes nothing: the sign-out is refused either way.
async fn unrun(
    capabilities: crucible_client_api::Capabilities,
    front: &mut dyn Front,
    agent: &ExternalAgent,
    why: &str,
) -> Account {
    let unrun = Confirmation {
        asks: format!("{} cannot be signed out.", agent.name()),
        facts: vec![
            why.to_owned(),
            format!("Agent: {}, run as {}.", agent.name(), agent.executable()),
        ],
        choices: vec![("Go back".to_owned(), "Change nothing.".to_owned())],
    };
    let _seen = confirmed(capabilities, front, &unrun).await;
    Account::Undecided
}

/// Tells `front` that `agent` cannot be connected for `why`, and that nothing
/// is changed, and answers the account to end it with. A catalog agent reaches
/// no provider of this build's, so nothing of it is written that a next run
/// could start from, and [`Account::Unmoved`] would say what is not so.
async fn unconnected(
    capabilities: crucible_client_api::Capabilities,
    front: &mut dyn Front,
    agent: &ExternalAgent,
    why: &str,
) -> Account {
    // A reason can be a whole sentence already, as a catalog refusal is, or a
    // clause that needs its full stop, as the sandbox policy's does: one full
    // stop ends the fact either way.
    let said = if why.ends_with('.') {
        why.to_owned()
    } else {
        format!("{why}.")
    };
    let refused = Confirmation {
        asks: format!("{} cannot be connected.", agent.name()),
        facts: vec![
            said,
            format!("Agent: {}, run as {}.", agent.name(), agent.executable()),
        ],
        choices: vec![("Go back".to_owned(), "Change nothing.".to_owned())],
    };
    let _seen = confirmed(capabilities, front, &refused).await;
    Account::Unconnected
}

/// Takes every instance in `sharing` out of use from Crucible, durably, and
/// only then signs `agent`'s credential scope out through its own program;
/// called only once [`still_running`] and [`launch_asked`] let it go on.
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
    // A run that could not be confirmed stopped is not met here: the sign-out
    // was refused for it before this was called.
    // Whether there was a run to stop is: where the write fails, only a
    // conversation that was on one of them has been stopped.
    let mut on_one = false;
    for one in sharing {
        on_one |= conversation.disconnect(one.name()).await.is_some();
    }
    if let Err(contended) = slot.advance(Stage::Quiesced) {
        return Account::Contended(contended);
    }
    let names: Vec<&str> = sharing.iter().map(ExternalAgent::name).collect();
    if let Err(problem) = desk
        .switching
        .recorded(remember::disabling_every(desk.switching.choosing, &names))
    {
        let _ = slot.finish(Ending::Failed);
        return if on_one {
            Account::ThisRunOnly(problem)
        } else {
            Account::Unremoved(problem)
        };
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

/// The row the configured catalog `agent` is run as, its route taught so it is
/// asked about; or what to say, with nothing started: its row cannot be made,
/// or it is disconnected, which only the user file puts back for an agent that
/// serves no provider.
fn catalog_row(conversation: &Conversation, agent: &ExternalAgent) -> Result<AgentRow, String> {
    if !agent.enabled() {
        return Err(crate::launching::disconnected(
            agent.name(),
            &agent.serves(),
        ));
    }
    let Some(steps) = conversation.steps() else {
        return Err("the sandbox policy its runs are confined by could not be built".to_owned());
    };
    steps.row_of(agent).map_err(|why| why.to_string())
}

/// The provider `row`'s agent signs in to, where this build offers it.
fn served(desk: &Desk<'_>, row: &crate::backends::Row) -> Option<&'static str> {
    let provider = row.driver.serves(None)?.provider()?;
    crate::providers::offered(desk.switching.providers)
        .any(|served| served.name == provider)
        .then_some(provider)
}

/// A connect asked for: the provider to reach, where the agent reaches one of
/// this build's, the instance to reach it through, the row of that instance's
/// agent, and whether the user file already names the instance or it is
/// written with the route.
struct Connecting<'a> {
    provider: Option<&'static str>,
    agent: &'a ExternalAgent,
    row: AgentRow,
    written: bool,
}

/// What a connect naming no instance goes through: the agent the client
/// picks, the instance of it, its row, and whether the user file already
/// names the instance.
type Chosen = (ExternalAgent, AgentRow, bool);

/// What the client picked when asked which agent to connect.
// A choice is made for one question and dropped with it; the row it carries is
// larger than the refusal beside it and is not collected.
#[allow(clippy::large_enum_variant)]
enum Picked {
    /// An instance to connect through.
    Through(Chosen),
    /// A move on an agent, in place of the connect.
    Moving(AgentAction),
    /// A catalog instance whose row is refused, and why.
    Refused(ExternalAgent, String),
}

/// What was chosen of what Crucible can do with a catalog agent.
enum Managed {
    /// One of the moves the agents door carries out.
    Moving(AgentAction),
    /// Moving the conversation onto its configured instance.
    Connecting(ExternalAgent),
}

/// The choice that moves a conversation onto a configured catalog instance.
const CONNECT: &str = "Connect";

/// A move offered as a choice: its name, what it does, and what the connect
/// ends with once it is chosen.
type Offered = (String, String, AgentAction);

/// Asks `front` which shipping agent to connect, and which
/// configured instance of it where there are several, then looks for its
/// program without starting it; answers nothing where the client cancelled
/// or answered nothing.
///
/// An agent with no configured instance is offered one named after it, run
/// as its usual command, or, for an agent whose release crucible installs and
/// whose command is not on the `PATH`, as that release; it is written only
/// with the route. An agent whose
/// program is not found is said to be so, and may be looked for again or
/// another picked, [`CHECKS`] times in all for each.
///
/// A client that claims [`Capability::Install`] is offered more, and the
/// move it chooses ends the connect: the release of an agent not installed
/// may be installed, and each agent the catalog offers is listed after the
/// shipping ones, as the agents door lists them, with a question of its own,
/// since no provider of this build's is reached through one. A row whose
/// release crucible installs says where its instance runs the user's own
/// program, or a version the catalog revoked. What the catalog offers is
/// read from the trust state beside the user's configuration file as it is
/// believed now: nothing is fetched. A client that does not claim it is
/// asked as though none of this were there.
async fn choose(
    conversation: &Conversation,
    request: &Request,
    desk: &Desk<'_>,
    front: &mut dyn Front,
) -> Option<Picked> {
    let capabilities = request.capabilities();
    let rows: Vec<&'static crate::backends::Row> = conversation.agents().shipping().collect();
    let shown = |row: &'static crate::backends::Row| shown_as(conversation, &AgentRow::Built(row));
    let releases = Releases::of(capabilities, desk);
    let (cataloged, more) = releases.as_ref().map_or_else(Default::default, listed);
    for _ in 0..CHECKS {
        let mut choices: Vec<(String, String)> = rows
            .iter()
            .map(|row| {
                let agent = shown(row);
                let said = row.driver.serves(None).map_or_else(
                    || format!("Run {agent}."),
                    |serves| format!("Reach {serves} through {agent}."),
                );
                let note = releases
                    .as_ref()
                    .and_then(|releases| releases.built(desk, row, conversation.steps()));
                (agent, noting(said, note))
            })
            .collect();
        let keys = releases.as_ref().map_or_else(Vec::new, |releases| {
            catalog_rows(releases, &cataloged, &mut choices)
        });
        choices.push(("Cancel".to_owned(), "Change nothing.".to_owned()));
        let mut facts = vec![
            "Crucible looks for the agent's program; nothing is started or written yet.".to_owned(),
        ];
        if more {
            facts.push("More agents are offered than are listed.".to_owned());
        }
        let agents = Confirmation {
            asks: "Which installed agent should Crucible use?".to_owned(),
            facts,
            choices,
        };
        let picked = confirmed(capabilities, front, &agents).await?;
        let Some(&row) = rows.get(picked) else {
            // Past the shipping agents, a catalog agent's row, and past those,
            // Cancel.
            let key = keys.get(picked.checked_sub(rows.len())?)?;
            match manage(front, capabilities, releases.as_ref()?, key).await? {
                Some(Managed::Moving(action)) => return Some(Picked::Moving(action)),
                Some(Managed::Connecting(agent)) => {
                    return Some(match catalog_row(conversation, &agent) {
                        Ok(row) => Picked::Through((agent, row, true)),
                        Err(why) => Picked::Refused(agent, why),
                    });
                }
                None => continue,
            }
        };
        let Some((agent, written)) = instance(
            desk,
            front,
            capabilities,
            (row, &shown(row)),
            conversation.steps(),
        )
        .await?
        else {
            continue;
        };
        let install = installable(releases.as_ref(), row);
        match found(
            conversation,
            desk,
            front,
            capabilities,
            &agent,
            (&AgentRow::Built(row), &shown(row)),
            install.as_ref(),
        )
        .await?
        {
            Looked::Startable => {
                return Some(Picked::Through((agent, AgentRow::Built(row), written)));
            }
            Looked::Moving(action) => return Some(Picked::Moving(action)),
            Looked::Back => {}
        }
    }
    None
}

/// `said`, followed by `note` where there is one.
fn noting(said: String, note: Option<String>) -> String {
    match note {
        Some(note) => format!("{said} {note}"),
        None => said,
    }
}

/// What the notes and moves offered to a client that claims
/// [`Capability::Install`] are read from, as the agents door reads them: the
/// catalog's trust state in crucible's home, the directory beside the user's
/// configuration file, as it is believed now, and crucible's store of the
/// releases it installs there.
struct Releases {
    home: PathBuf,
    trust: Trust,
    store: Store,
    /// The cached ACP Registry index, where one could be read: its offers are
    /// listed after the signed catalog's and their release is held to its own
    /// receipt, not to the signed trust state.
    registry: Option<RegistryIndex>,
}

impl Releases {
    /// Read for a client that claims [`Capability::Install`]; nothing for
    /// one that does not, or where the user file is in no directory.
    fn of(capabilities: crucible_client_api::Capabilities, desk: &Desk<'_>) -> Option<Self> {
        if !capabilities.has(Capability::Install) {
            return None;
        }
        let home = desk.switching.choosing.parent()?;
        Some(Self {
            home: home.to_path_buf(),
            trust: crate::catalog_fetch::trust_in(home),
            store: Store::new(home.join("programs")),
            registry: crate::catalog_fetch::cached_registry(home)
                .ok()
                .map(|read| read.index),
        })
    }

    /// Every release this client offers for `agent`: the signed catalog's
    /// where it lists the agent, else the ACP Registry's.
    fn offers(&self, key: &AgentKey) -> Vec<crucible_external_harnesses::catalog::Offer> {
        let signed = self.trust.offers(Some(key));
        if !signed.is_empty() {
            return signed;
        }
        self.registry
            .as_ref()
            .and_then(|index| index.offer(key).ok())
            .and_then(|offer| offer.offer().ok())
            .into_iter()
            .collect()
    }

    /// The registry receipt of the version `version` of `key`, where it is one
    /// and carries a probe for this platform: the launch's own test, so the
    /// screen shows nothing the launch would refuse.
    fn registry_receipt(&self, key: &AgentKey, version: &Version) -> Option<Receipt> {
        let receipt = self.store.receipt(key, version).ok().flatten()?;
        (receipt.source == ReleaseSource::Registry
            && receipt
                .unprobed(&crucible_external_harnesses::catalog::registry::platform())
                .is_none())
        .then_some(receipt)
    }

    /// The probe record the version `version` of `key` was installed under,
    /// where its registry receipt carries one for this platform.
    fn probe(&self, key: &AgentKey, version: &Version) -> Option<Probe> {
        self.registry_receipt(key, version)?
            .probe
            .map(|probe| *probe)
    }

    /// The sign-in methods a registry agent's installed release records, read
    /// from the cached index and the receipt's probe record, with no trust
    /// state and no re-probe.
    fn registry_methods(&self, key: &AgentKey, version: &Version) -> Vec<AuthMethod> {
        let Some(index) = &self.registry else {
            return Vec::new();
        };
        let Some(probe) = self.probe(key, version) else {
            return Vec::new();
        };
        let Ok(offer) = index.offer(key) else {
            return Vec::new();
        };
        let Some(receipt) = self.registry_receipt(key, version) else {
            return Vec::new();
        };
        offer
            .installed(&receipt, &probe)
            .ok()
            .and_then(|entry| {
                let target = entry.here().cloned()?;
                crate::backends::CatalogRow::registry(&entry, &target, index.read_date().into())
                    .ok()
            })
            .map_or_else(Vec::new, |row| {
                AgentRow::Catalog(row).methods(crate::backends::Started::STORED)
            })
    }

    /// The agent `row` runs, where crucible installs its release.
    fn installs(&self, row: &crate::backends::Row) -> Option<AgentKey> {
        (row.pinned)(&self.trust)?;
        AgentKey::new(row.driver.name()).ok()
    }

    /// What a choice says of an instance of the agent `key` run as
    /// `executable`: that it runs the user's own program, which lies in no
    /// version of the agent in the store, or a version the catalog revoked;
    /// nothing otherwise.
    fn note(&self, key: &AgentKey, executable: &str) -> Option<String> {
        let Some(version) = self.store.version_of(key, Path::new(executable)) else {
            return Some("Runs your own program, not a release Crucible installed.".to_owned());
        };
        // A registry release has no signed revocations; its receipt's own
        // hash and probe record are what hold it.
        let registry = self
            .store
            .receipt(key, &version)
            .ok()
            .flatten()
            .is_some_and(|receipt| receipt.source == ReleaseSource::Registry);
        (!registry
            && self
                .trust
                .revoked(key, version.version(), version.lock_revision()))
        .then(|| {
            format!(
                "Runs version {}, which the agent catalog revoked.",
                quoted(version.version())
            )
        })
    }

    /// What the row of the shipping agent `row` says, where crucible installs
    /// its release: of the one instance of it configured, or, where none is,
    /// of the one a connect would make, its command looked for on the `PATH`
    /// of `steps`, or that it asks which of two it runs. Nothing where several
    /// are, whose own choices say it.
    fn built(
        &self,
        desk: &Desk<'_>,
        row: &'static crate::backends::Row,
        steps: Option<&crate::launching::Steps>,
    ) -> Option<String> {
        let key = self.installs(row)?;
        let configured: Vec<ExternalAgent> = desk
            .switching
            .settings
            .external_agents()
            .into_iter()
            .filter(|configured| configured.driver() == row.driver)
            .collect();
        match configured.as_slice() {
            [one] => self.note(&key, one.executable()),
            [] => match would_run(&self.home, row, &self.trust, steps) {
                Running::One { executable, .. } => self.note(&key, &executable),
                Running::Either { .. } => Some(
                    "Asks whether to run your own program or the release Crucible installed."
                        .to_owned(),
                ),
            },
            _ => None,
        }
    }
}

/// The catalog agents a connect naming no instance lists after the shipping
/// agents, as the agents door lists them: each agent the signed catalog
/// offers, then each the ACP Registry offers, once, by its key, with the first
/// version offered, but one whose key is a word a built-in driver is spelled
/// with; at most [`LISTED`], and whether more are offered than are listed.
fn listed(releases: &Releases) -> (Vec<(AgentKey, Box<str>)>, bool) {
    let mut agents: Vec<(AgentKey, Box<str>)> = Vec::new();
    let mut more = false;
    for offer in releases.trust.offers(None) {
        let key = offer.entry.key;
        if crate::backends::kept_word(key.as_str())
            || agents.iter().any(|(listed, _)| *listed == key)
        {
            continue;
        }
        if agents.len() == LISTED {
            more = true;
            continue;
        }
        agents.push((key, offer.pin.version));
    }
    // The ACP Registry's offers come after the signed catalog's: a signed or
    // compiled entry is never shadowed by the unsigned registry.
    if let Some(index) = &releases.registry {
        for offer in index.offers() {
            let key = offer.entry().id.clone();
            if crate::backends::kept_word(key.as_str())
                || agents.iter().any(|(listed, _)| *listed == key)
            {
                continue;
            }
            if agents.len() == LISTED {
                more = true;
                continue;
            }
            agents.push((key, offer.version().into()));
        }
    }
    (agents, more)
}

/// Adds to `choices` a row for each catalog agent in `cataloged`, named by
/// its key and saying the version the catalog offers first, with the note on
/// the one instance of it configured, where one is, or that the instances could
/// not be read; but not one whose name is a choice already. Answers the agent
/// of each row added, in order.
fn catalog_rows<'a>(
    releases: &Releases,
    cataloged: &'a [(AgentKey, Box<str>)],
    choices: &mut Vec<(String, String)>,
) -> Vec<&'a AgentKey> {
    let mut keys = Vec::new();
    for (key, version) in cataloged {
        let name = quoted(key.as_str());
        if name == "Cancel" || choices.iter().any(|(taken, _)| *taken == name) {
            continue;
        }
        let said = format!(
            "Manage {name}, which the catalog offers at {}.",
            quoted(version)
        );
        let note = match instances_of(releases, key).as_deref() {
            Ok([one]) => releases.note(key, one.executable()),
            Ok(_) => None,
            Err(_) => Some("Its instances could not be read.".to_owned()),
        };
        choices.push((name, noting(said, note)));
        keys.push(key);
    }
    keys
}

/// The instances of the catalog agent `key` the user file names now, read as
/// the agents door reads them, so one the door added while this client ran is
/// found without a restart.
fn instances_of(releases: &Releases, key: &AgentKey) -> Result<Vec<ExternalAgent>, ConfigError> {
    let home = Home::find(&|name| {
        (name == crucible_config::HOME).then(|| releases.home.clone().into_os_string())
    })?;
    Ok(Settings::read_home(&home)?
        .external_agents()
        .into_iter()
        .filter(|one| one.driver() == Driver::Catalog && one.agent() == Some(key))
        .collect())
}

/// Asks `front` what Crucible should do with the catalog agent `key`, which
/// no provider of this build's is reached through, so connecting it writes no
/// route and only moves the conversation onto it.
///
/// The instance it speaks of is chosen as [`instance`] chooses one, among the
/// instances of `key`: the one configured, one asked for of several, or none.
/// It says which versions crucible's store holds of the agent, that instance
/// or that there is none, and the instance's note; then offers the moves that
/// apply, as [`moves`] finds them, a sign-in by each method the agent's entry
/// gives for a configured instance, connecting that instance, and going back.
/// Where the user file cannot
/// be read it says so, in what the door says of it, and offers going back alone,
/// since the door refuses every move on that same read. Answers the move chosen,
/// `Some(None)` where the client went back to the list of agents, and nothing
/// where it cancelled or answered nothing.
async fn manage(
    front: &mut dyn Front,
    capabilities: crucible_client_api::Capabilities,
    releases: &Releases,
    key: &AgentKey,
) -> Option<Option<Managed>> {
    let shown = quoted(key.as_str());
    let noted = |one: &ExternalAgent| releases.note(key, one.executable());
    let read = instances_of(releases, key);
    let unread = read
        .as_ref()
        .err()
        .map(|unread| quoted(&unread.to_string()));
    let instance = match among(
        front,
        capabilities,
        &shown,
        read.unwrap_or_default(),
        &noted,
    )
    .await?
    {
        Configured::One(one) => Some(one),
        Configured::Unconfigured => None,
        Configured::Back => return Some(None),
    };
    let installed = releases.store.installed(key);
    let running = instance
        .as_ref()
        .and_then(|one| releases.store.version_of(key, Path::new(one.executable())));
    let mut facts = vec![held(&installed)];
    facts.push(match (&instance, &unread) {
        (Some(one), _) => format!("Agent: {}, run as {}.", one.name(), one.executable()),
        (None, Some(unread)) => unread.clone(),
        (None, None) => {
            "No instance of it is configured, so there is nothing to sign in.".to_owned()
        }
    });
    facts.extend(instance.as_ref().and_then(noted));
    let mut offered = if unread.is_some() {
        Vec::new()
    } else {
        moves(releases, key, &installed, running.as_ref())
    };
    if let Some(one) = &instance {
        let named: Vec<String> = offered.iter().map(|(name, ..)| name.clone()).collect();
        let mut taken: Vec<&str> = named.iter().map(String::as_str).collect();
        taken.extend([CONNECT, "Go back"]);
        let methods = methods_of(releases, key, running.as_ref());
        let (signing, more) = sign_ins(one, methods, &taken);
        if more {
            facts.push(MORE_METHODS.to_owned());
        }
        offered.extend(signing);
    }
    let (mut choices, mut actions): (Vec<(String, String)>, Vec<Managed>) = offered
        .into_iter()
        .map(|(name, says, action)| ((name, says), Managed::Moving(action)))
        .unzip();
    // A row is made only for a release the store holds, so an instance run as
    // anything else has nothing to connect.
    if let Some(one) = instance.filter(|_| running.is_some()) {
        choices.push((
            CONNECT.to_owned(),
            "Move this conversation onto it.".to_owned(),
        ));
        actions.push(Managed::Connecting(one));
    }
    choices.push(("Go back".to_owned(), "Choose another agent.".to_owned()));
    let question = Confirmation {
        asks: format!("What should Crucible do with {shown}?"),
        facts,
        choices,
    };
    let picked = confirmed(capabilities, front, &question).await?;
    Some(actions.into_iter().nth(picked))
}

/// What a question about a catalog agent says of the versions crucible's
/// store holds of it, as `installed` read them: at most [`LISTED`] of them.
fn held(installed: &Result<Vec<Receipt>, StoreError>) -> String {
    match installed {
        Ok(receipts) if receipts.is_empty() => {
            "Crucible's store holds no version of it.".to_owned()
        }
        Ok(receipts) => {
            let mut versions: Vec<String> = receipts
                .iter()
                .take(LISTED)
                .map(|receipt| quoted(&receipt.version))
                .collect();
            if receipts.len() > LISTED {
                versions.push("and more".to_owned());
            }
            format!("Crucible's store holds {}.", versions.join(", "))
        }
        Err(unread) => format!(
            "Crucible's store of it could not be read: {}.",
            quoted(&unread.to_string())
        ),
    }
}

/// The moves on the catalog agent `key` that apply, as the agents door
/// carries them out, in order: an install of the version it would install,
/// the first offered, where the store holds none; an update, where the
/// instance spoken of runs `running`, a version from the store, other than
/// the first offered; and a roll back, where it runs one and the store holds
/// another the catalog did not revoke.
///
/// An update and a roll back name the version running and the direction
/// alone. The door picks the version either moves to when it carries the move
/// out, from what every instance runs and what `agents.update` stands at,
/// which the one instance spoken of here cannot say.
fn moves(
    releases: &Releases,
    key: &AgentKey,
    installed: &Result<Vec<Receipt>, StoreError>,
    running: Option<&Version>,
) -> Vec<Offered> {
    let Ok(agent) = Name::new(key.as_str()) else {
        return Vec::new();
    };
    let shown = quoted(key.as_str());
    let offers = releases.offers(key);
    let first = offers.first().map(|offer| &offer.pin);
    let mut offered = Vec::new();
    if let (Ok([]), Some(first)) = (installed.as_deref(), first)
        && let Ok(version) = Name::new(&first.version)
    {
        offered.push((
            "Install".to_owned(),
            format!("Install {shown} {}.", quoted(&first.version)),
            AgentAction::Install {
                agent: agent.clone(),
                version,
            },
        ));
    }
    if let (Some(running), Some(first)) = (running, first)
        && (running.version() != &*first.version || running.lock_revision() != first.lock_revision)
    {
        offered.push((
            "Update".to_owned(),
            format!("Update {shown} from {}.", quoted(running.version())),
            AgentAction::Update {
                agent: agent.clone(),
            },
        ));
    }
    let other = |running: &&Version| {
        installed.as_ref().is_ok_and(|receipts| {
            receipts.iter().any(|receipt| {
                (&*receipt.version != running.version()
                    || receipt.lock_revision != running.lock_revision())
                    && (receipt.source == ReleaseSource::Registry
                        || !releases
                            .trust
                            .revoked(key, &receipt.version, receipt.lock_revision))
            })
        })
    };
    if let Some(running) = running.filter(other) {
        offered.push((
            "Roll back".to_owned(),
            format!("Roll back {shown} from {}.", quoted(running.version())),
            AgentAction::Rollback { agent },
        ));
    }
    offered
}

/// The sign-in methods the entry of the catalog agent `key` gives, as the
/// agents door reads them: from the release `running` where the catalog
/// offers it, else from the first offered; for a registry release, from the
/// cached index and the receipt's probe record, with no re-probe.
fn methods_of(releases: &Releases, key: &AgentKey, running: Option<&Version>) -> Vec<AuthMethod> {
    if let Some(version) =
        running.filter(|version| releases.registry_receipt(key, version).is_some())
    {
        return releases.registry_methods(key, version);
    }
    let offers = releases.trust.offers(Some(key));
    let offer = offers
        .iter()
        .find(|offer| {
            running.is_some_and(|version| {
                version.version() == &*offer.pin.version
                    && version.lock_revision() == offer.pin.lock_revision
            })
        })
        .or_else(|| offers.first());
    offer
        .and_then(|offer| crate::backends::CatalogRow::new(&offer.entry, &offer.pin.target).ok())
        .map_or_else(Vec::new, |row| {
            AgentRow::Catalog(row).methods(crate::backends::Started::STORED)
        })
}

/// What a question offering sign-in methods says where it lists fewer than
/// the agent gives.
const MORE_METHODS: &str = "More sign-in methods are offered than are listed.";

/// One choice for each of `methods` the agents door carries out for the
/// configured `instance`, by the agent's own program or with a key kept for
/// it, named as the agent names the method, quoted; but not one whose name
/// reads as one in `taken` or one named before it, as [`folded`] reads them,
/// nor one a contract name cannot be made of. At most [`LISTED`], and whether
/// more were offered than are listed.
fn sign_ins(
    instance: &ExternalAgent,
    methods: Vec<AuthMethod>,
    taken: &[&str],
) -> (Vec<Offered>, bool) {
    let mut offered: Vec<Offered> = Vec::new();
    let mut more = false;
    for method in methods {
        let says = match (&method.method, &method.description) {
            (
                Method::Agent
                | Method::EnvVar { .. }
                | Method::Terminal {
                    route: Route::Piped,
                    ..
                },
                Some(description),
            ) => quoted(description),
            (
                Method::Terminal {
                    route: Route::Terminal,
                    ..
                },
                Some(description),
            ) => format!(
                "{} It runs outside the sandbox, on this terminal.",
                quoted(description)
            ),
            (
                Method::Terminal {
                    route: Route::Piped,
                    ..
                },
                None,
            ) => "Sign in by a command the agent runs, typed to at a terminal.".to_owned(),
            (
                Method::Terminal {
                    route: Route::Terminal,
                    ..
                },
                None,
            ) => "Sign in at the agent's own command, which runs outside the sandbox.".to_owned(),
            (Method::Agent, None) => "Sign in through the agent's own program.".to_owned(),
            (Method::EnvVar { .. }, None) => {
                "Sign in with a key Crucible keeps for the agent.".to_owned()
            }
        };
        let name = quoted(&method.name);
        let reads = folded(&name);
        if taken.iter().any(|fixed| folded(fixed) == reads)
            || offered.iter().any(|(named, ..)| folded(named) == reads)
        {
            continue;
        }
        let (Ok(instance), Ok(method)) = (Name::new(instance.name()), Name::new(&method.id)) else {
            continue;
        };
        if offered.len() == LISTED {
            more = true;
            continue;
        }
        offered.push((name, says, AgentAction::SignIn { instance, method }));
    }
    (offered, more)
}

/// A choice's name as it is told from another: without the whitespace around
/// it, and lowercased, so that "Cancel " and "cancel" read as "Cancel" does.
fn folded(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The install of `row`'s release the agents door would make, which a
/// connect offers where the agent's program is not installed: the release
/// the trust state `releases` reads pins for this platform. Nothing for an
/// agent whose release crucible does not install, or without `releases`, as
/// for a client that does not claim [`Capability::Install`].
fn installable(releases: Option<&Releases>, row: &crate::backends::Row) -> Option<AgentAction> {
    let pinned = (row.pinned)(&releases?.trust)?;
    Some(AgentAction::Install {
        agent: Name::new(row.driver.name()).ok()?,
        version: Name::new(&pinned.version).ok()?,
    })
}

/// What was chosen among an agent's configured instances.
enum Configured {
    /// The one configured, or the one picked of several.
    One(ExternalAgent),
    /// None is configured.
    Unconfigured,
    /// The client went back to the list of agents.
    Back,
}

/// The instance among `configured`, of the agent shown as `agent`, that a
/// question about the agent speaks of: the one configured, or one asked for
/// of several, each choice saying what `noted` says of it; nothing where the
/// client cancelled or answered nothing.
async fn among(
    front: &mut dyn Front,
    capabilities: crucible_client_api::Capabilities,
    agent: &str,
    mut configured: Vec<ExternalAgent>,
    noted: &dyn Fn(&ExternalAgent) -> Option<String>,
) -> Option<Configured> {
    if configured.len() > 1 {
        let mut choices: Vec<(String, String)> = configured
            .iter()
            .map(|one| {
                let said = format!("Run as {}; {}", one.executable(), profiled(one));
                (one.name().to_owned(), noting(said, noted(one)))
            })
            .collect();
        choices.push(("Go back".to_owned(), "Choose another agent.".to_owned()));
        let instances = Confirmation {
            asks: format!("Which {agent} should be used?"),
            facts: vec![format!("{agent} is configured {} times.", configured.len())],
            choices,
        };
        let picked = confirmed(capabilities, front, &instances).await?;
        return Some(if picked < configured.len() {
            Configured::One(configured.swap_remove(picked))
        } else {
            Configured::Back
        });
    }
    Some(
        configured
            .pop()
            .map_or(Configured::Unconfigured, Configured::One),
    )
}

/// The instance of `row`'s agent, shown as `agent`, a connect goes through:
/// the one configured, one asked for of several, or one named after the
/// agent where none is, its command looked for on the `PATH` of `steps`, as
/// [`would_run`] says, the client asked which where it names two;
/// `Some(None)` where the client went back to the list
/// of agents, and nothing where it cancelled or answered nothing. Asked of a
/// client that claims [`Capability::Install`], each of several says its note,
/// where crucible installs the agent's release.
async fn instance(
    desk: &Desk<'_>,
    front: &mut dyn Front,
    capabilities: crucible_client_api::Capabilities,
    (row, agent): (&'static crate::backends::Row, &str),
    steps: Option<&crate::launching::Steps>,
) -> Option<Option<(ExternalAgent, bool)>> {
    let configured: Vec<ExternalAgent> = desk
        .switching
        .settings
        .external_agents()
        .into_iter()
        .filter(|configured| configured.driver() == row.driver)
        .collect();
    // Only a choice among several says a note, so only then is it read.
    let releases = (configured.len() > 1)
        .then(|| Releases::of(capabilities, desk))
        .flatten();
    let key = releases
        .as_ref()
        .and_then(|releases| releases.installs(row));
    let noted = |one: &ExternalAgent| {
        releases
            .as_ref()
            .zip(key.as_ref())
            .and_then(|(releases, key)| releases.note(key, one.executable()))
    };
    match among(front, capabilities, agent, configured, &noted).await? {
        Configured::One(one) => return Some(Some((one, true))),
        Configured::Back => return Some(None),
        Configured::Unconfigured => {}
    }
    let home = desk.switching.choosing.parent()?;
    let trust = crate::catalog_fetch::trust_in(home);
    let agent = match would_run(home, row, &trust, steps) {
        Running::One { executable, own } => unwritten(home, row, &executable, own)?,
        Running::Either { stored, own, note } => {
            let question = either(agent, &stored, &own, note);
            let answer = confirmed(capabilities, front, &question).await?;
            match answered(home, row, &stored, answer)? {
                Answered::Written(agent) => agent,
                Answered::Back => return Some(None),
            }
        }
    };
    Some(Some((agent, false)))
}

/// The instance of `row`'s agent written beside the user file in `home` for
/// `answer`, the choice taken of [`either`]'s question over crucible's release
/// at `stored`: the release, or the user's own program named by its command;
/// [`Answered::Back`] where the person went back, and nothing where no
/// instance can be written.
fn answered(
    home: &Path,
    row: &'static crate::backends::Row,
    stored: &Path,
    answer: usize,
) -> Option<Answered> {
    let (executable, own) = match answer {
        0 => (stored.to_str()?.to_owned(), false),
        1 => (AgentRow::Built(row).command(), true),
        _ => return Some(Answered::Back),
    };
    unwritten(home, row, &executable, own).map(Answered::Written)
}

/// What an answer to [`either`]'s question comes to.
#[derive(Debug)]
enum Answered {
    /// The instance the chosen program is run as, not yet written.
    Written(ExternalAgent),
    /// The person went back, so nothing is written.
    Back,
}

/// The program a new instance of a built-in row's agent is run as.
#[derive(Debug, PartialEq, Eq)]
enum Running {
    /// The one there is: run as `executable`, the user's own program outside
    /// crucible's install store where `own` says so.
    One {
        /// What the instance names as its program.
        executable: String,
        /// Whether it is the user's own program.
        own: bool,
    },
    /// Crucible's installed release at `stored` and the user's own program at
    /// `own`, of an agent whose release keeps a sign-in of its own, between
    /// which the person chooses; `note` is what the agent's row says of the
    /// own program.
    Either {
        /// The installed release.
        stored: PathBuf,
        /// The user's own program, found on `PATH`.
        own: PathBuf,
        /// What the person is told of the own program before choosing it.
        note: Option<&'static str>,
    },
}

/// The program an instance of `row`'s agent named after it is run as: the
/// release crucible installs under `home`, installed or not yet, where
/// `trust` offers one, unless its command is on the `PATH` of `steps`, as one
/// installed by hand is; otherwise its command. Where the release is
/// installed and the program on `PATH` is the user's own, of an agent whose
/// release keeps a sign-in of its own, neither is taken: the person chooses.
fn would_run(
    home: &Path,
    row: &'static crate::backends::Row,
    trust: &Trust,
    steps: Option<&crate::launching::Steps>,
) -> Running {
    let built = AgentRow::Built(row);
    let command = built.command();
    let by_hand = steps.and_then(|steps| match steps.discover(&command, &built.commands()) {
        Discovery::Found(program) => Some((
            program.path().to_path_buf(),
            steps.origin(program.path()),
            steps.sandboxed(),
        )),
        Discovery::Missing | Discovery::NotExecutable(_) => None,
    });
    let managed = crate::installing::executable(home, row, trust);
    match (managed, by_hand) {
        (Some(stored), Some((own, Origin::Own, sandboxed)))
            if row.driver.profile_needed() && stored.is_file() =>
        {
            Running::Either {
                stored,
                own,
                note: built.own_note(sandboxed),
            }
        }
        (_, Some((_, origin, _))) => Running::One {
            executable: command,
            own: match origin {
                Origin::Own => true,
                Origin::Stored => false,
            },
        },
        (managed, None) => Running::One {
            executable: managed
                .as_deref()
                .and_then(Path::to_str)
                .map_or(command, str::to_owned),
            own: false,
        },
    }
}

/// The question that asks which of the agent shown as `agent`'s programs a
/// new instance runs: crucible's installed release at `stored`, or the user's
/// own program at `own`, of which `note` is said first.
fn either(agent: &str, stored: &Path, own: &Path, note: Option<&str>) -> Confirmation {
    let mut facts = vec![format!(
        "{agent} is installed by Crucible and is also on your PATH."
    )];
    facts.extend(note.map(str::to_owned));
    Confirmation {
        asks: format!("Which {agent} should be used?"),
        facts,
        choices: vec![
            (
                "Crucible's release".to_owned(),
                format!(
                    "Run {}, signed in in a profile directory of its own.",
                    stored.display()
                ),
            ),
            (
                "Your own program".to_owned(),
                format!("Run {} with no profile of its own.", own.display()),
            ),
            ("Go back".to_owned(), "Choose another agent.".to_owned()),
        ],
    }
}

/// The instance of `row`'s agent named after it that runs `executable`,
/// written beside the user file in `home`: one whose release shares its usual
/// home with another program of its vendor is given a profile directory of
/// its own there, unless it runs the user's own program, as `own` says, which
/// is given none, so it looks for its sign-in in its usual home.
fn unwritten(
    home: &Path,
    row: &'static crate::backends::Row,
    executable: &str,
    own: bool,
) -> Option<ExternalAgent> {
    let name = row.driver.name();
    let profile = if row.driver.profile_needed() && !own {
        Some(home.join("agents").join(name).to_str()?.to_owned())
    } else {
        None
    };
    // A row is a built-in driver's, so its instance names no catalog agent.
    ExternalAgent::unwritten(name, row.driver, executable, profile.as_deref(), None)
}

/// What the person is told of the configured `agent` of `row`, where its
/// program, found on the `PATH` of `steps`, is the user's own: what the row
/// says of one; nothing for a release in crucible's install store.
fn noted(
    steps: Option<&crate::launching::Steps>,
    row: &AgentRow,
    agent: &ExternalAgent,
) -> Option<&'static str> {
    let started = started(steps, row, agent)?;
    match started.origin {
        Origin::Own => row.own_note(started.sandbox.on),
        Origin::Stored => None,
    }
}

/// How the configured `agent` of `row` is started by `steps`: where its
/// program, found on their `PATH`, comes from, under the sandbox they start
/// a step under now, and whether it names a profile; nothing where it is not
/// found or no steps are taken.
fn started(
    steps: Option<&crate::launching::Steps>,
    row: &AgentRow,
    agent: &ExternalAgent,
) -> Option<Started> {
    let steps = steps?;
    let Discovery::Found(program) = steps.discover(agent.executable(), &row.commands()) else {
        return None;
    };
    Some(Started {
        origin: steps.origin(program.path()),
        sandbox: steps.sandboxing(),
        profiled: agent.profile().is_some(),
    })
}

/// How `row`'s agent is named to a person: as its route is warned about,
/// else by its driver, or a catalog agent by its key.
fn shown_as(conversation: &Conversation, row: &AgentRow) -> String {
    conversation
        .consent()
        .and_then(|consent| {
            let routes = consent.routes();
            routes
                .warned(row.route())
                .map(|warned| warned.shown.to_string())
        })
        .unwrap_or_else(|| row.named().to_owned())
}

/// What a start of `row`'s agent, shown as `shown`, is refused with where
/// nothing can be started, as the catalog's trust state in the directory beside
/// the user's configuration file, as it is now, bears on it; without that
/// directory, as a program that is not there is told with no release on offer.
fn unfound_in(desk: &Desk<'_>, row: &AgentRow, shown: &str) -> Option<[String; 2]> {
    match desk.switching.choosing.parent() {
        Some(home) => crate::launching::unfound(
            row,
            &Discovery::Missing,
            shown,
            &crate::catalog_fetch::trust_in(home),
        ),
        None => Discovery::Missing
            .refusal(shown, &crate::launching::looked_for(row))
            .map(|refused| crate::launching::renamed(&Discovery::Missing, refused)),
    }
}

/// What a look for an agent's program came to.
#[derive(Debug, PartialEq, Eq)]
enum Looked {
    /// The launcher would start it.
    Startable,
    /// The client went back to the list of agents.
    Back,
    /// The client chose this move on the agent in place of the connect.
    Moving(AgentAction),
}

/// Whether the launcher would start `agent`, of the row given and shown as
/// named: told where it would not, the program not installed, with the command
/// that installs it for an agent whose release crucible installs, or the row
/// withheld, the client may look again until it would or go back, or, where
/// the program is not installed and `install` is given, choose that install
/// instead, offered first; nothing where it cancelled or answered nothing. It
/// decides as a start does, so what is said here is what a start would
/// refuse.
// Each is an input of its own to the check, and a value gathering them would be
// built at each caller and taken apart here.
#[allow(clippy::too_many_arguments)]
async fn found(
    conversation: &Conversation,
    desk: &Desk<'_>,
    front: &mut dyn Front,
    capabilities: crucible_client_api::Capabilities,
    agent: &ExternalAgent,
    (row, shown): (&AgentRow, &str),
    install: Option<&AgentAction>,
) -> Option<Looked> {
    for _ in 0..CHECKS {
        // What a start would refuse, as the launcher decides it, so that what
        // is said here and what a start does cannot differ.
        let ([said, next], absent) = match conversation.steps() {
            Some(steps) => match steps.refusal(row, agent) {
                None => return Some(Looked::Startable),
                Some(Unlaunched::NotInstalled(sentences)) => (sentences, true),
                Some(other) => (
                    [other.to_string(), "Choose another agent.".to_owned()],
                    false,
                ),
            },
            None => match unfound_in(desk, row, shown) {
                Some(sentences) => (sentences, true),
                None => return Some(Looked::Startable),
            },
        };
        // An install is offered only for a program that is not there.
        let install = install.filter(|_| absent);
        let mut choices = Vec::new();
        if let Some(AgentAction::Install { version, .. }) = install {
            choices.push((
                "Install".to_owned(),
                format!("Install {shown} {}.", quoted(version.as_str())),
            ));
        }
        let again = choices.len();
        choices.push((
            CHECK_AGAIN.to_owned(),
            "Look for the agent's program again.".to_owned(),
        ));
        choices.push(("Go back".to_owned(), "Choose another agent.".to_owned()));
        // Why it cannot start is a fact, which the terminal keeps however short
        // its window, rather than the question it gives up first.
        let missing = Confirmation {
            asks: format!("{shown} cannot be started."),
            facts: vec![
                said,
                next,
                format!("Agent: {}, run as {}.", agent.name(), agent.executable()),
            ],
            choices,
        };
        let picked = confirmed(capabilities, front, &missing).await?;
        if picked == again {
            continue;
        }
        return Some(match install {
            Some(action) if picked < again => Looked::Moving(action.clone()),
            _ => Looked::Back,
        });
    }
    None
}

/// Reaches the provider asked for through its instance once the agent reads
/// signed in, asking `front` to check again where it does not.
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
        ref row,
        written,
    } = *asked;
    let capabilities = request.capabilities();
    // A client that did not say it answers account questions is asked nothing,
    // and an agent already signed in is no reason to act without its say.
    if !capabilities.has(Capability::Account) {
        return Account::Unconnected;
    }
    // Nothing of the agent's is started before its route has a yes, and a yes
    // is written down before anything is.
    if let Some(consent) = conversation.consent()
        && let Some(warned) = consent.asks(row.route())
    {
        // A launch that is refused whatever the answer is not asked about.
        let install = match row {
            AgentRow::Built(row) => installable(Releases::of(capabilities, desk).as_ref(), row),
            AgentRow::Catalog(_) => None,
        };
        match found(
            conversation,
            desk,
            front,
            capabilities,
            agent,
            (row, &shown_as(conversation, row)),
            install.as_ref(),
        )
        .await
        {
            Some(Looked::Startable) => {}
            Some(Looked::Moving(action)) => return Account::AgentChosen(action),
            Some(Looked::Back) | None => return Account::Unconnected,
        }
        if !super::deciding::warned(capabilities, front, &warned).await {
            return Account::Unconnected;
        }
        if let Err(problem) = consent.accept(&warned) {
            return Account::HoldUnkept(problem);
        }
    }
    let unstepped = || Reading::Unreadable(Failure::new("this run takes no agent steps"));
    let note = noted(conversation.steps(), row, agent);
    let how = started(conversation.steps(), row, agent);
    for _ in 0..CHECKS {
        let status = match conversation.steps() {
            Some(steps) => steps.account(agent, Step::Status).await,
            None => unstepped(),
        };
        let (question, moves) = match &status {
            // Signed in, it is already connected: there is nothing to ask.
            Reading::Signed(Authentication::SignedIn) => {
                let moved = moved(conversation, desk, provider, agent, written).await;
                return match (provider, moved) {
                    (None, Account::Unmoved(why)) => {
                        unconnected(capabilities, front, agent, &why).await
                    }
                    (_, moved) => moved,
                };
            }
            Reading::Signed(Authentication::SignedOut) => signed_out(
                agent,
                row,
                capabilities.has(Capability::Install).then_some(written),
                (note, how.unwrap_or(Started::STORED)),
                Platform::HERE,
            ),
            Reading::Signed(
                Authentication::Unknown | Authentication::Expired | Authentication::Unverifiable,
            )
            // The agent answered here, so it left no failure to give as the
            // reason.
            | Reading::Version(_) => (unverified(agent, None, note), Vec::new()),
            Reading::Unreadable(failure) => (unverified(agent, Some(failure), note), Vec::new()),
        };
        // Checking again is the first choice after the moves.
        let again = moves.len();
        let Some(picked) = confirmed(capabilities, front, &question).await else {
            return Account::Unconnected;
        };
        if let Some(action) = moves.into_iter().nth(picked) {
            return Account::AgentChosen(action);
        }
        if picked != again {
            return Account::Unconnected;
        }
    }
    Account::Unconnected
}

/// [`commit`], where a `provider` is reached through it, then moves
/// `conversation` onto `agent`, from whichever model
/// or route it was on, as [`Conversation::onto`] says. A conversation already
/// on `agent` stays where it is, and its next turn starts the agent again
/// where it was disconnected from it.
async fn moved(
    conversation: &mut Conversation,
    desk: &Desk<'_>,
    provider: Option<&str>,
    agent: &ExternalAgent,
    written: bool,
) -> Account {
    // An agent that reaches no provider of this build's has no route to write.
    let committed = provider.map_or(Account::Connected, |provider| {
        commit(desk, provider, agent, written)
    });
    if !matches!(committed, Account::Connected) {
        return committed;
    }
    if conversation.reconnect(agent.name()).is_some() {
        return committed;
    }
    let branch = crate::branching::current(desk.workspace.root());
    match conversation
        .onto(desk.sessions, desk.workspace, branch.as_deref(), agent)
        .await
    {
        Ok(left) => {
            // Nothing holds the session left after this; its log stays on the
            // disk where a resume can find it. Whether it closed cleanly is not
            // reported: the outcome says what the connect did, and the log's
            // own trouble is found again when it is resumed.
            let _unclosed = left.finish();
            Account::Moved(agent.name().into())
        }
        Err(reason) => Account::Unmoved(reason),
    }
}

/// Writes down that `provider` is reached through `agent`, and that `agent`
/// takes new work, under its scope's journal; `agent` itself too, where the
/// user file was not `written` with it.
fn commit(desk: &Desk<'_>, provider: &str, agent: &ExternalAgent, written: bool) -> Account {
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
    let wrote = if written {
        remember::connecting(desk.switching.choosing, provider, agent.name())
    } else {
        remember::installing(desk.switching.choosing, provider, agent)
    };
    if let Err(problem) = desk.switching.recorded(wrote) {
        let _ = slot.finish(Ending::Failed);
        return Account::ConnectUnsaved(problem);
    }
    // The route is written and in force whether or not the journal takes its
    // end; an open record is replaced by the next connect of the scope.
    let _ = slot.finish(Ending::Succeeded);
    Account::Connected
}

/// What the person is told where `agent`, of the row `row`, reads signed out,
/// and the move each choice before Check again ends the connect with.
///
/// A client that claims [`Capability::Install`], which gives whether the
/// instance is `written` in the user file, is offered first a sign-in by each
/// method of the agent's that the agents door carries out, where the instance
/// is written, since the door signs in only an instance the configuration
/// names; where it is not, it is told they are offered once it is. A client
/// that does not claim it gives `None` and is offered no move. A `note` on
/// the user's own program of the agent is said after its profile. The methods
/// offered, and what is said of signing in, are those of its program as it is
/// `started`, on `platform`.
fn signed_out(
    agent: &ExternalAgent,
    row: &AgentRow,
    written: Option<bool>,
    (note, started): (Option<&str>, Started),
    platform: Platform,
) -> (Confirmation, Vec<AgentAction>) {
    let _ = platform;
    let name = agent.name();
    let told = match row {
        AgentRow::Built(row) => (row.sign_in)(agent.profile().map(Path::new)),
        AgentRow::Catalog(_) => None,
    };
    let how = match told {
        Some(command) => {
            format!("Sign in: run `{command}` in a terminal, then choose Check again.")
        }
        None => "Sign in: open the agent and sign in there, then choose Check again.".to_owned(),
    };
    let mut facts = vec![
        format!("Agent: {name}, run as {}.", agent.executable()),
        profiled(agent),
    ];
    facts.extend(note.map(str::to_owned));
    facts.push(how);
    let methods = written.map_or_else(Vec::new, |_| row.methods(started));
    let (offered, more) = match written {
        Some(true) => sign_ins(agent, methods, &[CHECK_AGAIN, "Cancel"]),
        Some(false) if !methods.is_empty() => {
            facts.push(
                "Sign-in methods are offered once the instance is in the configuration.".to_owned(),
            );
            (Vec::new(), false)
        }
        _ => (Vec::new(), false),
    };
    if more {
        facts.push(MORE_METHODS.to_owned());
    }
    let (mut choices, moves): (Vec<(String, String)>, Vec<AgentAction>) = offered
        .into_iter()
        .map(|(named, says, action)| ((named, says), action))
        .unzip();
    choices.push((
        CHECK_AGAIN.to_owned(),
        "Read the agent's sign-in again.".to_owned(),
    ));
    choices.push(("Cancel".to_owned(), "Change nothing.".to_owned()));
    let question = Confirmation {
        asks: format!("{name} is not signed in."),
        facts,
        choices,
    };
    (question, moves)
}

/// What the person is told where `agent`'s sign-in could not be read, with
/// the one reason the agent's run gave, where it gave one, and after its
/// profile any `note` on the user's own program of the agent.
///
/// The reason is the harness's own words, bounded and with its withheld values
/// hidden, and only its first line: a front end takes control bytes out where
/// it draws it.
fn unverified(
    agent: &ExternalAgent,
    failure: Option<&Failure>,
    note: Option<&str>,
) -> Confirmation {
    let name = agent.name();
    let mut facts = vec![
        format!("Agent: {name}, run as {}.", agent.executable()),
        profiled(agent),
    ];
    facts.extend(note.map(str::to_owned));
    facts.push("Status: the agent did not say whether it is signed in.".to_owned());
    let reason = failure.and_then(reason);
    if let Some(reason) = reason {
        facts.push(format!("Reason: {reason}"));
    }
    Confirmation {
        asks: Ended::Unverified(Failure::new("")).says().to_owned(),
        facts,
        choices: vec![
            (
                CHECK_AGAIN.to_owned(),
                "Read the agent's sign-in again.".to_owned(),
            ),
            ("Cancel".to_owned(), "Change nothing.".to_owned()),
        ],
    }
}

/// The most characters a reason shown for an unread sign-in keeps.
pub(super) const REASON_CHARS: usize = 160;

/// The one line of `failure` a person is shown as its reason: its first line
/// that says anything, cut to [`REASON_CHARS`] with an ellipsis where it is
/// longer, and nothing where none does. A panel folds its facts whole, so a
/// reason as long as what an agent can write would leave no room for the
/// choices under it.
pub(super) fn reason(failure: &Failure) -> Option<String> {
    let line = failure
        .reason()
        .as_str()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    if line.chars().count() <= REASON_CHARS {
        return Some(line.to_owned());
    }
    let mut cut: String = line.chars().take(REASON_CHARS.saturating_sub(1)).collect();
    cut.push('…');
    Some(cut)
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
    // are gone, nothing can start it again. Whether there was a run to stop is:
    // where the write fails, only a conversation that was on the instance has
    // been stopped.
    let on_it = conversation.disconnect(agent.name()).await.is_some();
    for stage in [Stage::Quiesced, Stage::Committing] {
        if let Err(contended) = slot.advance(stage) {
            return Account::Contended(contended);
        }
    }
    match desk
        .switching
        .recorded(remember::removing(desk.switching.choosing, agent.name()))
    {
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
                problem if on_it => Account::RemovalUnsaved(problem),
                problem => Account::Unremoved(problem),
            }
        }
    }
}

/// Takes `agent` out of use from Crucible: refused new work, its run in this
/// conversation stopped, and `enabled: false` written down with every
/// provider's `access` through it taken away, so the next start asks that
/// provider directly.
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
    // Where the conversation was not on the instance there was no run to stop,
    // which counts as stopped; and where the write fails, only a conversation
    // that was on it has been stopped.
    let was_on = conversation.disconnect(agent.name()).await;
    let (on_it, stopped) = (was_on.is_some(), was_on.unwrap_or(true));
    for stage in [Stage::Quiesced, Stage::Committing] {
        if let Err(contended) = slot.advance(stage) {
            return Account::Contended(contended);
        }
    }
    match desk
        .switching
        .recorded(remember::disabling(desk.switching.choosing, agent.name()))
    {
        Ok(()) => match slot.finish(Ending::Succeeded) {
            Ok(_) => Account::Disconnected { stopped },
            Err(contended) => Account::Contended(contended),
        },
        Err(problem) => {
            // The record is closed as failed where it can be, and otherwise
            // left open for the next operation to find; either way the file
            // still enables the instance.
            let _ = slot.finish(Ending::Failed);
            if on_it {
                Account::ThisRunOnly(problem)
            } else {
                Account::Unremoved(problem)
            }
        }
    }
}

/// The credential scope `agent` signs in under: the agent it runs, as a
/// session on it is bound, and its profile directory, where it names one.
/// Instances share a scope exactly where both agree, and the journal of an
/// account operation on one is named by it, so the two cannot differ.
pub(super) fn scope(agent: &ExternalAgent) -> (Cow<'static, str>, Option<&str>) {
    (
        crate::launching::bound(agent.driver(), agent.agent()),
        agent.profile(),
    )
}

/// The coordinator of `agent`'s credential scope, its journal kept beside the
/// user file `choosing`.
fn coordinating(choosing: &Path, agent: &ExternalAgent) -> Result<Coordinator, Contended> {
    let directory = choosing
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join("accounts");
    let (bound, profile) = scope(agent);
    let scope = format!("{bound}:{}", profile.unwrap_or("default"));
    let journal = Journal::of(directory, &scope)?;
    let admission = if agent.enabled() {
        Admission::Enabled
    } else {
        Admission::Disabled
    };
    Ok(Coordinator::restore(journal, agent.name(), admission)?)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use crucible_external_harnesses::antigravity::own_note;
    use crucible_external_harnesses::fake_peer::{self, Script};

    use super::*;
    use crate::backends::ANTIGRAVITY;
    use crate::backends::tests::trusting;
    use crate::launching::tests::{antigravity_release, antigravity_steps};
    use crate::sample::Sample;

    /// The user's own Antigravity installed as `agy` in a directory of
    /// `sample`'s, outside crucible's install store, and the steps that look
    /// for programs there.
    fn own_agy(sample: &Sample) -> (PathBuf, crate::launching::Steps) {
        let bin = sample.root().join("bin");
        let own = fake_peer::install(&bin, "agy", &Script::of([])).expect("the fake peer");
        (own, antigravity_steps(sample, Some(OsString::from(&bin))))
    }

    /// Where crucible's installed release of Antigravity lies under the home
    /// of `sample`, whose trust state offers it, written there.
    fn stored_antigravity(sample: &Sample) -> (PathBuf, Trust) {
        let trust = trusting(sample, 2, &[antigravity_release("1.4.1")]);
        let stored = crate::installing::executable(&sample.home(), &ANTIGRAVITY, &trust)
            .expect("a release on offer");
        std::fs::create_dir_all(stored.parent().expect("a version directory"))
            .expect("a temporary directory");
        std::fs::write(&stored, b"").expect("a temporary directory");
        (stored, trust)
    }

    /// With crucible's release installed and the user's own `agy` on `PATH`,
    /// a new Antigravity instance is not given either silently: the person is
    /// asked, and each is offered with its path.
    #[tokio::test(flavor = "multi_thread")]
    async fn both_antigravity_programs_are_offered_when_both_exist() {
        let sample = Sample::new("accounts-antigravity-both");
        let (stored, trust) = stored_antigravity(&sample);
        let (own, steps) = own_agy(&sample);

        let running = would_run(&sample.home(), &ANTIGRAVITY, &trust, Some(&steps));

        let Running::Either {
            stored: offered,
            own: found,
            note,
        } = running
        else {
            panic!("{running:?}");
        };
        assert_eq!(offered, stored);
        assert_eq!(found, own);
        let question = either("Antigravity", &offered, &found, note);
        let said: Vec<&str> = question
            .choices
            .iter()
            .map(|(_, says)| says.as_str())
            .collect();
        for path in [&stored, &own] {
            let shown = path.display().to_string();
            assert!(
                said.iter().any(|says| says.contains(&shown)),
                "{shown} in {said:?}"
            );
        }
    }

    /// The user's own `agy`, chosen over crucible's release or found alone,
    /// is written with no profile directory, and what the person is told of
    /// it, before choosing it and of the instance afterwards, says whose
    /// sign-in it uses; crucible's release keeps the directory beside the
    /// user file.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_chooser_writes_an_own_agy_with_no_profile_directory_and_shows_the_note() {
        let alone = Sample::new("accounts-antigravity-own");
        let (_, steps) = own_agy(&alone);
        let home = alone.home();
        let trust = trusting(&alone, 2, &[antigravity_release("1.4.1")]);
        let Running::One { executable, own } = would_run(&home, &ANTIGRAVITY, &trust, Some(&steps))
        else {
            panic!("only the own program is there");
        };
        let written = unwritten(&home, &ANTIGRAVITY, &executable, own).expect("an instance");
        let both = Sample::new("accounts-antigravity-note");
        let (stored, _) = stored_antigravity(&both);
        let (found, beside) = own_agy(&both);
        let stored_instance =
            unwritten(&both.home(), &ANTIGRAVITY, &stored.to_string_lossy(), false)
                .expect("an instance");
        let note = own_note(beside.sandboxed());
        let question = either("Antigravity", &stored, &found, Some(note));
        let row = AgentRow::Built(&ANTIGRAVITY);

        assert_eq!(written.profile(), None, "{written:?}");
        assert!(
            question.facts.iter().any(|fact| fact == note),
            "{question:?}"
        );
        assert_eq!(noted(Some(&steps), &row, &written), Some(note));
        assert_eq!(noted(Some(&beside), &row, &stored_instance), None);
        let (signed_out, _) = signed_out(
            &written,
            &row,
            None,
            (Some(note), Started::STORED),
            Platform::HERE,
        );
        assert!(
            signed_out.facts.iter().any(|fact| fact == note),
            "{signed_out:?}"
        );
        let profile = both.home().join("agents").join("antigravity");
        assert_eq!(
            stored_instance.profile(),
            Some(profile.to_string_lossy().as_ref())
        );
    }

    /// The choice of the user's own program says what it is run with, which
    /// holds with the sandbox on and off, and leaves whose sign-in it then
    /// sees to the note beside it, which differs between the two.
    #[test]
    fn the_own_program_s_choice_holds_with_the_sandbox_on_and_off() {
        let stored = Path::new("/home/programs/antigravity/1.4.1/agy_acp_server.par");
        let own = Path::new("/usr/local/bin/agy");
        for sandboxed in [false, true] {
            let question = either("Antigravity", stored, own, Some(own_note(sandboxed)));

            let says = question
                .choices
                .iter()
                .find(|(name, _)| name == "Your own program")
                .map(|(_, says)| says.as_str());

            assert_eq!(
                says,
                Some(format!("Run {} with no profile of its own.", own.display()).as_str()),
                "sandboxed: {sandboxed}"
            );
        }
    }

    /// Each answer to the question over both programs writes what its choice
    /// says: crucible's release with the profile directory beside the user
    /// file, the user's own program by its command with none, and going back
    /// writes nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn each_answer_over_both_programs_writes_the_instance_its_choice_names() {
        let sample = Sample::new("accounts-antigravity-answered");
        let (stored, _) = stored_antigravity(&sample);
        let (own, _) = own_agy(&sample);
        let home = sample.home();
        let question = either("Antigravity", &stored, &own, None);
        let profile = home.join("agents").join("antigravity");

        let written: Vec<(&str, Option<Answered>)> = question
            .choices
            .iter()
            .enumerate()
            .map(|(answer, (name, _))| {
                (
                    name.as_str(),
                    answered(&home, &ANTIGRAVITY, &stored, answer),
                )
            })
            .collect();

        let [
            (release, Some(Answered::Written(release_instance))),
            (mine, Some(Answered::Written(own_instance))),
            (back, Some(Answered::Back)),
        ] = written.as_slice()
        else {
            panic!("{written:?}");
        };
        assert_eq!(*release, "Crucible's release");
        assert_eq!(release_instance.executable(), stored.to_string_lossy());
        assert_eq!(
            release_instance.profile(),
            Some(profile.to_string_lossy().as_ref())
        );
        assert_eq!(*mine, "Your own program");
        assert_eq!(
            own_instance.executable(),
            AgentRow::Built(&ANTIGRAVITY).command()
        );
        assert_eq!(own_instance.profile(), None, "{own_instance:?}");
        assert_eq!(*back, "Go back");
    }

    fn method(id: &str, name: &str) -> AuthMethod {
        AuthMethod {
            id: id.into(),
            name: name.into(),
            description: None,
            method: Method::Agent,
        }
    }

    /// A method named like a choice the signed-out question already gives,
    /// or like a method before it, is not offered, whatever the whitespace
    /// around its name and whatever its case.
    #[test]
    fn a_method_named_like_a_fixed_choice_or_an_earlier_one_is_not_offered() {
        let instance =
            ExternalAgent::unwritten("work", Driver::Antigravity, "antigravity", None, None)
                .unwrap();
        let methods = vec![
            method("trailing", "Cancel "),
            method("lower", "cancel"),
            method("again", " CHECK AGAIN"),
            method("key", "API key"),
            method("same", "api key "),
        ];

        let (offered, more) = sign_ins(&instance, methods, &[CHECK_AGAIN, "Cancel"]);

        let named: Vec<&str> = offered.iter().map(|(name, ..)| name.as_str()).collect();
        assert_eq!(named, ["API key"]);
        assert!(!more);
    }

    #[test]
    fn a_real_terminal_method_is_offered_with_the_word_that_it_runs_unconfined() {
        let instance =
            ExternalAgent::unwritten("work", Driver::Antigravity, "antigravity", None, None)
                .unwrap();
        let methods = vec![AuthMethod {
            id: "terminal".into(),
            name: "Sign in at a terminal".into(),
            description: None,
            method: Method::Terminal {
                command: "login".into(),
                args: Vec::new(),
                env: Vec::new(),
                route: Route::Terminal,
            },
        }];

        let (offered, more) = sign_ins(&instance, methods, &[]);

        assert_eq!(offered.len(), 1);
        assert!(!more);
        assert!(
            offered
                .first()
                .is_some_and(|(_, says, _)| says.contains("outside the sandbox")),
            "{offered:?}"
        );
    }
}
