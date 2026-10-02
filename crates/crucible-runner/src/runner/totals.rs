//! What a session has used so far, every response and every edit of it added
//! up.
//!
//! `/usage` reads this. It is a handful of integers, a duration and a cost,
//! overwritten in place: nothing per response is kept, so it is the same size
//! after a thousand answers as after one. It is this process's count and is
//! never written to the session; a session picked up starts counting again.
//!
//! **A response is counted once.** What a provider reports while a response
//! arrives is that response's total so far rather than an increment, so the
//! response being read is held apart and each new figure
//! replaces the last. It joins the settled sum when it ends. A provider that
//! sends one final figure and one that counts up as it goes come out the same,
//! which is the arithmetic [`Event::Spent`](crate::Event::Spent) already uses
//! for one turn.
//!
//! **A cost is stated only where every response was priced.** One response
//! nothing could price makes the sum a figure that leaves part of the session
//! out, and a reader shown `$0.40` for a session that also spent an unpriced
//! hour would read it as the whole. So the sum becomes
//! [`SessionCost::NotPriced`] and stays so.

use std::time::{Duration, Instant};

use crucible_types::{Changed, CostAmount, ProviderUsage};

/// What the session has cost, as far as it can be stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCost {
    /// No response has reported anything yet.
    Unspent,
    /// Every response was priced, and this is their sum.
    Priced(CostAmount),
    /// At least one response could not be priced: the model has no price, a
    /// provider reported no usage to price, or the amounts are in currencies
    /// that do not add up to one figure.
    NotPriced,
}

impl SessionCost {
    /// This cost and `other`, as one figure where both are figures.
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unspent, other) | (other, Self::Unspent) => other,
            (Self::Priced(one), Self::Priced(two)) => {
                one.checked_add(two).map_or(Self::NotPriced, Self::Priced)
            }
            (Self::NotPriced, _) | (_, Self::NotPriced) => Self::NotPriced,
        }
    }
}

/// The tokens and cost of one response, or of every settled one together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Heard {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    /// Whether anything was reported at all: a response that reported tokens
    /// and no usage to price them by is a response nothing priced.
    reported: bool,
    cost: SessionCost,
}

impl Heard {
    const NOTHING: Self = Self {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reported: false,
        cost: SessionCost::Unspent,
    };

    /// What this response, ended, adds to a sum.
    fn ended(self) -> Self {
        let cost = match self.cost {
            SessionCost::Unspent if self.reported => SessionCost::NotPriced,
            cost => cost,
        };
        Self { cost, ..self }
    }

    /// Two sums as one.
    fn and(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
            reported: self.reported || other.reported,
            cost: self.cost.and(other.cost),
        }
    }
}

/// What a session has used: tokens, cost, time and lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    /// Every response that has ended.
    settled: Heard,
    /// The response being read, as last reported.
    reading: Heard,
    /// How long requests have been out, summed.
    waited: Duration,
    /// When the session this counts started, in this process.
    started: Instant,
    added: u64,
    removed: u64,
}

impl Totals {
    /// A session that has used nothing yet, starting now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            settled: Heard::NOTHING,
            reading: Heard::NOTHING,
            waited: Duration::ZERO,
            started: Instant::now(),
            added: 0,
            removed: 0,
        }
    }

    fn all(&self) -> Heard {
        self.settled.and(self.reading.ended())
    }

    /// Input tokens sent, cache reads included.
    #[must_use]
    pub fn input(&self) -> u64 {
        self.all().input
    }

    /// Output tokens generated.
    #[must_use]
    pub fn output(&self) -> u64 {
        self.all().output
    }

    /// Input tokens served from a provider's cache.
    #[must_use]
    pub fn cache_read(&self) -> u64 {
        self.all().cache_read
    }

    /// Input tokens written to a provider's cache.
    #[must_use]
    pub fn cache_write(&self) -> u64 {
        self.all().cache_write
    }

    /// What the session has cost.
    #[must_use]
    pub fn cost(&self) -> SessionCost {
        self.all().cost
    }

    /// How long this session's requests have been out, summed.
    #[must_use]
    pub const fn api(&self) -> Duration {
        self.waited
    }

    /// When the session started, in this process.
    #[must_use]
    pub const fn started(&self) -> Instant {
        self.started
    }

    /// Lines edits put in.
    #[must_use]
    pub const fn added(&self) -> u64 {
        self.added
    }

    /// Lines edits took out.
    #[must_use]
    pub const fn removed(&self) -> u64 {
        self.removed
    }

    /// What the response being read carried, as last reported.
    pub(super) const fn carried(&mut self, tokens: u64) {
        self.reading.input = tokens;
        self.reading.reported = true;
    }

    /// What the response being read has generated, as last reported.
    pub(super) const fn spent(&mut self, tokens: u64) {
        self.reading.output = tokens;
        self.reading.reported = true;
    }

    /// A usage report for the response being read, merged with every earlier
    /// one of it, and what it was priced at.
    pub(super) fn used(&mut self, usage: &ProviderUsage, cost: Option<CostAmount>) {
        let input = &usage.input;
        let reading = &mut self.reading;
        if let Some(total) = input.total {
            reading.input = total;
        }
        if let Some(output) = usage.output {
            reading.output = output;
        }
        if let Some(read) = input.cache_read {
            reading.cache_read = read;
        }
        if let Some(written) = input.cache_write_or_creation {
            reading.cache_write = written;
        }
        reading.reported = true;
        reading.cost = cost.map_or(SessionCost::NotPriced, SessionCost::Priced);
    }

    /// The response being read has ended, after `waited` out.
    pub(super) fn answered(&mut self, waited: Duration) {
        self.settled = self.all();
        self.reading = Heard::NOTHING;
        self.waited = self.waited.saturating_add(waited);
    }

    /// An edit changed lines.
    pub(super) fn changed(&mut self, changed: Changed) {
        self.added = self.added.saturating_add(changed.added() as u64);
        self.removed = self.removed.saturating_add(changed.removed() as u64);
    }
}

impl Default for Totals {
    fn default() -> Self {
        Self::new()
    }
}
