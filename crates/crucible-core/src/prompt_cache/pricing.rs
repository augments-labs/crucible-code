//! Versioned provider/model pricing and exact unknown-preserving cost accounting.

use super::{PromptCacheRetentionClass, ProviderUsage};

/// ISO-4217-style currency code compiled into a reviewed pricing record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricingCurrency(&'static str);

impl PricingCurrency {
    /// Creates one static three-letter uppercase currency code.
    ///
    /// # Panics
    ///
    /// Panics when compiled pricing metadata is not exactly three uppercase
    /// ASCII letters.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        let bytes = code.as_bytes();
        assert!(
            matches!(bytes, [b'A'..=b'Z', b'A'..=b'Z', b'A'..=b'Z']),
            "pricing currency must have three uppercase ASCII letters"
        );
        Self(code)
    }

    /// Canonical currency code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Unit in which a published rate is stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricingUnit {
    /// Per one million tokens.
    MillionTokens,
    /// Per one million token-hours of retained storage.
    MillionTokenHours,
    /// A monetary total assembled from rates published in multiple units.
    Mixed,
}

/// Calendar date used for effective pricing selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PricingDate {
    year: u16,
    month: u8,
    day: u8,
}

impl PricingDate {
    /// Creates a validated Gregorian date for a compiled pricing record.
    ///
    /// # Panics
    ///
    /// Panics when compiled pricing metadata is outside the supported
    /// Gregorian date range.
    #[must_use]
    pub const fn new(year: u16, month: u8, day: u8) -> Self {
        assert!(year >= 1970 && year <= 9999, "pricing year is out of range");
        assert!(month >= 1 && month <= 12, "pricing month is out of range");
        assert!(
            day >= 1 && day <= days_in_month(year, month),
            "pricing day is out of range"
        );
        Self { year, month, day }
    }

    /// UTC date containing this non-negative Unix timestamp.
    #[must_use]
    pub fn from_unix_seconds(seconds: u64) -> Self {
        let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
        civil_from_unix_days(days)
    }

    /// Four-digit year.
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }

    /// One-based month.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    /// One-based day.
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }
}

const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

fn civil_from_unix_days(days: i64) -> PricingDate {
    // Howard Hinnant's civil-from-days transform. The timestamp input is
    // non-negative, and the final range check deliberately fails closed at the
    // compiled metadata type's year ceiling.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let year = u16::try_from(year).unwrap_or(u16::MAX).min(9_999);
    PricingDate::new(
        year,
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

/// Exact rate represented as nanocurrency units per published unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceRate {
    nanocurrency_per_unit: u64,
    unit: PricingUnit,
}

impl PriceRate {
    /// A token rate whose integer is one billionth of the record currency.
    #[must_use]
    pub const fn per_million(nanocurrency_per_unit: u64) -> Self {
        Self {
            nanocurrency_per_unit,
            unit: PricingUnit::MillionTokens,
        }
    }

    /// A storage rate per one million token-hours.
    #[must_use]
    pub const fn per_million_token_hours(nanocurrency_per_unit: u64) -> Self {
        Self {
            nanocurrency_per_unit,
            unit: PricingUnit::MillionTokenHours,
        }
    }

    /// Published rate in nanocurrency units.
    #[must_use]
    pub const fn nanocurrency_per_unit(self) -> u64 {
        self.nanocurrency_per_unit
    }

    /// Published quantity unit for this category rate.
    #[must_use]
    pub const fn unit(self) -> PricingUnit {
        self.unit
    }
}

/// Whether and how one usage category can be priced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageRate {
    /// The provider does not charge this category separately.
    NotApplicable,
    /// The category may apply but no reviewed rate is known.
    Unknown,
    /// Both usage and this rate are required for a complete total.
    Priced(PriceRate),
    /// Charge when the provider reports the category; absence is not a gap.
    Optional(PriceRate),
}

impl UsageRate {
    /// A required published rate.
    #[must_use]
    pub const fn priced(rate: PriceRate) -> Self {
        Self::Priced(rate)
    }

    /// A published rate for an optional provider bucket.
    #[must_use]
    pub const fn optional(rate: PriceRate) -> Self {
        Self::Optional(rate)
    }
}

/// Independent rates for one exact provider/model pricing band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheRates {
    /// Ordinary, non-cached input.
    pub uncached_input: UsageRate,
    /// Provider-reported cache reads.
    pub cache_read: UsageRate,
    /// Provider-reported cache creation/writes.
    pub cache_write_or_creation: UsageRate,
    /// Generated output, including reasoning unless reasoning has its own rate.
    pub output: UsageRate,
    /// Separately billed reasoning output.
    pub reasoning: UsageRate,
    /// Persistent cache storage.
    pub storage: UsageRate,
    /// Other provider-labelled billed usage.
    pub other: UsageRate,
}

impl PromptCacheRates {
    /// No reviewed rates.
    pub const UNKNOWN: Self = Self {
        uncached_input: UsageRate::Unknown,
        cache_read: UsageRate::Unknown,
        cache_write_or_creation: UsageRate::Unknown,
        output: UsageRate::Unknown,
        reasoning: UsageRate::Unknown,
        storage: UsageRate::Unknown,
        other: UsageRate::Unknown,
    };
}

/// Exact monetary amount in femtocurrency units (10^-15 currency).
///
/// Published rates are stored in nanocurrency per million tokens. Multiplying
/// that integer by tokens lands exactly in femtocurrency, so no request-size
/// rounding or floating-point drift is introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostAmount {
    currency: PricingCurrency,
    unit: PricingUnit,
    femtocurrency: u128,
}

impl CostAmount {
    /// Creates an exact amount under one pricing currency/unit provenance.
    #[must_use]
    pub const fn new(currency: PricingCurrency, unit: PricingUnit, femtocurrency: u128) -> Self {
        Self {
            currency,
            unit,
            femtocurrency,
        }
    }

    /// Exact amount in 10^-15 currency units.
    #[must_use]
    pub const fn femtocurrency(self) -> u128 {
        self.femtocurrency
    }

    /// Currency of the amount.
    #[must_use]
    pub const fn currency(self) -> PricingCurrency {
        self.currency
    }

    /// Published unit retained for safe aggregation diagnostics.
    #[must_use]
    pub const fn unit(self) -> PricingUnit {
        self.unit
    }

    /// Adds same-currency amounts without wrapping, retaining mixed rate-unit provenance.
    ///
    /// # Errors
    ///
    /// Returns an error when currencies differ or the exact sum overflows.
    pub fn checked_add(self, other: Self) -> Result<Self, PricingError> {
        if self.currency != other.currency {
            return Err(PricingError::CurrencyMismatch);
        }
        let unit = if self.unit == other.unit {
            self.unit
        } else {
            PricingUnit::Mixed
        };
        Ok(Self::new(
            self.currency,
            unit,
            self.femtocurrency
                .checked_add(other.femtocurrency)
                .ok_or(PricingError::Overflow)?,
        ))
    }
}

/// Why reviewed pricing could not be selected or calculated safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PricingError {
    /// More than one equally recent exact record matched.
    #[error("multiple equally effective pricing records matched one provider request")]
    Ambiguous,
    /// Monetary arithmetic exceeded the retained integer representation.
    #[error("prompt-cache cost arithmetic overflowed")]
    Overflow,
    /// Amounts in different currencies were combined.
    #[error("prompt-cache costs in different currencies cannot be combined")]
    CurrencyMismatch,
    /// A usage quantity was paired with a rate published in another unit.
    #[error("prompt-cache usage and pricing rate units do not match")]
    UnitMismatch,
}

/// One versioned price record for an exact route/model/input band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCachePricing {
    protocol: &'static str,
    endpoint: &'static str,
    model: &'static str,
    revision: Option<&'static str>,
    effective_from: PricingDate,
    effective_through: Option<PricingDate>,
    minimum_input_tokens: u64,
    maximum_input_tokens: Option<u64>,
    retention: PromptCacheRetentionClass,
    version: &'static str,
    source_url: &'static str,
    currency: PricingCurrency,
    unit: PricingUnit,
    rates: PromptCacheRates,
}

impl PromptCachePricing {
    /// Creates one open-ended, all-input-size reviewed record.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        protocol: &'static str,
        endpoint: &'static str,
        model: &'static str,
        revision: Option<&'static str>,
        effective_from: PricingDate,
        version: &'static str,
        source_url: &'static str,
        currency: PricingCurrency,
        unit: PricingUnit,
        rates: PromptCacheRates,
    ) -> Self {
        Self {
            protocol,
            endpoint,
            model,
            revision,
            effective_from,
            effective_through: None,
            minimum_input_tokens: 0,
            maximum_input_tokens: None,
            retention: PromptCacheRetentionClass::ProviderDefault,
            version,
            source_url,
            currency,
            unit,
            rates,
        }
    }

    /// Restricts this record to one inclusive input-token band.
    ///
    /// # Panics
    ///
    /// Panics when a compiled maximum is smaller than its minimum.
    #[must_use]
    pub const fn with_input_band(mut self, minimum: u64, maximum: Option<u64>) -> Self {
        assert!(match maximum {
            Some(maximum) => maximum >= minimum,
            None => true,
        });
        self.minimum_input_tokens = minimum;
        self.maximum_input_tokens = maximum;
        self
    }

    /// Restricts this record to the selected neutral retention class.
    #[must_use]
    pub const fn with_retention(mut self, retention: PromptCacheRetentionClass) -> Self {
        self.retention = retention;
        self
    }

    /// Makes this record expire after an inclusive date.
    ///
    /// # Panics
    ///
    /// Panics when a compiled end date precedes the effective start date.
    #[must_use]
    pub const fn through(mut self, date: PricingDate) -> Self {
        assert!(
            date.year > self.effective_from.year
                || (date.year == self.effective_from.year
                    && (date.month > self.effective_from.month
                        || (date.month == self.effective_from.month
                            && date.day >= self.effective_from.day)))
        );
        self.effective_through = Some(date);
        self
    }

    /// Version of the reviewed pricing record.
    #[must_use]
    pub const fn version(self) -> &'static str {
        self.version
    }

    /// Official source URL.
    #[must_use]
    pub const fn source_url(self) -> &'static str {
        self.source_url
    }

    /// Inclusive effective date.
    #[must_use]
    pub const fn effective_from(self) -> PricingDate {
        self.effective_from
    }

    /// Currency of every rate in this record.
    #[must_use]
    pub const fn currency(self) -> PricingCurrency {
        self.currency
    }

    /// Published unit of every rate in this record.
    #[must_use]
    pub const fn unit(self) -> PricingUnit {
        self.unit
    }

    /// Lower inclusive input bound.
    #[must_use]
    pub const fn minimum_input_tokens(self) -> u64 {
        self.minimum_input_tokens
    }

    /// Upper inclusive input bound, or no ceiling.
    #[must_use]
    pub const fn maximum_input_tokens(self) -> Option<u64> {
        self.maximum_input_tokens
    }

    /// Independent category rates.
    #[must_use]
    pub const fn rates(self) -> PromptCacheRates {
        self.rates
    }

    /// Calculates one attempt without converting absent usage/rates into zero.
    ///
    /// # Errors
    ///
    /// Returns an error for mismatched quantity units, currency mismatches, or
    /// exact arithmetic overflow.
    pub fn cost(self, usage: &ProviderUsage) -> Result<UsageCost, PricingError> {
        let (uncached_input, uncached_complete) = self.charge(
            self.rates.uncached_input,
            usage.input.uncached,
            PricingUnit::MillionTokens,
        )?;
        let (cache_read_input, read_complete) = self.charge(
            self.rates.cache_read,
            usage.input.cache_read,
            PricingUnit::MillionTokens,
        )?;
        let (cache_write_input, write_complete) = self.charge(
            self.rates.cache_write_or_creation,
            usage.input.cache_write_or_creation,
            PricingUnit::MillionTokens,
        )?;

        let reasoning_separate = matches!(
            self.rates.reasoning,
            UsageRate::Priced(_) | UsageRate::Optional(_)
        );
        let output_tokens = if reasoning_separate {
            match (usage.output, usage.reasoning) {
                (Some(output), Some(reasoning)) => output.checked_sub(reasoning),
                _ => None,
            }
        } else {
            usage.output
        };
        let (output, output_complete) =
            self.charge(self.rates.output, output_tokens, PricingUnit::MillionTokens)?;
        let (reasoning, reasoning_complete) = self.charge(
            self.rates.reasoning,
            usage.reasoning,
            PricingUnit::MillionTokens,
        )?;
        let (storage, storage_complete) = self.charge(
            self.rates.storage,
            usage.storage_token_hours,
            PricingUnit::MillionTokenHours,
        )?;
        let (other, other_complete) =
            self.charge(self.rates.other, None, PricingUnit::MillionTokens)?;

        let complete = uncached_complete
            && read_complete
            && write_complete
            && output_complete
            && reasoning_complete
            && storage_complete
            && other_complete;
        let total = if complete {
            let mut total: Option<CostAmount> = None;
            for amount in [
                uncached_input,
                cache_read_input,
                cache_write_input,
                output,
                reasoning,
                storage,
                other,
            ]
            .into_iter()
            .flatten()
            {
                total = Some(match total {
                    Some(current) => current.checked_add(amount)?,
                    None => amount,
                });
            }
            Some(total.unwrap_or_else(|| CostAmount::new(self.currency, self.unit, 0)))
        } else {
            None
        };

        Ok(UsageCost {
            uncached_input,
            cache_read_input,
            cache_write_input,
            output,
            reasoning,
            storage,
            other,
            total,
            pricing_version: Some(self.version),
            effective_from: Some(self.effective_from),
            source_url: Some(self.source_url),
            currency: Some(self.currency),
            unit: total.map(CostAmount::unit),
        })
    }

    fn charge(
        self,
        rule: UsageRate,
        quantity: Option<u64>,
        expected_unit: PricingUnit,
    ) -> Result<(Option<CostAmount>, bool), PricingError> {
        let (rate, required) = match rule {
            UsageRate::NotApplicable => return Ok((None, true)),
            UsageRate::Unknown => return Ok((None, false)),
            UsageRate::Priced(rate) => (rate, true),
            UsageRate::Optional(rate) => (rate, false),
        };
        if rate.unit() != expected_unit {
            return Err(PricingError::UnitMismatch);
        }
        let Some(quantity) = quantity else {
            return Ok((None, !required));
        };
        let amount = u128::from(quantity)
            .checked_mul(u128::from(rate.nanocurrency_per_unit))
            .ok_or(PricingError::Overflow)?;
        Ok((
            Some(CostAmount::new(self.currency, rate.unit(), amount)),
            true,
        ))
    }

    fn matches(self, query: PricingQuery<'_>) -> bool {
        if self.protocol != query.protocol
            || self.endpoint != query.endpoint
            || self.model != query.model
            || self.revision != query.revision
            || self.retention != query.retention
            || query.at < self.effective_from
            || self
                .effective_through
                .is_some_and(|through| query.at > through)
        {
            return false;
        }
        match query.input_tokens {
            Some(tokens) => {
                tokens >= self.minimum_input_tokens
                    && self
                        .maximum_input_tokens
                        .is_none_or(|maximum| tokens <= maximum)
            }
            None => self.minimum_input_tokens == 0 && self.maximum_input_tokens.is_none(),
        }
    }
}

/// Exact lookup keys for a provider pricing record.
#[derive(Debug, Clone, Copy)]
pub struct PricingQuery<'a> {
    /// Provider wire protocol.
    pub protocol: &'a str,
    /// Exact endpoint/deployment authority.
    pub endpoint: &'a str,
    /// Requested model ID.
    pub model: &'a str,
    /// Resolved model revision.
    pub revision: Option<&'a str>,
    /// Date on which the attempt is priced.
    pub at: PricingDate,
    /// Provider-visible input total, when reported.
    pub input_tokens: Option<u64>,
    /// Selected retention class whose write rate may differ.
    pub retention: PromptCacheRetentionClass,
}

impl PricingQuery<'_> {
    #[cfg(test)]
    fn fixture(
        model: &'static str,
        at: PricingDate,
        input_tokens: Option<u64>,
    ) -> PricingQuery<'static> {
        PricingQuery {
            protocol: "fixture-protocol",
            endpoint: "https://provider.invalid/v1",
            model,
            revision: Some(model),
            at,
            input_tokens,
            retention: PromptCacheRetentionClass::ProviderDefault,
        }
    }
}

/// Chooses the newest exact effective pricing record.
///
/// # Errors
///
/// Returns an error when equally recent exact records make selection
/// ambiguous.
pub fn select_pricing<'a>(
    records: &'a [PromptCachePricing],
    query: PricingQuery<'_>,
) -> Result<Option<&'a PromptCachePricing>, PricingError> {
    let mut selected: Option<&PromptCachePricing> = None;
    for record in records.iter().filter(|record| record.matches(query)) {
        match selected {
            None => selected = Some(record),
            Some(current) if record.effective_from > current.effective_from => {
                selected = Some(record);
            }
            Some(current) if record.effective_from == current.effective_from => {
                return Err(PricingError::Ambiguous);
            }
            Some(_) => {}
        }
    }
    Ok(selected)
}

/// Unknown-preserving monetary breakdown for one provider attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageCost {
    /// Ordinary input cost.
    pub uncached_input: Option<CostAmount>,
    /// Cache-read input cost.
    pub cache_read_input: Option<CostAmount>,
    /// Cache creation/write input cost.
    pub cache_write_input: Option<CostAmount>,
    /// Non-reasoning or all generated output cost, per record semantics.
    pub output: Option<CostAmount>,
    /// Separately priced reasoning cost.
    pub reasoning: Option<CostAmount>,
    /// Persistent cache storage cost.
    pub storage: Option<CostAmount>,
    /// Other documented category cost.
    pub other: Option<CostAmount>,
    /// Sum only when every applicable category is known.
    pub total: Option<CostAmount>,
    /// Pricing record version, absent when no exact record matched.
    pub pricing_version: Option<&'static str>,
    /// Effective date of the selected record.
    pub effective_from: Option<PricingDate>,
    /// Official price source.
    pub source_url: Option<&'static str>,
    /// Selected currency.
    pub currency: Option<PricingCurrency>,
    /// Unit provenance of the total, including `Mixed` across token and storage rates.
    pub unit: Option<PricingUnit>,
}

impl UsageCost {
    /// No exact pricing record was available.
    pub const UNKNOWN: Self = Self {
        uncached_input: None,
        cache_read_input: None,
        cache_write_input: None,
        output: None,
        reasoning: None,
        storage: None,
        other: None,
        total: None,
        pricing_version: None,
        effective_from: None,
        source_url: None,
        currency: None,
        unit: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputTokenUsage, ProviderUsage};

    const USD: PricingCurrency = PricingCurrency::new("USD");
    const SOURCE: &str = "https://provider.invalid/pricing";

    fn rates() -> PromptCacheRates {
        PromptCacheRates {
            uncached_input: UsageRate::priced(PriceRate::per_million(1_000_000_000)),
            cache_read: UsageRate::priced(PriceRate::per_million(100_000_000)),
            cache_write_or_creation: UsageRate::priced(PriceRate::per_million(1_250_000_000)),
            output: UsageRate::priced(PriceRate::per_million(5_000_000_000)),
            reasoning: UsageRate::NotApplicable,
            storage: UsageRate::NotApplicable,
            other: UsageRate::NotApplicable,
        }
    }

    fn record(model: &'static str, from: PricingDate) -> PromptCachePricing {
        PromptCachePricing::new(
            "fixture-protocol",
            "https://provider.invalid/v1",
            model,
            Some(model),
            from,
            "fixture-pricing-v1",
            SOURCE,
            USD,
            PricingUnit::MillionTokens,
            rates(),
        )
    }

    #[test]
    fn exact_route_model_revision_date_and_input_band_select_one_record() {
        let short =
            record("model-a", PricingDate::new(2026, 1, 1)).with_input_band(0, Some(272_000));
        let long = record("model-a", PricingDate::new(2026, 1, 1)).with_input_band(272_001, None);
        let newer = record("model-a", PricingDate::new(2026, 8, 1));
        let records = [short, long, newer];

        let selected = select_pricing(
            &records,
            PricingQuery {
                protocol: "fixture-protocol",
                endpoint: "https://provider.invalid/v1",
                model: "model-a",
                revision: Some("model-a"),
                at: PricingDate::new(2026, 7, 1),
                input_tokens: Some(272_001),
                retention: crate::PromptCacheRetentionClass::ProviderDefault,
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(selected.minimum_input_tokens(), 272_001);
        assert!(
            select_pricing(
                &records,
                PricingQuery {
                    endpoint: "https://proxy.invalid/v1",
                    ..PricingQuery::fixture("model-a", PricingDate::new(2026, 7, 1), Some(1))
                },
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn costs_are_exact_per_category_and_total_without_double_counting() {
        let usage = ProviderUsage::new(
            InputTokenUsage::disjoint(Some(100), Some(200), Some(300)).unwrap(),
            Some(40),
            None,
            None,
            &[],
        )
        .unwrap();

        let cost = record("model-a", PricingDate::new(2026, 1, 1))
            .cost(&usage)
            .unwrap();

        assert_eq!(
            cost.uncached_input.unwrap().femtocurrency(),
            100_000_000_000
        );
        assert_eq!(
            cost.cache_read_input.unwrap().femtocurrency(),
            20_000_000_000
        );
        assert_eq!(
            cost.cache_write_input.unwrap().femtocurrency(),
            375_000_000_000
        );
        assert_eq!(cost.output.unwrap().femtocurrency(), 200_000_000_000);
        assert_eq!(cost.total.unwrap().femtocurrency(), 695_000_000_000);
    }

    #[test]
    fn storage_uses_token_hours_and_joins_token_costs_in_one_currency_total() {
        let usage = ProviderUsage::new(
            InputTokenUsage::disjoint(Some(100), Some(200), Some(300)).unwrap(),
            Some(40),
            None,
            None,
            &[],
        )
        .unwrap()
        .with_storage_token_hours(50);
        let mut storage_rates = rates();
        storage_rates.storage = UsageRate::priced(PriceRate::per_million_token_hours(20_000_000));
        let pricing = PromptCachePricing::new(
            "fixture-protocol",
            "https://provider.invalid/v1",
            "model-a",
            Some("model-a"),
            PricingDate::new(2026, 1, 1),
            "fixture-pricing-v1",
            SOURCE,
            USD,
            PricingUnit::MillionTokens,
            storage_rates,
        );

        let cost = pricing.cost(&usage).unwrap();

        assert_eq!(cost.storage.unwrap().femtocurrency(), 1_000_000_000);
        assert_eq!(cost.storage.unwrap().unit(), PricingUnit::MillionTokenHours);
        assert_eq!(cost.total.unwrap().femtocurrency(), 696_000_000_000);
        assert_eq!(cost.total.unwrap().unit(), PricingUnit::Mixed);
    }

    #[test]
    fn a_category_rate_with_the_wrong_quantity_unit_is_rejected() {
        let usage = ProviderUsage::new(
            InputTokenUsage::inclusive_read(Some(100), Some(0)).unwrap(),
            Some(10),
            None,
            None,
            &[],
        )
        .unwrap()
        .with_storage_token_hours(50);
        let mut invalid = rates();
        invalid.storage = UsageRate::priced(PriceRate::per_million(20_000_000));
        let pricing = PromptCachePricing::new(
            "fixture-protocol",
            "https://provider.invalid/v1",
            "model-a",
            Some("model-a"),
            PricingDate::new(2026, 1, 1),
            "fixture-pricing-v1",
            SOURCE,
            USD,
            PricingUnit::MillionTokens,
            invalid,
        );

        assert_eq!(pricing.cost(&usage), Err(PricingError::UnitMismatch));
    }

    #[test]
    fn missing_usage_or_rate_keeps_the_category_and_total_unknown() {
        let usage = ProviderUsage::new(
            InputTokenUsage::inclusive_read(Some(100), None).unwrap(),
            Some(10),
            None,
            None,
            &[],
        )
        .unwrap();
        let mut unknown_rates = rates();
        unknown_rates.output = UsageRate::Unknown;
        let pricing = PromptCachePricing::new(
            "fixture-protocol",
            "https://provider.invalid/v1",
            "model-a",
            Some("model-a"),
            PricingDate::new(2026, 1, 1),
            "fixture-pricing-v1",
            SOURCE,
            USD,
            PricingUnit::MillionTokens,
            unknown_rates,
        );

        let cost = pricing.cost(&usage).unwrap();

        assert!(cost.uncached_input.is_none());
        assert!(cost.cache_read_input.is_none());
        assert!(cost.output.is_none());
        assert!(cost.total.is_none());
    }

    #[test]
    fn incompatible_currency_cannot_be_aggregated_and_mixed_rate_units_can() {
        let usd = CostAmount::new(USD, PricingUnit::MillionTokens, 1);
        let eur = CostAmount::new(PricingCurrency::new("EUR"), PricingUnit::MillionTokens, 1);
        let storage = CostAmount::new(USD, PricingUnit::MillionTokenHours, 1);

        assert_eq!(usd.checked_add(eur), Err(PricingError::CurrencyMismatch));
        assert_eq!(usd.checked_add(storage).unwrap().unit(), PricingUnit::Mixed);
    }

    #[test]
    fn unix_date_conversion_handles_epoch_and_leap_days() {
        assert_eq!(
            PricingDate::from_unix_seconds(0),
            PricingDate::new(1970, 1, 1)
        );
        assert_eq!(
            PricingDate::from_unix_seconds(951_782_400),
            PricingDate::new(2000, 2, 29)
        );
    }
}
