//! Changing who a conversation asks, and what it asks them for.
//!
//! Four things change that: a model taken, a rung taken, a credential given
//! and a credential forgotten. Each is a sequence with an order that matters —
//! a rung is checked before anything is retired, a provider is reached before
//! the one answering is let go, the cache made under one identity is retired
//! before another takes its place, and the name a session is asking under
//! moves in the same step as the provider behind it. That order is the same
//! whatever is showing it, so it is decided here, from values no terminal has
//! to supply, and answered with a value that says what happened and leaves
//! every sentence about it to whoever is showing it.
//!
//! What an answer deliberately carries beside the outcome: what a cache
//! retirement could not remove, and what could not be written down. Neither
//! undoes the switch. Both are something a reader is told, and a front end
//! that was only handed "done" would have nothing to tell them with.

use std::path::Path;

use crucible_auth::{AuthError, Store};
use crucible_config::Settings;
use crucible_models::Effort;
use crucible_provider::Unavailable;
use crucible_runtime::Cancel;
use crucible_types::PromptCacheResourceError;

use crate::providers::{
    CredentialSource, NO_PROVIDER_CHOSEN, NOTHING_TO_ASK, Providers, Served, Serving, offered,
    rungs,
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

impl Conversation {
    /// Asks for `name` from `selected` from the next turn on, and writes both
    /// down for the next run.
    ///
    /// `rung` is the rung the caller means to take with the model, where it
    /// has one in hand; without one, the rung already in force is what the
    /// model has to serve. Only Gemini's ladder is held to this: a rung the
    /// table does not list is elsewhere left for the vendor to refuse, but its
    /// encoder would carry an unsupported one into every later request.
    pub fn ask_for(
        &mut self,
        selected: Served,
        name: &str,
        rung: Option<Effort>,
        with: &Switching<'_>,
    ) -> Switched {
        let provider = selected.name;
        // Before retiring a cache or replacing the provider, so that a refusal
        // leaves the session exactly where it was.
        if let Some(effort) = rung.or(self.runner.effort())
            && !serves(with.providers, provider, name, effort)
        {
            return Switched::Unsupported(effort);
        }

        let retained = if self.serving == Some(provider) {
            if self.runner.model() == name {
                Retained::default()
            } else {
                match self.retire() {
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
            let retained = match self.retire() {
                Ok(retained) => retained,
                Err(problem) => return Switched::CacheHeld(problem),
            };
            self.runner.serve(set.provider);
            self.clearings_recorded(None);
            self.serving = Some(provider);
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

        // The provider first, because it is the half a machine holding two
        // credentials needs: a model written under a provider says what to ask
        // that provider for and never which provider to ask.
        let unwritten = remember::asking(with.choosing, provider)
            .and_then(|()| remember::choosing(with.choosing, provider, name))
            .err();

        Switched::Taken {
            retained,
            unwritten,
        }
    }

    /// Thinks at `effort` from the next turn on, and writes it down beside the
    /// model for the next run.
    pub fn think(&mut self, effort: Effort, with: &Switching<'_>) -> Rung {
        let Some(provider) = self.serving.filter(|_| !self.runner.model().is_empty()) else {
            return Rung::Unasked;
        };
        if !serves(with.providers, provider, self.runner.model(), effort) {
            return Rung::Unsupported;
        }

        self.runner.think(effort);

        Rung::Taken {
            unwritten: remember::thinking(with.choosing, provider, effort).err(),
        }
    }

    /// Hands this conversation the provider whose credential was just written
    /// down — unless another is already answering, which keeps everything it
    /// has.
    ///
    /// A credential never chooses a model or a rung. Where nobody was being
    /// asked, any model name left over from before is retired with the
    /// provider it belonged to, and the conversation is left asking for none.
    pub fn logged_in(&mut self, named: Served, with: &Switching<'_>) -> LoggedIn {
        let set = match (with.serving)(named, &with.logins.read()) {
            Ok(set) => set,
            Err(problem) => return LoggedIn::Unusable(problem),
        };
        if self.serving.is_some_and(|serving| serving != named.name) {
            return LoggedIn::Elsewhere;
        }

        let changed = self.serving != Some(named.name);
        let retained = match self.retire() {
            Ok(retained) => retained,
            Err(problem) => return LoggedIn::CacheHeld(problem),
        };
        self.runner.serve(set.provider);
        self.clearings_recorded(None);
        self.serving = Some(named.name);

        let unwritten = remember::asking(with.choosing, named.name).err();
        if changed {
            self.runner.ask("", UNKNOWN_CEILING, None, None);
        }

        LoggedIn::Serving {
            retained,
            unwritten,
        }
    }

    /// Forgets the credential stored for `named`, and sets the conversation up
    /// again from whatever is left where `named` was the one answering.
    ///
    /// The cache is retired first and the store changed second: a resource
    /// made with a credential is deleted with it, and once the credential is
    /// gone there is nothing left to delete it with.
    pub fn log_out(&mut self, named: Served, with: &Switching<'_>) -> LoggedOut {
        let answering = self.serving == Some(named.name);
        let retained = if answering {
            match self.retire() {
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
            return LoggedOut::Unforgotten { retained, problem };
        }
        if !answering {
            return LoggedOut::Kept;
        }

        let stored = with.logins.read();
        if let Ok(remaining) = (with.serving)(named, &stored) {
            self.runner.serve(remaining.provider);
            self.clearings_recorded(None);
            return LoggedOut::StillServed {
                retained,
                source: remaining.source,
            };
        }

        let reachable = offered(with.providers).any(|other| (with.serving)(other, &stored).is_ok());
        let warning = if reachable {
            NO_PROVIDER_CHOSEN
        } else {
            NOTHING_TO_ASK
        };
        self.runner.ask("", UNKNOWN_CEILING, None, None);
        self.runner.serve(Box::new(Unavailable::new(warning)));
        self.clearings_recorded(None);
        self.serving = None;
        LoggedOut::SignedOut { retained }
    }

    /// Retires the persistent cache resources only this conversation's
    /// identity owns, ahead of that identity changing.
    fn retire(&mut self) -> Result<Retained, PromptCacheResourceError> {
        self.runner
            .retire_prompt_cache(&Cancel::new())
            .map(|result| Retained {
                ambiguous: result.ambiguous,
                orphaned: result.orphaned,
            })
    }
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
