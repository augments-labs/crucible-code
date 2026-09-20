//! The providers this build serves, and which of them a machine can reach.
//!
//! One table names every provider, where its key is read from and the models
//! `/model` offers for it; the registry below is that table with collisions
//! refused. Everything after the registry is about credentials: which source
//! a provider's credential would come from, which provider a run with no flag
//! lands on, and the closure `/login` and `/logout` re-resolve a provider
//! through. Nothing here reads a secret out — a source is named, never its
//! value.

use std::fmt;

use crucible_auth::StoredCredentials;
use crucible_config::Settings;
use crucible_models::{Effort, ModelCapabilities, ModelError, ModelLimits, Provider};
use crucible_registry::{
    Collision, Provenance, Registered, Registry, RegistryError, RegistrySnapshot, SourceKind,
};

use crate::AppError;
use crate::models;
use crate::startup::{self, served};
use crate::subscription::Subscriptions;

/// The providers this is built with, and where each one's key is read from.
///
/// One list rather than two: the sentence a wrong name gets back is written
/// from it, and so is the check that refuses the name before anything is drawn.
/// [`startup::provider`] has one arm per entry, and adding a provider is an
/// edit to both in the same commit.
///
/// The models are what `/model` offers and not what anything asks for. No
/// default is written here and none may be: a model chosen at compile time is
/// chosen for somebody who never asked for it — it outlives the model, it is
/// asked for with whichever key happens to be set, and the first anyone hears
/// of the mismatch is a refusal from a vendor they did not mean to write to.
/// What to ask for comes from the person running it, through `--model` or
/// through `providers.<name>.model`, and where neither says, crucible asks
/// rather than guesses. An offer is how it asks; it is still they who answer.
///
/// So this list going stale costs a shortcut and nothing else. A model retired
/// since the build is one nobody picked without the vendor refusing it by name,
/// and a model released since is typed, which is the path that was there before
/// any of these were written down.
const PROVIDERS: [Served; 4] = [
    Served {
        name: "anthropic",
        shown: "Anthropic",
        key: "ANTHROPIC_API_KEY",
        build: startup::anthropic,
        reach: startup::anthropic_web,
        window: 200_000,
        models: &[
            Model::shown("claude-fable-5-1", "Claude Fable 5.1", EVERY),
            Model::new("claude-fable-5", EVERY),
            Model::new("claude-opus-5", EVERY),
            Model::new("claude-sonnet-5", EVERY),
            // The one model of this vendor's current three generations that
            // takes no rung: it reasons against a token budget rather than
            // against a word, and the field the other three read is one it has
            // never been served.
            Model::new("claude-haiku-4-5", NONE),
        ],
    },
    Served {
        name: "google",
        shown: "Google",
        key: "GEMINI_API_KEY",
        build: startup::google,
        reach: startup::google_web,
        // The model's full input capacity is available through configuration;
        // starting below the long-context pricing boundary keeps it deliberate.
        window: 200_000,
        models: &[
            Model::shown("gemini-3.8-flash", "Gemini 3.8 Flash", GEMINI),
            Model::shown("gemini-3.7-flash", "Gemini 3.7 Flash", GEMINI),
            Model::shown("gemini-3.6-flash", "Gemini 3.6 Flash", GEMINI),
            Model::shown("gemini-3.1-pro-preview", "Gemini 3.1 Pro Preview", GEMINI),
        ],
    },
    Served {
        name: "moonshot",
        shown: "MoonshotAI",
        key: "MOONSHOT_API_KEY",
        build: startup::moonshot,
        reach: startup::moonshot_web,
        window: 262_144,
        // Spelled the way the coding console spells them, that being the one
        // crucible asks. The open platform serves the same models under longer
        // names and does not serve the second of these at all, so a key from
        // there is a `baseUrl` and a typed name rather than a shorter list.
        models: &[
            Model::shown("k3", "K3", KIMI),
            // The same model held to a quarter of its context. Offered beside
            // it because the smaller context is a distinct provider offering
            // rather than a local preference.
            Model::shown("k3-256k", "K3-256k", KIMI),
            // The coding models are known by their product names; the wire
            // identifier stays the one the console serves them under.
            Model::shown("kimi-for-coding", "K2.7 Coding", KIMI),
            Model::shown("kimi-for-coding-highspeed", "K2.7 Coding Highspeed", KIMI),
        ],
    },
    Served {
        name: "openai",
        shown: "OpenAI",
        key: "OPENAI_API_KEY",
        build: startup::openai,
        reach: startup::openai_web,
        window: 272_000,
        // The `-pro` variants are left off: they answer in one piece rather
        // than streaming, and every turn here is drawn as it arrives.
        models: &[
            Model::shown("gpt-6-astra", "GPT-6 Astra", EVERY),
            Model::new("gpt-5.6-sol", EVERY),
            Model::new("gpt-5.6-terra", EVERY),
            Model::new("gpt-5.6-luna", EVERY),
            // One generation back and one rung short of the others.
            Model::new(
                "gpt-5.5",
                &[Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh],
            ),
        ],
    },
];

/// What the generated table knows about a model's limits, if it knows anything.
///
/// The one reader is [`Arm::builtin`], which joins this with the offer list
/// above into a record the registry then holds. Nothing else reads it: a limit
/// asked about after startup is asked of the registry, because a provider
/// registered at run time has limits and no row here.
///
/// Keyed on the name exactly as it was asked for. A name this build has never
/// heard of answers `None` rather than the nearest thing it resembles: two
/// models one word apart can differ five-fold in what they accept, and a
/// session run against the wrong one of them throws away most of itself before
/// anybody notices.
fn facts(provider: &str, model: &str) -> Option<models::Facts> {
    models::FACTS
        .iter()
        .find(|facts| facts.provider == provider && facts.model == model)
        .copied()
}

/// What a model that takes every rung crucible has is written with.
const EVERY: &[Effort] = &Effort::LADDER;

/// The supported intersection of Gemini thinking levels and this UI's ladder.
const GEMINI: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High];

/// The three rungs the Kimi thinking models serve.
///
/// It maps the two it does not serve onto these rather than refusing them, so
/// this is the one place a narrowed ladder is a courtesy instead of the
/// difference between a turn and an error. It is still narrowed: a rung offered
/// is a rung asked for, and two words that reach the same rung are two words
/// somebody has to be told are the same.
const KIMI: &[Effort] = &[Effort::Low, Effort::High, Effort::Max];

/// What a model that takes none at all is written with.
///
/// Not the same as a model this build has never heard of. That one is offered
/// all five and left to the vendor to refuse, because crucible knows nothing
/// about it either way; this one is a model crucible knows serves no rung, and
/// offering one would be inventing a fact rather than declining to have one.
const NONE: &[Effort] = &[];

/// One model a provider offers, and how hard it can be asked to think.
#[derive(Debug, Clone, Copy)]
pub struct Model {
    /// What `--model` and `/model` name it, spelled the way the vendor spells
    /// it, because it is the vendor that has to recognise it.
    pub name: &'static str,
    /// What a picker row calls it, where the product name differs from the wire
    /// identifier a configuration or command must carry.
    pub shown: &'static str,
    /// The rungs of [`Effort`] this model serves, weakest first, as its
    /// vendor's documentation had them when this was built.
    ///
    /// Empty is a model that takes no rung at all — several of these serve
    /// none, and two vendors refuse the request outright rather than ignoring
    /// the field. So this is what `/effort` walks rather than the whole ladder:
    /// a rung offered here that the model does not serve is a refusal crucible
    /// walked somebody into, one keystroke after showing them the word that
    /// caused it.
    pub rungs: &'static [Effort],
}

impl Model {
    /// One entry of the table above.
    const fn new(name: &'static str, rungs: &'static [Effort]) -> Self {
        Self {
            name,
            shown: name,
            rungs,
        }
    }

    /// One entry whose product name and wire identifier differ.
    const fn shown(name: &'static str, shown: &'static str, rungs: &'static [Effort]) -> Self {
        Self { name, shown, rungs }
    }
}

/// A provider this build has an arm for, and where its key is read from.
///
/// The arm is on the record. What builds the provider, what builds its web
/// sources and what window it manages against travel with the name rather than
/// being matched against it somewhere else, so a record registered under a new
/// name is a provider without anything downstream learning that it exists.
#[derive(Debug, Clone, Copy)]
pub struct Served {
    /// What `--model provider/…` and `providers.<name>` call it.
    pub name: &'static str,
    /// How the vendor spells it, for a list somebody is reading down rather
    /// than typing at. It reaches no argument, no config file and no request:
    /// a name that is capitalised in one place and lowercase in another is one
    /// somebody eventually types the wrong way round, so only [`Served::name`]
    /// is ever matched against.
    pub shown: &'static str,
    /// The variable its key is read from, unless `apiKeyEnv` names another.
    /// The *name* is what is written here; the value is read once, in
    /// [`startup::provider`], and goes no further than the header it signs.
    pub key: &'static str,
    /// The models `/model` offers for it, newest first, at most five.
    ///
    /// An offer and not a rung: nothing here is ever asked for unless somebody
    /// chose it, and a name that is not on the list is typed the way it always
    /// was. Which is what keeps this from being the model built into the build —
    /// the list is a shortcut past the vendor's documentation, and the vendor
    /// remains the authority on what it serves.
    ///
    /// That last sentence binds the rungs beside each name too. They are read
    /// off the same documentation and go stale the same way, and what a stale
    /// one normally costs is a rung missing from a panel. The Google encoder's
    /// narrower supported effort set is checked for typed choices as well;
    /// other providers keep their existing vendor-validated typed path.
    pub models: &'static [Model],
    /// Builds the provider from a resolved credential and address.
    ///
    /// The one place a name becomes a type. A fresh transport every call, and a
    /// credential read once inside it; nothing in any crate below has to learn
    /// that this provider exists.
    pub build: startup::Factory,
    /// Builds what answers the two web tools, or nothing where this provider
    /// serves neither. Never fails a start: a session without web tools is
    /// still the coding agent that was asked for.
    pub reach: startup::Reach,
    /// The context window a session manages against unless a setting says
    /// otherwise. Conservative on purpose: long context is available, and
    /// using it is a choice rather than the starting behavior.
    pub window: u32,
}

/// Why the built-in providers could not be assembled.
///
/// Every variant is a wiring defect rather than anything a user did, so each
/// one names the provider and the model it is about: this is read at a terminal
/// by whoever wrote the row, and the run stops before the first frame.
#[derive(Debug, thiserror::Error)]
pub enum ArmError {
    /// The registry refused the record.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// A model is offered and the generated table has no row for it.
    #[error("{provider} offers {model} and the model table has no row for it; run generate-models")]
    Unlisted {
        /// The provider offering it.
        provider: &'static str,
        /// The model with no row.
        model: &'static str,
    },
    /// A model's record could not be described from the row it has.
    #[error("{provider} cannot describe {model}: {why}")]
    Model {
        /// The provider offering it.
        provider: &'static str,
        /// The model that could not be described.
        model: &'static str,
        /// What the record refused.
        why: ModelError,
    },
}

/// One provider as the registry holds it: the arm, where it came from, and what
/// it can be asked for.
#[derive(Debug)]
pub struct Arm {
    served: Served,
    provenance: Provenance,
    models: Box<[ModelCapabilities]>,
}

impl Arm {
    /// A provider compiled into this build.
    ///
    /// The one place the offer list and the generated table of limits are
    /// joined. A model offered with no row stops the run here rather than
    /// answering "nothing known" at a reader: the two lists are written by hand
    /// and by a generator, and the whole point of a record is that a name on the
    /// panel has one.
    ///
    /// # Errors
    ///
    /// [`ArmError`] where the name does not fit a source identity, where a
    /// model is offered with no row, or where a row does not describe a model.
    fn builtin(served: Served) -> Result<Self, ArmError> {
        let provenance = Provenance::new(
            SourceKind::Builtin,
            format!("crucible:{}", served.name),
            format!("built-in {} provider", served.name),
        )
        .map_err(RegistryError::from)?;

        let mut models = Vec::with_capacity(served.models.len());
        for model in served.models {
            let Some(facts) = facts(served.name, model.name) else {
                return Err(ArmError::Unlisted {
                    provider: served.name,
                    model: model.name,
                });
            };
            let described = ModelCapabilities::new(
                model.name,
                model.shown,
                ModelLimits {
                    window: facts.window,
                    output: facts.output,
                    accepts: facts.accepts,
                },
                model.rungs,
            )
            .map_err(|why| ArmError::Model {
                provider: served.name,
                model: model.name,
                why,
            })?;
            models.push(described);
        }

        Ok(Self {
            served,
            provenance,
            models: models.into_boxed_slice(),
        })
    }

    /// The arm itself: what builds it, and what it is called.
    pub const fn served(&self) -> Served {
        self.served
    }

    /// What is known about one of them, or nothing where this build has never
    /// heard of the name.
    ///
    /// Matched exactly as it was asked for. A name one word from an offered one
    /// is a name nothing is known about, and borrowing the neighbour's figures
    /// is how a session comes to throw away half of itself.
    pub fn model(&self, named: &str) -> Option<&ModelCapabilities> {
        self.models.iter().find(|one| one.name() == named)
    }
}

impl Registered for Arm {
    fn id(&self) -> &str {
        self.served.name
    }

    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn retained_bytes(&self) -> usize {
        self.models.iter().fold(
            size_of::<Self>() + self.provenance.retained_bytes(),
            |so_far, model| so_far.saturating_add(model.retained_bytes()),
        )
    }
}

/// The providers a name is read against: one generation of the registry.
pub type Providers = RegistrySnapshot<Arm>;

/// The provider registry with every built-in provider in it, in the order the
/// panels list them.
///
/// Refusing collisions rather than ranking them: a provider is named in a flag,
/// a file and a picker, and two arms answering to one name is a key sent to
/// whichever of them registered last.
///
/// # Errors
///
/// [`ArmError`] where a built-in could not be registered — a name written twice
/// in `PROVIDERS`, one too long for a source identity, or a model offered
/// with no row in the generated table. All are wiring defects, and the sentence
/// names the provider.
pub fn providers() -> Result<Registry<Arm>, ArmError> {
    let registry = Registry::new(Collision::Refuse);
    let mut staged = registry.stage();
    for served in PROVIDERS {
        staged.register(Arm::builtin(served)?)?;
    }
    registry.commit(staged)?;
    Ok(registry)
}

/// Every provider of one generation, in the order they were registered.
pub fn offered(providers: &Providers) -> impl Iterator<Item = Served> + '_ {
    providers.entries().iter().map(|arm| arm.served())
}

/// What one provider is set up with, once it has a credential.
///
/// The two answers a launch reaches before the first prompt, in one value so
/// that `/login` can reach them again from the prompt. A usable credential is
/// what both waited on: a file that chose a model for a provider this machine
/// could not reach was a file saying nothing about this run, and the moment a
/// credential arrives it is saying something.
pub struct Resolved {
    /// What a request is written to.
    pub provider: Box<dyn Provider>,
    /// Which non-secret source supplied its credential.
    pub source: CredentialSource,
}

impl fmt::Debug for Resolved {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Resolved")
            .field("provider", &self.provider.name())
            .field("source", &self.source)
            .finish()
    }
}

/// Where the active credential came from, without any credential bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// An environment variable, named but never read back through this value.
    Environment(Box<str>),
    /// An API key written by `/login`.
    StoredKey,
    /// A renewable account login written by `/login`.
    Subscription,
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(name) => write!(out, "environment variable {name}"),
            Self::StoredKey => out.write_str("a stored API key"),
            Self::Subscription => out.write_str("a stored account login"),
        }
    }
}

/// Sets one provider up from the credentials in hand, the way the launch set
/// this run's up.
///
/// Boxed rather than borrowed because what it closes over are borrows of the
/// launch, and a lifetime here would follow the value that holds it into the
/// signature of everything a command is handed.
pub type Serving = Box<dyn Fn(Served, &StoredCredentials) -> Result<Resolved, AppError>>;

/// What crucible says when nothing on this machine is set up to answer.
///
/// Said under the welcome where a run starts with no key for any provider, and
/// again in place of any turn typed before there is one.
pub const NOTHING_TO_ASK: &str = "Warning: No models available. Use /login or set an API key environment variable. Then use /model to select a model.";

/// What it says when there is a provider to ask and nothing to ask it for.
///
/// A separate sentence rather than the one above, because the one above tells
/// somebody to set a key — and the key is the half they have already done. A
/// warning that names the wrong missing thing is worse than no warning: it
/// sends the reader to check something that was never wrong.
pub const NO_MODEL_CHOSEN: &str =
    "Warning: No model selected. Use /model to select the model to ask.";

/// What it says when several providers are authenticated and none was chosen.
///
/// Authentication made models reachable; it did not choose which vendor may
/// receive the next prompt. `/model` is the explicit joint provider/model
/// choice, so telling somebody to log in again would name the wrong missing
/// thing.
pub const NO_PROVIDER_CHOSEN: &str =
    "Warning: No provider selected. Use /model to select a provider and model.";

/// Which of the two a session with no model has to say.
///
/// The provider by name rather than by entry, because the name is what
/// [`crate::Conversation::serving`] still holds by the time this is asked again.
pub const fn unasked(provider: Option<&str>) -> &'static str {
    match provider {
        Some(_) => NO_MODEL_CHOSEN,
        None => NOTHING_TO_ASK,
    }
}

/// The startup warning after credential discovery has distinguished zero from
/// several available providers.
pub const fn opening_unasked(provider: Option<Served>, any_credential: bool) -> &'static str {
    match (provider, any_credential) {
        (Some(_), _) => NO_MODEL_CHOSEN,
        (None, true) => NO_PROVIDER_CHOSEN,
        (None, false) => NOTHING_TO_ASK,
    }
}

/// The provider names, for the sentence a name outside them gets back.
pub fn names(providers: &Providers) -> String {
    offered(providers)
        .map(|one| one.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The rungs `model` serves, as far as this build knows.
///
/// All five for a name the table does not hold, which is the answer for every
/// model released since the build and every one typed rather than picked.
/// Nothing is known about it either way, and the choice is between offering a
/// rung its vendor may refuse and withholding one it serves — the first is a
/// sentence back from the vendor, the second is crucible deciding what a model
/// it has never heard of can do.
pub fn rungs(providers: &Providers, provider: &str, model: &str) -> Vec<Effort> {
    capabilities(providers, provider, model)
        .map_or_else(|| EVERY.to_vec(), |one| one.rungs().to_vec())
}

/// What one generation knows about `model` on `provider`, if it knows anything.
///
/// Read off the registry rather than off the table this build was compiled
/// with, which is what lets a provider registered at run time describe models
/// this build has never heard of in the same words the built-in ones use.
pub fn capabilities<'a>(
    providers: &'a Providers,
    provider: &str,
    model: &str,
) -> Option<&'a ModelCapabilities> {
    providers.find(provider).and_then(|arm| arm.model(model))
}

/// How [`re_serving`] reads one environment variable by name, or learns it is
/// not set.
pub type Lookup = Box<dyn Fn(&str) -> Option<String>>;

/// The [`Serving`] a provider is set up again through once a run is under
/// way: after a credential is stored, after one is forgotten, and when
/// `/model` names a provider other than the one answering.
///
/// Everything is owned rather than borrowed, because the closure is kept for
/// the length of the run and a borrow would put its lifetime into every
/// signature the closure is passed through. The settings are the files this
/// run already read: nothing in them grows with the transcript. `from` is the
/// environment lookup, handed in like every other source this crate reads, so
/// that a caller with no environment to offer can say so.
pub fn re_serving(settings: Settings, subscriptions: Subscriptions, from: Lookup) -> Serving {
    Box::new(move |named: Served, stored: &StoredCredentials| {
        let auth = startup::ProviderAuth {
            settings: &settings,
            from: &*from,
            stored,
            subscriptions: &subscriptions,
        };
        let source = credential_source(named, auth).ok_or_else(|| AppError::Authentication {
            provider: named.name.into(),
        })?;

        // A provider resolved here is one a credential was just found for, so
        // the sentence the `None` arm refuses with is never reached; it is
        // spelled the way the launch would spell it for this provider anyway.
        Ok(Resolved {
            provider: startup::provider(Some(named), unasked(Some(named.name)), auth)?,
            source,
        })
    })
}

/// Which provider to ask when the flag named none, or `None` where this machine
/// has nothing set up to ask.
///
/// A credential says a provider can be *reached*. Only a statement about
/// providers chooses one, and `provider` in the configuration is the only
/// statement there is — everything under `providers.<name>` is a subordinate
/// clause about a provider already being asked. The remembered statement is
/// active only while that provider remains reachable. Otherwise the session
/// opens with no provider so `/login` can repair it; falling through to a
/// different credential would send a turn to a vendor nobody chose.
///
/// Below it, exactly one provider holding a credential is that provider. That
/// is not a choice between competitors — it is the absence of anything to
/// choose, which is what lets a first run work with one credential and no
/// configuration at all. A stored account login counts beside an exported
/// variable and a written-down key: it is read through [`credential_source`],
/// the same answer construction would act on, so discovery and construction
/// cannot disagree about what this machine holds.
///
/// Several credentials and nothing choosing between them leaves the provider
/// open rather than failing the launch. `/model` settles both halves
/// explicitly; picking one here would send a turn to a vendor over the
/// declaration order, and refusing to start would strand a machine that is one
/// command away from a choice.
///
/// # Errors
///
/// [`AppError::Provider`] when the configuration names a provider nothing here
/// registers.
pub fn chosen(
    providers: &Providers,
    auth: startup::ProviderAuth<'_>,
) -> Result<Option<Served>, AppError> {
    // Refused here where a name this build has nothing for is a sentence naming
    // the ones it has, rather than carried as "nobody chose" into a session that
    // would then look set up by a credential nobody named.
    if let Some(named) = auth.settings.provider() {
        let one = served(providers, named)?;
        return Ok(credential_source(one, auth).is_some().then_some(one));
    }

    let mut holding = available(providers, auth);
    let (Some(first), second) = (holding.next(), holding.next()) else {
        return Ok(None);
    };
    Ok(second.is_none().then_some(first))
}

/// Every provider crucible holds a usable credential for, in declaration order.
///
/// Two places to look and one entry either way. A provider whose credential is
/// both exported and written down is one provider: listed twice it would be two
/// to the question above, and somebody who exported the key they had already
/// logged in with would be asked to choose between a provider and itself.
pub fn available<'a>(
    providers: &'a Providers,
    auth: startup::ProviderAuth<'a>,
) -> impl Iterator<Item = Served> + 'a {
    offered(providers).filter(move |one| credential_source(*one, auth).is_some())
}

/// The source provider construction will select, without reading a secret out.
///
/// The order is the one [`startup::provider`] resolves in, and the two must
/// agree: `/logout` names what remains after a stored credential is removed,
/// and a source this computes that construction would not select is a sentence
/// that lies.
pub fn credential_source(one: Served, auth: startup::ProviderAuth<'_>) -> Option<CredentialSource> {
    let startup::ProviderAuth {
        settings,
        from,
        stored,
        subscriptions,
    } = auth;
    if settings.base_url(one.name).is_none()
        && subscriptions.supports(one.name)
        && stored.has_subscription(one.name)
    {
        return Some(CredentialSource::Subscription);
    }
    let variable = settings.api_key_env(one.name).unwrap_or(one.key);
    if from(variable).is_some_and(|value| !value.trim().is_empty()) {
        return Some(CredentialSource::Environment(variable.into()));
    }
    if stored.has_key(one.name) {
        return Some(CredentialSource::StoredKey);
    }
    None
}

#[cfg(test)]
mod tests;
