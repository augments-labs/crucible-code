//! Provider-neutral per-attempt token accounting.
//!
//! Provider protocols disagree about whether cache buckets are subsets of an
//! inclusive input total or disjoint values that must be added. Adapters make
//! that decision once at the wire boundary and hand the runner this shape.
//! Missing fields remain `None`; absence is never rewritten as zero.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::{PromptCacheOutcome, PromptCacheUsageReporting};

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

/// What a response has cost, counted in the tokens the model produced.
///
/// Output only. What a request carries is settled before it is sent and is the
/// same however long the answer takes, so it says nothing about the answer
/// somebody is currently waiting on — and one number that goes up while you
/// watch it is worth more than two that need adding.
///
/// A provider says this about the response it is in the middle of, as often as
/// it likes, each reading replacing the last. What a whole turn spent is the
/// sum over its responses, which is the runner's to add because the turn is
/// the runner's: a provider is asked several times and is told nothing about
/// the turn around those requests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spend(u64);

impl Spend {
    /// Nothing spent yet.
    pub const NONE: Self = Self(0);

    /// A reading of `tokens` produced.
    #[must_use]
    pub const fn new(tokens: u64) -> Self {
        Self(tokens)
    }

    /// How many tokens that is.
    #[must_use]
    pub const fn tokens(self) -> u64 {
        self.0
    }

    /// This and `other` together.
    ///
    /// Saturating, because a count that wrapped would read as a turn that spent
    /// nothing at the moment it spent the most.
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

/// What one response reported about itself, kept for a session picked up later.
///
/// A log holds messages, and messages alone say what a session *is* without
/// saying what any of it cost — so a session continued has always had to
/// estimate its own load until the first response of the new run reported one.
/// This is the fact that closes that gap: the four numbers a provider's report
/// and the request behind it come to, together, and covering exactly the
/// transcript that stood when it was written.
///
/// It is a fact about a request rather than about a session. Nothing here says
/// which model produced it or which transcript it covered — what makes it
/// usable again is the position it was written at, and that belongs to whoever
/// keeps it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Calibration {
    /// What that request carried.
    pub carried: Carried,
    /// What the response to it produced.
    pub spent: Spend,
    /// The request's content in bytes, which is what `carried` was counted
    /// over. The two together are this model's own bytes per token on this
    /// session's own text.
    pub sent: u64,
    /// The fixed part of those bytes: system instructions and tool schemas.
    pub overhead: u64,
}

/// What one request carried to the model, counted in tokens.
///
/// The other half of what a usage reading holds, and the half [`Spend`] is not:
/// that one counts what the model produced, and this counts what it was sent.
/// Both arrive in the same object from every provider crucible speaks, and only
/// one of them was ever read.
///
/// It is a **level rather than a total**, which is the whole of how it differs
/// from [`Spend`] and why it has no `and`. crucible holds no conversation state
/// at a vendor and sends the transcript whole on every request, so each reading
/// is what *that* request carried and the next one supersedes it. Adding two
/// together would count the same transcript twice and say a session was fuller
/// than it is — which, since this is what compaction is decided on, is the one
/// error here that spends somebody's context for them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Carried(u64);

impl Carried {
    /// Nothing reported yet.
    pub const NONE: Self = Self(0);

    /// A reading of `tokens` carried.
    #[must_use]
    pub const fn new(tokens: u64) -> Self {
        Self(tokens)
    }

    /// How many tokens that is.
    #[must_use]
    pub const fn tokens(self) -> u64 {
        self.0
    }
}

/// How long one of a plan's usage windows lasts, in crucible's own words.
///
/// A vendor that reports a window says how long it is, and [`Window::of`]
/// names that length: within 5% of five hours, a day, a week or a year it is
/// that window, and 28 to 31 days is a month. Any other length is kept as
/// itself and named by its length rounded, so a label on screen is never text
/// a response chose, and which slot a vendor reported a window in never
/// decides what it is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    /// A window of five hours.
    FiveHour,
    /// A window of a day.
    Daily,
    /// A window of a week.
    Weekly,
    /// A window of a month, 28 to 31 days.
    Monthly,
    /// A window of a year.
    Yearly,
    /// A window of any other length, in minutes.
    Lasting(u32),
}

/// Minutes in an hour, which window lengths are counted in.
const HOUR: u64 = 60;
/// Minutes in a day.
const DAY: u64 = 24 * HOUR;

impl Window {
    /// The window `minutes` long, named by its length; `None` for a window of
    /// no length, which is not one.
    #[must_use]
    pub fn of(minutes: u64) -> Option<Self> {
        // Within 5% of `length`, compared in whole minutes so no length is
        // rounded into a name it is not near.
        let near = |length: u64| minutes.abs_diff(length).saturating_mul(20) <= length;
        Some(match minutes {
            0 => return None,
            _ if near(5 * HOUR) => Self::FiveHour,
            _ if near(DAY) => Self::Daily,
            _ if near(7 * DAY) => Self::Weekly,
            _ if (28 * DAY..=31 * DAY).contains(&minutes) => Self::Monthly,
            _ if near(365 * DAY) => Self::Yearly,
            _ => Self::Lasting(u32::try_from(minutes).unwrap_or(u32::MAX)),
        })
    }

    /// How many minutes the window lasts, a month counted as thirty days:
    /// what windows are put in order by.
    #[must_use]
    pub const fn minutes(self) -> u64 {
        match self {
            Self::FiveHour => 5 * HOUR,
            Self::Daily => DAY,
            Self::Weekly => 7 * DAY,
            Self::Monthly => 30 * DAY,
            Self::Yearly => 365 * DAY,
            Self::Lasting(minutes) => minutes as u64,
        }
    }

    /// What the window is called inside a sentence: crucible's name for it,
    /// never words a response chose.
    #[must_use]
    pub fn named(self) -> String {
        match self {
            Self::FiveHour => "5-hour window".to_owned(),
            Self::Daily => "daily window".to_owned(),
            Self::Weekly => "weekly window".to_owned(),
            Self::Monthly => "monthly window".to_owned(),
            Self::Yearly => "yearly window".to_owned(),
            Self::Lasting(minutes) => {
                let minutes = u64::from(minutes);
                let hours = minutes.saturating_add(HOUR / 2) / HOUR;
                if minutes < HOUR {
                    format!("{minutes}-minute window")
                } else if hours < 48 {
                    format!("{hours}-hour window")
                } else {
                    let days = minutes.saturating_add(DAY / 2) / DAY;
                    format!("{days}-day window")
                }
            }
        }
    }

    /// Where the window falls among others of different names but one
    /// length, which only a length kept as itself can share.
    const fn rank(self) -> u8 {
        match self {
            Self::FiveHour => 0,
            Self::Daily => 1,
            Self::Weekly => 2,
            Self::Monthly => 3,
            Self::Yearly => 4,
            Self::Lasting(_) => 5,
        }
    }
}

/// Shortest first, which is the fixed order windows are shown and read in.
impl Ord for Window {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.minutes(), self.rank()).cmp(&(other.minutes(), other.rank()))
    }
}

impl PartialOrd for Window {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// How much of one window is used, in the vendor's measure of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Used {
    /// A share of the window, from 0 to 100.
    Percent(u8),
    /// `used` of the `total` the window allows, `used` never past `total`.
    Counted {
        /// How many have been used.
        used: u64,
        /// How many the window allows, never 0.
        total: u64,
    },
    /// A window the vendor says has no limit.
    Unlimited,
}

/// How much of one window has been used, and when it starts again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowReading {
    used: Used,
    resets_at: Option<SystemTime>,
}

impl WindowReading {
    /// `percent` of the window used, saturating at 100, resetting at
    /// `resets_at` where the vendor said when.
    #[must_use]
    pub fn new(percent: u8, resets_at: Option<SystemTime>) -> Self {
        Self {
            used: Used::Percent(percent.min(100)),
            resets_at,
        }
    }

    /// `used` of the `total` the window allows, saturating at the total;
    /// `None` for a total of 0, a window with nothing in it to use.
    #[must_use]
    pub fn counted(used: u64, total: u64, resets_at: Option<SystemTime>) -> Option<Self> {
        (total > 0).then_some(Self {
            used: Used::Counted {
                used: used.min(total),
                total,
            },
            resets_at,
        })
    }

    /// A window the vendor says has no limit, which has no reset to wait for.
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            used: Used::Unlimited,
            resets_at: None,
        }
    }

    /// How much of the window is used, in the vendor's measure.
    #[must_use]
    pub const fn used(self) -> Used {
        self.used
    }

    /// How much of the window is used, from 0 to 100; a count is its share of
    /// the total, rounded down, and an unlimited window is 0.
    #[must_use]
    pub fn percent(self) -> u8 {
        match self.used {
            Used::Percent(percent) => percent,
            Used::Counted { used, total } => {
                let share = u128::from(used).saturating_mul(100) / u128::from(total.max(1));
                u8::try_from(share.min(100)).unwrap_or(100)
            }
            Used::Unlimited => 0,
        }
    }

    /// Whether nothing more of the window can be used: 100%, or the whole of
    /// its count. An unlimited window never is.
    #[must_use]
    pub const fn spent(self) -> bool {
        match self.used {
            Used::Percent(percent) => percent >= 100,
            Used::Counted { used, total } => used >= total,
            Used::Unlimited => false,
        }
    }

    /// When the window starts again, where the vendor said.
    #[must_use]
    pub const fn resets_at(self) -> Option<SystemTime> {
        self.resets_at
    }
}

/// Most bytes a vendor's name for a group of limits is kept to, and the id of
/// the model a group is kept for.
pub const MAX_LIMIT_NAME_BYTES: usize = 64;
/// Most groups of limits one reading keeps, the plan-wide group among them.
pub const MAX_LIMIT_GROUPS: usize = 8;
/// Most windows one group of limits keeps.
pub const MAX_GROUP_WINDOWS: usize = 6;

/// A vendor's name for a group of limits, kept to be shown and to tell one
/// group from another; which requests a group holds back is its
/// [`ModelKey`]'s to say.
///
/// It is text a response chose, so it is never read for meaning: control
/// characters and Unicode format characters, which would reorder or hide what
/// is drawn, are taken out, it is cut to [`MAX_LIMIT_NAME_BYTES`] on a
/// character's boundary and ends in `…` where it was, and a name with nothing
/// left is not one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupName(Box<str>);

impl GroupName {
    /// The name `said` is, once kept; `None` where nothing of it is left.
    #[must_use]
    pub fn new(said: &str) -> Option<Self> {
        const CUT: &str = "…";
        let mut kept = String::new();
        let mut whole = true;
        for character in said.trim().chars().filter(|&c| !unshown(c)) {
            if kept.len().saturating_add(character.len_utf8()) > MAX_LIMIT_NAME_BYTES {
                whole = false;
                break;
            }
            kept.push(character);
        }
        if !whole {
            while kept.len().saturating_add(CUT.len()) > MAX_LIMIT_NAME_BYTES {
                kept.pop();
            }
            kept.push_str(CUT);
        }
        let kept = kept.trim();
        (!kept.is_empty()).then(|| Self(kept.into()))
    }

    /// The name as kept.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `character` is a control character, or a Unicode format character
/// (general category `Cf`): the bidi marks, embeddings, overrides and
/// isolates, the zero-width characters and the byte order mark among them.
/// Drawn, either reorders or hides the text around it. U+2065, unassigned
/// between the invisible operators and the isolates, is taken with them.
const fn unshown(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{890}'..='\u{891}'
                | '\u{8e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

/// Which requests a model's group of limits holds back, as the provider
/// module that read the group said.
///
/// Kept apart from the group's name, which is only ever drawn: the key is
/// what a request's model id is matched against, and the name is never read
/// for it.
///
/// Built only by [`ModelKey::exact`], which bounds the id. A key is kept in
/// a reading as long as the session, so one built around the constructor
/// would carry an unbounded id past every ceiling a reading keeps to:
///
/// ```compile_fail,E0599
/// use crucible_types::ModelKey;
///
/// let forged = ModelKey::Exact("\u{7}".repeat(4096).into());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelKey(Matching);

/// How a [`ModelKey`] matches a request's model id: private, so no key is
/// built past the bounds its constructor holds it to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Matching {
    /// A request to the model whose id is exactly this, spelled as the
    /// request spells it.
    Exact(Box<str>),
}

impl ModelKey {
    /// The model whose id is exactly `id`; `None` for an id that is empty,
    /// holds a control or a Unicode format character, or is longer than
    /// [`MAX_LIMIT_NAME_BYTES`]: no model's id is, and one cut to fit would be
    /// another model's.
    #[must_use]
    pub fn exact(id: &str) -> Option<Self> {
        (!id.is_empty() && id.len() <= MAX_LIMIT_NAME_BYTES && !id.chars().any(unshown))
            .then(|| Self(Matching::Exact(id.into())))
    }

    /// Whether a request to the model `model` is one this holds back.
    #[must_use]
    pub fn holds(&self, model: &str) -> bool {
        match &self.0 {
            Matching::Exact(id) => **id == *model,
        }
    }

    /// The one model this names, where it names one: what a reader looks
    /// the group's model up by.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        match &self.0 {
            Matching::Exact(id) => Some(id),
        }
    }
}

/// A group of limits a vendor keeps for one model: the name it is drawn
/// with, and which requests it holds back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelGroup {
    name: GroupName,
    key: Option<ModelKey>,
}

impl ModelGroup {
    /// The group drawn as `name`, holding back the requests `key` says; with
    /// no key, it holds back none by itself.
    #[must_use]
    pub const fn new(name: GroupName, key: Option<ModelKey>) -> Self {
        Self { name, key }
    }

    /// What the vendor called the group, to be drawn.
    #[must_use]
    pub const fn name(&self) -> &GroupName {
        &self.name
    }

    /// Which requests it holds back, where the provider module said.
    #[must_use]
    pub const fn key(&self) -> Option<&ModelKey> {
        self.key.as_ref()
    }

    /// Whether a request to `model` is one this group holds back.
    #[must_use]
    pub fn holds(&self, model: &str) -> bool {
        self.key.as_ref().is_some_and(|key| key.holds(model))
    }
}

/// Whom a group of limits holds back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Every request the sign-in makes, whichever model it asks.
    Plan,
    /// Requests to one model only.
    ///
    /// Which model is the provider module's to say, by the group's
    /// [`ModelKey`]: a request to any other model is not held back by it,
    /// and neither is one to a model named only in its drawn name.
    Model(ModelGroup),
}

/// One group of a plan's limits: whom it holds back, and its windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitGroup {
    scope: Scope,
    /// Shortest first, one reading per window, at most [`MAX_GROUP_WINDOWS`].
    windows: Vec<(Window, WindowReading)>,
}

impl LimitGroup {
    /// Whom the group holds back.
    #[must_use]
    pub const fn scope(&self) -> &Scope {
        &self.scope
    }

    /// Every window the group reports, shortest first whatever order the
    /// vendor named them in.
    pub fn windows(&self) -> impl Iterator<Item = (Window, WindowReading)> + '_ {
        self.windows.iter().copied()
    }

    /// What was read of `window`, if it was reported.
    #[must_use]
    pub fn reading(&self, window: Window) -> Option<WindowReading> {
        self.windows
            .iter()
            .find(|(each, _)| *each == window)
            .map(|(_, reading)| *reading)
    }

    /// This, with `window` read as `reading`: a window read twice keeps the
    /// later reading, and one past the ceiling is not kept. Whether it was.
    fn put(&mut self, window: Window, reading: WindowReading) -> bool {
        match self.windows.binary_search_by(|(each, _)| each.cmp(&window)) {
            Ok(at) => {
                if let Some(slot) = self.windows.get_mut(at) {
                    slot.1 = reading;
                }
                true
            }
            Err(at) if self.windows.len() < MAX_GROUP_WINDOWS => {
                self.windows.insert(at, (window, reading));
                true
            }
            Err(_) => false,
        }
    }
}

/// What a sign-in's plan is known to allow and to have used.
///
/// Groups of windows: the plan-wide group first, where one was reported, and
/// then a group for each model the vendor limits on its own, in the order
/// they were first read. Every part is bounded — [`MAX_LIMIT_GROUPS`] groups,
/// [`MAX_GROUP_WINDOWS`] windows in each, and a name of
/// [`MAX_LIMIT_NAME_BYTES`] — and what arrives past a ceiling is not kept,
/// but is said to have been: see [`PlanWindows::incomplete`].
///
/// It also holds the moment the reading arrived, so a reader can tell how old
/// the figures are. A later reading replaces the groups it names and leaves
/// the others: see [`PlanWindows::merge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanWindows {
    groups: Vec<LimitGroup>,
    arrived: SystemTime,
    /// Whether the vendor reported a group or a window not kept here.
    incomplete: bool,
}

impl PlanWindows {
    /// No window yet, read from a response that arrived at `arrived`.
    #[must_use]
    pub const fn new(arrived: SystemTime) -> Self {
        Self {
            groups: Vec::new(),
            arrived,
            incomplete: false,
        }
    }

    /// This, with the plan-wide `window` read as `reading`.
    #[must_use]
    pub fn with(self, window: Window, reading: WindowReading) -> Self {
        self.within(Scope::Plan, window, reading)
    }

    /// This, with `window` of the group `scope` read as `reading`; a window
    /// read twice keeps the later reading, and a group or a window past its
    /// ceiling is not kept.
    #[must_use]
    pub fn within(mut self, scope: Scope, window: Window, reading: WindowReading) -> Self {
        if let Some(group) = self.groups.iter_mut().find(|group| group.scope == scope) {
            self.incomplete |= !group.put(window, reading);
            return self;
        }
        let mut group = LimitGroup {
            scope,
            windows: Vec::new(),
        };
        self.incomplete |= !group.put(window, reading);
        self.add(group);
        self
    }

    /// This, saying the vendor reported more than a reader kept: what a
    /// reader that stopped reading at a ceiling of its own says.
    #[must_use]
    pub const fn cut(mut self) -> Self {
        self.incomplete = true;
        self
    }

    /// Whether the vendor reported a group or a window that is not here
    /// because a ceiling left it out: these are some of the plan's limits,
    /// not all of them.
    #[must_use]
    pub const fn incomplete(&self) -> bool {
        self.incomplete
    }

    /// Every group, the plan-wide one first.
    pub fn groups(&self) -> impl Iterator<Item = &LimitGroup> + '_ {
        self.groups.iter()
    }

    /// What was read of the plan-wide `window`, if it was reported.
    #[must_use]
    pub fn reading(&self, window: Window) -> Option<WindowReading> {
        self.groups
            .iter()
            .find(|group| group.scope == Scope::Plan)
            .and_then(|group| group.reading(window))
    }

    /// Whether no window was reported at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.windows.is_empty())
    }

    /// When the reading arrived.
    #[must_use]
    pub const fn arrived(&self) -> SystemTime {
        self.arrived
    }

    /// This, brought up to date by `later`: each group `later` reports
    /// replaces the one of the same scope, and a group it does not name is
    /// kept as it was. A response's head names only the groups it is about,
    /// so a group it is silent on is not one the plan stopped having, and a
    /// reading that left one out is not made whole by one silent on it: the
    /// two are incomplete where either was.
    #[must_use]
    pub fn merge(mut self, later: Self) -> Self {
        self.incomplete |= later.incomplete;
        for group in later.groups {
            match self
                .groups
                .iter_mut()
                .find(|each| each.scope == group.scope)
            {
                Some(kept) => *kept = group,
                None => self.add(group),
            }
        }
        self.arrived = later.arrived;
        self
    }

    /// The window that holds back a request to `model` as of `now`, and when
    /// it starts again: a used-up plan-wide window, or a used-up window of a
    /// group whose [`ModelKey`] holds `model`. A group kept for another model,
    /// or with no key, holds back nothing asked of this one.
    ///
    /// Used up is spent with a reset still to come. A spent window with no
    /// reset is not one: when it starts again is not known, so whether it
    /// already has is not either, and only the vendor's refusal can say. Of
    /// several, the one that starts again last, since nothing can be sent
    /// before it does.
    #[must_use]
    pub fn exhausted(&self, model: &str, now: SystemTime) -> Option<(Window, SystemTime)> {
        self.groups
            .iter()
            .filter(|group| match &group.scope {
                Scope::Plan => true,
                Scope::Model(group) => group.holds(model),
            })
            .flat_map(LimitGroup::windows)
            .filter(|(_, reading)| reading.spent())
            .filter_map(|(window, reading)| {
                reading
                    .resets_at()
                    .filter(|at| *at > now)
                    .map(|at| (window, at))
            })
            .max_by_key(|(_, at)| *at)
    }

    /// Keeps `group` where there is room: the plan-wide group first, every
    /// other after the ones already kept.
    fn add(&mut self, group: LimitGroup) {
        if self.groups.len() >= MAX_LIMIT_GROUPS {
            self.incomplete = true;
            return;
        }
        if group.scope == Scope::Plan {
            self.groups.insert(0, group);
        } else {
            self.groups.push(group);
        }
    }
}

/// An instant written as ISO 8601 in UTC, to the second:
/// `2026-10-05T09:00:00Z`.
///
/// For a reader that is not somebody at a terminal, which is why no zone is
/// read: the same instant is spelled the same on every machine. An instant
/// before 1970 is written as its first second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utc(SystemTime);

impl Utc {
    /// `at`, to be written.
    #[must_use]
    pub const fn new(at: SystemTime) -> Self {
        Self(at)
    }
}

impl fmt::Display for Utc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self
            .0
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let date = crate::PricingDate::from_unix_seconds(seconds);
        let time = seconds % 86_400;
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            date.year(),
            date.month(),
            date.day(),
            time / 3_600,
            time % 3_600 / 60,
            time % 60
        )
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

    #[test]
    fn a_window_reported_past_full_reads_as_full() {
        assert_eq!(WindowReading::new(150, None).percent(), 100);
        assert_eq!(WindowReading::new(100, None).percent(), 100);
        assert_eq!(WindowReading::new(31, None).percent(), 31);
    }

    #[test]
    fn windows_read_in_the_fixed_order_whatever_order_they_arrived_in() {
        let reading = |percent| WindowReading::new(percent, None);
        let windows = PlanWindows::new(SystemTime::UNIX_EPOCH)
            .with(Window::Monthly, reading(9))
            .with(Window::FiveHour, reading(12))
            .with(Window::Weekly, reading(31));

        let read: Vec<_> = windows
            .groups()
            .flat_map(LimitGroup::windows)
            .map(|(window, reading)| (window, reading.percent()))
            .collect();
        assert_eq!(
            read,
            [
                (Window::FiveHour, 12),
                (Window::Weekly, 31),
                (Window::Monthly, 9)
            ]
        );
        assert!(!windows.is_empty());
        assert!(PlanWindows::new(SystemTime::UNIX_EPOCH).is_empty());
    }

    #[test]
    fn a_window_read_twice_keeps_one_reading_the_later() {
        let windows = PlanWindows::new(SystemTime::UNIX_EPOCH)
            .with(Window::Weekly, WindowReading::new(31, None))
            .with(Window::Weekly, WindowReading::new(40, None));

        assert_eq!(windows.groups().flat_map(LimitGroup::windows).count(), 1);
        assert_eq!(
            windows.reading(Window::Weekly).map(WindowReading::percent),
            Some(40)
        );
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(seconds)
    }

    #[test]
    fn plan_limit_a_window_at_a_hundred_with_its_reset_to_come_is_exhausted() {
        let now = at(1_000);
        let windows = PlanWindows::new(now)
            .with(Window::FiveHour, WindowReading::new(100, Some(at(2_000))))
            .with(Window::Weekly, WindowReading::new(40, Some(at(9_000))));

        assert_eq!(
            windows.exhausted(MODEL, now),
            Some((Window::FiveHour, at(2_000)))
        );
    }

    #[test]
    fn plan_limit_of_several_used_up_the_one_that_starts_again_last_is_named() {
        let now = at(1_000);
        let windows = PlanWindows::new(now)
            .with(Window::FiveHour, WindowReading::new(100, Some(at(2_000))))
            .with(Window::Weekly, WindowReading::new(100, Some(at(9_000))))
            .with(Window::Monthly, WindowReading::new(100, Some(at(5_000))));

        assert_eq!(
            windows.exhausted(MODEL, now),
            Some((Window::Weekly, at(9_000)))
        );
    }

    #[test]
    fn plan_limit_short_of_a_hundred_past_its_reset_or_with_none_is_not_exhausted() {
        let now = at(1_000);
        for reading in [
            WindowReading::new(99, Some(at(2_000))),
            WindowReading::new(100, Some(at(1_000))),
            WindowReading::new(100, Some(at(500))),
            WindowReading::new(100, None),
        ] {
            let windows = PlanWindows::new(now).with(Window::Weekly, reading);
            assert_eq!(windows.exhausted(MODEL, now), None, "{reading:?}");
        }
    }

    #[test]
    fn plan_limit_an_instant_is_written_as_iso_8601_in_utc() {
        assert_eq!(Utc::new(at(0)).to_string(), "1970-01-01T00:00:00Z");
        assert_eq!(
            Utc::new(at(1_791_190_800)).to_string(),
            "2026-10-05T09:00:00Z"
        );
        assert_eq!(
            Utc::new(at(951_868_799)).to_string(),
            "2000-02-29T23:59:59Z"
        );
        assert_eq!(
            Utc::new(UNIX_EPOCH - std::time::Duration::from_secs(1)).to_string(),
            "1970-01-01T00:00:00Z"
        );
    }

    #[test]
    fn plan_limit_each_window_has_a_name_of_crucibles_own() {
        let names: Vec<_> = [
            Window::FiveHour,
            Window::Daily,
            Window::Weekly,
            Window::Monthly,
            Window::Yearly,
        ]
        .into_iter()
        .map(Window::named)
        .collect();
        assert_eq!(
            names,
            [
                "5-hour window",
                "daily window",
                "weekly window",
                "monthly window",
                "yearly window"
            ]
        );
    }

    /// The model the turns in these readings ask.
    const MODEL: &str = "gpt-5.5";

    #[test]
    fn plan_limit_a_window_is_named_by_its_length_within_five_percent() {
        for (minutes, window) in [
            (285, Window::FiveHour),
            (299, Window::FiveHour),
            (300, Window::FiveHour),
            (315, Window::FiveHour),
            (316, Window::Lasting(316)),
            (284, Window::Lasting(284)),
            (1_440, Window::Daily),
            (1_368, Window::Daily),
            (1_512, Window::Daily),
            (10_080, Window::Weekly),
            (28 * 1_440, Window::Monthly),
            (31 * 1_440, Window::Monthly),
            (525_600, Window::Yearly),
            (499_320, Window::Yearly),
            (180, Window::Lasting(180)),
        ] {
            assert_eq!(Window::of(minutes), Some(window), "{minutes} minutes");
        }
        assert_eq!(Window::of(0), None);
    }

    #[test]
    fn plan_limit_a_window_of_any_other_length_is_named_by_that_length_rounded() {
        for (minutes, named) in [
            (180, "3-hour window"),
            (316, "5-hour window"),
            (45, "45-minute window"),
            (60, "1-hour window"),
            (1_367, "23-hour window"),
            (2_880, "2-day window"),
            (14 * 1_440, "14-day window"),
        ] {
            let window = Window::of(minutes).map(Window::named);
            assert_eq!(window.as_deref(), Some(named), "{minutes} minutes");
        }
    }

    #[test]
    fn plan_limit_windows_are_put_in_order_by_length() {
        let reading = WindowReading::new(1, None);
        let windows = PlanWindows::new(UNIX_EPOCH)
            .with(Window::Yearly, reading)
            .with(Window::Weekly, reading)
            .with(Window::Lasting(180), reading)
            .with(Window::Daily, reading)
            .with(Window::FiveHour, reading);
        let read: Vec<_> = windows
            .groups()
            .flat_map(LimitGroup::windows)
            .map(|(window, _)| window)
            .collect();
        assert_eq!(
            read,
            [
                Window::Lasting(180),
                Window::FiveHour,
                Window::Daily,
                Window::Weekly,
                Window::Yearly
            ]
        );
    }

    #[test]
    fn plan_limit_a_count_reads_as_its_share_and_a_total_of_none_is_no_window() {
        let counted = WindowReading::counted(412, 1_500, None).unwrap();
        assert_eq!(
            counted.used(),
            Used::Counted {
                used: 412,
                total: 1_500
            }
        );
        assert_eq!(counted.percent(), 27);
        assert!(!counted.spent());
        assert!(WindowReading::counted(1_500, 1_500, None).unwrap().spent());
        assert_eq!(
            WindowReading::counted(2_000, 1_500, None).unwrap().used(),
            Used::Counted {
                used: 1_500,
                total: 1_500
            }
        );
        assert_eq!(WindowReading::counted(0, 0, None), None);
        assert!(!WindowReading::unlimited().spent());
        assert_eq!(WindowReading::unlimited().percent(), 0);
        assert!(WindowReading::new(100, None).spent());
    }

    #[test]
    fn plan_limit_a_group_name_is_stripped_cut_and_never_empty() {
        assert_eq!(
            GroupName::new("GPT-5.3-Codex-Spark").map(|name| name.as_str().to_owned()),
            Some("GPT-5.3-Codex-Spark".to_owned())
        );
        assert_eq!(
            GroupName::new("\u{1b}[31mred\u{7}\n").map(|name| name.as_str().to_owned()),
            Some("[31mred".to_owned())
        );
        assert_eq!(GroupName::new(" \u{0}\t "), None);
        assert_eq!(GroupName::new(""), None);

        let long = "é".repeat(MAX_LIMIT_NAME_BYTES);
        let kept = GroupName::new(&long).unwrap();
        assert!(kept.as_str().len() <= MAX_LIMIT_NAME_BYTES);
        assert!(kept.as_str().ends_with('…'));
        let exact = "a".repeat(MAX_LIMIT_NAME_BYTES);
        assert_eq!(GroupName::new(&exact).unwrap().as_str(), exact);
    }

    /// The group a provider module keeps for the model `name`, drawn as it.
    fn model(name: &str) -> Scope {
        Scope::Model(ModelGroup::new(
            GroupName::new(name).unwrap(),
            ModelKey::exact(name),
        ))
    }

    #[test]
    fn plan_limit_groups_put_the_plan_first_and_keep_no_more_than_the_ceiling() {
        let reading = WindowReading::new(5, None);
        let mut windows =
            PlanWindows::new(UNIX_EPOCH).within(model("first"), Window::Weekly, reading);
        windows = windows.with(Window::Weekly, reading);
        for each in 0..MAX_LIMIT_GROUPS {
            windows = windows.within(model(&format!("model-{each}")), Window::Weekly, reading);
        }
        let scopes: Vec<_> = windows.groups().map(LimitGroup::scope).cloned().collect();
        assert_eq!(scopes.len(), MAX_LIMIT_GROUPS);
        assert_eq!(scopes.first(), Some(&Scope::Plan));
        assert_eq!(scopes.get(1), Some(&model("first")));

        let mut many = PlanWindows::new(UNIX_EPOCH);
        for minutes in 1..=u64::try_from(MAX_GROUP_WINDOWS + 3).unwrap() {
            many = many.with(Window::of(minutes * 1_000).unwrap(), reading);
        }
        assert_eq!(
            many.groups().flat_map(LimitGroup::windows).count(),
            MAX_GROUP_WINDOWS
        );
    }

    #[test]
    fn plan_limit_a_reading_past_a_ceiling_says_it_is_incomplete_and_one_at_it_does_not() {
        let reading = WindowReading::new(5, None);
        let groups = |count: usize| {
            (0..count).fold(PlanWindows::new(UNIX_EPOCH), |windows, each| {
                windows.within(model(&format!("model-{each}")), Window::Weekly, reading)
            })
        };
        assert!(!groups(MAX_LIMIT_GROUPS).incomplete());
        assert!(groups(MAX_LIMIT_GROUPS + 1).incomplete());

        let windows = |count: u64| {
            (1..=count).fold(PlanWindows::new(UNIX_EPOCH), |windows, minutes| {
                windows.with(Window::of(minutes * 1_000).unwrap(), reading)
            })
        };
        let at = u64::try_from(MAX_GROUP_WINDOWS).unwrap();
        assert!(!windows(at).incomplete());
        assert!(windows(at + 1).incomplete());
        // A window read again is the same window, not one past the ceiling.
        assert!(
            !windows(at)
                .with(Window::of(1_000).unwrap(), reading)
                .incomplete()
        );

        assert!(PlanWindows::new(UNIX_EPOCH).cut().incomplete());
        assert!(!PlanWindows::new(UNIX_EPOCH).incomplete());
    }

    #[test]
    fn plan_limit_a_merge_is_incomplete_where_either_reading_was_or_it_cut_a_group() {
        let reading = WindowReading::new(5, None);
        let full = (0..MAX_LIMIT_GROUPS).fold(PlanWindows::new(at(1)), |windows, each| {
            windows.within(model(&format!("model-{each}")), Window::Weekly, reading)
        });
        assert!(!full.clone().merge(PlanWindows::new(at(2))).incomplete());

        let another = PlanWindows::new(at(2)).within(model("another"), Window::Weekly, reading);
        assert!(full.clone().merge(another).incomplete());

        let cut = PlanWindows::new(at(2)).cut();
        assert!(PlanWindows::new(at(1)).merge(cut.clone()).incomplete());
        assert!(cut.merge(PlanWindows::new(at(3))).incomplete());
    }

    #[test]
    fn plan_limit_a_later_reading_replaces_only_the_groups_it_names() {
        let earlier = PlanWindows::new(at(1))
            .with(Window::FiveHour, WindowReading::new(10, None))
            .with(Window::Weekly, WindowReading::new(20, None))
            .within(model("spark"), Window::Weekly, WindowReading::new(30, None));
        let later = PlanWindows::new(at(2)).within(
            model("spark"),
            Window::FiveHour,
            WindowReading::new(40, None),
        );

        let merged = earlier.clone().merge(later);

        assert_eq!(merged.arrived(), at(2));
        assert_eq!(
            merged.reading(Window::Weekly),
            Some(WindowReading::new(20, None))
        );
        let spark: Vec<_> = merged
            .groups()
            .filter(|group| *group.scope() == model("spark"))
            .flat_map(LimitGroup::windows)
            .collect();
        assert_eq!(spark, [(Window::FiveHour, WindowReading::new(40, None))]);

        let plan = PlanWindows::new(at(3)).with(Window::Weekly, WindowReading::new(50, None));
        let merged = earlier.merge(plan);
        assert_eq!(merged.reading(Window::FiveHour), None);
        assert_eq!(
            merged.reading(Window::Weekly),
            Some(WindowReading::new(50, None))
        );
        assert_eq!(merged.groups().count(), 2);
    }

    #[test]
    fn plan_limit_a_spent_window_holds_back_only_the_models_it_applies_to() {
        let now = at(1_000);
        let spent = WindowReading::new(100, Some(at(2_000)));
        let fresh = WindowReading::new(10, Some(at(3_000)));

        let plan_wide = PlanWindows::new(now).with(Window::Weekly, spent);
        assert_eq!(
            plan_wide.exhausted(MODEL, now),
            Some((Window::Weekly, at(2_000)))
        );
        assert_eq!(
            plan_wide.exhausted("spark", now),
            Some((Window::Weekly, at(2_000)))
        );

        let one_model = PlanWindows::new(now).with(Window::Weekly, fresh).within(
            model("spark"),
            Window::FiveHour,
            spent,
        );
        assert_eq!(
            one_model.exhausted("spark", now),
            Some((Window::FiveHour, at(2_000)))
        );
        assert_eq!(one_model.exhausted(MODEL, now), None);

        let counted = PlanWindows::new(now).within(
            model(MODEL),
            Window::Daily,
            WindowReading::counted(9, 9, Some(at(2_000))).unwrap(),
        );
        assert_eq!(
            counted.exhausted(MODEL, now),
            Some((Window::Daily, at(2_000)))
        );
    }

    #[test]
    fn plan_limit_a_model_group_holds_back_what_its_key_says_and_not_what_it_is_called() {
        let now = at(1_000);
        let spent = WindowReading::new(100, Some(at(2_000)));
        let drawn = |key: Option<ModelKey>| {
            let group = ModelGroup::new(GroupName::new(MODEL).unwrap(), key);
            PlanWindows::new(now).within(Scope::Model(group), Window::FiveHour, spent)
        };

        assert_eq!(drawn(None).exhausted(MODEL, now), None);
        assert_eq!(drawn(ModelKey::exact("spark")).exhausted(MODEL, now), None);
        assert_eq!(
            drawn(ModelKey::exact("spark")).exhausted("spark", now),
            Some((Window::FiveHour, at(2_000)))
        );
        assert_eq!(
            drawn(ModelKey::exact("spark")).exhausted("Spark", now),
            None
        );
        assert_eq!(
            drawn(ModelKey::exact("spark")).exhausted("spark-2", now),
            None
        );
    }

    #[test]
    fn plan_limit_a_model_key_is_an_id_kept_whole_or_none() {
        let key = ModelKey::exact("gpt-5.3-codex-spark").unwrap();
        assert_eq!(key.model(), Some("gpt-5.3-codex-spark"));
        assert!(key.holds("gpt-5.3-codex-spark"));
        assert!(!key.holds("gpt-5.3-codex-spark "));

        let exact = "m".repeat(MAX_LIMIT_NAME_BYTES);
        assert!(ModelKey::exact(&exact).is_some());
        let long = "m".repeat(MAX_LIMIT_NAME_BYTES + 1);
        assert_eq!(ModelKey::exact(&long), None);
        assert_eq!(ModelKey::exact(""), None);
        assert_eq!(ModelKey::exact("gpt\u{1b}[31m"), None);
    }

    /// Every Unicode format character a vendor could put in a name to
    /// reorder or hide what is drawn: the bidi marks, embeddings, overrides
    /// and isolates, the zero-width characters, the byte order mark, and the
    /// rest of the format category.
    const FORMATS: &[char] = &[
        '\u{ad}',
        '\u{600}',
        '\u{61c}',
        '\u{180e}',
        '\u{200b}',
        '\u{200c}',
        '\u{200d}',
        '\u{200e}',
        '\u{200f}',
        '\u{202a}',
        '\u{202b}',
        '\u{202c}',
        '\u{202d}',
        '\u{202e}',
        '\u{2060}',
        '\u{2061}',
        '\u{2062}',
        '\u{2063}',
        '\u{2064}',
        '\u{2065}',
        '\u{2066}',
        '\u{2067}',
        '\u{2068}',
        '\u{2069}',
        '\u{206f}',
        '\u{feff}',
        '\u{fff9}',
        '\u{110bd}',
        '\u{1d173}',
        '\u{e0001}',
        '\u{e007f}',
    ];

    #[test]
    fn plan_limit_a_group_name_drops_format_characters_and_a_key_refuses_them() {
        for &format in FORMATS {
            let said = format!("gpt-5{format}-ini");
            assert_eq!(
                GroupName::new(&said).map(|name| name.as_str().to_owned()),
                Some("gpt-5-ini".to_owned()),
                "U+{:04X}",
                u32::from(format)
            );
            assert_eq!(GroupName::new(&format!(" {format}{format} ")), None);
            assert_eq!(ModelKey::exact(&said), None, "U+{:04X}", u32::from(format));
        }
        // Letters of any script are kept.
        assert_eq!(GroupName::new("модель-ü").unwrap().as_str(), "модель-ü");
        assert!(ModelKey::exact("модель-ü").is_some());
    }
}
