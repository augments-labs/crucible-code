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

/// The discriminant `access.type` is spelled with.
///
/// Its own type so the spellings the shape declares and the ones [`Access`]
/// is read from are one list.
#[derive(Clone, Copy)]
enum Kind {
    Subscription,
    ApiKey,
    ExternalAgent,
}

impl Kind {
    const ALL: [Self; 3] = [Self::Subscription, Self::ApiKey, Self::ExternalAgent];

    const fn name(self) -> &'static str {
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

    fn of(written: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|one| one.name() == written)
    }
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

#[cfg(test)]
mod tests;
