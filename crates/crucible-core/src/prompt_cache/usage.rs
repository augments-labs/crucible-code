//! Provider-neutral per-attempt token accounting.
//!
//! Provider protocols disagree about whether cache buckets are subsets of an
//! inclusive input total or disjoint values that must be added. Adapters make
//! that decision once at the wire boundary and hand the runner this shape.
//! Missing fields remain `None`; absence is never rewritten as zero.

use super::{PromptCacheOutcome, PromptCacheUsageReporting};

/// Maximum provider-labelled numeric details retained for one usage report.
pub const MAX_PROVIDER_USAGE_DETAILS: usize = 16;
/// Maximum bytes in one static provider detail label.
pub const MAX_PROVIDER_USAGE_DETAIL_LABEL_BYTES: usize = 64;

/// Why a provider usage report could not be normalized safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    /// A subset was larger than its inclusive total.
    #[error("a prompt-cache usage subset exceeded its inclusive input total")]
    SubsetExceedsTotal,
    /// Disjoint or aggregate token counts overflowed `u64`.
    #[error("prompt-cache usage token arithmetic overflowed")]
    Overflow,
    /// A reported aggregate contradicted its known components.
    #[error("provider usage total contradicted its known input/output components")]
    ContradictoryTotal,
    /// More diagnostic numeric fields arrived than the retained bound.
    #[error("provider usage reported too many numeric detail fields")]
    TooManyDetails,
    /// A detail label was empty, unbounded, or not a safe static identifier.
    #[error("provider usage detail label was invalid")]
    InvalidDetailLabel,
}

/// Normalized input-token categories for one provider attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputTokenUsage {
    /// Complete provider-visible input occupying the context window.
    pub total: Option<u64>,
    /// Input not served from a cache, where derivable or reported.
    pub uncached: Option<u64>,
    /// Provider-reported cache-read input.
    pub cache_read: Option<u64>,
    /// Provider-reported cache creation/write input.
    pub cache_write_or_creation: Option<u64>,
}

impl InputTokenUsage {
    /// No input fields were reported.
    pub const UNKNOWN: Self = Self {
        total: None,
        uncached: None,
        cache_read: None,
        cache_write_or_creation: None,
    };

    /// An inclusive total with a documented cached-read subset.
    ///
    /// Used by protocols such as Kimi where `cached_tokens` is part of
    /// `prompt_tokens`, not a second quantity to add. When the subset is
    /// absent, uncached input stays unknown.
    ///
    /// # Errors
    ///
    /// Returns an error when the reported subset exceeds the inclusive total.
    pub fn inclusive_read(total: Option<u64>, read: Option<u64>) -> Result<Self, UsageError> {
        let uncached = match (total, read) {
            (Some(total), Some(read)) => total
                .checked_sub(read)
                .map(Some)
                .ok_or(UsageError::SubsetExceedsTotal)?,
            _ => None,
        };
        Ok(Self {
            total,
            uncached,
            cache_read: read,
            cache_write_or_creation: None,
        })
    }

    /// An inclusive total with documented read and write/creation subsets.
    ///
    /// # Errors
    ///
    /// Returns an error when subsets overflow or exceed the inclusive total.
    pub fn inclusive_read_write(
        total: Option<u64>,
        read: Option<u64>,
        write: Option<u64>,
    ) -> Result<Self, UsageError> {
        let uncached = subtract_subsets(total, read, write)?;
        Ok(Self {
            total,
            uncached,
            cache_read: read,
            cache_write_or_creation: write,
        })
    }

    /// Disjoint uncached, read, and write/creation buckets.
    ///
    /// A total is derived only when every bucket is present. A missing cache
    /// field is unknown, not a reported zero.
    ///
    /// # Errors
    ///
    /// Returns an error when the disjoint sum overflows.
    pub fn disjoint(
        uncached: Option<u64>,
        read: Option<u64>,
        write: Option<u64>,
    ) -> Result<Self, UsageError> {
        let total = match (uncached, read, write) {
            (Some(uncached), Some(read), Some(write)) => Some(
                uncached
                    .checked_add(read)
                    .and_then(|value| value.checked_add(write))
                    .ok_or(UsageError::Overflow)?,
            ),
            _ => None,
        };
        Ok(Self {
            total,
            uncached,
            cache_read: read,
            cache_write_or_creation: write,
        })
    }

    /// Provider-reported outcome under the capability's reporting contract.
    #[must_use]
    pub fn outcome(self, reporting: PromptCacheUsageReporting) -> PromptCacheOutcome {
        let read = self.cache_read;
        let write = self.cache_write_or_creation;
        if read.is_some_and(|tokens| tokens > 0) && write.is_some_and(|tokens| tokens > 0) {
            return PromptCacheOutcome::ReadAndWrite;
        }
        if read.is_some_and(|tokens| tokens > 0) {
            return PromptCacheOutcome::Read;
        }
        if write.is_some_and(|tokens| tokens > 0) {
            return PromptCacheOutcome::Write;
        }

        let complete_zero = match reporting {
            PromptCacheUsageReporting::None => false,
            PromptCacheUsageReporting::ReadTokens => read == Some(0),
            PromptCacheUsageReporting::ReadAndWriteTokens => read == Some(0) && write == Some(0),
        };
        if complete_zero {
            PromptCacheOutcome::NoActivity
        } else {
            PromptCacheOutcome::Unreported
        }
    }
}

fn subtract_subsets(
    total: Option<u64>,
    read: Option<u64>,
    write: Option<u64>,
) -> Result<Option<u64>, UsageError> {
    if let Some(total) = total
        && (read.is_some_and(|read| read > total) || write.is_some_and(|write| write > total))
    {
        return Err(UsageError::SubsetExceedsTotal);
    }
    match (total, read, write) {
        (Some(total), Some(read), Some(write)) => {
            let cached = read.checked_add(write).ok_or(UsageError::Overflow)?;
            total
                .checked_sub(cached)
                .map(Some)
                .ok_or(UsageError::SubsetExceedsTotal)
        }
        _ => Ok(None),
    }
}

/// One bounded, provider-labelled numeric explanation field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderNumericDetail {
    /// Static adapter-owned label, never provider-controlled response text.
    pub label: &'static str,
    /// Reported numeric value.
    pub value: u64,
}

impl ProviderNumericDetail {
    /// Validates a static detail label before retaining it.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or non-identifier label.
    pub fn new(label: &'static str, value: u64) -> Result<Self, UsageError> {
        if label.is_empty()
            || label.len() > MAX_PROVIDER_USAGE_DETAIL_LABEL_BYTES
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(UsageError::InvalidDetailLabel);
        }
        Ok(Self { label, value })
    }
}

/// Normalized usage for one provider response/attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUsage {
    /// Provider-visible input categories.
    pub input: InputTokenUsage,
    /// Generated output tokens, where reported.
    pub output: Option<u64>,
    /// Reasoning tokens, where separately reported. This is ordinarily a
    /// subset of output and is never added a second time.
    pub reasoning: Option<u64>,
    /// Complete input plus output, where reported or safely derivable.
    pub total: Option<u64>,
    /// Persistent cached-content storage in whole token-hours, where the
    /// provider reports it or an adapter can derive it without rounding.
    pub storage_token_hours: Option<u64>,
    details: Box<[ProviderNumericDetail]>,
}

impl ProviderUsage {
    /// A partial or complete normalized report.
    ///
    /// # Errors
    ///
    /// Returns an error for contradictory totals, invalid detail fields,
    /// impossible subsets, or arithmetic overflow.
    pub fn new(
        input: InputTokenUsage,
        output: Option<u64>,
        reasoning: Option<u64>,
        reported_total: Option<u64>,
        details: &[ProviderNumericDetail],
    ) -> Result<Self, UsageError> {
        if details.len() > MAX_PROVIDER_USAGE_DETAILS {
            return Err(UsageError::TooManyDetails);
        }
        for detail in details {
            ProviderNumericDetail::new(detail.label, detail.value)?;
        }
        if let (Some(reasoning), Some(output)) = (reasoning, output)
            && reasoning > output
        {
            return Err(UsageError::SubsetExceedsTotal);
        }
        let derived = match (input.total, output) {
            (Some(input), Some(output)) => {
                Some(input.checked_add(output).ok_or(UsageError::Overflow)?)
            }
            _ => None,
        };
        if let (Some(reported), Some(derived)) = (reported_total, derived)
            && reported != derived
        {
            return Err(UsageError::ContradictoryTotal);
        }
        if let Some(reported) = reported_total
            && (input.total.is_some_and(|input| input > reported)
                || output.is_some_and(|output| output > reported))
        {
            return Err(UsageError::ContradictoryTotal);
        }
        Ok(Self {
            input,
            output,
            reasoning,
            total: reported_total.or(derived),
            storage_token_hours: None,
            details: details.into(),
        })
    }

    /// Adds a documented whole-token-hour storage quantity.
    #[must_use]
    pub const fn with_storage_token_hours(mut self, token_hours: u64) -> Self {
        self.storage_token_hours = Some(token_hours);
        self
    }

    /// Provider-labelled numeric fields retained under the fixed bound.
    #[must_use]
    pub fn details(&self) -> &[ProviderNumericDetail] {
        &self.details
    }

    /// Combines partial reports from one attempt, with newly reported fields
    /// replacing prior levels and absent fields preserving what was known.
    ///
    /// # Errors
    ///
    /// Returns an error when the merged report would be contradictory,
    /// invalid, or exceed a retained bound.
    pub fn merged(&self, newer: &Self) -> Result<Self, UsageError> {
        let input = InputTokenUsage {
            total: newer.input.total.or(self.input.total),
            uncached: newer.input.uncached.or(self.input.uncached),
            cache_read: newer.input.cache_read.or(self.input.cache_read),
            cache_write_or_creation: newer
                .input
                .cache_write_or_creation
                .or(self.input.cache_write_or_creation),
        };
        let mut details = self.details.to_vec();
        for detail in newer.details.iter().copied() {
            if let Some(existing) = details.iter_mut().find(|one| one.label == detail.label) {
                *existing = detail;
            } else {
                details.push(detail);
            }
        }
        let mut merged = Self::new(
            input,
            newer.output.or(self.output),
            newer.reasoning.or(self.reasoning),
            newer.total.or(self.total),
            &details,
        )?;
        merged.storage_token_hours = newer.storage_token_hours.or(self.storage_token_hours);
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inclusive_cached_input_is_subtracted_not_added() {
        let input = InputTokenUsage::inclusive_read(Some(900), Some(700)).unwrap();

        assert_eq!(input.total, Some(900));
        assert_eq!(input.uncached, Some(200));
        assert_eq!(input.cache_read, Some(700));
    }

    #[test]
    fn disjoint_anthropic_buckets_make_one_total() {
        let input = InputTokenUsage::disjoint(Some(200), Some(700), Some(300)).unwrap();
        assert_eq!(input.total, Some(1_200));
    }

    #[test]
    fn missing_details_and_partial_buckets_stay_unknown() {
        assert_eq!(
            InputTokenUsage::inclusive_read(Some(900), None)
                .unwrap()
                .uncached,
            None
        );
        assert_eq!(
            InputTokenUsage::disjoint(Some(200), None, Some(300))
                .unwrap()
                .total,
            None
        );
        assert_eq!(
            InputTokenUsage::inclusive_read_write(Some(900), Some(200), None)
                .unwrap()
                .uncached,
            None
        );
        assert_eq!(
            InputTokenUsage::inclusive_read_write(Some(900), None, Some(200))
                .unwrap()
                .uncached,
            None
        );
    }

    #[test]
    fn impossible_relationships_and_overflow_are_rejected() {
        assert_eq!(
            InputTokenUsage::inclusive_read(Some(10), Some(11)),
            Err(UsageError::SubsetExceedsTotal)
        );
        assert_eq!(
            InputTokenUsage::disjoint(Some(u64::MAX), Some(1), Some(0)),
            Err(UsageError::Overflow)
        );
    }

    #[test]
    fn outcomes_require_provider_reported_activity_or_complete_zeroes() {
        let unknown = InputTokenUsage::inclusive_read(Some(100), None).unwrap();
        let miss = InputTokenUsage::inclusive_read(Some(100), Some(0)).unwrap();
        let hit = InputTokenUsage::inclusive_read(Some(100), Some(80)).unwrap();

        assert_eq!(
            unknown.outcome(PromptCacheUsageReporting::ReadTokens),
            PromptCacheOutcome::Unreported
        );
        assert_eq!(
            miss.outcome(PromptCacheUsageReporting::ReadTokens),
            PromptCacheOutcome::NoActivity
        );
        assert_eq!(
            hit.outcome(PromptCacheUsageReporting::ReadTokens),
            PromptCacheOutcome::Read
        );
    }

    #[test]
    fn reasoning_is_a_subset_and_totals_are_checked() {
        let input = InputTokenUsage::inclusive_read(Some(10), Some(0)).unwrap();
        assert!(ProviderUsage::new(input, Some(5), Some(6), None, &[]).is_err());
        assert!(ProviderUsage::new(input, Some(5), Some(3), Some(14), &[]).is_err());
        assert_eq!(
            ProviderUsage::new(input, Some(5), Some(3), None, &[])
                .unwrap()
                .total,
            Some(15)
        );
    }

    #[test]
    fn provider_numeric_details_are_bounded_and_static_safe_labels() {
        assert!(ProviderNumericDetail::new("cached_tokens", 1).is_ok());
        assert_eq!(
            ProviderNumericDetail::new("not safe", 1),
            Err(UsageError::InvalidDetailLabel)
        );
        let too_many = vec![ProviderNumericDetail::new("count", 1).unwrap(); 17];
        assert_eq!(
            ProviderUsage::new(InputTokenUsage::UNKNOWN, None, None, None, &too_many),
            Err(UsageError::TooManyDetails)
        );
    }
}
