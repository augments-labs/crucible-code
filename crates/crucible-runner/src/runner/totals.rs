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
//! **A response's cost joins the sum when it ends, by one rule.** A response
//! that completed with a usage report its model's prices cover adds that
//! price. A model with no price, a report no price fits, amounts in
//! currencies that do not add up, or a sum too large to carry make the sum
//! [`SessionCost::NotPriced`] whatever it was before, a lower bound included,
//! and it stays so: a reader shown `$0.40` for a session that also spent an
//! unpriced hour would read it as the whole. A response that completed without
//! saying what it used is one nothing priced, so it is the same; that leaves
//! [`SessionCost::Unspent`] meaning only that no response has ended. A response
//! that ended without completing, stopped, failed or asked again, on a model
//! with a price, still counts its tokens as last reported and adds whatever of
//! them its last report priced, and the sum becomes
//! [`SessionCost::AtLeast`]: what the provider billed for it is not known, and
//! everything else is. Later priced responses keep adding to that floor. A
//! request that never went out, or that the provider turned away, is no
//! response and adds nothing.
//!
//! Whether the model has a price is asked of the pricing record when the
//! request goes out, before anything is reported, so a short report and an
//! unpriced model are told apart there and never read off a missing total.

use std::time::{Duration, Instant};

use crucible_models::PromptCachePricing;
use crucible_types::{Changed, CostAmount, ProviderUsage};

/// What the session has cost, as far as it can be stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCost {
    /// No response has ended yet.
    Unspent,
    /// Every response was priced, and this is their sum.
    Priced(CostAmount),
    /// A response ended before the provider said what it cost, so the
    /// session spent this or more.
    AtLeast(CostAmount),
    /// No sum is the session's: a model has no price, a report fits none, a
    /// completed response reported no usage to price, the amounts are in
    /// currencies that do not add up to one figure, or the sum is too large
    /// to carry.
    NotPriced,
}

/// Whether the model a request went to has a price, asked as it went out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Listed {
    /// Every quantity a report carries has a rate, in this currency and unit.
    Priced {
        /// Nothing, in that currency and unit.
        nothing: CostAmount,
    },
    /// No record, or one missing a rate: no report of this model is priced.
    Unpriced,
}

impl Listed {
    /// What `record`, the model's pricing record if it has one, says.
    pub(super) fn of(record: Option<&PromptCachePricing>) -> Self {
        record
            .filter(|record| record.prices_in_full())
            .map_or(Self::Unpriced, |record| Self::Priced {
                nothing: CostAmount::new(record.currency(), record.unit(), 0),
            })
    }
}

/// What one usage report was priced at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Price {
    /// The report's model has no price at its size, or the price could not be
    /// applied to it.
    Unpriced,
    /// A model priced in full, and a report short of a quantity its price
    /// needs, such as one that opens an answer before any output.
    Short,
    /// The report priced in full.
    Of(CostAmount),
}

impl Price {
    /// `report` under `record`, the pricing record found for it if any.
    pub(super) fn of(record: Option<&PromptCachePricing>, report: &ProviderUsage) -> Self {
        match record {
            Some(record) if record.prices_in_full() => {
                record.cost(report).map_or(Self::Unpriced, |cost| {
                    cost.total.map_or(Self::Short, Self::Of)
                })
            }
            // A record is looked up by the report's input, so a report without
            // one finds none; whether that is the model's gap or the report's
            // is what was asked as the request went out.
            None if report.input.total.is_none() => Self::Short,
            Some(_) | None => Self::Unpriced,
        }
    }
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
            (Self::AtLeast(one) | Self::Priced(one), Self::AtLeast(two) | Self::Priced(two)) => {
                one.checked_add(two).map_or(Self::NotPriced, Self::AtLeast)
            }
        }
    }
}

/// What is known of the cost of the response being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// Its model's price has not been asked: no request for it has gone out,
    /// or the one made was never sent or was turned away. Ended so, it adds
    /// nothing where nothing was reported, and is not priced where something
    /// was.
    Unasked,
    /// Its model has no price, or its last report fits none.
    Unpriced,
    /// Its model is priced in full.
    Priced {
        /// What its last report priced, or nothing where none has.
        last: CostAmount,
        /// Whether that report carried every quantity the price needs.
        full: bool,
    },
}

impl Pending {
    /// After a report priced at `price`.
    fn reported(self, price: Price) -> Self {
        match (price, self) {
            (Price::Of(last), _) => Self::Priced { last, full: true },
            (Price::Short, Self::Priced { last, .. }) => Self::Priced {
                last: CostAmount::new(last.currency(), last.unit(), 0),
                full: false,
            },
            (Price::Short | Price::Unpriced, _) => Self::Unpriced,
        }
    }

    /// What the response adds to the cost, ended, `completed` or not, where
    /// it `reported` anything.
    const fn ended(self, completed: bool, reported: bool) -> SessionCost {
        match self {
            Self::Priced { last, full: true } if completed => SessionCost::Priced(last),
            Self::Priced { last, .. } if !completed => SessionCost::AtLeast(last),
            Self::Unasked if !reported => SessionCost::Unspent,
            Self::Priced { .. } | Self::Unasked | Self::Unpriced => SessionCost::NotPriced,
        }
    }
}

/// The tokens of one response, or of every settled one together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Heard {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    /// Whether anything was reported at all: a response that reported tokens
    /// and no usage to price them by is a response nothing priced.
    reported: bool,
}

impl Heard {
    const NOTHING: Self = Self {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reported: false,
    };

    /// Two sums as one.
    fn and(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
            reported: self.reported || other.reported,
        }
    }
}

/// What a session has used: tokens, cost, time and lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    /// Every response that has ended.
    settled: Heard,
    /// What every response that has ended cost.
    cost: SessionCost,
    /// The response being read, as last reported.
    reading: Heard,
    /// What is known of its cost, decided when it ends.
    pending: Pending,
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
            cost: SessionCost::Unspent,
            reading: Heard::NOTHING,
            pending: Pending::Unasked,
            waited: Duration::ZERO,
            started: Instant::now(),
            added: 0,
            removed: 0,
        }
    }

    /// Every token reported, the response being read included.
    fn all(&self) -> Heard {
        self.settled.and(self.reading)
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

    /// What the responses that have ended cost.
    #[must_use]
    pub const fn cost(&self) -> SessionCost {
        self.cost
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

    /// The request being read has gone out, to a model `listed` says the
    /// price of.
    pub(super) const fn asked(&mut self, listed: Listed) {
        self.pending = match listed {
            Listed::Priced { nothing } => Pending::Priced {
                last: nothing,
                full: false,
            },
            Listed::Unpriced => Pending::Unpriced,
        };
    }

    /// A usage report for the response being read, merged with every earlier
    /// one of it, and what it was priced at.
    pub(super) fn used(&mut self, usage: &ProviderUsage, price: Price) {
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
        self.pending = self.pending.reported(price);
    }

    /// The response being read has ended, after `waited` out; `completed`
    /// where the provider said it was done, rather than it being stopped,
    /// failing or being asked again.
    pub(super) fn answered(&mut self, waited: Duration, completed: bool) {
        let cost = self.pending.ended(completed, self.reading.reported);
        self.cost = self.cost.and(cost);
        self.settled = self.settled.and(self.reading);
        self.reading = Heard::NOTHING;
        self.pending = Pending::Unasked;
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
