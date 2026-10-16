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
//! A conversation on an installed agent's route keeps that route, its session
//! and its run whatever credential for the provider the route reaches is given
//! or forgotten: the agent answers, and the credential is written down as the
//! way that provider is reached from the next run on. One given there for
//! another provider moves the conversation off the route onto it, in a new
//! session, where that session can be started; otherwise it is only stored.
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

use crate::providers::{
    CredentialSource, NO_PROVIDER_CHOSEN, NOTHING_TO_ASK, Providers, Served, Serving, Sourcing,
    offered, rungs,
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
    /// run on.
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
/// is on an installed agent's route, which keeps answering either way.
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
    LoggedIn::NextLaunch {
        provider: named.name,
        unwritten: remember::reaching(with.choosing, named.name, &access).err(),
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
            return Switched::Unreachable(AppError::Installed {
                driver: self.settled().serving(),
            });
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

    /// Thinks at `effort` from the next turn on, and writes it down beside the
    /// model for the next run.
    ///
    /// An installed agent is asked at no rung: it chooses how hard it thinks.
    pub fn think(&mut self, effort: Effort, with: &Switching<'_>) -> Rung {
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

    /// Hands this conversation the provider whose credential was just written
    /// down — unless another is already answering, which keeps everything it
    /// has, or the conversation is on an installed agent's route, which keeps
    /// answering; [`Conversation::logged_in_moving`] moves off that route.
    ///
    /// A credential never chooses a model or a rung. Where nobody was being
    /// asked, any model name left over from before is retired with the
    /// provider it belonged to, and the conversation is left asking for none.
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
        if changed {
            self.runner.ask("", UNKNOWN_CEILING, None, None);
        }
        self.reach(named, with);
        self.hastened_as_kept(with.choosing);

        LoggedIn::Serving {
            retained,
            unwritten,
        }
    }

    /// [`Conversation::logged_in`], and where that leaves the credential only
    /// stored, beside an installed agent's route that does not reach `named`,
    /// the conversation moves off the route onto `named`, in a new session
    /// started under `sessions`. The session left stays on the disk where a
    /// resume can find it.
    ///
    /// Only a provider this run's settings reach through no installed agent
    /// moves: one they route through an agent cannot be set up until the next
    /// launch reads the access its login rewrote, or one the user gives it.
    pub async fn logged_in_moving(
        &mut self,
        named: Served,
        with: &Switching<'_>,
        sessions: &Path,
        workspace: &Workspace,
    ) -> LoggedIn {
        let agented = matches!(
            with.settings.access(named.name),
            Some(crucible_config::Access::ExternalAgent { .. })
        );
        match self.logged_in(named, with).await {
            LoggedIn::Stored { provider } if !agented => self
                .off_the_route(named, with, sessions, workspace)
                .await
                .unwrap_or(LoggedIn::Stored { provider }),
            logged => logged,
        }
    }

    /// Moves the conversation off the installed agent's route it is on onto
    /// `named`, which asks no model until one is chosen: nothing where the new
    /// session's log could not be started, which leaves everything as it was.
    ///
    /// The cache the kept runner made under the identity it last asked is
    /// retired first, as a switch retires it, and nothing moves where it
    /// cannot be.
    async fn off_the_route(
        &mut self,
        named: Served,
        with: &Switching<'_>,
        sessions: &Path,
        workspace: &Workspace,
    ) -> Option<LoggedIn> {
        let set = match (with.serving)(named, &with.logins.read()) {
            Ok(set) => set,
            Err(problem) => return Some(LoggedIn::Unusable(problem)),
        };
        let retained = if self.runner.prompt_cache_retirement_pending() {
            match self.runner.retire_prompt_cache(&Cancel::new()).await {
                Ok(result) => Retained {
                    ambiguous: result.ambiguous,
                    orphaned: result.orphaned,
                },
                Err(problem) => return Some(LoggedIn::CacheHeld(problem)),
            }
        } else {
            Retained::default()
        };
        let branch = crate::branching::current(workspace.root());
        let session = Session::start(sessions, workspace, branch.as_deref()).ok()?;
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
        if changed {
            self.runner.ask("", UNKNOWN_CEILING, None, None);
        }
        self.reach(named, with);
        self.hastened_as_kept(with.choosing);

        Some(LoggedIn::MovedOff {
            provider: named.name,
            retained,
            unwritten,
        })
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
