//! Changing who a conversation asks, and what it asks them for.
//!
//! Four things change that: a model taken, a rung taken, a credential given
//! and a credential forgotten. Each is a sequence with an order that matters —
//! a rung is checked before anything is retired, a provider is reached before
//! the one answering is let go, the cache made under one identity is retired
//! before another takes its place, and the name a session is asking under
//! moves in the same step as the provider behind it, with the web sources
//! built again for both. That order is the same
//! whatever is showing it, so it is decided here, from values no terminal has
//! to supply, and answered with a value that says what happened and leaves
//! every sentence about it to whoever is showing it.
//!
//! A credential given on an installed agent's route for the provider the route
//! reaches is written down as the way that provider is reached, with its
//! default model and rung where nothing names a model for it, and read so for
//! the rest of the run through [`Rewritten`]; once written, the conversation
//! moves off the route onto that provider, in a new session, as it does for a
//! credential given there for another provider, where that session can be
//! started. Otherwise the route keeps answering and the credential is only
//! stored. One forgotten there leaves the route, its session and its run.
//! A model taken there of a provider the agent does not reach moves it the
//! same way, onto that model, where a credential reaches the provider.
//! A model taken of a provider this run's settings route through an installed
//! agent the conversation is not on moves it onto that agent instead, in a
//! new session, the agent to be started on that model; it is refused before
//! anything moves where the agent chooses its own model, is disconnected, or
//! the model does not serve the rung asked.
//!
//! What an answer deliberately carries beside the outcome: what a cache
//! retirement could not remove, and what could not be written down. Neither
//! undoes the switch. Both are something a reader is told, and a front end
//! that was only handed "done" would have nothing to tell them with.

use std::path::Path;
use std::sync::Arc;

use crucible_auth::{AuthError, Store, StoredCredentials};
use crucible_config::Settings;
use crucible_models::Effort;
use crucible_provider::Unavailable;
use crucible_runtime::Cancel;
use crucible_session::Session;
use crucible_types::{PromptCacheResourceError, Transcript};
use crucible_workspace::Workspace;

use crate::access::Rewritten;
use crate::providers::{
    CredentialSource, NO_PROVIDER_CHOSEN, NOTHING_TO_ASK, Providers, Served, Serving, Sourcing,
    default_rung, offered, rungs,
};
use crate::remember::{self, RememberError};
use crate::startup::{UNKNOWN_CEILING, accepts, ceiling, window};
use crate::{AppError, Conversation};

/// What a switch is decided from: one generation of the registry, the files
/// this run read, how a provider is set up from the credentials in hand, the
/// store those credentials are written in, and the file a choice is written
/// down to.
#[derive(Clone, Copy)]
pub struct Switching<'a> {
    /// The providers a name is read against.
    pub providers: &'a Providers,
    /// What the configuration files said.
    pub settings: &'a Settings,
    /// Sets one provider up from the credentials in hand.
    pub serving: &'a Serving,
    /// Builds the web sources for one provider and model from the same.
    pub sourcing: &'a Sourcing,
    /// Where `/login` writes a credential and `/logout` forgets one.
    pub logins: &'a Store,
    /// The user configuration file a choice is written down in.
    pub choosing: &'a Path,
    /// How a login during this run rewrote a provider's access, which
    /// [`Switching::serving`] and [`Switching::sourcing`] read too.
    pub rewritten: &'a Rewritten,
}

impl std::fmt::Debug for Switching<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Switching")
            .field("choosing", &self.choosing)
            .finish_non_exhaustive()
    }
}

/// What a cache retirement left behind because it could not tell whose it was
/// or could not reach it. Nothing, almost always.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Retained {
    /// Resources whose deletion could not be confirmed either way.
    pub ambiguous: usize,
    /// Resources remembered here that the provider no longer answers for.
    pub orphaned: usize,
}

impl Retained {
    /// Whether there is anything to tell a reader about.
    #[must_use]
    pub const fn any(self) -> bool {
        self.ambiguous > 0 || self.orphaned > 0
    }
}

/// What came of asking for a model.
#[derive(Debug)]
pub enum Switched {
    /// The model does not serve the rung it would have been asked at. Nothing
    /// was retired, reached or changed.
    Unsupported(Effort),
    /// The provider could not be set up from the credentials in hand. The one
    /// answering still is.
    Unreachable(AppError),
    /// The cache made under the identity being left could not be retired, so
    /// the identity was not left.
    CacheHeld(PromptCacheResourceError),
    /// The next turn asks for it.
    Taken {
        /// What retiring the old identity's cache left behind.
        retained: Retained,
        /// Why the choice will not outlive this run, where it will not.
        unwritten: Option<RememberError>,
    },
    /// The conversation was on an installed agent's route that does not reach
    /// the provider, and moved off it onto the provider and the model, in a
    /// new session; the session it left stays on the disk where a resume can
    /// find it.
    MovedOff {
        /// The provider the model is asked of.
        provider: &'static str,
        /// What retiring the old identity's cache left behind.
        retained: Retained,
        /// Why the choice will not outlive this run, where it will not.
        unwritten: Option<RememberError>,
    },
    /// The provider is reached through an installed agent the conversation
    /// was not on, and it moved onto that agent, to be started on the model,
    /// in a new session; the session it left stays on the disk where a resume
    /// can find it.
    MovedOnto {
        /// The configured instance of the agent moved onto.
        instance: Box<str>,
        /// Why the choice will not outlive this run, where it will not.
        unwritten: Option<RememberError>,
    },
}

/// Why a conversation stayed on the installed agent's route it was to move
/// off.
#[derive(Debug)]
enum Unmoved {
    /// The provider could not be set up from the credentials in hand.
    Unusable(AppError),
    /// The cache made under the identity being left could not be retired.
    CacheHeld(PromptCacheResourceError),
    /// The new session's log could not be started.
    Unstarted(AppError),
}

/// What a move off the route was to ask the provider it moves onto.
#[derive(Clone, Copy)]
enum Onto<'a> {
    /// What [`first_asked`] names, as a login asks it.
    First,
    /// This model, at the rung carried where it serves one.
    Model(&'a str, Option<Effort>),
}

/// What came of asking for a rung.
#[derive(Debug)]
pub enum Rung {
    /// Nobody is being asked, or no model is: a rung is one word in a request
    /// to a model, and there is none to put it in.
    Unasked,
    /// The model in force does not serve it. The rung in force still is.
    Unsupported,
    /// The next turn thinks this hard.
    Taken {
        /// Why the rung will not outlive this run, where it will not.
        unwritten: Option<RememberError>,
    },
}

/// What came of a credential having just been written down.
#[derive(Debug)]
pub enum LoggedIn {
    /// Written, and nothing could be set up from it: what the next run here
    /// would meet, said now.
    Unusable(AppError),
    /// The conversation keeps the installed agent's route it is on for the
    /// provider, which is reached directly with the credential from the next
    /// run on: the move off the route could not be made, or the access could
    /// not be written.
    NextLaunch {
        /// The provider.
        provider: &'static str,
        /// Why the next run will still reach it through the agent, where it
        /// will.
        unwritten: Option<RememberError>,
    },
    /// The conversation keeps the installed agent's route it is on, and the
    /// credential is stored for another provider: nothing else changed. A
    /// provider this run reaches through another agent, or one whose new
    /// session could not be started, comes to this.
    Stored {
        /// The provider the credential is for.
        provider: &'static str,
    },
    /// Another provider is answering and keeps the session it has. A
    /// credential says a provider can be reached, never which to ask.
    Elsewhere,
    /// The cache made under the identity being left could not be retired.
    CacheHeld(PromptCacheResourceError),
    /// The provider the credential is for is the one asked from here on.
    Serving {
        /// What retiring the old identity's cache left behind.
        retained: Retained,
        /// Why the provider will not be the next run's, where it will not.
        unwritten: Option<RememberError>,
    },
    /// The conversation was on an installed agent's route that does not reach
    /// the provider, and moved off it onto the provider, in a new session; the
    /// session it left stays on the disk where a resume can find it.
    MovedOff {
        /// The provider the credential is for.
        provider: &'static str,
        /// What retiring the old identity's cache left behind.
        retained: Retained,
        /// Why the provider will not be the next run's, where it will not.
        unwritten: Option<RememberError>,
    },
}

/// What came of forgetting a stored credential.
#[derive(Debug)]
pub enum LoggedOut {
    /// The provider is the one answering, and the cache made with its
    /// credential could not be retired. The credential is still stored.
    CacheHeld(PromptCacheResourceError),
    /// The store could not be changed.
    Unforgotten {
        /// What the retirement that came first left behind.
        retained: Retained,
        /// Why the credential is still there.
        problem: AuthError,
    },
    /// Forgotten, and another provider is the one answering: nothing about the
    /// session moved.
    Kept,
    /// Forgotten, and the provider is still reached through the installed
    /// agent the conversation is on: nothing about the session moved.
    Routed {
        /// The configured instance.
        instance: Box<str>,
    },
    /// Forgotten, and the provider still answers from a credential the store
    /// never held or still holds.
    StillServed {
        /// What the retirement left behind.
        retained: Retained,
        /// Where the credential now in use comes from.
        source: CredentialSource,
    },
    /// Forgotten, and with it the last way of reaching the provider: nobody is
    /// asked until `/login` or `/model` says who.
    SignedOut {
        /// What the retirement left behind.
        retained: Retained,
    },
}

/// What a credential just stored for `named` comes to while the conversation
/// is on an installed agent's route, before any move off it.
///
/// `routed` says whether `named` is a provider the route stands in for: one
/// the files read at launch reach through its instance, or the one
/// [`Conversation::reached`] names. The kind of credential stored decides how
/// that provider is reached next: an account by its subscription, a key as a
/// key.
fn beside_the_route(routed: bool, named: Served, with: &Switching<'_>) -> LoggedIn {
    if !routed {
        return LoggedIn::Stored {
            provider: named.name,
        };
    }
    let access = match with.logins.read().held(named.name) {
        Some(held) if held.kind == crucible_auth::Kind::Account => {
            crucible_config::Access::Subscription
        }
        Some(_) => crucible_config::Access::ApiKey,
        None => {
            return LoggedIn::Unusable(AppError::Authentication {
                provider: named.name.into(),
            });
        }
    };
    // In the one update that rewrites the access, so that the next launch
    // finds both or neither, and a refusal leaves the route written as it was.
    let (model, effort, taken) = first_asked(named, with);
    let default = taken.then_some((model.as_str(), effort));
    let unwritten = remember::reaching_defaulted(with.choosing, named.name, &access, default).err();
    // Written down, it is how this run reaches the provider too, so that the
    // move off the route sets it up as the next launch would.
    if unwritten.is_none() {
        with.rewritten.rewrite(named.name, access);
    }
    LoggedIn::NextLaunch {
        provider: named.name,
        unwritten,
    }
}

/// What a login asks `named` for where nobody was being asked: the model the
/// user file names for it as it stands now, else the one the files read at
/// launch name, each at the rung beside it; else its default, at the rung
/// configured where the default serves it, else at [`default_rung`]'s. Only
/// the last is a default taken, answered `true` to be written down.
///
/// The user file is read again because `/model` writes to it during the run,
/// and the files read at launch are not read again.
fn first_asked(named: Served, with: &Switching<'_>) -> (String, Option<Effort>, bool) {
    let provider = named.name;
    let (filed, filed_rung) = remember::chosen(with.choosing, provider);
    if let Some(model) = filed {
        return (model, filed_rung, false);
    }
    if let Some(model) = with.settings.model(provider).map(str::trim)
        && !model.is_empty()
    {
        return (model.to_owned(), with.settings.effort(provider), false);
    }
    let served = rungs(with.providers, provider, named.default);
    let configured = filed_rung
        .or_else(|| with.settings.effort(provider))
        .filter(|rung| served.contains(rung));
    (
        named.default.to_owned(),
        configured.or_else(|| default_rung(&served)),
        true,
    )
}

/// Whether this run's settings reach `named` through any installed agent.
fn agented(settings: &Settings, named: Served) -> bool {
    through(settings, named).is_some()
}

/// The installed agent this run's settings reach `named` through, where they
/// name one.
fn through(settings: &Settings, named: Served) -> Option<Box<str>> {
    match settings.access(named.name) {
        Some(crucible_config::Access::ExternalAgent { instance }) => Some(instance),
        _ => None,
    }
}

/// Whether `named` is reached through the installed agent `instance`.
fn routes(settings: &Settings, named: Served, instance: &str) -> bool {
    matches!(
        settings.access(named.name),
        Some(crucible_config::Access::ExternalAgent { instance: through })
            if *through == *instance
    )
}

impl Conversation {
    /// Asks for `name` from `selected` from the next turn on, and writes both
    /// down for the next run.
    ///
    /// `rung` is the rung the caller means to take with the model, where it
    /// has one in hand; without one, the rung already in force is what the
    /// model has to serve. Only Gemini's ladder is held to this: a rung the
    /// table does not list is elsewhere left for the vendor to refuse, but its
    /// encoder would carry an unsupported one into every later request.
    pub async fn ask_for(
        &mut self,
        selected: Served,
        name: &str,
        rung: Option<Effort>,
        with: &Switching<'_>,
    ) -> Switched {
        let Some(runner) = self.native() else {
            return self.ask_agent_for(selected.name, name, rung, with);
        };
        let provider = selected.name;
        // The model in force chosen again keeps its speed; any other goes back
        // to standard, since the price shown for fast was that model's.
        let another = self.serving != Some(provider) || runner.model() != name;
        // Before retiring a cache or replacing the provider, so that a refusal
        // leaves the session exactly where it was.
        if let Some(effort) = rung.or(runner.effort())
            && !serves(with.providers, provider, name, effort)
        {
            return Switched::Unsupported(effort);
        }

        let retained = if self.serving == Some(provider) {
            if self.runner.model() == name {
                Retained::default()
            } else {
                match self.retire().await {
                    Ok(retained) => retained,
                    Err(problem) => return Switched::CacheHeld(problem),
                }
            }
        } else {
            // Reached before the one answering is let go: a provider that
            // cannot be set up is a session left as it was.
            let set = match (with.serving)(selected, &with.logins.read()) {
                Ok(set) => set,
                Err(problem) => return Switched::Unreachable(problem),
            };
            let retained = match self.retire().await {
                Ok(retained) => retained,
                Err(problem) => return Switched::CacheHeld(problem),
            };
            self.runner.serve(set.provider);
            self.clearings_recorded().await;
            self.serving = Some(provider);
            self.source = Some(set.source);
            retained
        };

        // One generation, read once: the ceiling, the window and what the
        // model reads are three answers about the same model, and three
        // separate reads could take them from three generations.
        self.runner.ask(
            name,
            ceiling(with.providers, provider, name),
            Some(window(with.providers, selected, name, with.settings)),
            accepts(with.providers, provider, name),
        );
        // And the web sources, for the model and provider now asked: a search
        // names the model a turn would, signed as that provider's turns are.
        self.reach(selected, with);

        // The provider first, because it is the half a machine holding two
        // credentials needs: a model written under a provider says what to ask
        // that provider for and never which provider to ask.
        let unwritten = remember::asking(with.choosing, provider)
            .and_then(|()| remember::choosing(with.choosing, provider, name))
            .err();
        let slowed = if another {
            self.slowed(provider, with.choosing)
        } else {
            None
        };

        Switched::Taken {
            retained,
            unwritten: unwritten.or(slowed),
        }
    }

    /// [`Conversation::ask_for`], and where the conversation is on an
    /// installed agent's route that does not reach `selected`, it moves off
    /// the route onto `selected` and `name` instead, in a new session started
    /// under `sessions`. The session left stays on the disk where a resume can
    /// find it.
    ///
    /// The rung carried onto `name` is `rung`, else the one the agent was
    /// started at, where `name` serves it. Where this run's settings reach
    /// `selected` through an installed agent, and the conversation is on no
    /// agent that reaches `selected`, it moves onto that agent instead, in a
    /// new session, to be started on `name`: `selected` has no model to ask
    /// but through it.
    pub async fn ask_for_moving(
        &mut self,
        selected: Served,
        (name, rung): (&str, Option<Effort>),
        with: &Switching<'_>,
        (sessions, workspace): (&Path, &Workspace),
    ) -> Switched {
        let reached = self
            .agent_driver()
            .is_some_and(|driver| driver.provider() == selected.name);
        if !reached && let Some(instance) = through(with.settings, selected) {
            return self
                .onto_the_agent(
                    (selected, instance),
                    (name, rung),
                    with,
                    (sessions, workspace),
                )
                .await;
        }
        if self.instance().is_none() || reached {
            return self.ask_for(selected, name, rung, with).await;
        }
        // Before anything is retired or started, so that a refusal leaves the
        // route as it was.
        if let Some(rung) = rung
            && !serves(with.providers, selected.name, name, rung)
        {
            return Switched::Unsupported(rung);
        }
        let carried = rung.or_else(|| self.settled().effort());
        match self
            .off_the_route(
                selected,
                Onto::Model(name, carried),
                with,
                (sessions, workspace),
            )
            .await
        {
            Ok((retained, unwritten)) => Switched::MovedOff {
                provider: selected.name,
                retained,
                unwritten,
            },
            Err(Unmoved::Unusable(problem) | Unmoved::Unstarted(problem)) => {
                Switched::Unreachable(problem)
            }
            Err(Unmoved::CacheHeld(problem)) => Switched::CacheHeld(problem),
        }
    }

    /// Moves the conversation onto the installed agent `instance`, which this
    /// run's settings reach `selected` through, in a new session started
    /// under `sessions`, to be started on `name` from its first turn; and
    /// writes the model and its rung into the agent's entry. The session left
    /// stays on the disk where a resume can find it.
    ///
    /// The rung is `rung`, else the one in force where `name` serves it, else
    /// `name`'s default, as [`Self::ask_agent_for`] takes it. Refused before
    /// anything moves where the access is no route to `selected`, the agent
    /// chooses its own model or is disconnected, or `name` does not serve
    /// `rung`. Nothing is sent to the agent here: its first run waits for the
    /// first turn, and for the yes its launch is held to.
    async fn onto_the_agent(
        &mut self,
        (selected, instance): (Served, Box<str>),
        (name, rung): (&str, Option<Effort>),
        with: &Switching<'_>,
        (sessions, workspace): (&Path, &Workspace),
    ) -> Switched {
        let provider = selected.name;
        let entry = match crate::access::agent(provider, instance, with.settings) {
            Ok(entry) => entry,
            Err(refused) => return Switched::Unreachable(AppError::Access(refused)),
        };
        let unstartable = |reason: String| {
            Switched::Unreachable(AppError::Unstartable {
                instance: entry.name().into(),
                reason: reason.into(),
            })
        };
        if !entry.driver().takes_model() {
            return Switched::Unreachable(AppError::ExternalAgent {
                provider: provider.into(),
                instance: entry.name().into(),
            });
        }
        if !entry.enabled() {
            return unstartable(crate::launching::disconnected(entry.name(), provider));
        }
        // Held to the known ladder, for the reason `ask_agent_for` holds a
        // rung to it.
        let served = rungs(with.providers, provider, name);
        if let Some(rung) = rung
            && !served.contains(&rung)
        {
            return Switched::Unsupported(rung);
        }
        let effort = rung
            .or_else(|| self.settled().effort().filter(|one| served.contains(one)))
            .or_else(|| default_rung(&served));

        let branch = crate::branching::current(workspace.root());
        let left = match self
            .onto(sessions, workspace, branch.as_deref(), &entry)
            .await
        {
            Ok(left) => left,
            Err(reason) => return unstartable(reason),
        };
        // Nothing holds the session left after this; its log stays on the
        // disk where a resume can find it, and its own trouble is found again
        // when it is resumed.
        let _unclosed = left.finish();
        self.choose_agent_model(name, effort);

        Switched::MovedOnto {
            instance: entry.name().into(),
            unwritten: remember::choosing_agent(with.choosing, entry.name(), name, effort).err(),
        }
    }

    /// Thinks at `effort` from the next turn on, and writes it down beside the
    /// model for the next run.
    ///
    /// On an installed agent's route it is the agent's next run that starts at
    /// it, and its entry it is written into; an agent that chooses its own
    /// model is asked at no rung.
    pub fn think(&mut self, effort: Effort, with: &Switching<'_>) -> Rung {
        if self.instance().is_some() {
            return self.think_agent(effort, with);
        }
        let serving = self.serving;
        let Some(runner) = self.native_mut() else {
            return Rung::Unasked;
        };
        let Some(provider) = serving.filter(|_| !runner.model().is_empty()) else {
            return Rung::Unasked;
        };
        if !serves(with.providers, provider, runner.model(), effort) {
            return Rung::Unsupported;
        }

        runner.think(effort);

        Rung::Taken {
            unwritten: remember::thinking(with.choosing, provider, effort).err(),
        }
    }

    /// Starts the installed agent the conversation is on with `name` of
    /// `provider` from its next run, at `rung` where one is named, else at
    /// the rung in force where `name` serves it, else at `name`'s default; and
    /// writes both into the agent's entry.
    ///
    /// Only an agent that is started with a model, and only on a model of the
    /// one provider it reaches: any other is a provider this route cannot
    /// reach, which [`Conversation::ask_for_moving`] moves off the route for.
    fn ask_agent_for(
        &mut self,
        provider: &str,
        name: &str,
        rung: Option<Effort>,
        with: &Switching<'_>,
    ) -> Switched {
        let reaches = self
            .agent_driver()
            .is_some_and(|driver| driver.takes_model() && driver.provider() == provider);
        let Some(instance) = self.instance().filter(|_| reaches).map(Box::<str>::from) else {
            return Switched::Unreachable(AppError::Installed {
                driver: self.settled().serving(),
            });
        };
        // Held to the known ladder whatever the vendor, as `serves` is not:
        // the agent is started with the rung, and one its model refuses
        // would refuse every run started from the entry it is written into.
        let served = rungs(with.providers, provider, name);
        if let Some(rung) = rung
            && !served.contains(&rung)
        {
            return Switched::Unsupported(rung);
        }
        let effort = rung
            .or_else(|| self.settled().effort().filter(|one| served.contains(one)))
            .or_else(|| default_rung(&served));

        self.choose_agent_model(name, effort);
        Switched::Taken {
            retained: Retained::default(),
            unwritten: remember::choosing_agent(with.choosing, &instance, name, effort).err(),
        }
    }

    /// Starts the installed agent the conversation is on at `effort` from its
    /// next run, and writes it into the agent's entry: where the agent is
    /// started on a model, and that model's known ladder holds it, for the
    /// reason [`Self::ask_agent_for`] holds a rung to it.
    fn think_agent(&mut self, effort: Effort, with: &Switching<'_>) -> Rung {
        let Some(driver) = self.agent_driver().filter(|driver| driver.takes_model()) else {
            return Rung::Unasked;
        };
        let settled = self.settled();
        let model = settled.model();
        if model.is_empty() {
            return Rung::Unasked;
        }
        if !rungs(with.providers, driver.provider(), model).contains(&effort) {
            return Rung::Unsupported;
        }
        let Some(instance) = self.instance().map(Box::<str>::from) else {
            return Rung::Unasked;
        };

        self.agent_effort(effort);
        Rung::Taken {
            unwritten: remember::thinking_agent(with.choosing, &instance, effort).err(),
        }
    }

    /// Asks `named`, which nobody was being asked, for what [`first_asked`]
    /// names, and writes a default taken down for the next run: what could not
    /// be written, if anything.
    ///
    /// The rung left over was asked of the model asked before, so where none
    /// is named it is cleared rather than carried onto a model that may serve
    /// none.
    fn ask_first(&mut self, named: Served, with: &Switching<'_>) -> Option<RememberError> {
        let (model, effort, taken) = first_asked(named, with);
        self.ask_at(named, &model, effort, with);
        taken
            .then(|| remember::defaulting(with.choosing, named.name, &model, effort).err())
            .flatten()
    }

    /// Asks `named` for `model` from the next turn on, at `effort` where
    /// `model` serves it and at no rung otherwise.
    fn ask_at(&mut self, named: Served, model: &str, effort: Option<Effort>, with: &Switching<'_>) {
        let provider = named.name;
        self.runner.ask(
            model,
            ceiling(with.providers, provider, model),
            Some(window(with.providers, named, model, with.settings)),
            accepts(with.providers, provider, model),
        );
        match effort.filter(|effort| serves(with.providers, provider, model, *effort)) {
            Some(effort) => self.runner.think(effort),
            None => self.runner.unthink(),
        }
    }

    /// Hands this conversation the provider whose credential was just written
    /// down — unless another is already answering, which keeps everything it
    /// has, or the conversation is on an installed agent's route, which keeps
    /// answering; [`Conversation::logged_in_moving`] moves off that route.
    ///
    /// A credential for the provider being asked keeps its model and rung.
    /// Where nobody was being asked, any model name left over from before is
    /// retired with the provider it belonged to, and the conversation asks for
    /// the model the configuration names, else for the provider's default.
    pub async fn logged_in(&mut self, named: Served, with: &Switching<'_>) -> LoggedIn {
        if let Some(instance) = self.instance() {
            let routed = routes(with.settings, named, instance)
                || self.reached(with.settings).as_deref() == Some(named.name);
            return beside_the_route(routed, named, with);
        }
        let set = match (with.serving)(named, &with.logins.read()) {
            Ok(set) => set,
            Err(problem) => {
                self.reread(with);
                return LoggedIn::Unusable(problem);
            }
        };
        if self.serving.is_some_and(|serving| serving != named.name) {
            return LoggedIn::Elsewhere;
        }

        let changed = self.serving != Some(named.name);
        let retained = match self.retire().await {
            Ok(retained) => retained,
            Err(problem) => {
                self.reread(with);
                return LoggedIn::CacheHeld(problem);
            }
        };
        self.runner.serve(set.provider);
        self.clearings_recorded().await;
        self.serving = Some(named.name);
        self.source = Some(set.source);

        let unwritten = remember::asking(with.choosing, named.name).err();
        let defaulted = if changed {
            self.ask_first(named, with)
        } else {
            None
        };
        self.reach(named, with);
        self.hastened_as_kept(with.choosing);

        LoggedIn::Serving {
            retained,
            unwritten: unwritten.or(defaulted),
        }
    }

    /// [`Conversation::logged_in`], and where that leaves the credential only
    /// stored, beside an installed agent's route that does not reach `named`,
    /// or written down as how `named` is reached in place of the route that
    /// does, the conversation moves off the route onto `named`, in a new
    /// session started under `sessions`. The session left stays on the disk
    /// where a resume can find it.
    ///
    /// A provider this run's settings route through another installed agent
    /// does not move: nothing rewrote how it is reached. Where the provider
    /// the route reaches cannot be moved onto after all, it is still reached
    /// directly from the next launch, as was written.
    pub async fn logged_in_moving(
        &mut self,
        named: Served,
        with: &Switching<'_>,
        sessions: &Path,
        workspace: &Workspace,
    ) -> LoggedIn {
        let agented = agented(with.settings, named);
        // What is said where the move cannot be made: what the login alone
        // came to.
        let (provider, unmoved) = match self.logged_in(named, with).await {
            LoggedIn::Stored { provider } if !agented => (provider, LoggedIn::Stored { provider }),
            LoggedIn::NextLaunch {
                provider,
                unwritten: None,
            } => (
                provider,
                LoggedIn::NextLaunch {
                    provider,
                    unwritten: None,
                },
            ),
            logged => return logged,
        };
        match self
            .off_the_route(named, Onto::First, with, (sessions, workspace))
            .await
        {
            Ok((retained, unwritten)) => LoggedIn::MovedOff {
                provider,
                retained,
                unwritten,
            },
            Err(Unmoved::Unusable(problem)) => LoggedIn::Unusable(problem),
            Err(Unmoved::CacheHeld(problem)) => LoggedIn::CacheHeld(problem),
            Err(Unmoved::Unstarted(_)) => unmoved,
        }
    }

    /// Moves the conversation off the installed agent's route it is on onto
    /// `named`, asking it what `onto` says: [`Onto::First`] as
    /// [`Conversation::logged_in`] asks a provider nobody was being asked, a
    /// model as [`Conversation::ask_for`] asks it. Answers what retiring the
    /// cache left behind and what could not be written down.
    ///
    /// The cache the kept runner made under the identity it last asked is
    /// retired first, as a switch retires it, and nothing moves where it
    /// cannot be: a refusal, or a new session whose log could not be started,
    /// leaves everything as it was.
    async fn off_the_route(
        &mut self,
        named: Served,
        onto: Onto<'_>,
        with: &Switching<'_>,
        (sessions, workspace): (&Path, &Workspace),
    ) -> Result<(Retained, Option<RememberError>), Unmoved> {
        let set = (with.serving)(named, &with.logins.read()).map_err(Unmoved::Unusable)?;
        let retained = if self.runner.prompt_cache_retirement_pending() {
            match self.runner.retire_prompt_cache(&Cancel::new()).await {
                Ok(result) => Retained {
                    ambiguous: result.ambiguous,
                    orphaned: result.orphaned,
                },
                Err(problem) => return Err(Unmoved::CacheHeld(problem)),
            }
        } else {
            Retained::default()
        };
        let branch = crate::branching::current(workspace.root());
        let session = Session::start(sessions, workspace, branch.as_deref())
            .map_err(|problem| Unmoved::Unstarted(problem.into()))?;
        let left = self.inside(Arc::new(session), Transcript::new()).await;
        // Nothing holds the session left after this; its log stays on the disk
        // where a resume can find it, and its own trouble is found again then.
        let _unclosed = left.finish();

        let changed = self.serving != Some(named.name);
        self.runner.serve(set.provider);
        self.clearings_recorded().await;
        self.serving = Some(named.name);
        self.source = Some(set.source);
        let unwritten = remember::asking(with.choosing, named.name).err();
        let defaulted = match onto {
            Onto::First if changed => self.ask_first(named, with),
            Onto::First => None,
            // The rung goes with the model, as `/model` writes the two, so
            // that the next run asks what this one now does.
            Onto::Model(model, effort) => {
                self.ask_at(named, model, effort, with);
                let effort =
                    effort.filter(|effort| serves(with.providers, named.name, model, *effort));
                remember::choosing(with.choosing, named.name, model)
                    .and_then(|()| {
                        effort.map_or(Ok(()), |effort| {
                            remember::thinking(with.choosing, named.name, effort)
                        })
                    })
                    .err()
            }
        };
        self.reach(named, with);
        self.hastened_as_kept(with.choosing);

        Ok((retained, unwritten.or(defaulted)))
    }

    /// Forgets the credential stored for `named`, and sets the conversation up
    /// again from whatever is left where `named` was the one answering.
    ///
    /// The cache is retired first and the store changed second: a resource
    /// made with a credential is deleted with it, and once the credential is
    /// gone there is nothing left to delete it with.
    pub async fn log_out(&mut self, named: Served, with: &Switching<'_>) -> LoggedOut {
        let routed = self
            .instance()
            .filter(|instance| routes(with.settings, named, instance))
            .map(Box::from);
        let answering = self.serving() == Some(named.name);
        let retained = if answering {
            match self.retire().await {
                Ok(retained) => retained,
                Err(problem) => return LoggedOut::CacheHeld(problem),
            }
        } else {
            Retained::default()
        };

        // Whether there was still one there to forget is not the question. A
        // second crucible having taken it in between leaves it gone, which is
        // what was asked for.
        if let Err(problem) = with.logins.forget(named.name) {
            self.reread(with);
            return LoggedOut::Unforgotten { retained, problem };
        }
        if !answering {
            self.reread(with);
            return routed.map_or(LoggedOut::Kept, |instance| LoggedOut::Routed { instance });
        }

        let stored = with.logins.read();
        if let Ok(remaining) = (with.serving)(named, &stored) {
            self.runner.serve(remaining.provider);
            self.clearings_recorded().await;
            self.source = Some(remaining.source.clone());
            self.reach(named, with);
            self.hastened_as_kept(with.choosing);
            return LoggedOut::StillServed {
                retained,
                source: remaining.source,
            };
        }

        let reachable = reachable(with, &stored);
        let warning = if reachable {
            NO_PROVIDER_CHOSEN
        } else {
            NOTHING_TO_ASK
        };
        self.runner.ask("", UNKNOWN_CEILING, None, None);
        self.runner.hasten(crucible_models::Speed::Standard);
        self.runner.serve(Box::new(Unavailable::new(warning)));
        self.clearings_recorded().await;
        self.serving = None;
        self.source = None;
        self.reachable = reachable;
        self.web.stop();
        LoggedOut::SignedOut { retained }
    }

    /// Reads again whether any provider could be set up from the store, where
    /// nobody is being asked.
    ///
    /// Every path out of [`Self::logged_in`] and [`Self::log_out`] that leaves
    /// nobody chosen ends here or in setting the flag from the same read, so
    /// what [`Self::missing`] answers is the store as it was read after the
    /// change. A terminal reads the store again each time it says what is
    /// missing; a change another process makes later reaches the answer here
    /// only at the next `/login` or `/logout`.
    fn reread(&mut self, with: &Switching<'_>) {
        if self.serving.is_none() {
            self.reachable = reachable(with, &with.logins.read());
        }
    }

    /// Builds the web sources again for `named` and the model now asked of it,
    /// from the credential it is set up with now; none where no model is.
    fn reach(&self, named: Served, with: &Switching<'_>) {
        let model = self.runner.model();
        if model.is_empty() {
            self.web.stop();
        } else {
            self.web.follow(
                named.name,
                (with.sourcing)(named, model, &with.logins.read()),
            );
        }
    }

    /// Retires the persistent cache resources only this conversation's
    /// identity owns, ahead of that identity changing.
    async fn retire(&mut self) -> Result<Retained, PromptCacheResourceError> {
        self.retire_prompt_cache(&Cancel::new())
            .await
            .map(|result| Retained {
                ambiguous: result.ambiguous,
                orphaned: result.orphaned,
            })
    }
}

/// Whether any provider offered has a credential in `stored` or beside it.
///
/// A provider refused for anything but a missing credential or the access its
/// configuration chose, such as an address a setting names that cannot be sent
/// to, still counts: it is one [`crate::providers::available`] lists, and the
/// terminal's sentence and a client's answer are read from that list. One its
/// access refuses is one that list leaves out.
fn reachable(with: &Switching<'_>, stored: &StoredCredentials) -> bool {
    offered(with.providers).any(|other| {
        !matches!(
            (with.serving)(other, stored),
            Err(AppError::Authentication { .. } | AppError::Access(_))
        )
    })
}

/// Whether `model` on `provider` may be asked at `effort`.
///
/// Only Gemini's known ladder refuses here. Its encoder supports three rungs
/// and checks every request, so a fourth would refuse every later turn; every
/// other vendor is sent what was asked and answers for itself.
#[must_use]
pub fn serves(providers: &Providers, provider: &str, model: &str, effort: Effort) -> bool {
    provider != "google" || rungs(providers, provider, model).contains(&effort)
}
