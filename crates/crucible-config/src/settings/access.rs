//! How each provider is reached, and the installed agents it may be reached
//! through, read back as types.
//!
//! Both blocks are read only from the configuration file in the user's home
//! directory, which the shape enforces. What is read here is a statement and
//! not a route: whether the credential it names is there, and whether it can
//! be used at the address configured, is decided by the application's one
//! resolver, which reads these types and nothing else of the two blocks.
//!
//! An instance of a built-in driver serves the one provider its driver signs
//! in to. A catalog instance serves the catalog agent its `agent` key names,
//! and no provider. Configuration cannot read the catalog, so it checks the
//! key's spelling alone: never a driver's word, the native runner's name or a
//! name Windows keeps for a device.
//!
//! The document has already been walked against the shape and checked, so a
//! value that reaches these readers is one of the spellings below; a reader
//! that met anything else would be reading a document that was never checked,
//! and answers as if the key were absent.

use std::collections::BTreeMap;
use std::fmt;

use crucible_models::Effort;
use crucible_types::AgentKey;
use serde_json::Value;

use super::Settings;
use crate::ConfigError;
use crate::document::{Document, Origin};

/// The spellings `access.type` accepts, in the order a reader is told them.
pub(crate) const KINDS: &[&str] = &[
    Kind::Subscription.name(),
    Kind::ApiKey.name(),
    Kind::ExternalAgent.name(),
];

/// The one `access.type` that is reached through an instance.
pub(crate) const EXTERNAL: &str = Kind::ExternalAgent.name();

/// The spellings `externalAgents.<instance>.driver` accepts.
pub(crate) const DRIVERS: &[&str] = &[
    Driver::Claude.name(),
    Driver::Antigravity.name(),
    Driver::Catalog.name(),
];

/// What `externalAgents.<instance>.agent` is spelled with, as an editor
/// checks it.
///
/// [`AgentKey`] is what refuses one, and [`reserved`] what refuses a word a
/// key never takes; the test beside the schema holds this pattern to the
/// same alphabet and the same length.
pub(crate) const AGENT: &str = "^[a-z0-9-]{1,32}$";

/// The name of the runner that answers a conversation with a provider's own
/// model, which a session it ran is listed under, so no catalog agent's key
/// may be spelled as it.
///
/// The session store spells it too; a test in the application, which reads
/// both, holds the two to one word.
pub const NATIVE_RUNNER: &str = "native";

/// The names Windows keeps for its devices, which no directory there can
/// have: a key names the directory its agent's releases are kept in.
const DEVICES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The spellings `externalAgents.<instance>.authentication` accepts: the
/// agent's own account login, the one way this build knows.
pub(crate) const AUTHENTICATIONS: &[&str] = &["account"];

/// The most agent instances one document may name.
///
/// Far beyond what somebody configures by hand, and small enough that a
/// block that grew by accident is refused by name rather than walked.
pub(crate) const INSTANCES: usize = 32;

/// How one provider is reached, as `providers.<name>.access` states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// The account `/login` signed in, at the vendor's own address.
    Subscription,
    /// An API key: the variable's, then the one `/login` stored.
    ApiKey,
    /// The installed agent named under `externalAgents`.
    ExternalAgent {
        /// The `externalAgents` entry it is reached through.
        instance: Box<str>,
    },
}

impl Access {
    /// The `access` value that states this, as the user file spells it.
    pub(crate) fn written(&self) -> Value {
        match self {
            Self::Subscription => serde_json::json!({"type": Kind::Subscription.name()}),
            Self::ApiKey => serde_json::json!({"type": Kind::ApiKey.name()}),
            Self::ExternalAgent { instance } => serde_json::json!({
                "type": Kind::ExternalAgent.name(),
                "instance": instance,
            }),
        }
    }
}

/// The discriminant `access.type` is spelled with.
///
/// Its own type so the spellings the shape declares and the ones [`Access`]
/// is read from are one list.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Subscription,
    ApiKey,
    ExternalAgent,
}

impl Kind {
    const ALL: [Self; 3] = [Self::Subscription, Self::ApiKey, Self::ExternalAgent];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::ApiKey => "apiKey",
            Self::ExternalAgent => "externalAgent",
        }
    }
}

/// Which agent an instance is, and so how crucible speaks to it.
///
/// A built-in driver is one agent. [`Driver::Catalog`] is any agent the
/// catalog reviewed, and which one an instance of it runs is the key its
/// `agent` names, so it is answered by [`Driver::serves`] given that key and
/// never by the driver alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Driver {
    /// Claude Code.
    Claude,
    /// Antigravity.
    Antigravity,
    /// An agent the catalog reviewed, named by its key.
    Catalog,
}

impl Driver {
    /// Every driver this build declares, in the order the shape lists them.
    pub const ALL: [Self; 3] = [Self::Claude, Self::Antigravity, Self::Catalog];

    /// How the driver is spelled in a configuration file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Antigravity => "antigravity",
            Self::Catalog => "catalog",
        }
    }

    /// The key an instance of this driver names its profile directory with,
    /// where it names one.
    ///
    /// Each agent calls the directory it keeps its sign-in under by its own
    /// name, and a key that meant one thing to one agent and another to the
    /// next would be read by neither. Two drivers share a key only where it
    /// means the same to both: a catalog agent's is Antigravity's.
    #[must_use]
    pub const fn profile_key(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("configDir"),
            Self::Antigravity | Self::Catalog => Some("profileDir"),
        }
    }

    /// Whether an instance must name its profile directory: one whose usual
    /// home another program of the same vendor keeps its own sign-in in would
    /// otherwise share it.
    #[must_use]
    pub const fn profile_needed(self) -> bool {
        match self {
            Self::Claude | Self::Catalog => false,
            Self::Antigravity => true,
        }
    }

    /// Whether it is started with a model `externalAgents.<name>.model`
    /// names. An agent that is not chooses its own, and a model written for it
    /// would be one nothing reads.
    #[must_use]
    pub const fn takes_model(self) -> bool {
        match self {
            Self::Claude => true,
            Self::Antigravity | Self::Catalog => false,
        }
    }

    /// What an instance of it serves, given the catalog agent its entry
    /// names where it names one: a built-in driver's agent signs in to its
    /// own vendor and asks only that vendor, so it serves that one provider
    /// whatever else is written; a catalog instance serves the agent its key
    /// names, and with no key it serves nothing.
    #[must_use]
    pub fn serves(self, agent: Option<&AgentKey>) -> Option<Serves> {
        match self {
            Self::Claude => Some(Serves::Provider("anthropic")),
            Self::Antigravity => Some(Serves::Provider("google")),
            Self::Catalog => agent.cloned().map(Serves::Agent),
        }
    }

    fn of(written: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|one| one.name() == written)
    }
}

/// What a catalog agent's key is never spelled as, and why, where `word` is
/// one: a built-in driver's word or the native runner's name, which routes,
/// sessions and credentials are already named by, or a name Windows keeps for
/// a device, which no directory can be named.
#[must_use]
pub(crate) fn reserved(word: &str) -> Option<&'static str> {
    if Driver::ALL.iter().any(|driver| driver.name() == word) {
        Some("a driver's word")
    } else if word == NATIVE_RUNNER {
        Some("the native runner's name")
    } else if DEVICES.contains(&word) {
        Some("a name Windows keeps for a device")
    } else {
        None
    }
}

/// Why `written` cannot be a catalog agent's key, as a refusal says it;
/// nothing where it can.
#[must_use]
pub(crate) fn unkeyed(written: &str) -> Option<String> {
    match AgentKey::new(written) {
        Err(refused) => Some(refused.to_string()),
        Ok(_) => reserved(written)
            .map(|what| format!("{written:?} is {what}, so it is never a catalog agent's key")),
    }
}

/// `written` as a catalog agent's key, where it can be one.
fn keyed(written: &str) -> Option<AgentKey> {
    AgentKey::new(written)
        .ok()
        .filter(|key| reserved(key.as_str()).is_none())
}

/// What an installed agent serves: the turns of one provider of this
/// build's, or the catalog agent it is, which reaches none of them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Serves {
    /// The one provider it signs in to and asks, by the name `providers`
    /// gives it: no other provider is reached through it.
    Provider(&'static str),
    /// The catalog agent of this key, which signs in to its own vendor and
    /// chooses its own model, so no provider of this build's is reached
    /// through it.
    Agent(AgentKey),
}

impl Serves {
    /// The provider it serves, where it serves one.
    #[must_use]
    pub const fn provider(&self) -> Option<&'static str> {
        match self {
            Self::Provider(provider) => Some(provider),
            Self::Agent(_) => None,
        }
    }

    /// The catalog agent it is, where it is one.
    #[must_use]
    pub const fn agent(&self) -> Option<&AgentKey> {
        match self {
            Self::Provider(_) => None,
            Self::Agent(key) => Some(key),
        }
    }
}

/// The provider by the name `providers` gives it, or the catalog agent by its
/// key.
impl fmt::Display for Serves {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(provider) => out.write_str(provider),
            Self::Agent(key) => out.write_str(key.as_str()),
        }
    }
}

/// The most characters a model an agent is started with is named in.
pub(crate) const MODEL_CHARS: usize = 128;

/// What [`model_name`] holds a model to, as a refusal says it.
pub const MODEL_NAMED: &str = "a model is named with letters, digits and . _ - : @ / [ ] only, \
     at most 128 of them, and never starting with -, since it is handed to the agent's program \
     as an argument";

/// Whether `found` names a model an agent can be started with: letters,
/// digits and `. _ - : @ / [ ]`, at most 128 of them, and never a
/// leading `-`, since it is handed to the agent's program as an argument of
/// its own and must never be read there as an option.
#[must_use]
pub fn model_name(found: &str) -> bool {
    !found.is_empty()
        && found.len() <= MODEL_CHARS
        && !found.starts_with('-')
        && found
            .chars()
            .all(|one| one.is_ascii_alphanumeric() || "._-:@/[]".contains(one))
}

/// One installed agent, as the document states it.
///
/// Nothing in it has been resolved: `executable` has not been looked for and
/// `profile` names a directory nothing has opened. No field is a secret.
/// A catalog instance always names its agent, and no other instance names
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgent {
    name: Box<str>,
    enabled: bool,
    driver: Driver,
    /// What it serves, as its driver answers given its `agent`: held rather
    /// than asked again, so it is settled once, where the instance is made.
    serves: Serves,
    executable: Box<str>,
    profile: Option<Box<str>>,
    model: Option<Box<str>>,
    effort: Option<Effort>,
}

impl ExternalAgent {
    /// The name `access.instance` uses for it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether new work may be sent through it.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Which agent it is.
    #[must_use]
    pub const fn driver(&self) -> Driver {
        self.driver
    }

    /// The catalog agent its `agent` names, where it is a catalog instance.
    #[must_use]
    pub const fn agent(&self) -> Option<&AgentKey> {
        self.serves.agent()
    }

    /// What it serves: its driver's provider, or the catalog agent it is.
    #[must_use]
    pub fn serves(&self) -> Serves {
        self.serves.clone()
    }

    /// Its program: an absolute path, or a bare name for `PATH` to answer.
    #[must_use]
    pub fn executable(&self) -> &str {
        &self.executable
    }

    /// The profile directory its driver's key names, where one is written.
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// The model it is started with, as its provider names it, where one is
    /// written; otherwise the provider's default.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The effort it is started at, where one is written; otherwise the
    /// model's default rung.
    #[must_use]
    pub const fn effort(&self) -> Option<Effort> {
        self.effort
    }

    /// An instance named `name` of `driver`, running the catalog agent
    /// `agent` where it is a catalog instance, run as `executable` with its
    /// profile directory at `profile`, as a configuration that has not been
    /// written yet would state it, taking no new work until it is.
    ///
    /// Nothing where the two disagree: a catalog instance names the agent it
    /// runs, and an instance of a built-in driver names none.
    #[must_use]
    pub fn unwritten(
        name: &str,
        driver: Driver,
        executable: &str,
        profile: Option<&str>,
        agent: Option<&AgentKey>,
    ) -> Option<Self> {
        if agent.is_some() != (driver == Driver::Catalog) {
            return None;
        }
        Some(Self {
            name: name.into(),
            enabled: false,
            driver,
            serves: driver.serves(agent)?,
            executable: executable.into(),
            profile: profile.map(Into::into),
            model: None,
            effort: None,
        })
    }

    fn read(name: &str, record: &Value) -> Option<Self> {
        let driver = Driver::of(record.get("driver")?.as_str()?)?;
        // An `agent` beside a built-in driver is one the check refused, and is
        // read as if it were not written.
        let agent = match driver {
            Driver::Catalog => Some(keyed(record.get("agent")?.as_str()?)?),
            Driver::Claude | Driver::Antigravity => None,
        };
        Some(Self {
            name: name.into(),
            enabled: record
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            driver,
            serves: driver.serves(agent.as_ref())?,
            executable: record.get("executable")?.as_str()?.into(),
            profile: driver
                .profile_key()
                .and_then(|key| record.get(key))
                .and_then(Value::as_str)
                .map(Into::into),
            model: record.get("model").and_then(Value::as_str).map(Into::into),
            effort: record
                .get("effort")
                .and_then(Value::as_str)
                .and_then(|effort| effort.parse().ok()),
        })
    }
}

/// What the user file says of the installed agents and of how each provider
/// is reached: the two blocks only that file may write, which an account
/// write during the run changes in it.
///
/// Held whole, as the file states them once a write is done, so that a run
/// reading them in place of what its launch read sees every instance
/// installed, enabled, disabled, given a model or removed, and every route
/// written or taken away, and not only the last change.
#[derive(Debug, Clone, PartialEq)]
pub struct Routes {
    /// The `externalAgents` block, where the file has one.
    agents: Option<Value>,
    /// Each provider's `access`, by provider.
    access: BTreeMap<String, Value>,
}

impl Routes {
    /// The two blocks as `text`, the user file `file`, states them.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] where `text` is not configuration the user file may
    /// hold.
    pub fn of(text: &str, file: &str) -> Result<Self, ConfigError> {
        let checked = if text.trim().is_empty() { "{}" } else { text };
        let document = Document::parse(checked, file, Origin::User)?;
        let value = document.value();
        let access = value
            .get("providers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(provider, block)| Some((provider.clone(), block.get("access")?.clone())))
            .collect();
        Ok(Self {
            agents: value.get("externalAgents").cloned(),
            access,
        })
    }
}

impl Settings {
    /// How `provider` is reached, or `None` where its block does not say —
    /// which is every configuration written before the key existed.
    #[must_use]
    pub fn access(&self, provider: &str) -> Option<Access> {
        let block = self.value.get("providers")?.get(provider)?.get("access")?;
        let written = block.get("type")?.as_str()?;
        match Kind::ALL.into_iter().find(|kind| kind.name() == written)? {
            Kind::Subscription => Some(Access::Subscription),
            Kind::ApiKey => Some(Access::ApiKey),
            Kind::ExternalAgent => Some(Access::ExternalAgent {
                instance: block.get("instance")?.as_str()?.into(),
            }),
        }
    }

    /// These settings, with the installed agents and how each provider is
    /// reached as `routes` states them: what the account writes during the
    /// run left in the user file, read in place of what the launch read,
    /// since the files are not read again. Both blocks are read only from
    /// that file, so what it states of them is all there is.
    #[must_use]
    pub fn routed(&self, routes: &Routes) -> Self {
        let mut settings = self.clone();
        let Some(document) = object(&mut settings.value) else {
            return settings;
        };
        match &routes.agents {
            Some(agents) => {
                document.insert("externalAgents".into(), agents.clone());
            }
            None => {
                document.remove("externalAgents");
            }
        }
        if let Some(providers) = document.get_mut("providers").and_then(Value::as_object_mut) {
            for block in providers.values_mut().filter_map(Value::as_object_mut) {
                block.remove("access");
            }
        }
        if !routes.access.is_empty()
            && let Some(providers) = object(document.entry("providers").or_insert(Value::Null))
        {
            for (provider, access) in &routes.access {
                let block = providers.entry(provider.as_str()).or_insert(Value::Null);
                if let Some(block) = object(block) {
                    block.insert("access".into(), access.clone());
                }
            }
        }
        settings
    }

    /// The first provider whose `access` reaches it through the agent
    /// instance `instance`, in the order the document names providers.
    #[must_use]
    pub fn referencing(&self, instance: &str) -> Option<String> {
        referencing(&self.value, instance)
    }

    /// Every agent instance the document names, in the order it names them.
    ///
    /// Empty where none is written, which is the ordinary machine.
    #[must_use]
    pub fn external_agents(&self) -> Vec<ExternalAgent> {
        let Some(instances) = self.value.get("externalAgents").and_then(Value::as_object) else {
            return Vec::new();
        };
        instances
            .iter()
            .filter_map(|(name, record)| ExternalAgent::read(name, record))
            .collect()
    }
}

/// `value` as an object, made an empty one where it is anything else.
fn object(value: &mut Value) -> Option<&mut serde_json::Map<String, Value>> {
    if !value.is_object() {
        *value = Value::Object(serde_json::Map::new());
    }
    value.as_object_mut()
}

#[cfg(test)]
mod tests;

/// The first provider in `document` whose `access` reaches it through the
/// agent instance `instance`.
pub(crate) fn referencing(document: &Value, instance: &str) -> Option<String> {
    let providers = document.get("providers")?.as_object()?;
    providers.iter().find_map(|(provider, block)| {
        let access = block.get("access")?;
        let through = access.get("type")?.as_str() == Some(Kind::ExternalAgent.name())
            && access.get("instance")?.as_str() == Some(instance);
        through.then(|| provider.clone())
    })
}
