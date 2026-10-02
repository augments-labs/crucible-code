//! The commands answered at once, by whoever holds the conversation.

use std::path::Path;
use std::sync::Arc;

use crucible_client_api::{
    CacheOutcome, CleanOutcome, ClearOutcome, Command, EffortOutcome, ErrorCode, Forced,
    LoginOutcome, LogoutOutcome, ModelOutcome, Name, NotesOutcome, Outcome, Palette, Problem,
    Refusal, Request, Resource, Response, ResumeOutcome, SandboxOutcome, SettingOutcome,
    SpeedOutcome, Standing, Theme, ThemeOutcome, Usage,
};
use crucible_runner::{Breakdown, PromptCacheCleanup};
use crucible_runtime::Cancel;
use crucible_session::{Session, SessionError};
use crucible_tools::Mode;
use crucible_types::{PromptCacheResourceError, PromptCacheResourceRecord, SessionId};
use crucible_workspace::Workspace;

use super::reading;
use crate::Conversation;
use crate::providers::{CredentialSource, Served, offered};
use crate::remember::{self, RememberError};
use crate::speed::Hastened;
use crate::switching::{LoggedIn, LoggedOut, Rung, Switched, Switching};

/// What the host lends a command: the registry generation and files a switch
/// is decided from, where sessions are kept, the workspace they belong to,
/// which syntax themes there are, and the release notes.
///
/// Lent by whoever stands on the host and never read out of a request, which
/// is what keeps a request a set of names: the provider it names is looked up
/// in this registry, the session it names under this workspace, and the syntax
/// theme it names among the ones this host reads code in.
#[derive(Debug, Clone, Copy)]
pub struct Desk<'a> {
    /// What a switch is decided from.
    pub switching: Switching<'a>,
    /// The directory sessions are recorded under.
    pub sessions: &'a Path,
    /// The workspace a session belongs to.
    pub workspace: &'a Workspace,
    /// Whether this host reads fenced code in a syntax theme of this name.
    ///
    /// Asked before a name a client sent is written down. The themes belong to
    /// whatever draws, which this crate does not reach, so the host answers.
    pub reads: fn(&str) -> bool,
    /// What the environment the host was started in says a variable is.
    ///
    /// Asked before a setting is written, since a setting the environment
    /// decides is the environment's: writing it down would change nothing.
    pub environment: fn(&str) -> Option<String>,
    /// The release notes: every release where no version is named, and
    /// otherwise the release of the version named, as it was written.
    ///
    /// The changelog is the one the host was built from, and its format
    /// belongs to the host, so the host answers, in the contract's bounded
    /// values, or refuses where it cannot put them in those.
    pub notes: fn(Option<&str>) -> Result<NotesOutcome, Refusal>,
}

/// What came of `/clear`.
#[derive(Debug)]
pub enum Cleared {
    /// Nothing had been said, so the session in hand is already the empty one.
    Nothing,
    /// A new session is being recorded into.
    Started {
        /// What the session left said about its log, where closing it said
        /// anything.
        unclosed: Option<Box<str>>,
    },
    /// The new log could not be started; the session in hand is untouched.
    Failed(SessionError),
}

/// What came of `/resume`.
#[derive(Debug)]
pub enum Resumed {
    /// The session named is the one already being recorded into.
    Same,
    /// The session named was picked back up.
    Picked {
        /// What the session left said about its log, where closing it said
        /// anything.
        unclosed: Option<Box<str>>,
    },
    /// No session of this workspace answers to the name.
    Unknown,
    /// Its log could not be read; the session in hand is untouched.
    Failed(SessionError),
}

/// What came of `/settings` changing one row.
#[derive(Debug)]
pub enum Setting {
    /// Written down, for this run and the next.
    Remembered,
    /// Something nearer than the user's own file decides it, so nothing was
    /// written.
    Forced(crucible_config::Forced),
    /// It could not be written down.
    Unwritten(RememberError),
}

/// What came of a command [`perform`] was given, in the application's own
/// values.
#[derive(Debug)]
pub enum Performed {
    /// Nothing was done, and why.
    Refused(Refusal),
    /// `/clear`.
    Cleared(Cleared),
    /// `/resume`.
    Resumed(Resumed),
    /// `/model`.
    Model(Switched),
    /// `/effort`.
    Effort(Rung),
    /// `/fast`.
    Speed(Hastened),
    /// `/mode`, by name or by stepping: the mode now in force.
    Mode(Mode),
    /// `/login`, once the credential was stored.
    Login(LoggedIn),
    /// `/logout`.
    Logout(LoggedOut),
    /// `/cache`: the persistent resources remembered.
    Cache(Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError>),
    /// `/cache clean`.
    Cleaned(Result<PromptCacheCleanup, PromptCacheResourceError>),
    /// `/sandbox`: whether commands run confined from here on.
    Sandbox {
        /// What was asked for.
        enabled: bool,
        /// Why nothing changed, where nothing did.
        unchanged: Option<crate::sandbox::Unchanged>,
    },
    /// `/theme`: written down for the next run, or why not.
    Theme(Result<(), RememberError>),
    /// `/settings`: one row written down, or why not.
    Setting(Setting),
    /// `/help`.
    Help,
    /// `/release-notes`, as the host answered it.
    Notes(NotesOutcome),
    /// `/context`: how the window of the next request is spent.
    Context {
        /// The model the request is for.
        model: Box<str>,
        /// The runner's count of it.
        breakdown: Breakdown,
    },
    /// `/usage`: what the session has used, and the plan windows its vendor
    /// last reported, read off the runner as it was asked; nothing is asked
    /// of a vendor.
    Usage(Box<Usage>),
    /// `/exit`.
    Leaving,
}

/// Carries out `request` against `conversation`, where it is one of the
/// commands answered at once.
///
/// A prompt, a compaction, a cancel and a decision are not: the first two are
/// [`turn`](super::turn)'s, the third is [`interrupt`](super::interrupt)'s,
/// and a decision settles through the [`Front`](super::Front) a stopped turn is
/// asking, while that turn is stopped. Handed here they are refused by name and
/// nothing is done: a decision that arrives as a command of its own finds
/// nothing pending, whatever identity it names, and is refused as stale —
/// here and at every other door, so a client reads one answer to it.
pub async fn perform(
    conversation: &mut Conversation,
    request: &Request,
    desk: &Desk<'_>,
) -> Performed {
    match request.command() {
        Command::Prompt(_) | Command::Compact | Command::Cancel => {
            Performed::Refused(ErrorCode::Busy.into())
        }
        // No turn is stopped here, so there is no action this could be about.
        Command::Decide(_) => Performed::Refused(ErrorCode::StaleDecision.into()),
        Command::Clear => Performed::Cleared(clear(conversation, desk).await),
        Command::Resume(id) => Performed::Resumed(resume(conversation, desk, id).await),
        Command::SelectModel {
            provider,
            model,
            effort,
        } => match served(desk, provider.as_str()) {
            Ok(selected) => Performed::Model(
                conversation
                    .ask_for(
                        selected,
                        model.as_str(),
                        effort.map(reading::effort),
                        &desk.switching,
                    )
                    .await,
            ),
            Err(refused) => Performed::Refused(refused),
        },
        Command::SetEffort(rung) => {
            Performed::Effort(conversation.think(reading::effort(*rung), &desk.switching))
        }
        Command::SetSpeed(pace) => {
            Performed::Speed(conversation.hasten(reading::speed(*pace), &desk.switching))
        }
        Command::SetMode(mode) => {
            let mode = reading::mode_in(*mode);
            conversation.switch(mode);
            Performed::Mode(mode)
        }
        Command::CycleMode => Performed::Mode(conversation.cycle()),
        Command::Login { provider } => match served(desk, provider.as_str()) {
            Ok(named) => Performed::Login(conversation.logged_in(named, &desk.switching).await),
            Err(refused) => Performed::Refused(refused),
        },
        Command::Logout { provider } => match served(desk, provider.as_str()) {
            Ok(named) => Performed::Logout(conversation.log_out(named, &desk.switching).await),
            Err(refused) => Performed::Refused(refused),
        },
        Command::InspectCache => Performed::Cache(conversation.prompt_cache_resources().await),
        Command::CleanCache => {
            Performed::Cleaned(conversation.clean_prompt_cache(&Cancel::new()).await)
        }
        Command::Sandbox { enabled } => Performed::Sandbox {
            enabled: *enabled,
            unchanged: crate::sandbox::choosing(
                desk.switching.settings,
                desk.workspace,
                desk.switching.choosing,
                *enabled,
            )
            .await
            .err(),
        },
        Command::Theme(_) | Command::Setting { .. } => keep(request, desk),
        Command::Help => Performed::Help,
        Command::ReleaseNotes { version } => {
            match (desk.notes)(version.as_ref().map(Name::as_str)) {
                Ok(notes) => Performed::Notes(notes),
                Err(refused) => Performed::Refused(refused),
            }
        }
        Command::Context => Performed::Context {
            model: conversation.runner().model().into(),
            breakdown: conversation.runner().breakdown(),
        },
        Command::Usage => {
            let runner = conversation.runner();
            Performed::Usage(Box::new(reading::usage(
                runner.model(),
                &runner.breakdown(),
                &runner.totals(),
                runner.plan_limits().as_ref(),
            )))
        }
        Command::Exit => Performed::Leaving,
    }
}

/// Carries out `request` where it is about how this machine's sessions are
/// drawn, and so about no conversation.
///
/// Apart from [`perform`] for the reason [`interrupt`](super::interrupt) is: a
/// front end is asked for another look while a turn has the conversation, and
/// what is written down is the host's file and nothing the turn holds. Any
/// other command is refused by name and nothing is done.
///
/// A syntax theme is a name a client chose, and it is looked up before it is
/// written: one this host does not read code in is refused as
/// [`ErrorCode::InvalidArgument`] and the file is left as it was, rather than
/// made to name a theme the next start cannot find.
///
/// A setting is held to the same: a key that is not one of the menu's rows, or
/// a value its row does not take, is refused as
/// [`ErrorCode::InvalidArgument`] and nothing is written. The rows are only
/// ever settings that loosen nothing, so a client cannot reach a permission or
/// the sandbox through this door. A setting the environment or a project file
/// decides is answered as forced and not written, since writing it would
/// change nothing.
pub fn keep(request: &Request, desk: &Desk<'_>) -> Performed {
    match request.command() {
        Command::Setting { name, value } => setting(desk, name.as_str(), value.as_str()),
        Command::Theme(Theme::Drawing(palette)) => Performed::Theme(remember::drawing(
            desk.switching.choosing,
            Palette::as_str(*palette),
        )),
        Command::Theme(Theme::Syntax(name)) if !(desk.reads)(name.as_str()) => {
            Performed::Refused(ErrorCode::InvalidArgument.into())
        }
        Command::Theme(Theme::Syntax(name)) => {
            Performed::Theme(remember::syntax(desk.switching.choosing, name.as_str()))
        }
        // No action is pending here either, whatever a turn elsewhere waits on.
        Command::Decide(_) => Performed::Refused(ErrorCode::StaleDecision.into()),
        Command::Prompt(_)
        | Command::Compact
        | Command::Cancel
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
        | Command::Help
        | Command::ReleaseNotes { .. }
        | Command::Context
        | Command::Usage
        | Command::Exit => Performed::Refused(ErrorCode::Busy.into()),
    }
}

/// Writes `word` down as what the row `key` is set to, where nothing nearer
/// decides it.
fn setting(desk: &Desk<'_>, key: &str, word: &str) -> Performed {
    let Some(row) = crucible_config::row(key).filter(|row| row.takes(word)) else {
        return Performed::Refused(ErrorCode::InvalidArgument.into());
    };
    if row.values() == crucible_config::Values::Named && !(desk.reads)(word) {
        return Performed::Refused(ErrorCode::InvalidArgument.into());
    }
    let environment = |name: &str| (desk.environment)(name);
    Performed::Setting(match desk.switching.settings.forced(row, &environment) {
        Some(forced) => Setting::Forced(forced),
        None => match remember::setting(desk.switching.choosing, row, word) {
            Ok(()) => Setting::Remembered,
            Err(problem) => Setting::Unwritten(problem),
        },
    })
}

/// The provider `name` is in the registry the host lent.
fn served(desk: &Desk<'_>, name: &str) -> Result<Served, Refusal> {
    offered(desk.switching.providers)
        .find(|served| served.name == name)
        .ok_or_else(|| ErrorCode::UnknownProvider.into())
}

async fn clear(conversation: &mut Conversation, desk: &Desk<'_>) -> Cleared {
    // A session that has said nothing is already the empty one this would go
    // and open, and a second log would leave two files for a session that
    // never happened.
    if conversation.runner().transcript().is_empty() {
        return Cleared::Nothing;
    }

    // Read now rather than carried from startup, because a conversation can
    // outlive a checkout: the session starting records where the person is.
    let branch = crate::branching::current(desk.workspace.root());
    match conversation
        .clear(desk.sessions, desk.workspace, branch.as_deref())
        .await
    {
        Ok(left) => Cleared::Started {
            unclosed: closed(&left),
        },
        Err(problem) => Cleared::Failed(problem),
    }
}

async fn resume(conversation: &mut Conversation, desk: &Desk<'_>, id: &SessionId) -> Resumed {
    // Answered before the log is opened. This session's own claim is on that
    // file, so reopening it would come back as held by another crucible.
    if conversation.session().id() == Some(id) {
        return Resumed::Same;
    }

    match conversation.resume(desk.sessions, desk.workspace, id).await {
        Ok(left) => Resumed::Picked {
            unclosed: closed(&left),
        },
        Err(SessionError::Unknown { .. }) => Resumed::Unknown,
        Err(problem) => Resumed::Failed(problem),
    }
}

/// Closes the log of the session just left: the last chance to learn that it
/// stopped being written, since after this nothing holds it.
fn closed(left: &Arc<Session>) -> Option<Box<str>> {
    left.finish()
}

impl Performed {
    /// The answer to `request`, as a client is sent it.
    #[must_use]
    pub fn response(&self, request: &Request) -> Response {
        Response {
            correlation: Some(request.correlation()),
            outcome: self.outcome(),
        }
    }

    /// What happened, as a client is told it.
    #[must_use]
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Refused(refusal) => Outcome::Refused(*refusal),
            Self::Cleared(cleared) => Outcome::Cleared(match cleared {
                Cleared::Nothing => ClearOutcome::Nothing,
                Cleared::Started { unclosed } => ClearOutcome::Started {
                    unclosed: unclosed.as_ref().map(|said| Problem::failed(said)),
                },
                Cleared::Failed(problem) => ClearOutcome::Failed(Problem::failed(problem)),
            }),
            Self::Resumed(resumed) => Outcome::Resumed(match resumed {
                Resumed::Same => ResumeOutcome::Same,
                Resumed::Picked { unclosed } => ResumeOutcome::Picked {
                    unclosed: unclosed.as_ref().map(|said| Problem::failed(said)),
                },
                Resumed::Unknown => ResumeOutcome::Unknown,
                Resumed::Failed(problem) => ResumeOutcome::Failed(Problem::failed(problem)),
            }),
            Self::Model(switched) => Outcome::Model(model(switched)),
            Self::Effort(rung) => Outcome::Effort(match rung {
                Rung::Unasked => EffortOutcome::Unasked,
                Rung::Unsupported => EffortOutcome::Unsupported,
                Rung::Taken { unwritten } => EffortOutcome::Taken {
                    unwritten: unwritten.as_ref().map(|problem| Problem::failed(problem)),
                },
            }),
            Self::Speed(hastened) => Outcome::Speed(match hastened {
                Hastened::Unasked => SpeedOutcome::Unasked,
                Hastened::Unsupported => SpeedOutcome::Unsupported,
                Hastened::Own => SpeedOutcome::Own,
                Hastened::Taken { unwritten } => SpeedOutcome::Taken {
                    unwritten: unwritten.as_ref().map(|problem| Problem::failed(problem)),
                },
            }),
            Self::Mode(mode) => Outcome::Mode(reading::mode_out(*mode)),
            Self::Login(logged_in) => Outcome::Login(login(logged_in)),
            Self::Logout(logged_out) => Outcome::Logout(logout(logged_out)),
            Self::Cache(listed) => Outcome::Cache(cache(listed)),
            Self::Cleaned(cleaned) => Outcome::Cleaned(match cleaned {
                Ok(counted) => CleanOutcome::Counted {
                    inspected: reading::count(counted.inspected),
                    deleted: reading::count(counted.deleted),
                    ambiguous: reading::count(counted.ambiguous),
                    orphaned: reading::count(counted.orphaned),
                },
                Err(problem) => CleanOutcome::Failed(Problem::failed(problem)),
            }),
            Self::Sandbox { enabled, unchanged } => Outcome::Sandbox(match unchanged {
                None => SandboxOutcome::Set { enabled: *enabled },
                Some(problem) => SandboxOutcome::Unchanged(Problem::failed(problem)),
            }),
            Self::Theme(remembered) => Outcome::Theme(match remembered {
                Ok(()) => ThemeOutcome::Remembered,
                Err(problem) => ThemeOutcome::Unwritten(Problem::failed(problem)),
            }),
            Self::Setting(setting) => Outcome::Setting(match setting {
                Setting::Remembered => SettingOutcome::Remembered,
                Setting::Forced(crucible_config::Forced::Environment) => {
                    SettingOutcome::Forced(Forced::Environment)
                }
                Setting::Forced(crucible_config::Forced::Project) => {
                    SettingOutcome::Forced(Forced::Project)
                }
                Setting::Unwritten(problem) => SettingOutcome::Unwritten(Problem::failed(problem)),
            }),
            Self::Help => Outcome::help(),
            Self::Notes(notes) => Outcome::Notes(notes.clone()),
            Self::Context { model, breakdown } => {
                Outcome::Context(reading::context(model, breakdown))
            }
            Self::Usage(usage) => Outcome::Usage(usage.as_ref().clone()),
            Self::Leaving => Outcome::Leaving,
        }
    }
}

fn model(switched: &Switched) -> ModelOutcome {
    match switched {
        Switched::Unsupported(_) => ModelOutcome::Unsupported,
        Switched::Unreachable(problem) => ModelOutcome::Unreachable(Problem::failed(problem)),
        Switched::CacheHeld(problem) => ModelOutcome::CacheHeld(Problem::failed(problem)),
        Switched::Taken {
            retained,
            unwritten,
        } => ModelOutcome::Taken {
            retained: reading::retained(*retained),
            unwritten: unwritten.as_ref().map(|problem| Problem::failed(problem)),
        },
    }
}

fn login(logged_in: &LoggedIn) -> LoginOutcome {
    match logged_in {
        LoggedIn::Unusable(problem) => LoginOutcome::Unusable(Problem::failed(problem)),
        LoggedIn::Elsewhere => LoginOutcome::Elsewhere,
        LoggedIn::CacheHeld(problem) => LoginOutcome::CacheHeld(Problem::failed(problem)),
        LoggedIn::Serving {
            retained,
            unwritten,
        } => LoginOutcome::Serving {
            retained: reading::retained(*retained),
            unwritten: unwritten.as_ref().map(|problem| Problem::failed(problem)),
        },
    }
}

fn logout(logged_out: &LoggedOut) -> LogoutOutcome {
    match logged_out {
        LoggedOut::CacheHeld(problem) => LogoutOutcome::CacheHeld(Problem::failed(problem)),
        LoggedOut::Unforgotten { retained, problem } => LogoutOutcome::Unforgotten {
            retained: reading::retained(*retained),
            problem: Problem::failed(problem),
        },
        LoggedOut::Kept => LogoutOutcome::Kept,
        LoggedOut::StillServed { retained, source } => LogoutOutcome::StillServed {
            retained: reading::retained(*retained),
            standing: match source {
                // The variable's name and never what it holds.
                CredentialSource::Environment(variable) => {
                    Standing::Environment(crucible_client_api::Text::cut(variable))
                }
                CredentialSource::StoredKey => Standing::StoredKey,
                CredentialSource::Subscription => Standing::Subscription,
            },
        },
        LoggedOut::SignedOut { retained } => LogoutOutcome::SignedOut {
            retained: reading::retained(*retained),
        },
    }
}

fn cache(
    listed: &Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError>,
) -> CacheOutcome {
    use crucible_client_api::Text;
    use crucible_client_api::bounds::ITEMS;

    match listed {
        Ok(records) => CacheOutcome::Listed {
            resources: records
                .iter()
                .take(ITEMS)
                .map(|record| {
                    let owner = record.binding().owner();
                    Resource {
                        state: Text::cut(record.state().as_str()),
                        expires_at: record.expires_at(),
                        isolation: Text::cut(owner.isolation().as_str()),
                        exclusive: owner.exclusive(),
                        protocol: Text::cut(record.binding().protocol()),
                    }
                })
                .collect(),
            truncated: records.len() > ITEMS,
        },
        Err(problem) => CacheOutcome::Failed(Problem::failed(problem)),
    }
}
