//! What a session has used, and how much of each plan window a vendor said
//! was gone.
//!
//! A [`Usage`] is read off the conversation when a client asks for it, and
//! asking sends nothing anywhere: every figure in it is one the conversation
//! already holds. Its numbers are counts and amounts, never what they were
//! counted from.
//!
//! **A cost that is not known is not zero.** [`Cost`] says so in its own arm,
//! so a client cannot draw `$0.00` for a session it has no price for.
//!
//! While a turn runs, the same figures are streamed rather than asked for:
//! [`Used`] and [`Limits`] each cross as progress of their own whenever the
//! turn's figures move, beside the [`Context`] `/context` is streamed, so a
//! client with no terminal reads `/usage` mid-turn from what the turn last
//! reported, as the terminal does.
//!
//! **Plan windows are the vendor's figure, as of the last response that
//! carried them.** At most one per [`Window`], and none at all for a vendor or
//! a credential that does not report them; a client says "not reported" rather
//! than inventing a bar.

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
    /// At least one response could not be priced, so no sum is the session's.
    NotPriced,
}

impl Cost {
    fn written(&self) -> Value {
        match self {
            Self::Unspent => Writing::kind("unspent"),
            Self::Priced { currency, micros } => Writing::kind("priced")
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
            "not_priced" => Self::NotPriced,
            _ => return Err(Refusal::new(ErrorCode::Malformed)),
        };
        fields.done()?;
        Ok(cost)
    }
}

/// A span a plan's use is counted over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    /// Five hours.
    FiveHour,
    /// A week.
    Weekly,
    /// A month.
    Monthly,
}

impl Window {
    /// Every window, in the order a client shows them.
    pub const EVERY: [Self; 3] = [Self::FiveHour, Self::Weekly, Self::Monthly];

    const fn field(self) -> &'static str {
        match self {
            Self::FiveHour => "five_hour",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

/// How much of one window is used, and when it starts again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    /// How much of the window is used.
    pub used: Percent,
    /// When the window starts again, in seconds since the Unix epoch, where
    /// the vendor said.
    pub resets_at: Option<u64>,
}

impl Limit {
    fn written(self) -> Value {
        Writing::new()
            .with("used", self.used.get())
            .maybe("resets_at", self.resets_at)
            .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let limit = Self {
            used: u8::try_from(fields.number("used")?)
                .ok()
                .and_then(Percent::new)
                .ok_or_else(|| Refusal::new(ErrorCode::Malformed))?,
            resets_at: fields.maybe_number("resets_at")?,
        };
        fields.done()?;
        Ok(limit)
    }
}

/// The plan windows a vendor reported, one reading at most per window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    /// The five-hour window, where it was reported.
    pub five_hour: Option<Limit>,
    /// The weekly window, where it was reported.
    pub weekly: Option<Limit>,
    /// The monthly window, where it was reported.
    pub monthly: Option<Limit>,
}

impl Limits {
    /// The reading of `window`, where there is one.
    #[must_use]
    pub const fn of(&self, window: Window) -> Option<Limit> {
        match window {
            Window::FiveHour => self.five_hour,
            Window::Weekly => self.weekly,
            Window::Monthly => self.monthly,
        }
    }

    /// Whether no window was reported.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.weekly.is_none() && self.monthly.is_none()
    }

    pub(crate) fn written(&self) -> Value {
        Window::EVERY
            .into_iter()
            .fold(Writing::new(), |object, window| {
                object.maybe(window.field(), self.of(window).map(Limit::written))
            })
            .finish()
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let mut each = |window: Window| fields.maybe(window.field()).map(Limit::read).transpose();
        let limits = Self {
            five_hour: each(Window::FiveHour)?,
            weekly: each(Window::Weekly)?,
            monthly: each(Window::Monthly)?,
        };
        fields.done()?;
        Ok(limits)
    }
}

/// What the session's requests and edits have added up to so far.
///
/// Read whole between turns as part of a [`Usage`], and streamed as
/// [`Progress::Used`](crate::Progress::Used) while a turn runs, each time the
/// turn's figures move: what a client with no terminal reads `/usage` from
/// mid-turn, as the terminal does.
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
