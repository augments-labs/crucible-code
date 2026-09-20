//! The envelope a command arrives in: which protocol, what the sender can do,
//! and which of its requests this is.

use serde_json::Value;

use crate::command::Command;
use crate::error::{ErrorCode, Refusal};
use crate::wire::{Fields, Writing, frame, parsed};

/// Which revision of this protocol a frame is written in.
///
/// A number rather than the release string: two builds of one release speak
/// the same protocol, and two releases may too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u16);

impl Version {
    /// The one revision this build speaks.
    ///
    /// It moves whenever a frame changes in a way a build speaking the old
    /// number would refuse or misread: a field added that must be there, one
    /// renamed, moved or taken away. No release speaks this contract yet, which
    /// is the only reason its frames have changed under the number 1; a test
    /// holds the number and what the frames are made of together, so that after
    /// the first release one cannot move without the other being looked at.
    pub const CURRENT: Self = Self(1);

    /// The revision numbered `number`, spoken or not.
    #[must_use]
    pub const fn numbered(number: u16) -> Self {
        Self(number)
    }

    /// The number.
    #[must_use]
    pub const fn number(self) -> u16 {
        self.0
    }

    /// Whether this build speaks it.
    #[must_use]
    pub const fn spoken(self) -> bool {
        self.0 == Self::CURRENT.0
    }
}

/// Something a client says it can do, which the application then relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// It can be put a permission question and will send a decision back. A
    /// turn for a client without this is never stopped on one: the question is
    /// answered no, as it is wherever there is nobody to ask.
    Permissions,
    /// It can be put the questions a model asks of the person, and answer them.
    Questions,
    /// It wants the provisional progress of a turn as well as its outcome.
    Progress,
}

impl Capability {
    /// Every capability this build has heard of.
    pub const EVERY: [Self; 3] = [Self::Permissions, Self::Questions, Self::Progress];

    /// The word it crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permissions => "permissions",
            Self::Questions => "questions",
            Self::Progress => "progress",
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Self::Permissions => 1,
            Self::Questions => 2,
            Self::Progress => 4,
        }
    }
}

/// The capabilities one request claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Capabilities(u8);

impl Capabilities {
    /// None at all.
    #[must_use]
    pub const fn none() -> Self {
        Self(0)
    }

    /// Every one this build has heard of.
    #[must_use]
    pub const fn every() -> Self {
        Self::none()
            .with(Capability::Permissions)
            .with(Capability::Questions)
            .with(Capability::Progress)
    }

    /// These, and `one`.
    #[must_use]
    pub const fn with(self, one: Capability) -> Self {
        Self(self.0 | one.bit())
    }

    /// Whether `one` was claimed.
    #[must_use]
    pub const fn has(self, one: Capability) -> bool {
        self.0 & one.bit() != 0
    }
}

/// Which of a client's requests this is, chosen by the client and handed back
/// on the response so the two can be paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Correlation(u64);

impl Correlation {
    /// The identity numbered `number`.
    #[must_use]
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    #[must_use]
    pub const fn number(self) -> u64 {
        self.0
    }
}

/// One command, in the envelope every command travels in.
///
/// There is no way to build one that names an unspoken version: a request made
/// here is written in [`Version::CURRENT`], and one decoded was refused unless
/// it said so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    capabilities: Capabilities,
    correlation: Correlation,
    command: Command,
}

/// A frame that was refused, and whose request it was where that much could
/// still be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    /// The request's identity, where the frame was well enough formed to hold
    /// one; a refusal that can be paired is worth more to its sender.
    pub correlation: Option<Correlation>,
    /// Why.
    pub refusal: Refusal,
}

impl Request {
    /// `command`, from a client claiming `capabilities`, as its request
    /// `correlation`.
    #[must_use]
    pub const fn new(
        capabilities: Capabilities,
        correlation: Correlation,
        command: Command,
    ) -> Self {
        Self {
            capabilities,
            correlation,
            command,
        }
    }

    /// The protocol revision it is written in.
    #[must_use]
    pub const fn version(&self) -> Version {
        Version::CURRENT
    }

    /// What its sender can do.
    #[must_use]
    pub const fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// Which request it is.
    #[must_use]
    pub const fn correlation(&self) -> Correlation {
        self.correlation
    }

    /// What it asks for.
    #[must_use]
    pub const fn command(&self) -> &Command {
        &self.command
    }

    /// The frame this request travels as.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] where the frame would be over
    /// [`crate::bounds::FRAME_BYTES`].
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        let capabilities: Vec<Value> = Capability::EVERY
            .into_iter()
            .filter(|one| self.capabilities.has(*one))
            .map(|one| Value::from(one.as_str()))
            .collect();

        frame(
            &Writing::new()
                .with("version", Version::CURRENT.number())
                .with("capabilities", capabilities)
                .with("correlation", self.correlation.number())
                .with("command", self.command.written())
                .finish(),
        )
    }

    /// The request `bytes` spell.
    ///
    /// The length is checked before the bytes are parsed, the version before
    /// anything else in the frame is believed, and the capabilities before the
    /// command: a frame from a protocol this build does not speak may mean
    /// something else by every other word in it.
    ///
    /// # Errors
    ///
    /// [`Refused`], carrying the code and the request's identity where the
    /// frame held one. A frame refused while it was being read was never read
    /// as far as an identity, and carries none: [`ErrorCode::TooLarge`] for one
    /// that is too long, holds too many entries in one list or object, holds
    /// more values in all than [`VALUES`](crate::bounds::VALUES) or nests too
    /// deep, and [`ErrorCode::Malformed`] for a key said twice or what is not
    /// JSON.
    pub fn decode(bytes: &[u8]) -> Result<Self, Refused> {
        let unpaired = |refusal| Refused {
            correlation: None,
            refusal,
        };
        let mut fields = Fields::of(parsed(bytes).map_err(unpaired)?).map_err(unpaired)?;

        let correlation = Correlation::new(fields.number("correlation").map_err(unpaired)?);
        let paired = |refusal| Refused {
            correlation: Some(correlation),
            refusal,
        };

        let version = fields.number("version").map_err(paired)?;
        if !u16::try_from(version).is_ok_and(|number| Version::numbered(number).spoken()) {
            return Err(paired(ErrorCode::UnsupportedVersion.into()));
        }

        let mut capabilities = Capabilities::none();
        for claimed in fields.list("capabilities").map_err(paired)? {
            let one = claimed
                .as_str()
                .and_then(|word| {
                    Capability::EVERY
                        .into_iter()
                        .find(|one| one.as_str() == word)
                })
                .ok_or_else(|| paired(ErrorCode::UnknownCapability.into()))?;
            capabilities = capabilities.with(one);
        }

        let command = fields
            .take("command")
            .and_then(Command::read)
            .map_err(paired)?;
        fields.done().map_err(paired)?;

        Ok(Self::new(capabilities, correlation, command))
    }
}
