//! Versioned provider/model pricing and exact unknown-preserving cost accounting.

use crucible_types::{
    CostAmount, PricingCurrency, PricingDate, PricingError, PricingUnit, PromptCacheRetentionClass,
    ProviderUsage, UsageCost,
};

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
            date.year() > self.effective_from.year()
                || (date.year() == self.effective_from.year()
                    && (date.month() > self.effective_from.month()
                        || (date.month() == self.effective_from.month()
                            && date.day() >= self.effective_from.day())))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_types::{InputTokenUsage, ProviderUsage};

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
                retention: crucible_types::PromptCacheRetentionClass::ProviderDefault,
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
