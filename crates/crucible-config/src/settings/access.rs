//! How each provider is reached, and the installed agents it may be reached
//! through, read back as types.
//!
//! Both blocks are read only from the configuration file in the user's home
//! directory, which the shape enforces. What is read here is a statement and
//! not a route: whether the credential it names is there, and whether it can
//! be used at the address configured, is decided by the application's one
//! resolver, which reads these types and nothing else of the two blocks.
//!
//! The document has already been walked against the shape and checked, so a
//! value that reaches these readers is one of the spellings below; a reader
//! that met anything else would be reading a document that was never checked,
//! and answers as if the key were absent.

use crucible_models::Effort;
use serde_json::Value;

use super::Settings;

/// The spellings `access.type` accepts, in the order a reader is told them.
pub(crate) const KINDS: &[&str] = &[
    Kind::Subscription.name(),
    Kind::ApiKey.name(),
    Kind::ExternalAgent.name(),
];

/// The one `access.type` that is reached through an instance.
pub(crate) const EXTERNAL: &str = Kind::ExternalAgent.name();

/// The spellings `externalAgents.<instance>.driver` accepts.
pub(crate) const DRIVERS: &[&str] = &[Driver::Claude.name(), Driver::Antigravity.name()];

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Driver {
    /// Claude Code.
    Claude,
    /// Antigravity.
    Antigravity,
}

impl Driver {
    /// Every driver this build declares, in the order the shape lists them.
    pub const ALL: [Self; 2] = [Self::Claude, Self::Antigravity];

    /// How the driver is spelled in a configuration file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Antigravity => "antigravity",
        }
    }

    /// The key an instance of this driver names its profile directory with.
    ///
    /// One key per driver because each agent calls the directory it keeps its
    /// sign-in under by its own name, and a key that meant one thing to one
    /// agent and another to the next would be read by neither.
    #[must_use]
    pub const fn profile_key(self) -> &'static str {
        match self {
            Self::Claude => "configDir",
            Self::Antigravity => "profileDir",
        }
    }

    /// Whether an instance must name its profile directory: one whose usual
    /// home another program of the same vendor keeps its own sign-in in would
    /// otherwise share it.
    #[must_use]
    pub const fn profile_needed(self) -> bool {
        match self {
            Self::Claude => false,
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
            Self::Antigravity => false,
        }
    }

    /// The one provider it reaches, by the name `providers` gives it: the
    /// agent signs in to its own vendor and asks only that vendor, so no
    /// other provider is reached through it.
    #[must_use]
    pub const fn provider(self) -> &'static str {
        match self {
            Self::Claude => "anthropic",
            Self::Antigravity => "google",
        }
    }

    fn of(written: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|one| one.name() == written)
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgent {
    name: Box<str>,
    enabled: bool,
    driver: Driver,
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

    /// An instance named `name` of `driver`, run as `executable` with its
    /// profile directory at `profile`, as a configuration that has not been
    /// written yet would state it, taking no new work until it is.
    #[must_use]
    pub fn unwritten(name: &str, driver: Driver, executable: &str, profile: Option<&str>) -> Self {
        Self {
            name: name.into(),
            enabled: false,
            driver,
            executable: executable.into(),
            profile: profile.map(Into::into),
            model: None,
            effort: None,
        }
    }

    fn read(name: &str, record: &Value) -> Option<Self> {
        let driver = Driver::of(record.get("driver")?.as_str()?)?;
        Some(Self {
            name: name.into(),
            enabled: record
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            driver,
            executable: record.get("executable")?.as_str()?.into(),
            profile: record
                .get(driver.profile_key())
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

    /// These settings, with `provider` reached as `access` states: what a
    /// login during the run rewrote the user file to, read in place of what
    /// the launch read, since the files are not read again.
    #[must_use]
    pub fn reaching(&self, provider: &str, access: &Access) -> Self {
        let mut reaching = self.clone();
        let block = object(&mut reaching.value)
            .map(|document| document.entry("providers").or_insert(Value::Null))
            .and_then(object)
            .map(|providers| providers.entry(provider).or_insert(Value::Null))
            .and_then(object);
        if let Some(block) = block {
            block.insert("access".into(), access.written());
        }
        reaching
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
