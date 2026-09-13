//! What a request cost, in exact amounts that keep unknown categories unknown.
//!
//! Rates and the tables they come from are `crucible-models`; the amounts a
//! rate produces are recorded beside the attempt that incurred them.

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
