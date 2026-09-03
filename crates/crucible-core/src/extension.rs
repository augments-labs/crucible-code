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

use std::fmt;

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

/// The most bytes one manifest file may hold.
///
/// Checked before the text is parsed rather than after, because a parser handed
/// an arbitrarily large document has already done the work by the time anything
/// could refuse it — and a manifest is a file whoever published the extension
/// wrote.
pub const EXTENSION_MANIFEST_BYTES: usize = 16 * 1024;

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
    /// The text was not JSON.
    #[error("line {line} column {column}: {problem}")]
    Malformed {
        /// Where the parser stopped.
        line: usize,
        /// Where in that line.
        column: usize,
        /// What it said, without the position stated again.
        problem: Box<str>,
    },
    /// A required key was not written.
    #[error("{field} must be written")]
    Missing {
        /// Which key.
        field: &'static str,
    },
    /// A key held the wrong kind of value.
    #[error("{field} must be {wanted}")]
    WrongType {
        /// Which key.
        field: &'static str,
        /// What it accepts.
        wanted: &'static str,
    },
    /// A key crucible does not have.
    #[error("{key} is not a manifest key; accepted: {}", .accepted.join(", "))]
    UnknownKey {
        /// What was written.
        key: Box<str>,
        /// What is accepted instead.
        accepted: &'static [&'static str],
    },
    /// A spelling whose meaning this build does not fix.
    #[error("{field} names {name}, which this crucible does not know")]
    Unrecognised {
        /// Which list it was in.
        field: &'static str,
        /// What was written.
        name: Box<str>,
    },
    /// A protocol version that is not two numbers.
    #[error("{found} is not a protocol version such as 1.3")]
    BadProtocol {
        /// What was written.
        found: Box<str>,
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
    /// Every capability there is.
    ///
    /// Read by the lookup below, so the spellings a manifest may write are
    /// exactly the ones [`Self::as_str`] produces and the two cannot disagree.
    ///
    /// Whether this list is *complete* is the one thing nothing here checks:
    /// Rust cannot enumerate a variant, and a capability added to the enum but
    /// not to this list is one no manifest can ask for. The exhaustive match in
    /// [`Self::as_str`] is what puts the author in this file; adding the line
    /// below it is theirs to remember.
    pub const EVERY: &'static [Self] = &[
        Self::RegisterTools,
        Self::RegisterCommands,
        Self::ObserveLifecycle,
        Self::ReadRunContext,
        Self::WriteSessionState,
        Self::ContributeSkills,
        Self::AskTheOperator,
    ];

    /// The capability this spelling names, where it names one.
    #[must_use]
    pub fn named(spelling: &str) -> Option<Self> {
        Self::EVERY
            .iter()
            .copied()
            .find(|one| one.as_str() == spelling)
    }

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
    /// Every kind of contribution there is.
    ///
    /// Listed for the reason [`ExtensionCapability::EVERY`] is, with the same
    /// unchecked completeness.
    pub const EVERY: &'static [Self] = &[Self::Tools, Self::Commands, Self::Skills, Self::Policy];

    /// The contribution this spelling names, where it names one.
    #[must_use]
    pub fn named(spelling: &str) -> Option<Self> {
        Self::EVERY
            .iter()
            .copied()
            .find(|one| one.as_str() == spelling)
    }

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
    /// The protocol this build of crucible speaks.
    ///
    /// Declared here rather than worked out from the crate version, because
    /// the two answer different questions and move at different speeds: a
    /// release happens whenever anything ships, and this changes only when the
    /// shape of what crosses the wire does.
    pub const HOST: Self = Self::new(1, 0);

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

impl fmt::Display for ExtensionProtocol {
    /// The two numbers, as a manifest writes them.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}.{}", self.major, self.minor)
    }
}

/// Why this build would not host an extension.
///
/// Data rather than a message, because the two facts either answer names —
/// what this build speaks, and which crucible is running — are already in the
/// hand of whoever asked. Saying them again here would be this crate writing
/// prose for a screen it cannot see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionUnhosted {
    /// Written against a protocol whose frames this build does not know.
    Protocol,
    /// It works with a crucible later than the one running.
    Newer,
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

    /// Reads one manifest from the text of a manifest file.
    ///
    /// `found` is where the file was, which the file itself cannot say. The
    /// digest is taken over `text` for the same reason: a manifest stating its
    /// own would be a file asserting it had not changed since somebody trusted
    /// it.
    ///
    /// Nothing here opens a file, resolves the entrypoint or starts anything.
    ///
    /// # Errors
    ///
    /// [`ExtensionError`] for text over [`EXTENSION_MANIFEST_BYTES`], for text
    /// that is not a JSON object, for a key crucible does not have, for a
    /// required key that is missing or holds the wrong kind of value, for a
    /// capability or contribution spelling this build does not know, for a
    /// protocol version that is not two numbers — and for everything
    /// [`Self::read`] refuses.
    pub fn parse(text: &str, found: SourceKind) -> Result<Self, ExtensionError> {
        parse::manifest(text, found)
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

    /// What this build and this extension would speak, or why they would not.
    ///
    /// `running` is the crucible being run, which this crate has no way to ask
    /// for: the version belongs to the binary, and a library reading its own
    /// would answer for itself rather than for the program that was started.
    ///
    /// Nothing here is a trust decision. An extension this build could host is
    /// still one nobody has said may run, and the two are asked separately so
    /// that neither answer can be mistaken for the other.
    ///
    /// # Errors
    ///
    /// [`ExtensionUnhosted::Protocol`] where the majors differ, which is asked
    /// first because it is the deeper refusal: a crucible new enough to satisfy
    /// the minimum would still be two programs that cannot exchange a frame.
    /// [`ExtensionUnhosted::Newer`] where it names a crucible later than this
    /// one.
    pub fn hosted(&self, running: &str) -> Result<ExtensionProtocol, ExtensionUnhosted> {
        let agreed = self
            .requests
            .protocol
            .agreed(ExtensionProtocol::HOST)
            .ok_or(ExtensionUnhosted::Protocol)?;

        if crate::version::later(&self.requests.minimum, running) {
            return Err(ExtensionUnhosted::Newer);
        }

        Ok(agreed)
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

mod parse;
pub(crate) mod wire;

#[cfg(test)]
mod tests;
