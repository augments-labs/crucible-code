//! What an extension says it is, read before any of it is run.
//!
//! An extension is somebody else's program. Discovering one must not start it,
//! so everything here is inert: a manifest is a record parsed from bounded
//! text, and holding one grants nothing. What it may do is decided afterwards,
//! by a trust decision this crate does not make and a host that hands out
//! capabilities one at a time.
//!
//! Two halves, because they are answered by different parties. An
//! [`ExtensionIdentity`] is what the extension is and where the bytes came from, which
//! is the half a trust decision is made against. [`ExtensionRequests`] is what it asks
//! of the host, which is the half a capability grant is made against. A
//! manifest that asks for nothing is legal; a manifest that promises what it
//! never asked for is not, and is refused here rather than part-way through a
//! registration.

use crate::registry::SourceKind;

/// The most bytes an extension identifier may retain.
///
/// Generous next to `vendor.plugin` and small enough that a manifest carrying
/// a document where a name belongs is refused at the boundary rather than kept
/// for the life of the process.
pub const EXTENSION_ID_BYTES: usize = 128;

/// The most bytes any other retained spelling in a manifest may hold.
pub const EXTENSION_TEXT_BYTES: usize = 512;

/// The most capabilities or contributions one manifest may name.
pub const EXTENSION_REQUESTS: usize = 16;

/// Why a manifest could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtensionError {
    /// A retained spelling was empty.
    #[error("{field} must not be empty")]
    Empty {
        /// Which field.
        field: &'static str,
    },
    /// A retained spelling crossed its boundary.
    #[error("{field} is {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Which field.
        field: &'static str,
        /// Its boundary.
        maximum: usize,
        /// What was supplied.
        actual: usize,
    },
    /// An identifier was not source-qualified.
    #[error("{id} is not a source-qualified identifier such as vendor.plugin")]
    Unqualified {
        /// What was supplied.
        id: Box<str>,
    },
    /// A list crossed its boundary.
    #[error("{field} names {actual}; the maximum is {maximum}")]
    TooMany {
        /// Which list.
        field: &'static str,
        /// Its boundary.
        maximum: usize,
        /// What was supplied.
        actual: usize,
    },
    /// A list named the same thing twice.
    #[error("{id} names {what} twice in {field}")]
    Repeated {
        /// The extension that named it.
        id: Box<str>,
        /// Which list.
        field: &'static str,
        /// What was repeated.
        what: &'static str,
    },
    /// A contribution was promised without the capability that permits it.
    #[error("{id} contributes {what} without requesting {needs}")]
    Unasked {
        /// The extension that promised it.
        id: Box<str>,
        /// What was promised.
        what: &'static str,
        /// The capability it would need.
        needs: &'static str,
    },
}

/// One thing a host will let an extension do, asked for by name.
///
/// Asked for, never assumed: an extension that does not name a capability
/// cannot be granted it later by configuration, because configuration may
/// narrow what was asked and never widen it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtensionCapability {
    /// Register tools and toolsets.
    RegisterTools,
    /// Register commands.
    RegisterCommands,
    /// Observe immutable lifecycle events.
    ObserveLifecycle,
    /// Read a restricted view of the run and session.
    ReadRunContext,
    /// Append namespaced state to the session.
    WriteSessionState,
    /// Contribute skills, prompts and resources.
    ContributeSkills,
    /// Ask the person running crucible to select, confirm or answer.
    AskTheOperator,
}

impl ExtensionCapability {
    /// The configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegisterTools => "registerTools",
            Self::RegisterCommands => "registerCommands",
            Self::ObserveLifecycle => "observeLifecycle",
            Self::ReadRunContext => "readRunContext",
            Self::WriteSessionState => "writeSessionState",
            Self::ContributeSkills => "contributeSkills",
            Self::AskTheOperator => "askTheOperator",
        }
    }
}

/// What an extension declares it contributes.
///
/// Separate from the capability that permits it because the two answer
/// different questions: a capability is what the host must allow, and a
/// contribution is what a registration will later carry. A host reads the
/// second to know which registries to stage, and refuses a manifest whose two
/// halves disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtensionContribution {
    /// Tools and toolsets.
    Tools,
    /// Commands.
    Commands,
    /// Skills, prompts and resources.
    Skills,
    /// Middleware and policy handlers.
    Policy,
}

impl ExtensionContribution {
    /// The configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Commands => "commands",
            Self::Skills => "skills",
            Self::Policy => "policy",
        }
    }

    /// The capability a host must grant before this may be staged.
    #[must_use]
    pub const fn needs(self) -> ExtensionCapability {
        match self {
            Self::Tools => ExtensionCapability::RegisterTools,
            Self::Commands => ExtensionCapability::RegisterCommands,
            Self::Skills => ExtensionCapability::ContributeSkills,
            Self::Policy => ExtensionCapability::ObserveLifecycle,
        }
    }
}

/// A protocol version, agreed before a word is exchanged.
///
/// Major is compatibility and minor is reach: a different major is two
/// programs that cannot speak, and a lower minor on either side is the pair
/// agreeing to the smaller vocabulary they both know. Nothing here reads a
/// patch level, because a wire protocol that needs one has changed its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtensionProtocol {
    major: u16,
    minor: u16,
}

impl ExtensionProtocol {
    /// A version.
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// The compatibility half.
    #[must_use]
    pub const fn major(self) -> u16 {
        self.major
    }

    /// The reach half.
    #[must_use]
    pub const fn minor(self) -> u16 {
        self.minor
    }

    /// What this and `host` can both speak, where they can speak at all.
    ///
    /// `None` is a different major, which is the honest answer rather than an
    /// attempt at the subset: two programs disagreeing about the shape of a
    /// frame have no smaller vocabulary in common to fall back to.
    #[must_use]
    pub const fn agreed(self, host: Self) -> Option<Self> {
        if self.major != host.major {
            return None;
        }
        Some(Self {
            major: self.major,
            minor: if self.minor < host.minor {
                self.minor
            } else {
                host.minor
            },
        })
    }
}

/// What an extension is, and where its bytes came from.
///
/// The half a trust decision is made against. `digest` is over the bytes as
/// they were read, so a manifest that has changed since it was trusted is a
/// different manifest and gets asked again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionIdentity {
    /// The source-qualified identifier, such as `vendor.plugin`.
    pub id: Box<str>,
    /// The extension's own version, spelled however its author spells it.
    pub version: Box<str>,
    /// What would be started, resolved by the host and not by this record.
    pub entrypoint: Box<str>,
    /// A digest over the manifest bytes as they were read.
    pub digest: Box<str>,
    /// Where the manifest was found.
    pub found: SourceKind,
}

/// What an extension asks of the host.
///
/// The half a capability grant is made against. Every list here is what was
/// asked for and never what was allowed: a grant is narrower than this or
/// equal to it, and the host owns that decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionRequests {
    /// The protocol version its author wrote it against.
    pub protocol: ExtensionProtocol,
    /// The oldest crucible it says it works with.
    pub minimum: Box<str>,
    /// What it asks the host to let it do.
    pub capabilities: Box<[ExtensionCapability]>,
    /// What it says it will contribute.
    pub contributions: Box<[ExtensionContribution]>,
}

/// One extension's manifest, parsed and holding nothing that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionManifest {
    identity: ExtensionIdentity,
    requests: ExtensionRequests,
}

impl ExtensionManifest {
    /// Reads one manifest.
    ///
    /// # Errors
    ///
    /// [`ExtensionError`] where a spelling is empty or over its boundary,
    /// where the identifier is not source-qualified, where a list is over its
    /// boundary or names the same thing twice, or where a contribution is
    /// promised without the capability that permits it. That last one is
    /// refused here rather than at registration: a manifest whose two halves
    /// disagree would otherwise be trusted, started, and only then found to be
    /// staging something nobody agreed to.
    pub fn read(
        identity: ExtensionIdentity,
        requests: ExtensionRequests,
    ) -> Result<Self, ExtensionError> {
        qualified(&identity.id)?;
        bounded("extension version", &identity.version)?;
        bounded("entrypoint", &identity.entrypoint)?;
        bounded("digest", &identity.digest)?;
        bounded("minimum version", &requests.minimum)?;

        counted("capabilities", requests.capabilities.len())?;
        counted("contributions", requests.contributions.len())?;

        if let Some(repeat) = repeated(&requests.capabilities, ExtensionCapability::as_str) {
            return Err(ExtensionError::Repeated {
                id: identity.id,
                field: "capabilities",
                what: repeat,
            });
        }
        if let Some(repeat) = repeated(&requests.contributions, ExtensionContribution::as_str) {
            return Err(ExtensionError::Repeated {
                id: identity.id,
                field: "contributions",
                what: repeat,
            });
        }

        for contribution in &requests.contributions {
            let needs = contribution.needs();
            if !requests.capabilities.contains(&needs) {
                return Err(ExtensionError::Unasked {
                    id: identity.id,
                    what: contribution.as_str(),
                    needs: needs.as_str(),
                });
            }
        }

        Ok(Self { identity, requests })
    }

    /// What it is, and where its bytes came from.
    #[must_use]
    pub const fn identity(&self) -> &ExtensionIdentity {
        &self.identity
    }

    /// What it asks of the host.
    #[must_use]
    pub const fn requests(&self) -> &ExtensionRequests {
        &self.requests
    }

    /// The source-qualified identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.identity.id
    }

    /// Whether it asked for this, which is not whether it was granted it.
    #[must_use]
    pub fn asked_for(&self, capability: ExtensionCapability) -> bool {
        self.requests.capabilities.contains(&capability)
    }

    /// The bytes this manifest keeps alive.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.identity
            .id
            .len()
            .saturating_add(self.identity.version.len())
            .saturating_add(self.identity.entrypoint.len())
            .saturating_add(self.identity.digest.len())
            .saturating_add(self.requests.minimum.len())
            .saturating_add(size_of_val(&*self.requests.capabilities))
            .saturating_add(size_of_val(&*self.requests.contributions))
    }
}

/// Whether an identifier names who it came from as well as what it is.
///
/// A bare name collides across vendors, and the collision is settled by
/// whichever was registered first — which is a silent way for one author's
/// extension to answer for another's. The separator is the whole requirement;
/// what is either side of it belongs to whoever publishes it.
fn qualified(id: &str) -> Result<(), ExtensionError> {
    bounded_to("extension id", id, EXTENSION_ID_BYTES)?;
    let mut halves = id.split('.');
    let named = halves.next().is_some_and(|half| !half.is_empty())
        && halves.next().is_some_and(|half| !half.is_empty());
    if named {
        Ok(())
    } else {
        Err(ExtensionError::Unqualified { id: id.into() })
    }
}

/// One retained spelling, held to [`EXTENSION_TEXT_BYTES`].
fn bounded(field: &'static str, value: &str) -> Result<(), ExtensionError> {
    bounded_to(field, value, EXTENSION_TEXT_BYTES)
}

/// One retained spelling, held to its own boundary.
fn bounded_to(field: &'static str, value: &str, maximum: usize) -> Result<(), ExtensionError> {
    if value.is_empty() {
        return Err(ExtensionError::Empty { field });
    }
    if value.len() > maximum {
        return Err(ExtensionError::TooLong {
            field,
            maximum,
            actual: value.len(),
        });
    }
    Ok(())
}

/// One list, held to [`EXTENSION_REQUESTS`].
fn counted(field: &'static str, actual: usize) -> Result<(), ExtensionError> {
    if actual > EXTENSION_REQUESTS {
        return Err(ExtensionError::TooMany {
            field,
            maximum: EXTENSION_REQUESTS,
            actual,
        });
    }
    Ok(())
}

/// The first entry these name twice, where they name one twice.
///
/// Quadratic over a list [`EXTENSION_REQUESTS`] long, which is smaller than
/// the allocation any cleverer answer would need.
fn repeated<T: Copy + PartialEq>(
    listed: &[T],
    spelled: fn(T) -> &'static str,
) -> Option<&'static str> {
    listed.iter().enumerate().find_map(|(at, one)| {
        listed
            .get(at.saturating_add(1)..)?
            .contains(one)
            .then(|| spelled(*one))
    })
}

#[cfg(test)]
mod tests;
