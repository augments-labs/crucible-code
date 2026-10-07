//! What `crucible auth status`, `crucible auth login` and `crucible auth
//! logout` do, outside any conversation: the login store and the variables a
//! launch would read a key from, said or changed by provider.
//!
//! **Status** reads the store through [`Store::inventory`], which holds names
//! and lapse times and never a key, takes no lock and writes nothing, and says
//! which credential a launch would use by the rule a launch uses. Nothing is
//! sent and nothing is renewed: an account login whose access has lapsed is
//! said to be expired, since whether its account still renews it is something
//! only its vendor could say, and whether a vendor accepts any credential is
//! never known here and is said not to be.
//!
//! **Login** goes through the routes `/login` goes through. A key is read
//! once from standard input, bounded at [`MAX_SECRET`] bytes and refused
//! rather than cut where it is longer, checked against its row the way the
//! key box checks it, and written with [`Store::keep`] under its row's stored
//! name. An account is signed in to through this build's subscription
//! registry, on the run's own renewals owner, whose client asks
//! [`content_use`]'s hold before each request; the warned route is asked
//! about first, and the yes is written down only once the credential is.
//!
//! **Logout** takes the provider's credentials out of the store with one
//! locked write, [`Store::forgotten`], under the same lock a renewal holds
//! across its rotation, and leaves every other name in the file. A variable
//! in the environment is not the store's and is never touched: what a launch
//! would still find there is said, by name.
//!
//! A word typed on the command line is never said back. A provider is named
//! by this build's registry or a row's stored name, and a word that is
//! neither is refused with the names that are. A key is never an argument.

use std::io::Read;

use crucible_auth::{AuthError, Inventory, Kind, LoginUpdate, OAuthError, Store};
use crucible_config::{ConfigError, Home, Settings};
use crucible_types::shown::{Escaping, escaped};
use serde_core::Serialize as _;
use serde_json::ser::Serializer;
use serde_json::{Value, json};

use crate::content_use::{self, Warned};
use crate::providers::{self, CredentialSource, Misfit, Providers, Row, Rows, Served};
use crate::services;
use crate::subscription::{Route, Subscriptions};

#[cfg(test)]
mod tests;

/// The version of the status document; a field changing meaning is a new one.
pub const FORMAT_VERSION: u64 = 1;

/// What the status document calls itself.
pub const KIND: &str = "auth-status";

/// The longest key read from standard input, in bytes: the bound `/login`'s
/// key box holds a key to.
pub const MAX_SECRET: usize = 16 * 1024;

/// The whitespace read around a key beside it, in bytes: a key at the bound
/// is still taken with the line break `echo` puts after it, and the hidden
/// prompt holds the same room.
pub const SURROUNDING: usize = 64;

/// The longest variable name a report holds, in bytes. A name is the
/// configuration's to choose, so it is bounded and its cut said.
const MAX_NAME: usize = 256;

/// The longest sentence a report says about what could not be read, in bytes.
const MAX_PROBLEM: usize = 1024;

/// Why an auth command did not do what it was asked: a sentence that names
/// no key, no word read off the command line, and no path.
#[derive(Debug, thiserror::Error)]
pub enum Refused {
    /// The word names neither a provider this build serves nor a row.
    #[error("no provider or login row is called that; this build serves {known}")]
    Unknown {
        /// The provider names this build serves.
        known: String,
    },
    /// The row chosen takes no key.
    #[error(
        "{shown} takes no API key; run `crucible auth login {stored}` in a terminal to sign in"
    )]
    Keyless {
        /// The row's shown name.
        shown: &'static str,
        /// The name it is stored under.
        stored: &'static str,
    },
    /// crucible's home directory was not found.
    #[error("crucible's home directory was not found: {0}")]
    Homeless(#[source] ConfigError),
    /// The user's configuration could not be read, so what a credential is
    /// used with cannot be settled.
    #[error("the configuration could not be read; run `crucible config check` to see why")]
    Unconfigured,
    /// This build's providers could not be assembled.
    #[error("this build's providers could not be assembled: {0}")]
    Unbuilt(#[from] providers::ArmError),
    /// The login store could not be changed.
    #[error("{}", stored(.0))]
    Store(#[source] AuthError),
    /// Standard input could not be read.
    #[error("the key could not be read from standard input: {}", .0.kind())]
    Unread(#[source] std::io::Error),
    /// More than [`MAX_SECRET`] bytes arrived.
    #[error("the key is longer than {MAX_SECRET} bytes; nothing was stored")]
    Oversized,
    /// Nothing but whitespace arrived.
    #[error("no key arrived; nothing was stored")]
    Empty,
    /// What arrived is not UTF-8 text.
    #[error("the key is not UTF-8 text; nothing was stored")]
    Binary,
    /// What arrived holds whitespace or a control character inside it.
    #[error("the key holds whitespace or a control character; nothing was stored")]
    Spaced,
    /// The key does not fit its row, as the key box would say.
    #[error("{0}; nothing was stored")]
    Misfit(String),
    /// The runtime an account sign-in runs on would not start.
    #[error(transparent)]
    Unstarted(#[from] crate::runtime::Unstarted),
    /// The account sign-in did not complete.
    #[error("{0}; nothing was stored")]
    SignIn(#[source] OAuthError),
}

/// The sentence for a store that could not be written, naming no path.
fn stored(problem: &AuthError) -> &'static str {
    match problem {
        AuthError::Busy { .. } => {
            "another crucible is writing its login store; nothing was changed, try again in a moment"
        }
        AuthError::Unreadable { .. } => {
            "crucible cannot read its login store, so it will not write over it; move it aside and \
             try again"
        }
        AuthError::TooLarge { .. } => {
            "crucible's login store is larger than it will read, so it will not write over it"
        }
        AuthError::Unwritable { .. } => {
            "crucible cannot write its login store; fix the permissions of its home directory and \
             try again"
        }
        AuthError::Unreleased { .. } => {
            "what had to go with the change could not be taken out of the configuration, so \
             nothing was changed"
        }
    }
}

/// A key read for a login: redacted from `Debug`, with no `Display`, and
/// handed to nothing but the store.
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Secret(<redacted>)")
    }
}

impl Secret {
    /// Reads one key from `input`: at most [`MAX_SECRET`] bytes after
    /// surrounding whitespace is set aside, refused rather than cut where
    /// more arrive.
    ///
    /// # Errors
    ///
    /// [`Refused`] where `input` could not be read, or what it held is too
    /// long, empty, not text, or holds whitespace or a control character.
    pub fn read(input: &mut dyn Read) -> Result<Self, Refused> {
        let mut bytes = Vec::new();
        // Room for the line break a pipe ends with and a little more around
        // a key at the bound, and one byte past that, so a run that sends
        // more is told from one that does not without holding more.
        let room = MAX_SECRET.saturating_add(SURROUNDING);
        let ceiling = u64::try_from(room.saturating_add(1)).unwrap_or(u64::MAX);
        input
            .take(ceiling)
            .read_to_end(&mut bytes)
            .map_err(Refused::Unread)?;
        if bytes.len() > room {
            return Err(Refused::Oversized);
        }
        Self::typed(&String::from_utf8(bytes).map_err(|_| Refused::Binary)?)
    }

    /// A key typed at a hidden prompt, held to what [`Secret::read`] holds
    /// one to.
    ///
    /// # Errors
    ///
    /// [`Refused`] where it is too long, empty, or holds whitespace or a
    /// control character.
    pub fn typed(text: &str) -> Result<Self, Refused> {
        let key = text.trim();
        if key.len() > MAX_SECRET {
            return Err(Refused::Oversized);
        }
        if key.is_empty() {
            return Err(Refused::Empty);
        }
        if key
            .chars()
            .any(|one| one.is_whitespace() || one.is_control())
        {
            return Err(Refused::Spaced);
        }
        Ok(Self(key.to_owned()))
    }
}

/// How a login asks for its credential, once the word is settled.
#[derive(Debug, Clone)]
pub enum Login {
    /// A key, written under its row's stored name.
    Key(Way),
    /// An account sign-in.
    Account(Way),
    /// A key or an account sign-in, stored under the same name, whichever
    /// the person chooses.
    Either {
        /// The key row.
        key: Way,
        /// The account row.
        account: Way,
    },
}

/// One `/login` row, as an auth command names it.
#[derive(Debug, Clone)]
pub struct Way(Row);

impl Way {
    /// Its name in the `/login` list.
    #[must_use]
    pub fn shown(&self) -> &'static str {
        self.0.shown
    }

    /// The name its credential is stored under.
    #[must_use]
    pub fn stored(&self) -> &'static str {
        self.0.stored
    }

    /// The provider it signs in to.
    #[must_use]
    pub fn provider(&self) -> &'static str {
        self.0.provider
    }
}

/// What a login wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// The row it was written under.
    pub shown: &'static str,
    /// Its stored name.
    pub stored: &'static str,
    /// The rows of the same provider whose credentials it replaced.
    pub replaced: Vec<&'static str>,
    /// Sentences about what a launch will do with it.
    pub notes: Vec<String>,
}

/// What a logout took out, and what a launch would still find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forgotten {
    /// The provider.
    pub provider: &'static str,
    /// The rows whose credentials went, by shown name and stored name.
    pub went: Vec<(&'static str, String)>,
    /// What a launch would still sign this provider in with, in a sentence,
    /// where anything is left.
    pub remaining: Option<String>,
}

/// What an account sign-in needs from whoever is at the terminal.
pub trait Signer {
    /// Shows `warned`'s vendor's words and whether the person says yes.
    fn agrees(&mut self, warned: &Warned) -> bool;
    /// Which of `ways`, each a short name and what it does, is chosen, by
    /// index; `None` for none.
    fn chooses(&mut self, title: &str, ways: &[(&'static str, &'static str)]) -> Option<usize>;
    /// Shows the page to finish the sign-in at, and opens `browser` in a
    /// browser, with every variable in `withheld` kept from what it starts.
    fn visit(&mut self, page: &str, code: Option<&str>, browser: &str, withheld: &[&str]);
    /// Says what the sign-in is doing now.
    fn progress(&mut self, message: &str);
}

/// How an account sign-in ended, short of a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signed {
    /// It completed and the credential was written.
    In(Kept),
    /// The person said no to the vendor's terms, or chose no way.
    Declined,
}

/// The environment, asked whether a variable is set: what it answers is
/// never said.
pub type Lookup = Box<dyn Fn(&str) -> Option<String>>;

/// Everything an auth command reads from outside, as values.
pub struct Desk {
    home: Home,
    from: Lookup,
    settings: Result<Settings, ConfigError>,
    rows: Rows,
    providers: Providers,
}

impl std::fmt::Debug for Desk {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Desk")
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}

impl Desk {
    /// The desk of `home`, with the user's own configuration file read: the
    /// one file a provider's address and key variable are read from. `from`
    /// is the environment, asked only whether a key's variable is set.
    ///
    /// # Errors
    ///
    /// [`Refused`] where no home was found or this build's providers could
    /// not be assembled.
    pub fn open(home: Result<Home, ConfigError>, from: Lookup) -> Result<Self, Refused> {
        let home = home.map_err(Refused::Homeless)?;
        let settings = Settings::read_home(&home);
        Ok(Self {
            home,
            from,
            settings,
            rows: Rows::production(),
            providers: providers::providers()?.snapshot(),
        })
    }

    /// The provider `word` names: its own name, or a row's stored name.
    ///
    /// # Errors
    ///
    /// [`Refused::Unknown`] where it names neither.
    pub fn provider(&self, word: &str) -> Result<&'static str, Refused> {
        if let Some(one) = self.served(word) {
            return Ok(one.name);
        }
        self.rows
            .all()
            .iter()
            .find(|row| row.stored == word)
            .and_then(|row| self.served(row.provider))
            .map(|one| one.name)
            .ok_or_else(|| self.unknown())
    }

    /// The row a login for `word` writes to: with `key`, the key row a
    /// provider's variable belongs to or the key row stored under `word`;
    /// without, the account row stored under `word` where there is one, and
    /// its key row otherwise. Where a key row and an account row share the
    /// name, which of the two is the person's to say.
    ///
    /// # Errors
    ///
    /// [`Refused::Unknown`] where `word` names no provider or row, and
    /// [`Refused::Keyless`] for a key asked of a row that takes none.
    pub fn login(&self, word: &str, key: bool) -> Result<Login, Refused> {
        let provider = self.provider(word)?;
        let account = self.rows.of(Kind::Account, word);
        let keyed = self.rows.of(Kind::Key, word).or_else(|| {
            (word == provider)
                .then(|| self.rows.environment(provider))
                .flatten()
        });
        match (key, account, keyed) {
            (true, _, Some(row)) | (false, None, Some(row)) => Ok(Login::Key(Way(row.clone()))),
            (false, Some(account), Some(keyed)) if account.stored == keyed.stored => {
                Ok(Login::Either {
                    key: Way(keyed.clone()),
                    account: Way(account.clone()),
                })
            }
            (false, Some(row), _) => Ok(Login::Account(Way(row.clone()))),
            (true, Some(row), None) => Err(Refused::Keyless {
                shown: row.shown,
                stored: row.stored,
            }),
            (_, None, None) => Err(self.unknown()),
        }
    }

    fn served(&self, name: &str) -> Option<Served> {
        providers::offered(&self.providers).find(|one| one.name == name)
    }

    fn unknown(&self) -> Refused {
        Refused::Unknown {
            known: providers::names(&self.providers),
        }
    }

    fn settings(&self) -> Result<&Settings, Refused> {
        self.settings.as_ref().map_err(|_| Refused::Unconfigured)
    }

    /// The store a change goes through: one that takes a credential out
    /// takes its route's yes out of the user's own file first, and one that
    /// moves a provider's credential takes that provider's speed out of it,
    /// as the store a conversation writes through does.
    fn store(&self, consent: &content_use::Consent, settings: &Settings) -> Store {
        let file = crucible_config::user(&self.home);
        Store::in_home(self.home.path())
            .naming(self.rows.names())
            .letting_go(content_use::letting_go(
                consent,
                file.clone(),
                self.rows.clone(),
                settings,
            ))
            .moving(crate::speed::moving(file))
    }

    fn inventory(&self) -> Inventory {
        Store::in_home(self.home.path())
            .naming(self.rows.names())
            .inventory()
    }

    /// Writes `secret` under `way`'s stored name, once it fits the row.
    ///
    /// # Errors
    ///
    /// [`Refused`] where the key does not fit its row, the configuration
    /// cannot be read, or the store cannot be written.
    pub fn keep(&self, way: &Way, secret: &Secret) -> Result<Kept, Refused> {
        let settings = self.settings()?;
        if let Some(misfit) = self.rows.misfit(&way.0, &secret.0) {
            return Err(Refused::Misfit(unfitting(&misfit)));
        }
        let consent = self.consent(settings);
        let dropped = self
            .store(&consent, settings)
            .keep(way.0.stored, &secret.0)
            .map_err(Refused::Store)?;
        let replaced = dropped
            .iter()
            .filter_map(|held| self.rows.of(held.kind, &held.name))
            .map(|row| row.shown)
            .collect();
        let mut notes = self.after(way, settings);
        let route = content_use::row_route(&way.0);
        if consent.asks(&route).is_some() {
            notes.push(format!(
                "{}'s vendor says it may use what is sent; crucible asks before the first \
                 request",
                way.0.shown
            ));
        }
        Ok(Kept {
            shown: way.0.shown,
            stored: way.0.stored,
            replaced,
            notes,
        })
    }

    /// What a launch will do with the credential `way` now holds, where it is
    /// not simply to use it.
    fn after(&self, way: &Way, settings: &Settings) -> Vec<String> {
        let mut notes = Vec::new();
        let Some(one) = self.served(way.0.provider) else {
            return notes;
        };
        let inventory = self.inventory();
        let source = providers::sourced(
            one,
            settings,
            &*self.from,
            inventory.held(one.name),
            self.rows.subscribes(one.name),
        );
        if let Some(CredentialSource::Environment(variable)) = source {
            notes.push(format!(
                "{} is set, and a launch uses it before the stored credential",
                named(&variable)
            ));
        }
        if way.0.kind == Kind::Account && settings.base_url(one.name).is_some() {
            notes.push(format!(
                "providers.{}.baseUrl is set, so a launch does not use this account login until \
                 it is removed",
                one.name
            ));
        }
        notes
    }

    fn consent(&self, settings: &Settings) -> content_use::Consent {
        let consent = content_use::Consent::new(content_use::Routes::production());
        Self::agreed(&consent, &self.home, settings);
        consent
    }

    fn agreed(consent: &content_use::Consent, home: &Home, settings: &Settings) {
        consent.keeps_in(crucible_config::user(home));
        consent.recorded(settings.content_accepted().into_iter().map(str::to_owned));
    }

    /// Takes every credential `provider` holds out of the store, in one
    /// locked write, and says what a launch would still find.
    ///
    /// # Errors
    ///
    /// [`Refused`] where the configuration cannot be read, or the store
    /// cannot be written; nothing was taken out then.
    pub fn forget(&self, provider: &str) -> Result<Forgotten, Refused> {
        let settings = self.settings()?;
        let one = self.served(provider).ok_or_else(|| self.unknown())?;
        let consent = self.consent(settings);
        let went = self
            .store(&consent, settings)
            .forgotten(one.name)
            .map_err(Refused::Store)?
            .into_iter()
            .map(|held| {
                let shown = self
                    .rows
                    .of(held.kind, &held.name)
                    .map_or("", |row| row.shown);
                (shown, held.name)
            })
            .collect();
        let inventory = self.inventory();
        let remaining = providers::sourced(
            one,
            settings,
            &*self.from,
            inventory.held(one.name),
            self.rows.subscribes(one.name),
        )
        .map(|source| match source {
            CredentialSource::Environment(variable) => format!(
                "a launch still signs {} in with {}, which is the shell's and not Crucible's to \
                 remove; unset it there",
                one.name,
                named(&variable)
            ),
            CredentialSource::StoredKey | CredentialSource::Subscription => format!(
                "another crucible stored a credential for {} since; run this again to take it out",
                one.name
            ),
        });
        Ok(Forgotten {
            provider: one.name,
            went,
            remaining,
        })
    }

    /// Signs in to `way`'s account, asking `signer` what only the person at
    /// the terminal can answer.
    ///
    /// # Errors
    ///
    /// [`Refused`] where the configuration cannot be read, the runtime will
    /// not start, or the sign-in failed.
    pub fn sign_in(&self, way: &Way, signer: &mut dyn Signer) -> Result<Signed, Refused> {
        let settings = self.settings()?;
        // The services are shut down before what came of the sign-in is
        // said; renewals still running then were the sign-in's own.
        let (outcome, _unfinished) =
            services::serving(|services| self.signing(services, way, settings, signer));
        outcome
    }

    fn signing(
        &self,
        services: &services::Services,
        way: &Way,
        settings: &Settings,
        signer: &mut dyn Signer,
    ) -> Result<Signed, Refused> {
        let consent = services.consent();
        Self::agreed(consent, &self.home, settings);
        let route = content_use::row_route(&way.0);
        if let Some(warned) = consent.asks(&route).copied() {
            if !signer.agrees(&warned) {
                return Ok(Signed::Declined);
            }
            consent.give(&route);
        }
        let mut outcome = self.signed(services, way, settings, signer);
        if let Ok(Signed::In(kept)) = &mut outcome
            && consent.given(&route)
            && let Some(warned) = consent.routes().warned(&route).copied()
            && let Err(problem) = consent.accept(&warned)
        {
            // The credential is stored; the yes was not written down, so the
            // first request asks again.
            kept.notes.push(format!(
                "your yes could not be written down, so crucible asks again before the first \
                 request: {problem}"
            ));
        }
        consent.withdraw(&route);
        outcome
    }

    fn signed(
        &self,
        services: &services::Services,
        way: &Way,
        settings: &Settings,
        signer: &mut dyn Signer,
    ) -> Result<Signed, Refused> {
        let subscriptions = Subscriptions::production(services.renewals());
        let routes = subscriptions.routes(way.0.stored);
        let route = match routes.as_slice() {
            [] => return Err(Refused::SignIn(OAuthError::Method)),
            [only] => *only,
            several => {
                let ways: Vec<_> = several.iter().map(|one| (one.shown, one.says)).collect();
                let title = several.first().map_or("", |one| one.title());
                let Some(chosen) = signer
                    .chooses(title, &ways)
                    .and_then(|at| several.get(at).copied())
                else {
                    return Ok(Signed::Declined);
                };
                chosen
            }
        };
        services.renewals().runs_on(services.runtime().handle()?);
        let before = self.inventory().held(way.0.provider);
        let consent = services.consent();
        let attempt = subscriptions
            .start(route, self.store(consent, settings))
            .map_err(Refused::SignIn)?;
        let withheld: Vec<&str> = providers::key_variables(&self.providers, settings).collect();
        Self::waited(&attempt, route, signer, &withheld)?;
        let replaced = before
            .filter(|held| held.kind != Kind::Account || held.name != way.0.stored)
            .and_then(|held| self.rows.of(held.kind, &held.name))
            .map(|row| row.shown)
            .into_iter()
            .collect();
        Ok(Signed::In(Kept {
            shown: way.0.shown,
            stored: way.0.stored,
            replaced,
            notes: self.after(way, settings),
        }))
    }

    fn waited(
        attempt: &crucible_auth::LoginAttempt,
        route: Route,
        signer: &mut dyn Signer,
        withheld: &[&str],
    ) -> Result<(), Refused> {
        signer.progress(route.title());
        loop {
            match attempt.wait(std::time::Duration::from_millis(100)) {
                Ok(Some(LoginUpdate::Complete)) => return Ok(()),
                Ok(Some(LoginUpdate::Authorize {
                    browser_uri,
                    shown_uri,
                    user_code,
                    manual: _,
                })) => signer.visit(&shown_uri, user_code.as_deref(), &browser_uri, withheld),
                Ok(Some(LoginUpdate::Progress { message })) => signer.progress(message),
                Ok(None) => {}
                Err(problem) => return Err(Refused::SignIn(problem)),
            }
        }
    }

    /// What every provider, or the one named, is signed in with, read from
    /// the store's names and the environment's variables alone.
    ///
    /// `now` is the time a lapse is measured against, in seconds since the
    /// Unix epoch.
    #[must_use]
    pub fn status(&self, provider: Option<&'static str>, now: u64) -> Status {
        let inventory = self.inventory();
        let said = inventory.trouble().or_else(|| {
            self.settings.as_ref().err().map(
                |_| "the configuration could not be read; run `crucible config check` to see why",
            )
        });
        // The store's sentence quotes the names it holds, and those are any
        // other process's to choose: it is cut once, and the header and
        // every provider's reason say that one cut sentence.
        let trouble = said.map(|said| cut(said, MAX_PROBLEM));
        let entries = providers::offered(&self.providers)
            .filter(|one| provider.is_none_or(|named| named == one.name))
            .map(|one| self.entry(one, &inventory, trouble.as_deref(), now))
            .collect();
        Status {
            named: provider.is_some(),
            trouble_cut: said.is_some_and(|said| said.len() > MAX_PROBLEM),
            trouble,
            entries,
        }
    }

    fn entry(&self, one: Served, inventory: &Inventory, trouble: Option<&str>, now: u64) -> Entry {
        let settings = self.settings.as_ref().ok();
        let configured = settings.and_then(|settings| settings.api_key_env(one.name));
        let looked = configured.unwrap_or(one.key);
        let variable = Variable {
            set: (self.from)(looked).is_some_and(|value| !value.trim().is_empty()),
            configured: configured.is_some(),
            cut: looked.len() > MAX_NAME,
            name: cut(looked, MAX_NAME),
        };
        let mut stored: Vec<Stored> = self
            .rows
            .all()
            .iter()
            .filter(|row| row.provider == one.name && inventory.holds(row.kind, row.stored))
            .map(|row| Stored {
                name: row.stored,
                kind: row.kind,
                lapses: (row.kind == Kind::Account)
                    .then(|| inventory.lapses(row.stored))
                    .flatten(),
            })
            .collect();
        stored.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);
        let (source, state, reason) = match (trouble, settings) {
            (Some(said), _) => (None, State::Unverified, format!("not settled: {said}")),
            (None, None) => (
                None,
                State::Unverified,
                "not settled: the configuration could not be read".to_owned(),
            ),
            (None, Some(settings)) => {
                let (source, state, reason) = self.settle(one, settings, inventory, now);
                let reason = reason.unwrap_or_else(|| match state {
                    State::Configured => format!(
                        "{} is set, and a launch uses it before anything stored",
                        variable.name
                    ),
                    _ => format!("nothing is stored and {} is not set", variable.name),
                });
                (source, state, reason)
            }
        };
        Entry {
            provider: one.name,
            state,
            source,
            based: settings.is_some_and(|settings| settings.base_url(one.name).is_some()),
            variable,
            stored,
            reason,
        }
    }

    /// Where `one` stands by the launch's own rule, and why in a sentence
    /// where the sentence is not about its variable: `None` where it is
    /// signed in with that variable, or with nothing.
    fn settle(
        &self,
        one: Served,
        settings: &Settings,
        inventory: &Inventory,
        now: u64,
    ) -> (Option<Source>, State, Option<String>) {
        let held = inventory.held(one.name);
        let source = providers::sourced(
            one,
            settings,
            &*self.from,
            held.clone(),
            self.rows.subscribes(one.name),
        );
        let kind = source.as_ref().map(Source::of);
        let (state, reason) = match source {
            None if held.is_some() => (
                State::Absent,
                Some(format!(
                    "the account login stored is not used while providers.{}.baseUrl is set",
                    one.name
                )),
            ),
            None => (State::Absent, None),
            Some(CredentialSource::Subscription) => {
                match held.and_then(|held| inventory.lapses(&held.name)) {
                    Some(at) if at <= now => (
                        State::Expired,
                        Some(
                            "the stored account login's access has lapsed; a launch renews it \
                             where the account still allows, which only the vendor can say"
                                .to_owned(),
                        ),
                    ),
                    Some(_) => (State::Configured, Some("a stored account login".to_owned())),
                    None => (
                        State::Unverified,
                        Some("a stored account login with no lapse time".to_owned()),
                    ),
                }
            }
            Some(CredentialSource::StoredKey) => {
                (State::Configured, Some("a stored API key".to_owned()))
            }
            Some(CredentialSource::Environment(_)) => (State::Configured, None),
        };
        (kind, state, reason)
    }
}

/// What the key box says of a key that does not fit its row, without its
/// mark of urgency.
fn unfitting(misfit: &Misfit) -> String {
    match *misfit {
        Misfit::Unmarked(mark) => format!("not a key for this row; its keys start {mark}"),
        Misfit::Another(row) => format!("that is a key for {row}; name that row instead"),
        Misfit::Refused(mark) => {
            format!("not a key for this row; keys starting {mark} are another kind")
        }
        Misfit::Shared(mark) => {
            format!("keys starting {mark} are for another row; name it by its plan and site")
        }
    }
}

/// A variable's name as a sentence says it: with every character a terminal
/// would act on or hide written as its escape, bounded at [`MAX_NAME`] bytes.
fn named(name: &str) -> String {
    cut(&escaped(name), MAX_NAME)
}

/// `text` at most `bound` bytes long, cut on a character boundary.
fn cut(text: &str, bound: usize) -> String {
    let mut end = bound.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default().to_owned()
}

/// What `auth status` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    named: bool,
    trouble: Option<String>,
    trouble_cut: bool,
    entries: Vec<Entry>,
}

/// One provider's line of it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    provider: &'static str,
    state: State,
    source: Option<Source>,
    variable: Variable,
    based: bool,
    stored: Vec<Stored>,
    reason: String,
}

/// The variable a launch would read a provider's key from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Variable {
    /// Its name, as the configuration gave it, cut at [`MAX_NAME`] bytes.
    name: String,
    /// Whether it is set to something other than whitespace.
    set: bool,
    /// Whether `apiKeyEnv` named it, rather than the provider's own.
    configured: bool,
    /// Whether its name was cut at [`MAX_NAME`].
    cut: bool,
}

/// A credential the store holds for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stored {
    name: &'static str,
    kind: Kind,
    lapses: Option<u64>,
}

/// Where a provider stands, as far as can be said without asking anyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// A launch would sign it in with a credential.
    Configured,
    /// A launch would find nothing to sign it in with.
    Absent,
    /// Its stored account login's access has lapsed.
    Expired,
    /// What it holds could not be settled here.
    Unverified,
}

impl State {
    /// How the document spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Absent => "absent",
            Self::Expired => "expired",
            Self::Unverified => "unverified",
        }
    }
}

/// Which kind of source a launch would take a credential from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Environment,
    StoredKey,
    Account,
}

impl Source {
    fn of(source: &CredentialSource) -> Self {
        match source {
            CredentialSource::Environment(_) => Self::Environment,
            CredentialSource::StoredKey => Self::StoredKey,
            CredentialSource::Subscription => Self::Account,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::StoredKey => "stored-key",
            Self::Account => "account",
        }
    }
}

const fn kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Key => "key",
        Kind::Account => "account",
    }
}

impl Status {
    fn truncated(&self) -> bool {
        self.trouble_cut || self.entries.iter().any(|entry| entry.variable.cut)
    }

    /// `complete` where every provider was settled and nothing was cut,
    /// `incomplete` otherwise.
    #[must_use]
    pub fn status(&self) -> &'static str {
        let unsettled = self
            .entries
            .iter()
            .any(|entry| entry.state == State::Unverified);
        if self.trouble.is_some() || unsettled || self.truncated() {
            "incomplete"
        } else {
            "complete"
        }
    }

    /// 0 where it is complete and the provider named, if one was, has a
    /// credential; 1 otherwise.
    #[must_use]
    pub fn exit(&self) -> u8 {
        let missing = self.named
            && self
                .entries
                .iter()
                .any(|entry| entry.state == State::Absent);
        u8::from(self.status() != "complete" || missing)
    }

    /// The document, on one line.
    ///
    /// Its strings hold the text they stand for, bounded, and are written
    /// through [`Escaping`]: a control or format character a configuration
    /// or the store chose leaves as a `\u` escape, which reads back as the
    /// same character.
    #[must_use]
    pub fn json(&self) -> Vec<u8> {
        let providers: Vec<Value> = self
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "provider": entry.provider,
                    "state": entry.state.as_str(),
                    "source": entry.source.map(Source::as_str),
                    "variable": entry.variable.name,
                    "variable_set": entry.variable.set,
                    "variable_configured": entry.variable.configured,
                    "base_url_configured": entry.based,
                    "stored": entry.stored.iter().map(|one| json!({
                        "name": one.name,
                        "kind": kind(one.kind),
                        "expires_at": one.lapses,
                    })).collect::<Vec<_>>(),
                    "reason": entry.reason,
                })
            })
            .collect();
        let document = json!({
            "format_version": FORMAT_VERSION,
            "kind": KIND,
            "status": self.status(),
            "acceptance": "unchecked",
            "problem": self.trouble,
            "providers": providers,
            "truncated": self.truncated(),
        });
        written(&document)
    }

    /// The report as a person reads it, with lapse times measured from `now`.
    ///
    /// A sentence that quotes a name the configuration or the store chose is
    /// written [`escaped`], a line break in it among the rest, so it stays on
    /// its own line.
    #[must_use]
    pub fn human(&self, now: u64) -> String {
        use std::fmt::Write as _;

        let mut out = format!("crucible auth status: {}\n", self.status());
        if let Some(trouble) = &self.trouble {
            let _ = writeln!(out, "  {}", escaped(trouble));
        }
        for entry in &self.entries {
            let _ = writeln!(
                out,
                "  {:<10} {}: {}",
                entry.state.as_str(),
                entry.provider,
                escaped(&entry.reason)
            );
            for one in &entry.stored {
                let lapse = one.lapses.map_or_else(String::new, |at| {
                    if at <= now {
                        format!(", lapsed {} ago", span(now.saturating_sub(at)))
                    } else {
                        format!(", lapses in {}", span(at.saturating_sub(now)))
                    }
                });
                let _ = writeln!(
                    out,
                    "             stored {} {}{lapse}",
                    kind(one.kind),
                    one.name
                );
            }
        }
        out.push_str("  whether a vendor accepts any of these is not checked here\n");
        out
    }
}

/// A span of seconds in the largest whole unit it makes.
fn span(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// The status document for a run that settled nothing, so a script reading
/// standard output never finds it empty.
#[must_use]
pub fn failed(problem: &Refused) -> Vec<u8> {
    let said = problem.to_string();
    let document = json!({
        "format_version": FORMAT_VERSION,
        "kind": KIND,
        "status": "failed",
        "acceptance": "unchecked",
        "problem": cut(&said, MAX_PROBLEM),
        "providers": [],
        "truncated": said.len() > MAX_PROBLEM,
    });
    written(&document)
}

/// `document` on one line, through [`Escaping`], ending in a line break.
fn written(document: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    // Into memory a `Value` is always written: its keys are strings, its
    // numbers finite, and a `Vec` refuses no byte.
    let _ = document.serialize(&mut Serializer::with_formatter(&mut bytes, Escaping));
    bytes.push(b'\n');
    bytes
}

/// What a login wrote, as the person who asked reads it.
#[must_use]
pub fn kept(kept: &Kept) -> String {
    use std::fmt::Write as _;

    let mut out = format!(
        "stored a credential for {} under {}\n",
        kept.shown, kept.stored
    );
    for row in &kept.replaced {
        let _ = writeln!(out, "  it replaced the one stored for {row}");
    }
    for note in &kept.notes {
        let _ = writeln!(out, "  {note}");
    }
    out
}

/// What a logout took out, as the person who asked reads it.
#[must_use]
pub fn forgotten(forgotten: &Forgotten) -> String {
    use std::fmt::Write as _;

    let mut out = if forgotten.went.is_empty() {
        format!(
            "nothing was stored by Crucible for {}\n",
            forgotten.provider
        )
    } else {
        let mut out = String::new();
        for (shown, name) in &forgotten.went {
            let _ = writeln!(
                out,
                "removed the credential stored for {shown} under {name}"
            );
        }
        out
    };
    if let Some(remaining) = &forgotten.remaining {
        let _ = writeln!(out, "  {remaining}");
    }
    out
}
