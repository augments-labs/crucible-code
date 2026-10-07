//! What `crucible sandbox inspect --json` writes: the confinement a command
//! started here would run under, as one bounded document.
//!
//! This is a report a person or a script reads once, not a frame a client and
//! the host trade, so it says its own [`FORMAT_VERSION`] and [`KIND`] rather
//! than a protocol [`Version`](crate::Version), and no request asks for it.
//! Everything else is as strict as the frames beside it: each value has a
//! hand-written form, a field a reader does not know refuses the document,
//! and the lists are refused over [`ITEMS`] entries rather than cut.
//!
//! No field is a path. Each place a command may reach, and the directory it
//! starts in, crosses as the digest the sandbox record names it by, so the
//! document can be pasted into an issue without carrying the tree it was
//! written in. Every word that came from a backend is a [`Name`] or a [`Text`],
//! bounded where it is made.
//!
//! Three things are kept apart because a reader has to be able to tell them
//! apart. What a backend claims for each feature is [`Claim::Enforced`],
//! [`Claim::Observed`] or [`Claim::Unsupported`], and every ceiling carries
//! the claim it rests on. A backend version that only running the backend
//! could have told is [`BackendVersion::Unverified`], with the reason, and so
//! is whatever the inspection left unchecked. And `confined` is never said of
//! a backend whose provenance is `compatibility`, which is an ordinary
//! subprocess: such a document is refused where it is made and where it is
//! read.

use serde_json::Value;

use crate::bounds::{ITEMS, Name, Text};
use crate::error::{ErrorCode, Refusal};
use crate::wire::{Fields, Writing, frame, parsed, text, written};

/// The number every document here says it is written in.
pub const FORMAT_VERSION: u64 = 1;

/// The word every document here is named by.
pub const KIND: &str = "sandbox-inspection";

/// The provenance of a backend that is an ordinary subprocess.
const COMPATIBILITY: &str = "compatibility";

/// A SHA-256 digest, written as 64 lowercase hexadecimal digits.
///
/// Digests are what the sandbox record names a place by instead of its path,
/// so one is never a secret and its `Debug` shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    /// The digits.
    #[must_use]
    pub fn hex(&self) -> String {
        use std::fmt::Write as _;

        self.0
            .iter()
            .fold(String::with_capacity(64), |mut out, byte| {
                let _ = write!(out, "{byte:02x}");
                out
            })
    }

    fn read(value: &Value) -> Result<Self, Refusal> {
        let digits = value.as_str().ok_or(ErrorCode::Malformed)?.as_bytes();
        let lower = |digit: &u8| digit.is_ascii_digit() || (b'a'..=b'f').contains(digit);
        if digits.len() != 64 || !digits.iter().all(lower) {
            return Err(ErrorCode::Malformed.into());
        }
        let value = |digit: u8| match digit {
            b'a'..=b'f' => digit - b'a' + 10,
            _ => digit - b'0',
        };
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            if let [high, low] = pair {
                *byte = value(*high) << 4 | value(*low);
            }
        }
        Ok(Self(bytes))
    }
}

/// How strongly a backend holds one feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The backend makes it so: a command cannot get past it.
    Enforced,
    /// The backend writes it down as it happens and stops nothing.
    Observed,
    /// The backend does neither.
    Unsupported,
}

impl Claim {
    /// The word this crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enforced => "enforced",
            Self::Observed => "observed",
            Self::Unsupported => "unsupported",
        }
    }

    fn named(word: &str) -> Result<Self, Refusal> {
        match word {
            "enforced" => Ok(Self::Enforced),
            "observed" => Ok(Self::Observed),
            "unsupported" => Ok(Self::Unsupported),
            _ => Err(ErrorCode::Malformed.into()),
        }
    }
}

/// One feature, and how strongly the backend holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    /// The feature, by the word the sandbox names it with.
    pub feature: Name,
    /// How strongly it is held.
    pub claim: Claim,
}

/// What a backend's version is known to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendVersion {
    /// The version, stated by something that did not have to run the backend
    /// to know it.
    Stated(Name),
    /// Not known: only running the backend could have said it, and an
    /// inspection runs nothing. The words say so.
    Unverified(Text),
}

/// Who would do the confining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    /// Its name.
    pub name: Name,
    /// Its version, where that is known.
    pub version: BackendVersion,
    /// Where it came from: `system`, `bundled`, `remote` or `compatibility`.
    pub provenance: Name,
    /// The digest taken over its executable, where one was.
    pub build: Option<Digest>,
    /// Every feature, and how strongly this backend holds it.
    pub capabilities: Vec<Capability>,
}

/// What a command could reach over the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// Nothing.
    Closed,
    /// The hosts a list allows, counted rather than named.
    Domains {
        /// How many host patterns are allowed.
        allowed: u64,
        /// How many are denied.
        denied: u64,
        /// Whether a command may listen on a local port.
        local_binding: bool,
        /// How many Unix sockets it may reach.
        unix_sockets: u64,
    },
}

/// One place a command could reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// How: `read_only`, `read_write` or `unreadable`.
    pub access: Name,
    /// Why it is reachable at all, by the word the sandbox gives the reason.
    pub provenance: Name,
    /// The digest the place is named by.
    pub identity: Digest,
}

/// What a ceiling's amount counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Seconds, and the nanoseconds beside them.
    Seconds,
    /// Bytes.
    Bytes,
    /// Things: processes, files, commands.
    Count,
    /// Millionths of a unit of cost.
    Micros,
}

impl Unit {
    /// The word this crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Seconds => "seconds",
            Self::Bytes => "bytes",
            Self::Count => "count",
            Self::Micros => "micros",
        }
    }

    fn named(word: &str) -> Result<Self, Refusal> {
        match word {
            "seconds" => Ok(Self::Seconds),
            "bytes" => Ok(Self::Bytes),
            "count" => Ok(Self::Count),
            "micros" => Ok(Self::Micros),
            _ => Err(ErrorCode::Malformed.into()),
        }
    }
}

/// One ceiling a plan states, beside the claim it rests on.
///
/// The pair is the point: a ceiling the backend only observes is one a
/// command can walk past while it is written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ceiling {
    /// The feature the ceiling is, by the word the sandbox names it with.
    pub feature: Name,
    /// How much, in [`unit`](Self::unit).
    pub amount: u64,
    /// The nanoseconds beyond `amount`, for a ceiling in seconds; otherwise 0.
    pub nanos: u32,
    /// What `amount` counts.
    pub unit: Unit,
    /// How strongly the backend holds it, or unsupported where there is none.
    pub claim: Claim,
}

/// One policy, as what a command would be confined by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Whether the policy confines at all.
    pub enabled: bool,
    /// The digest of the whole policy.
    pub policy: Digest,
    /// The digest the directory a command starts in is named by.
    pub cwd: Digest,
    /// The places a command could reach, at most [`ITEMS`] of them.
    pub roots: Vec<Root>,
    /// How many more places there are than `roots` holds.
    pub omitted: u64,
    /// How many patterns are hidden inside those places.
    pub hidden: u64,
    /// What a command could reach over the network.
    pub network: Network,
    /// The ceilings the policy states, and none it does not.
    pub ceilings: Vec<Ceiling>,
    /// The digest of the rules a command line is checked against.
    pub commands: Digest,
    /// How many entries would be staged into the sandbox.
    pub staged: u64,
    /// Whether the sandbox outlives one command.
    pub persistent: bool,
    /// Whether it keeps snapshots.
    pub snapshots: bool,
}

/// Whether confinement may be turned off here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// It may.
    Optional,
    /// The project's configuration requires it.
    Required,
}

impl Requirement {
    /// The word this crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }

    fn named(word: &str) -> Result<Self, Refusal> {
        match word {
            "optional" => Ok(Self::Optional),
            "required" => Ok(Self::Required),
            _ => Err(ErrorCode::Malformed.into()),
        }
    }
}

/// The confinement a command here would run under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    /// Whether confinement is turned on.
    pub enabled: bool,
    /// Whether it may be turned off.
    pub mode: Requirement,
    /// The policy as configuration asked for it.
    pub requested: Plan,
    /// The policy a command would actually be given.
    pub effective: Plan,
    /// The backend that would confine, where one was found.
    pub backend: Option<Backend>,
    /// What this inspection could not check without running something, where
    /// it left anything unchecked.
    pub unchecked: Option<Text>,
    /// Why no command could be run here, where none could.
    pub refusal: Option<Text>,
    /// Whether a command would run inside the backend's boundary.
    pub confined: bool,
}

impl Inspected {
    /// Whether every list is within [`ITEMS`].
    fn within(&self) -> bool {
        [&self.requested, &self.effective]
            .iter()
            .all(|plan| plan.roots.len() <= ITEMS && plan.ceilings.len() <= ITEMS)
            && self
                .backend
                .as_ref()
                .is_none_or(|backend| backend.capabilities.len() <= ITEMS)
    }

    /// Whether this says only what it may: nanoseconds only beside seconds, a
    /// refusal for a missing backend, and `confined` only of a backend found,
    /// accepting the policy, enabled, and not an ordinary subprocess.
    fn truthful(&self) -> bool {
        let nanos = [&self.requested, &self.effective].iter().all(|plan| {
            plan.ceilings.iter().all(|ceiling| {
                ceiling.nanos < 1_000_000_000
                    && (ceiling.nanos == 0 || ceiling.unit == Unit::Seconds)
            })
        });
        let found = self.backend.as_ref();
        let confinable = found.is_some_and(|backend| backend.provenance.as_str() != COMPATIBILITY)
            && self.refusal.is_none()
            && self.effective.enabled;
        nanos && (found.is_some() || self.refusal.is_some()) && (!self.confined || confinable)
    }

    /// Whether any words here were cut, or any list left something out.
    fn truncated(&self) -> bool {
        let cut = |text: Option<&Text>| text.is_some_and(Text::truncated);
        cut(self.unchecked.as_ref())
            || cut(self.refusal.as_ref())
            || self.requested.omitted > 0
            || self.effective.omitted > 0
            || self
                .backend
                .as_ref()
                .is_some_and(|backend| match &backend.version {
                    BackendVersion::Unverified(why) => why.truncated(),
                    BackendVersion::Stated(_) => false,
                })
    }
}

/// What `crucible sandbox inspect --json` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspection {
    /// A report was made. Boxed, as it is many times the size of a failure.
    Inspected(Box<Inspected>),
    /// None could be: the directory, crucible's files or the policy could not
    /// be read. The words are the sentence a person would have been shown.
    Failed(Text),
}

impl Inspection {
    /// The status word: `ready` where a command could run, `refused` where a
    /// backend was found and would not take the policy, `unavailable` where
    /// none was found, and `failed` where no report could be made.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::Inspected(inspected) => match (&inspected.backend, &inspected.refusal) {
                (None, _) => "unavailable",
                (Some(_), Some(_)) => "refused",
                (Some(_), None) => "ready",
            },
            Self::Failed(_) => "failed",
        }
    }

    /// The document, as one line ending in a newline.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] for a report over a list or document ceiling,
    /// and [`ErrorCode::Malformed`] for one that says what it may not:
    /// `confined` of a backend that was not found, refused the policy, or is
    /// an ordinary subprocess, or no backend and no refusal.
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        let document = match self {
            Self::Inspected(inspected) => {
                if !inspected.within() {
                    return Err(ErrorCode::TooLarge.into());
                }
                if !inspected.truthful() {
                    return Err(ErrorCode::Malformed.into());
                }
                Writing::new()
                    .with("enabled", inspected.enabled)
                    .with("mode", inspected.mode.as_str())
                    .with("requested", plan(&inspected.requested))
                    .with("effective", plan(&inspected.effective))
                    .maybe("backend", inspected.backend.as_ref().map(backend))
                    .maybe("unchecked", inspected.unchecked.as_ref().map(written))
                    .maybe("refusal", inspected.refusal.as_ref().map(written))
                    .with("confined", inspected.confined)
                    .with("truncated", inspected.truncated())
            }
            Self::Failed(problem) => Writing::new()
                .text("problem", problem)
                .with("truncated", problem.truncated()),
        };
        let mut bytes = frame(
            &document
                .with("format_version", FORMAT_VERSION)
                .with(crate::wire::KIND, KIND)
                .with("status", self.status())
                .finish(),
        )?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// The document `bytes` spell.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnsupportedVersion`] for another format version, and
    /// [`Refusal`] for anything but one whole, bounded document that says what
    /// it may: a status that disagrees with the fields, a truncation flag that
    /// does, and a report [`encode`](Self::encode) would refuse are all
    /// refused.
    pub fn decode(bytes: &[u8]) -> Result<Self, Refusal> {
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let mut fields = Fields::of(parsed(bytes)?)?;
        if fields.number("format_version")? != FORMAT_VERSION {
            return Err(ErrorCode::UnsupportedVersion.into());
        }
        if fields.kind()? != KIND {
            return Err(ErrorCode::Malformed.into());
        }
        let status = fields.string("status")?;
        let truncated = fields.flag("truncated")?;

        let inspection = if status == "failed" {
            Self::Failed(fields.text("problem")?)
        } else {
            let inspected = Inspected {
                enabled: fields.flag("enabled")?,
                mode: Requirement::named(&fields.string("mode")?)?,
                requested: read_plan(fields.take("requested")?)?,
                effective: read_plan(fields.take("effective")?)?,
                backend: fields.maybe("backend").map(read_backend).transpose()?,
                unchecked: fields.maybe("unchecked").map(text).transpose()?,
                refusal: fields.maybe("refusal").map(text).transpose()?,
                confined: fields.flag("confined")?,
            };
            if !inspected.truthful() || inspected.truncated() != truncated {
                return Err(ErrorCode::Malformed.into());
            }
            Self::Inspected(Box::new(inspected))
        };
        fields.done()?;
        let cut = match &inspection {
            Self::Failed(problem) => problem.truncated() == truncated,
            Self::Inspected(_) => true,
        };
        if inspection.status() != status || !cut {
            return Err(ErrorCode::Malformed.into());
        }
        Ok(inspection)
    }
}

fn backend(backend: &Backend) -> Value {
    let version = match &backend.version {
        BackendVersion::Stated(name) => Writing::kind("stated").with("name", name.as_str()),
        BackendVersion::Unverified(why) => Writing::kind("unverified").text("why", why),
    };
    let capabilities: Vec<Value> = backend
        .capabilities
        .iter()
        .map(|one| {
            Writing::new()
                .with("feature", one.feature.as_str())
                .with("claim", one.claim.as_str())
                .finish()
        })
        .collect();
    Writing::new()
        .with("name", backend.name.as_str())
        .with("version", version.finish())
        .with("provenance", backend.provenance.as_str())
        .maybe("build", backend.build.as_ref().map(Digest::hex))
        .with("capabilities", capabilities)
        .finish()
}

fn read_backend(value: Value) -> Result<Backend, Refusal> {
    let mut fields = Fields::of(value)?;
    let mut version = Fields::of(fields.take("version")?)?;
    let stated = match version.kind()?.as_str() {
        "stated" => BackendVersion::Stated(version.name("name")?),
        "unverified" => BackendVersion::Unverified(version.text("why")?),
        _ => return Err(ErrorCode::Malformed.into()),
    };
    version.done()?;
    let backend = Backend {
        name: fields.name("name")?,
        version: stated,
        provenance: fields.name("provenance")?,
        build: fields
            .maybe("build")
            .map(|digits| Digest::read(&digits))
            .transpose()?,
        capabilities: fields
            .list("capabilities")?
            .into_iter()
            .map(|one| {
                let mut one = Fields::of(one)?;
                let capability = Capability {
                    feature: one.name("feature")?,
                    claim: Claim::named(&one.string("claim")?)?,
                };
                one.done()?;
                Ok(capability)
            })
            .collect::<Result<_, Refusal>>()?,
    };
    fields.done()?;
    Ok(backend)
}

fn plan(plan: &Plan) -> Value {
    let roots: Vec<Value> = plan
        .roots
        .iter()
        .map(|root| {
            Writing::new()
                .with("access", root.access.as_str())
                .with("provenance", root.provenance.as_str())
                .with("identity", root.identity.hex())
                .finish()
        })
        .collect();
    let network = match plan.network {
        Network::Closed => Writing::kind("closed"),
        Network::Domains {
            allowed,
            denied,
            local_binding,
            unix_sockets,
        } => Writing::kind("domains")
            .with("allowed", allowed)
            .with("denied", denied)
            .with("local_binding", local_binding)
            .with("unix_sockets", unix_sockets),
    };
    let ceilings: Vec<Value> = plan
        .ceilings
        .iter()
        .map(|ceiling| {
            Writing::new()
                .with("feature", ceiling.feature.as_str())
                .with("amount", ceiling.amount)
                .maybe("nanos", (ceiling.nanos > 0).then_some(ceiling.nanos))
                .with("unit", ceiling.unit.as_str())
                .with("claim", ceiling.claim.as_str())
                .finish()
        })
        .collect();
    Writing::new()
        .with("enabled", plan.enabled)
        .with("policy", plan.policy.hex())
        .with("cwd", plan.cwd.hex())
        .with("roots", roots)
        .with("omitted", plan.omitted)
        .with("hidden", plan.hidden)
        .with("network", network.finish())
        .with("ceilings", ceilings)
        .with("commands", plan.commands.hex())
        .with("staged", plan.staged)
        .with("persistent", plan.persistent)
        .with("snapshots", plan.snapshots)
        .finish()
}

fn read_plan(value: Value) -> Result<Plan, Refusal> {
    let mut fields = Fields::of(value)?;
    let mut network = Fields::of(fields.take("network")?)?;
    let reach = match network.kind()?.as_str() {
        "closed" => Network::Closed,
        "domains" => Network::Domains {
            allowed: network.number("allowed")?,
            denied: network.number("denied")?,
            local_binding: network.flag("local_binding")?,
            unix_sockets: network.number("unix_sockets")?,
        },
        _ => return Err(ErrorCode::Malformed.into()),
    };
    network.done()?;
    let plan = Plan {
        enabled: fields.flag("enabled")?,
        policy: Digest::read(&fields.take("policy")?)?,
        cwd: Digest::read(&fields.take("cwd")?)?,
        roots: fields
            .list("roots")?
            .into_iter()
            .map(|root| {
                let mut root = Fields::of(root)?;
                let read = Root {
                    access: root.name("access")?,
                    provenance: root.name("provenance")?,
                    identity: Digest::read(&root.take("identity")?)?,
                };
                root.done()?;
                Ok(read)
            })
            .collect::<Result<_, Refusal>>()?,
        omitted: fields.number("omitted")?,
        hidden: fields.number("hidden")?,
        network: reach,
        ceilings: fields
            .list("ceilings")?
            .into_iter()
            .map(|ceiling| {
                let mut ceiling = Fields::of(ceiling)?;
                let read = Ceiling {
                    feature: ceiling.name("feature")?,
                    amount: ceiling.number("amount")?,
                    nanos: ceiling
                        .maybe_number("nanos")?
                        .map(|nanos| {
                            u32::try_from(nanos)
                                .ok()
                                .filter(|nanos| (1..1_000_000_000).contains(nanos))
                                .ok_or_else(|| Refusal::new(ErrorCode::Malformed))
                        })
                        .transpose()?
                        .unwrap_or(0),
                    unit: Unit::named(&ceiling.string("unit")?)?,
                    claim: Claim::named(&ceiling.string("claim")?)?,
                };
                ceiling.done()?;
                Ok(read)
            })
            .collect::<Result<_, Refusal>>()?,
        commands: Digest::read(&fields.take("commands")?)?,
        staged: fields.number("staged")?,
        persistent: fields.flag("persistent")?,
        snapshots: fields.flag("snapshots")?,
    };
    fields.done()?;
    Ok(plan)
}

#[cfg(test)]
pub(crate) mod tests;
