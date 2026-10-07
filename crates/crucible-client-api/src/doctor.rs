//! What `crucible doctor --json` writes: whether this host is ready to run a
//! conversation, as one bounded document of checks.
//!
//! Like the [`inspection`](crate::inspection) beside it, this is a report a
//! person or a script reads once, so it says its own [`FORMAT_VERSION`] and
//! [`KIND`], no request asks for it, a field a reader does not know refuses the
//! document, and the list of checks is refused over [`ITEMS`] entries rather
//! than cut.
//!
//! Each [`Check`] is named by an id a script can hold on to, says how it came
//! out, why, and — for anything but [`Status::Ok`] — what to do about it. The
//! words are a [`Text`], so no reason can run past the ceiling unmarked, and
//! none of them is a path or a credential: the host writes them that way, and
//! no field here is a place for either. An id is said once, so a reader never
//! has to choose between two answers to the same question.
//!
//! The document's status and the exit code are both read from the checks and
//! from nothing else: a failure anywhere is `failed` and exits 2, a warning
//! with no failure is `warnings` and exits 1, and anything else is `healthy`
//! and exits 0. A check that could not run counts toward neither, because
//! whatever kept it from running is a check of its own that says so.

use crate::bounds::{ITEMS, Name, Text};
use crate::error::{ErrorCode, Refusal};
use crate::wire::{Fields, Writing, frame, parsed, text, written};

/// The number every document here says it is written in.
pub const FORMAT_VERSION: u64 = 1;

/// The word every document here is named by.
pub const KIND: &str = "doctor";

/// How one check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// What it looked at is ready.
    Ok,
    /// It works, but not as it should, or not for long.
    Warning,
    /// It does not work, and a conversation that needs it will not start.
    Failed,
    /// It was not looked at: something it rests on failed, or this host is
    /// not one it can be looked at on. The reason says which.
    Unavailable,
}

impl Status {
    /// The word this crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }

    fn named(word: &str) -> Result<Self, Refusal> {
        match word {
            "ok" => Ok(Self::Ok),
            "warning" => Ok(Self::Warning),
            "failed" => Ok(Self::Failed),
            "unavailable" => Ok(Self::Unavailable),
            _ => Err(ErrorCode::Malformed.into()),
        }
    }
}

/// One thing the doctor looked at, and how it came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Which check this is, by an id that does not change between releases.
    pub id: Name,
    /// How it came out.
    pub status: Status,
    /// Why, in words for a person.
    pub reason: Text,
    /// What to do about it: always there for a warning or a failure, never for
    /// a check that is fine, and there for one that could not run where
    /// something can be done.
    pub remedy: Option<Text>,
}

impl Check {
    /// Whether this says only what it may: a remedy for every problem, and
    /// none for a check that has none.
    fn truthful(&self) -> bool {
        match self.status {
            Status::Ok => self.remedy.is_none(),
            Status::Warning | Status::Failed => self.remedy.is_some(),
            Status::Unavailable => true,
        }
    }

    fn truncated(&self) -> bool {
        self.reason.truncated() || self.remedy.as_ref().is_some_and(Text::truncated)
    }
}

/// What `crucible doctor --json` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Every check, in the order it was made, at most [`ITEMS`] of them.
    pub checks: Vec<Check>,
}

impl Report {
    /// The status word: `failed` where any check failed, `warnings` where any
    /// warned and none failed, and `healthy` otherwise.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self.exit() {
            2 => "failed",
            1 => "warnings",
            _ => "healthy",
        }
    }

    /// The code `crucible doctor` exits with: 2 where any check failed, 1
    /// where any warned and none failed, and 0 otherwise.
    #[must_use]
    pub fn exit(&self) -> u8 {
        let any = |status: Status| self.checks.iter().any(|check| check.status == status);
        if any(Status::Failed) {
            2
        } else {
            u8::from(any(Status::Warning))
        }
    }

    /// Whether each check says only what it may, and each id is said once.
    fn truthful(&self) -> bool {
        let mut ids = std::collections::BTreeSet::new();
        self.checks
            .iter()
            .all(|check| check.truthful() && ids.insert(check.id.as_str()))
    }

    fn truncated(&self) -> bool {
        self.checks.iter().any(Check::truncated)
    }

    /// The document, as one line ending in a newline.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] for more than [`ITEMS`] checks or a document
    /// over the frame ceiling, and [`ErrorCode::Malformed`] for one that says
    /// what it may not: a warning or failure with no remedy, a check that is
    /// fine with one, or an id said twice.
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        if self.checks.len() > ITEMS {
            return Err(ErrorCode::TooLarge.into());
        }
        if !self.truthful() {
            return Err(ErrorCode::Malformed.into());
        }
        let checks: Vec<serde_json::Value> = self
            .checks
            .iter()
            .map(|check| {
                Writing::new()
                    .with("id", check.id.as_str())
                    .with("status", check.status.as_str())
                    .with("reason", written(&check.reason))
                    .maybe("remedy", check.remedy.as_ref().map(written))
                    .finish()
            })
            .collect();
        let mut bytes = frame(
            &Writing::new()
                .with("checks", checks)
                .with("truncated", self.truncated())
                .with("format_version", FORMAT_VERSION)
                .with(crate::wire::KIND, KIND)
                .with("status", self.status())
                .finish(),
        )?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// The report `bytes` spell.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnsupportedVersion`] for another format version, and
    /// [`Refusal`] for anything but one whole, bounded document that says what
    /// it may: a status or a truncation flag that disagrees with the checks,
    /// and a report [`encode`](Self::encode) would refuse, are both refused.
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
        let checks = fields
            .list("checks")?
            .into_iter()
            .map(|one| {
                let mut one = Fields::of(one)?;
                let check = Check {
                    id: one.name("id")?,
                    status: Status::named(&one.string("status")?)?,
                    reason: one.text("reason")?,
                    remedy: one.maybe("remedy").map(text).transpose()?,
                };
                one.done()?;
                Ok(check)
            })
            .collect::<Result<_, Refusal>>()?;
        fields.done()?;
        let report = Self { checks };
        if !report.truthful() || report.truncated() != truncated || report.status() != status {
            return Err(ErrorCode::Malformed.into());
        }
        Ok(report)
    }
}

#[cfg(test)]
pub(crate) mod tests;
