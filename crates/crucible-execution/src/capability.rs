//! What a backend claims it can do, and how sure that claim is.
//!
//! Each [`Capability`] is claimed [`Claim::Supported`], [`Claim::Unsupported`]
//! or [`Claim::Unknown`], with the [`Scope`] the claim was learned at: a fact
//! of the driver, of the installed version, of one instance, or of one
//! session. Nothing is supported until something says so, so a fresh
//! [`Capabilities`] claims nothing. Confinement is claimed in the sandbox's
//! own three words, and its absence is unknown rather than unsupported.
//!
//! A claim read from a cache is history, not a fact about the program now
//! running: [`Capabilities::observed`] turns a cached observation into
//! [`Claim::Unknown`] whatever it says, so a stale model list or version probe
//! can never be the evidence an operation is admitted on.

use std::fmt;

/// One thing a backend may or may not be able to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Stopping a running turn.
    Cancel,
    /// Adding to a running turn without stopping it.
    Steer,
    /// Opening a session it ran before.
    Resume,
    /// Starting a new session from a point in an old one.
    Fork,
    /// Taking a session back to an earlier point.
    Rewind,
    /// Making room in a session's context.
    Compaction,
    /// Changing the model a session runs on.
    ModelSwitch,
    /// Asking the user a question and taking the answer.
    Questions,
    /// Allowing one action once.
    PermissionOnce,
    /// Allowing an action for the rest of the session.
    PermissionSession,
    /// Reporting what a turn used.
    Usage,
    /// Reporting how full a session's context is.
    ContextOccupancy,
    /// Reporting which account it is signed in as.
    AccountStatus,
    /// Signing out.
    Logout,
    /// Keeping its own state apart from another instance's.
    ProfileIsolation,
}

impl Capability {
    /// Every capability, in declaration order.
    pub const ALL: [Self; 15] = [
        Self::Cancel,
        Self::Steer,
        Self::Resume,
        Self::Fork,
        Self::Rewind,
        Self::Compaction,
        Self::ModelSwitch,
        Self::Questions,
        Self::PermissionOnce,
        Self::PermissionSession,
        Self::Usage,
        Self::ContextOccupancy,
        Self::AccountStatus,
        Self::Logout,
        Self::ProfileIsolation,
    ];

    /// Its name in a sentence.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cancel => "cancelling a turn",
            Self::Steer => "steering a turn",
            Self::Resume => "resuming a session",
            Self::Fork => "forking a session",
            Self::Rewind => "rewinding a session",
            Self::Compaction => "compaction",
            Self::ModelSwitch => "switching models",
            Self::Questions => "questions",
            Self::PermissionOnce => "allowing once",
            Self::PermissionSession => "allowing for the session",
            Self::Usage => "usage reporting",
            Self::ContextOccupancy => "context occupancy",
            Self::AccountStatus => "account status",
            Self::Logout => "signing out",
            Self::ProfileIsolation => "profile isolation",
        }
    }

    /// Where its claim is held in [`Capabilities`].
    const fn slot(self) -> usize {
        self as usize
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        form.write_str(self.name())
    }
}

/// Where a claim was learned, which is how far it can be trusted to reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// True of every instance the driver can make.
    Driver,
    /// True of the version of the program installed here.
    InstalledVersion,
    /// True of this instance.
    Instance,
    /// True of one session only.
    Session,
}

/// Whether a backend can do one thing, and where that was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Claim {
    /// It can.
    Supported(Scope),
    /// It cannot.
    Unsupported(Scope),
    /// Nothing has said, or what said so cannot be trusted now.
    Unknown,
}

impl Claim {
    /// Whether this claim admits the operation.
    #[must_use]
    pub const fn supported(self) -> bool {
        matches!(self, Self::Supported(_))
    }
}

/// How the actions a backend takes on this machine are held in, in the
/// sandbox's own three words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confinement {
    /// Nothing holds them in.
    Unsupported,
    /// They are watched, and nothing stops one that reaches too far.
    Observed,
    /// The platform refuses one that reaches too far.
    HardEnforced,
}

/// A confinement claim and where it was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Confined {
    /// How far actions are held in.
    level: Confinement,
    /// Where that was learned.
    scope: Scope,
}

impl Confined {
    /// Confinement `level`, learned at `scope`.
    #[must_use]
    pub const fn new(level: Confinement, scope: Scope) -> Self {
        Self { level, scope }
    }

    /// How far actions are held in.
    #[must_use]
    pub const fn level(self) -> Confinement {
        self.level
    }

    /// Where that was learned.
    #[must_use]
    pub const fn scope(self) -> Scope {
        self.scope
    }
}

/// Whether a reading is about now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Freshness {
    /// Read from the program as it runs.
    Live,
    /// Kept from an earlier reading.
    Cached,
}

/// A value somebody read, where it was read, and whether it is about now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Observation<T> {
    /// What was read.
    value: T,
    /// What it is true of.
    scope: Scope,
    /// Whether it was read now.
    freshness: Freshness,
}

impl<T> Observation<T> {
    /// `value`, read just now from the running program, true of `scope`.
    pub const fn live(value: T, scope: Scope) -> Self {
        Self {
            value,
            scope,
            freshness: Freshness::Live,
        }
    }

    /// `value`, kept from an earlier reading, said to be true of `scope`.
    pub const fn cached(value: T, scope: Scope) -> Self {
        Self {
            value,
            scope,
            freshness: Freshness::Cached,
        }
    }

    /// What was read.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// What it is true of.
    pub const fn scope(&self) -> Scope {
        self.scope
    }

    /// Whether it was read now.
    pub const fn freshness(&self) -> Freshness {
        self.freshness
    }
}

/// Every claim a backend makes, one per [`Capability`], and its confinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// The claim for each capability, at its [`Capability::slot`].
    claims: [Claim; Capability::ALL.len()],
    /// How far its actions are held in, or nothing where nothing has said.
    confinement: Option<Confined>,
}

impl Capabilities {
    /// Claims of nothing: every capability unknown, confinement unknown.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            claims: [Claim::Unknown; Capability::ALL.len()],
            confinement: None,
        }
    }

    /// These claims, with `capability` claimed as `claim`.
    #[must_use]
    pub fn with(mut self, capability: Capability, claim: Claim) -> Self {
        if let Some(held) = self.claims.get_mut(capability.slot()) {
            *held = claim;
        }
        self
    }

    /// These claims, with `capability` claimed from `observation`.
    ///
    /// A live reading is supported or unsupported at its own scope. A cached
    /// one is unknown whatever it says: what a program could do when somebody
    /// last asked is no evidence of what the one running now can.
    #[must_use]
    pub fn observed(self, capability: Capability, observation: Observation<bool>) -> Self {
        let claim = match (observation.freshness(), *observation.value()) {
            (Freshness::Cached, _) => Claim::Unknown,
            (Freshness::Live, true) => Claim::Supported(observation.scope()),
            (Freshness::Live, false) => Claim::Unsupported(observation.scope()),
        };
        self.with(capability, claim)
    }

    /// These claims, with confinement claimed as `confined`.
    #[must_use]
    pub const fn confined(mut self, confined: Confined) -> Self {
        self.confinement = Some(confined);
        self
    }

    /// What is claimed for `capability`.
    #[must_use]
    pub fn claim(&self, capability: Capability) -> Claim {
        self.claims
            .get(capability.slot())
            .copied()
            .unwrap_or(Claim::Unknown)
    }

    /// Whether `capability` is claimed as supported.
    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.claim(capability).supported()
    }

    /// How far its actions are held in, or nothing where nothing has said.
    #[must_use]
    pub const fn confinement(&self) -> Option<Confined> {
        self.confinement
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::unknown()
    }
}
