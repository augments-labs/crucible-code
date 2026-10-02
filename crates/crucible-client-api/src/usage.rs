//! What a session has used, and how much of each limit its plan has a vendor
//! said was gone.
//!
//! A [`Usage`] is read off the conversation when a client asks for it with
//! [`Command::Usage`](crate::Command::Usage), and that sends nothing anywhere:
//! every figure in it is one the conversation already holds. Asked for with
//! [`Command::AskLimits`](crate::Command::AskLimits), the same value comes
//! back after the plan's vendor, where it keeps a source a sign-in can read,
//! was asked how much of each limit is used. Its numbers are counts and
//! amounts, never what they were counted from.
//!
//! **A cost that is not known is not zero.** [`Cost`] says so in its own arm,
//! so a client cannot draw `$0.00` for a session it has no price for, and a
//! sum that leaves out an answer stopped before the provider said what it
//! cost has an arm of its own as well, so it is not drawn as the whole.
//!
//! While a turn runs, the same figures are streamed rather than asked for,
//! beside the [`Context`] `/context` is streamed: [`Used`] crosses as progress
//! of its own when a response ends and when an edit changes lines, and
//! [`Limits`] when a response reports some of its plan's limits. A client with
//! no terminal reads `/usage` mid-turn from what the turn last sent, as the
//! terminal does.
//!
//! **Plan limits are the vendor's figures, and only those.** A plan has one
//! limit for everything and may keep others for models of their own, each a
//! [`LimitGroup`] of windows named by their length. None at all are sent for a
//! vendor or a credential that reports none; a client says "not reported"
//! rather than inventing a bar. A model's name is the vendor's, cut and
//! stripped, and the only words of a vendor's that cross here. No age crosses
//! with them: when the figures came is not carried, so a client tells a reset
//! already past by comparing [`Limit::resets_at`] with its own clock, as the
//! terminal does. Every list is bounded by the ceilings the plan's reading
//! keeps, and a frame over them is refused.

use crucible_types::{MAX_GROUP_WINDOWS, MAX_LIMIT_GROUPS, MAX_LIMIT_NAME_BYTES};
use serde_json::Value;

use crate::bounds::Name;
use crate::context::Context;
use crate::error::{ErrorCode, Refusal};
use crate::snapshot::Percent;
use crate::wire::{Fields, Writing};

/// What the session has cost, as far as it can be stated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cost {
    /// Nothing has been asked yet.
    Unspent,
    /// Every response was priced, and this is their sum.
    Priced {
        /// The currency the prices are in, as its code: `USD`.
        currency: Name,
        /// The sum, in millionths of the currency.
        micros: u64,
    },
    /// An answer ended before the provider said what it cost, so the session
    /// cost this or more: what that answer was billed beyond its last report
    /// is not known, and may be nothing.
    AtLeast {
        /// The currency the prices are in, as its code: `USD`.
        currency: Name,
        /// The sum known, in millionths of the currency.
        micros: u64,
    },
    /// No sum is the session's: a model had no price, a report fit none, a
    /// response completed without saying what it used, amounts were in
    /// currencies that do not add up, the sum was too large for `micros` to
    /// carry, or its currency has a code this contract cannot name.
    NotPriced,
}

impl Cost {
    fn written(&self) -> Value {
        match self {
            Self::Unspent => Writing::kind("unspent"),
            Self::Priced { currency, micros } => Writing::kind("priced")
                .with("currency", currency.as_str())
                .with("micros", *micros),
            Self::AtLeast { currency, micros } => Writing::kind("at_least")
                .with("currency", currency.as_str())
                .with("micros", *micros),
            Self::NotPriced => Writing::kind("not_priced"),
        }
        .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let cost = match fields.kind()?.as_str() {
            "unspent" => Self::Unspent,
            "priced" => Self::Priced {
                currency: fields.name("currency")?,
                micros: fields.number("micros")?,
            },
            "at_least" => Self::AtLeast {
                currency: fields.name("currency")?,
                micros: fields.number("micros")?,
            },
            "not_priced" => Self::NotPriced,
            _ => return Err(Refusal::new(ErrorCode::Malformed)),
        };
        fields.done()?;
        Ok(cost)
    }
}

/// A span a plan's use is counted over, named by its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    /// Five hours.
    FiveHour,
    /// A day.
    Daily,
    /// A week.
    Weekly,
    /// A month.
    Monthly,
    /// A year.
    Yearly,
    /// Any other length.
    Lasting {
        /// How long the window is, in minutes; never 0.
        minutes: u32,
    },
}

impl Window {
    fn written(self) -> Value {
        match self {
            Self::FiveHour => Writing::kind("five_hour"),
            Self::Daily => Writing::kind("daily"),
            Self::Weekly => Writing::kind("weekly"),
            Self::Monthly => Writing::kind("monthly"),
            Self::Yearly => Writing::kind("yearly"),
            Self::Lasting { minutes } => Writing::kind("lasting").with("minutes", minutes),
        }
        .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let window = match fields.kind()?.as_str() {
            "five_hour" => Self::FiveHour,
            "daily" => Self::Daily,
            "weekly" => Self::Weekly,
            "monthly" => Self::Monthly,
            "yearly" => Self::Yearly,
            "lasting" => Self::Lasting {
                minutes: u32::try_from(fields.number("minutes")?)
                    .ok()
                    .filter(|minutes| *minutes > 0)
                    .ok_or_else(|| Refusal::new(ErrorCode::Malformed))?,
            },
            _ => return Err(Refusal::new(ErrorCode::Malformed)),
        };
        fields.done()?;
        Ok(window)
    }
}

/// How much of one window is used, in the vendor's measure of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// A share of the window.
    Percent(Percent),
    /// `used` of the `total` the window allows.
    Counted {
        /// How many have been used, never past `total`.
        used: u64,
        /// How many the window allows, never 0.
        total: u64,
    },
    /// A window the vendor says has no limit.
    Unlimited,
}

impl Reading {
    fn written(self) -> Value {
        match self {
            Self::Percent(percent) => Writing::kind("percent").with("used", percent.get()),
            Self::Counted { used, total } => Writing::kind("counted")
                .with("used", used)
                .with("total", total),
            Self::Unlimited => Writing::kind("unlimited"),
        }
        .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let reading = match fields.kind()?.as_str() {
            "percent" => Self::Percent(
                u8::try_from(fields.number("used")?)
                    .ok()
                    .and_then(Percent::new)
                    .ok_or_else(|| Refusal::new(ErrorCode::Malformed))?,
            ),
            "counted" => {
                let used = fields.number("used")?;
                let total = fields.number("total")?;
                if total == 0 || used > total {
                    return Err(Refusal::new(ErrorCode::Malformed));
                }
                Self::Counted { used, total }
            }
            "unlimited" => Self::Unlimited,
            _ => return Err(Refusal::new(ErrorCode::Malformed)),
        };
        fields.done()?;
        Ok(reading)
    }
}

/// How much of one window is used, and when it starts again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    /// The span the use is counted over.
    pub window: Window,
    /// How much of it is used.
    pub reading: Reading,
    /// When the window starts again, in seconds since the Unix epoch, where
    /// the vendor said.
    pub resets_at: Option<u64>,
}

impl Limit {
    fn written(self) -> Value {
        Writing::new()
            .with("window", self.window.written())
            .with("used", self.reading.written())
            .maybe("resets_at", self.resets_at)
            .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let limit = Self {
            window: Window::read(fields.take("window")?)?,
            reading: Reading::read(fields.take("used")?)?,
            resets_at: fields.maybe_number("resets_at")?,
        };
        fields.done()?;
        Ok(limit)
    }
}

/// The windows of one limit a plan has: plan-wide, or one the vendor keeps
/// for a model of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitGroup {
    /// What the model the limit is kept for is called: the name crucible
    /// knows the model by, else the vendor's name for the limit, cut and
    /// stripped of control characters. `None` for the plan-wide limit.
    pub model: Option<Name>,
    /// Its windows, shortest first, at most [`MAX_GROUP_WINDOWS`].
    pub limits: Vec<Limit>,
}

impl LimitGroup {
    fn written(&self) -> Value {
        Writing::new()
            .maybe("model", self.model.as_ref().map(Name::as_str))
            .with(
                "windows",
                self.limits
                    .iter()
                    .map(|limit| limit.written())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let model = fields
            .maybe("model")
            .map(|value| {
                let said = value.as_str().ok_or(ErrorCode::Malformed)?;
                if said.len() > MAX_LIMIT_NAME_BYTES {
                    return Err(Refusal::new(ErrorCode::TooLarge));
                }
                Name::new(said)
            })
            .transpose()?;
        let limits = fields.list("windows")?;
        if limits.len() > MAX_GROUP_WINDOWS {
            return Err(Refusal::new(ErrorCode::TooLarge));
        }
        let group = Self {
            model,
            limits: limits
                .into_iter()
                .map(Limit::read)
                .collect::<Result<_, _>>()?,
        };
        fields.done()?;
        Ok(group)
    }
}

/// The limits a plan has, as far as its vendor said: the plan-wide one first
/// where there is one, then one for each model the vendor keeps a limit for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Limits {
    /// Each limit, at most [`MAX_LIMIT_GROUPS`].
    pub groups: Vec<LimitGroup>,
}

impl Limits {
    /// Whether no window was reported.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.limits.is_empty())
    }

    /// Written as the list of its groups and nothing around it, which keeps
    /// a window inside a `/usage` answer within half the nesting a frame may
    /// have.
    pub(crate) fn written(&self) -> Value {
        Value::Array(self.groups.iter().map(LimitGroup::written).collect())
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let Value::Array(groups) = value else {
            return Err(Refusal::new(ErrorCode::Malformed));
        };
        if groups.len() > MAX_LIMIT_GROUPS {
            return Err(Refusal::new(ErrorCode::TooLarge));
        }
        Ok(Self {
            groups: groups
                .into_iter()
                .map(LimitGroup::read)
                .collect::<Result<_, _>>()?,
        })
    }
}

/// What the session's requests and edits have added up to so far.
///
/// Read whole between turns as part of a [`Usage`], and streamed as
/// [`Progress::Used`](crate::Progress::Used) while a turn runs, when a
/// response ends and when an edit changes lines: what a client with no
/// terminal reads `/usage` from mid-turn, as the terminal does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Used {
    /// What it has cost.
    pub cost: Cost,
    /// How long its requests have been out, in milliseconds, summed.
    pub api_ms: u64,
    /// How long since it started, in milliseconds.
    pub wall_ms: u64,
    /// Lines edits put in.
    pub added: u64,
    /// Lines edits took out.
    pub removed: u64,
    /// Input tokens sent, cache reads included.
    pub input: u64,
    /// Output tokens generated.
    pub output: u64,
    /// Input tokens served from a provider's cache.
    pub cache_read: u64,
    /// Input tokens written to a provider's cache.
    pub cache_write: u64,
}

impl Used {
    pub(crate) fn written(&self) -> Value {
        Writing::new()
            .with("cost", self.cost.written())
            .with("api_ms", self.api_ms)
            .with("wall_ms", self.wall_ms)
            .with("added", self.added)
            .with("removed", self.removed)
            .with("input", self.input)
            .with("output", self.output)
            .with("cache_read", self.cache_read)
            .with("cache_write", self.cache_write)
            .finish()
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let used = Self {
            cost: Cost::read(fields.take("cost")?)?,
            api_ms: fields.number("api_ms")?,
            wall_ms: fields.number("wall_ms")?,
            added: fields.number("added")?,
            removed: fields.number("removed")?,
            input: fields.number("input")?,
            output: fields.number("output")?,
            cache_read: fields.number("cache_read")?,
            cache_write: fields.number("cache_write")?,
        };
        fields.done()?;
        Ok(used)
    }
}

/// What the session has used so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    /// What its requests and edits have added up to.
    pub used: Used,
    /// How the window of the next request is spent.
    pub context: Context,
    /// The plan windows the vendor reported, where it did.
    pub limits: Limits,
}

impl Usage {
    pub(crate) fn written(&self) -> Value {
        Writing::new()
            .with("used", self.used.written())
            .with("context", self.context.written())
            .with("limits", self.limits.written())
            .finish()
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let usage = Self {
            used: Used::read(fields.take("used")?)?,
            context: Context::read(fields.take("context")?)?,
            limits: Limits::read(fields.take("limits")?)?,
        };
        fields.done()?;
        Ok(usage)
    }
}
