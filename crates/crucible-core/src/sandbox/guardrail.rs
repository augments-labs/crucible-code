//! Bounded command allow/deny guardrails, separate from permission approval.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use sha2::{Digest, Sha256};

use super::service::SandboxCommand;

/// Maximum independent parent/descendant filters in one effective policy.
pub const MAX_SANDBOX_GUARDRAIL_LAYERS: usize = 16;

/// Maximum allow/deny rules retained across all guardrail layers.
pub const MAX_SANDBOX_GUARDRAIL_RULES: usize = 128;

/// Maximum words in one exact, prefix, or anchored rule.
pub const MAX_SANDBOX_GUARDRAIL_WORDS: usize = 128;

/// Maximum aggregate encoded bytes in one rule.
pub const MAX_SANDBOX_GUARDRAIL_BYTES: usize = 16 * 1024;

/// Whether a matching rule permits or rejects an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxGuardrailEffect {
    /// This filter may admit matching commands.
    Allow,
    /// A match refuses the command even if an allow rule also matches.
    Deny,
}

/// Which immutable command image a guardrail is evaluating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxCommandStage {
    /// The host-selected invocation before a trusted adapter transformation.
    Requested,
    /// The invocation after every trusted program/argument transformation.
    Effective,
}

/// Redacted outcome retained by audit/events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxGuardrailDecision {
    /// Every independent filter admitted the command.
    Allowed,
    /// A deny matched or one filter's allow set did not match.
    Denied,
}

/// One exact, prefix, or per-word anchored glob rule.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxCommandRule {
    effect: SandboxGuardrailEffect,
    matcher: RuleMatcher,
}

impl SandboxCommandRule {
    /// Matches the complete program/argument vector exactly.
    ///
    /// # Errors
    ///
    /// Empty or oversized patterns are rejected.
    pub fn exact<I, S>(
        effect: SandboxGuardrailEffect,
        words: I,
    ) -> Result<Self, SandboxGuardrailError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Ok(Self {
            effect,
            matcher: RuleMatcher::Exact(bounded_words(words)?),
        })
    }

    /// Matches an invocation whose leading words are exactly this sequence.
    ///
    /// # Errors
    ///
    /// Empty or oversized patterns are rejected.
    pub fn prefix<I, S>(
        effect: SandboxGuardrailEffect,
        words: I,
    ) -> Result<Self, SandboxGuardrailError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Ok(Self {
            effect,
            matcher: RuleMatcher::Prefix(bounded_words(words)?),
        })
    }

    /// Matches the complete vector with one anchored glob per word.
    ///
    /// Globs never span argument boundaries. They are compiled during policy
    /// construction, before a command can reach a backend.
    ///
    /// # Errors
    ///
    /// Empty, malformed, or oversized patterns are rejected.
    pub fn anchored<I, S>(
        effect: SandboxGuardrailEffect,
        patterns: I,
    ) -> Result<Self, SandboxGuardrailError>
    where
        I: IntoIterator<Item = S>,
        S: Into<Box<str>>,
    {
        let patterns: Vec<Box<str>> = patterns.into_iter().map(Into::into).collect();
        if patterns
            .iter()
            .any(|pattern| pattern.as_bytes().contains(&0))
        {
            return Err(SandboxGuardrailError::InvalidRule);
        }
        validate_shape(patterns.len(), patterns.iter().map(|pattern| pattern.len()))?;
        let patterns = patterns
            .into_iter()
            .map(WordPattern::new)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            effect,
            matcher: RuleMatcher::Anchored(patterns.into_boxed_slice()),
        })
    }

    fn matches(&self, command: &SandboxCommand, stage: SandboxCommandStage) -> bool {
        let (program, arguments) = command.image(stage);
        let words: Vec<&OsStr> = std::iter::once(program.as_os_str())
            .chain(arguments.iter().map(OsString::as_os_str))
            .collect();
        self.matcher.matches(&words)
    }

    fn update_digest(&self, digest: &mut Sha256) {
        digest.update([self.effect as u8]);
        self.matcher.update_digest(digest);
    }
}

impl std::fmt::Debug for SandboxCommandRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxCommandRule")
            .field("effect", &self.effect)
            .field("matcher", &self.matcher.kind())
            .field("words", &self.matcher.len())
            .finish()
    }
}

/// One top-level or descendant command filter.
#[derive(Clone, PartialEq, Eq)]
struct Filter(Box<[SandboxCommandRule]>);

/// Immutable intersection of independent command filters.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SandboxCommandPolicy {
    filters: Box<[Filter]>,
}

impl SandboxCommandPolicy {
    /// No command-specific restriction. Permission and kernel confinement still
    /// apply independently.
    #[must_use]
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Builds one allow/deny filter.
    ///
    /// Deny matches always win. If the filter contains any allow rule, at
    /// least one must match; a filter containing only denies admits everything
    /// those denies do not match.
    ///
    /// # Errors
    ///
    /// Oversized rule sets are rejected.
    pub fn new(
        rules: impl IntoIterator<Item = SandboxCommandRule>,
    ) -> Result<Self, SandboxGuardrailError> {
        let rules: Vec<_> = rules.into_iter().collect();
        if rules.len() > MAX_SANDBOX_GUARDRAIL_RULES {
            return Err(SandboxGuardrailError::TooManyRules);
        }
        if rules.is_empty() {
            return Ok(Self::allow_all());
        }
        Ok(Self {
            filters: vec![Filter(rules.into_boxed_slice())].into_boxed_slice(),
        })
    }

    /// Intersects a descendant filter with every parent filter.
    ///
    /// # Errors
    ///
    /// The bounded layer/rule ceilings are enforced across the result.
    pub fn intersect(parent: &Self, descendant: &Self) -> Result<Self, SandboxGuardrailError> {
        let layers = parent
            .filters
            .len()
            .saturating_add(descendant.filters.len());
        let rules = parent
            .filters
            .iter()
            .chain(descendant.filters.iter())
            .fold(0_usize, |total, filter| {
                total.saturating_add(filter.0.len())
            });
        if layers > MAX_SANDBOX_GUARDRAIL_LAYERS || rules > MAX_SANDBOX_GUARDRAIL_RULES {
            return Err(SandboxGuardrailError::TooManyRules);
        }
        Ok(Self {
            filters: parent
                .filters
                .iter()
                .chain(descendant.filters.iter())
                .cloned()
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        })
    }

    /// Evaluates one immutable command image without logging its arguments.
    #[must_use]
    pub fn evaluate(
        &self,
        command: &SandboxCommand,
        stage: SandboxCommandStage,
    ) -> SandboxGuardrailDecision {
        for filter in &self.filters {
            if filter.0.iter().any(|rule| {
                rule.effect == SandboxGuardrailEffect::Deny && rule.matches(command, stage)
            }) {
                return SandboxGuardrailDecision::Denied;
            }
            let mut allows = filter
                .0
                .iter()
                .filter(|rule| rule.effect == SandboxGuardrailEffect::Allow);
            if allows.clone().next().is_some() && !allows.any(|rule| rule.matches(command, stage)) {
                return SandboxGuardrailDecision::Denied;
            }
        }
        SandboxGuardrailDecision::Allowed
    }

    pub(super) fn digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"crucible-sandbox-guardrail-v1\0");
        for filter in &self.filters {
            digest.update((filter.0.len() as u64).to_be_bytes());
            for rule in &filter.0 {
                rule.update_digest(&mut digest);
            }
        }
        digest.finalize().into()
    }
}

impl std::fmt::Debug for SandboxCommandPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rules = self.filters.iter().fold(0_usize, |total, filter| {
            total.saturating_add(filter.0.len())
        });
        f.debug_struct("SandboxCommandPolicy")
            .field("layers", &self.filters.len())
            .field("rules", &rules)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
enum RuleMatcher {
    Exact(Box<[OsString]>),
    Prefix(Box<[OsString]>),
    Anchored(Box<[WordPattern]>),
}

impl RuleMatcher {
    fn matches(&self, words: &[&OsStr]) -> bool {
        match self {
            Self::Exact(expected) => expected.len() == words.len() && exact(expected, words),
            Self::Prefix(expected) => expected.len() <= words.len() && exact(expected, words),
            Self::Anchored(patterns) => {
                patterns.len() == words.len()
                    && patterns
                        .iter()
                        .zip(words)
                        .all(|(pattern, word)| pattern.matcher.is_match(Path::new(word)))
            }
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Exact(_) => "exact",
            Self::Prefix(_) => "prefix",
            Self::Anchored(_) => "anchored",
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Exact(words) | Self::Prefix(words) => words.len(),
            Self::Anchored(patterns) => patterns.len(),
        }
    }

    fn update_digest(&self, digest: &mut Sha256) {
        digest.update([match self {
            Self::Exact(_) => 0,
            Self::Prefix(_) => 1,
            Self::Anchored(_) => 2,
        }]);
        match self {
            Self::Exact(words) | Self::Prefix(words) => {
                for word in words {
                    digest.update(
                        u64::try_from(word.as_encoded_bytes().len())
                            .unwrap_or(u64::MAX)
                            .to_be_bytes(),
                    );
                    digest.update(word.as_encoded_bytes());
                }
            }
            Self::Anchored(patterns) => {
                for pattern in patterns {
                    digest.update(
                        u64::try_from(pattern.source.len())
                            .unwrap_or(u64::MAX)
                            .to_be_bytes(),
                    );
                    digest.update(pattern.source.as_bytes());
                }
            }
        }
    }
}

fn exact(expected: &[OsString], words: &[&OsStr]) -> bool {
    expected
        .iter()
        .zip(words)
        .all(|(expected, actual)| expected.as_os_str() == *actual)
}

#[derive(Clone)]
struct WordPattern {
    source: Box<str>,
    matcher: GlobMatcher,
}

impl WordPattern {
    fn new(source: Box<str>) -> Result<Self, SandboxGuardrailError> {
        let matcher = GlobBuilder::new(&source)
            .literal_separator(false)
            .build()
            .map_err(|_| SandboxGuardrailError::InvalidPattern)?
            .compile_matcher();
        Ok(Self { source, matcher })
    }
}

impl PartialEq for WordPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for WordPattern {}

fn bounded_words<I, S>(words: I) -> Result<Box<[OsString]>, SandboxGuardrailError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let words: Vec<OsString> = words.into_iter().map(Into::into).collect();
    if words
        .iter()
        .any(|word| word.as_encoded_bytes().contains(&0))
    {
        return Err(SandboxGuardrailError::InvalidRule);
    }
    validate_shape(
        words.len(),
        words.iter().map(|word| word.as_encoded_bytes().len()),
    )?;
    Ok(words.into_boxed_slice())
}

fn validate_shape(
    words: usize,
    lengths: impl IntoIterator<Item = usize>,
) -> Result<(), SandboxGuardrailError> {
    let mut bytes = 0_usize;
    let mut empty = false;
    for length in lengths {
        bytes = bytes.saturating_add(length);
        empty |= length == 0;
    }
    if words == 0
        || words > MAX_SANDBOX_GUARDRAIL_WORDS
        || empty
        || bytes > MAX_SANDBOX_GUARDRAIL_BYTES
    {
        return Err(SandboxGuardrailError::InvalidRule);
    }
    Ok(())
}

/// Why a command guardrail contract was rejected before use.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SandboxGuardrailError {
    /// One rule must contain a bounded non-empty word vector.
    #[error("sandbox command guardrail rule is empty or exceeds its bound")]
    InvalidRule,
    /// Anchored patterns must compile without reinterpretation.
    #[error("sandbox command guardrail pattern is invalid")]
    InvalidPattern,
    /// The effective intersection is bounded.
    #[error("sandbox command guardrail has too many rules or layers")]
    TooManyRules,
}

// The fixtures are POSIX absolute paths, which no Windows path type accepts;
// Windows has no confinement backend to give them a native shape.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::SandboxEnvironment;

    fn command(arguments: &[&str]) -> SandboxCommand {
        SandboxCommand::new(
            "/usr/bin/cargo",
            arguments.iter().map(OsString::from),
            SandboxEnvironment::empty(),
        )
        .expect("command")
    }

    #[test]
    fn every_guardrail_word_is_nonempty_and_nul_free() {
        assert!(matches!(
            SandboxCommandRule::exact(SandboxGuardrailEffect::Allow, ["/bin/sh", ""]),
            Err(SandboxGuardrailError::InvalidRule)
        ));
        assert!(matches!(
            SandboxCommandRule::prefix(SandboxGuardrailEffect::Allow, ["/bin/sh", "bad\0word"]),
            Err(SandboxGuardrailError::InvalidRule)
        ));
        assert!(matches!(
            SandboxCommandRule::anchored(SandboxGuardrailEffect::Allow, ["/bin/sh", ""]),
            Err(SandboxGuardrailError::InvalidRule)
        ));
    }

    #[test]
    fn deny_wins_and_allow_sets_default_to_refusal() {
        let policy = SandboxCommandPolicy::new([
            SandboxCommandRule::prefix(SandboxGuardrailEffect::Allow, ["/usr/bin/cargo", "test"])
                .expect("allow"),
            SandboxCommandRule::exact(
                SandboxGuardrailEffect::Deny,
                ["/usr/bin/cargo", "test", "--doc"],
            )
            .expect("deny"),
        ])
        .expect("policy");

        assert_eq!(
            policy.evaluate(&command(&["test", "--lib"]), SandboxCommandStage::Effective),
            SandboxGuardrailDecision::Allowed
        );
        assert_eq!(
            policy.evaluate(&command(&["test", "--doc"]), SandboxCommandStage::Effective),
            SandboxGuardrailDecision::Denied
        );
        assert_eq!(
            policy.evaluate(&command(&["build"]), SandboxCommandStage::Effective),
            SandboxGuardrailDecision::Denied
        );
    }

    #[test]
    fn anchored_patterns_match_each_complete_word() {
        let policy = SandboxCommandPolicy::new([SandboxCommandRule::anchored(
            SandboxGuardrailEffect::Allow,
            ["/usr/bin/cargo", "t*", "--package", "crucible-?ore"],
        )
        .expect("pattern")])
        .expect("policy");

        assert_eq!(
            policy.evaluate(
                &command(&["test", "--package", "crucible-core"]),
                SandboxCommandStage::Requested,
            ),
            SandboxGuardrailDecision::Allowed
        );
        assert_eq!(
            policy.evaluate(
                &command(&["test", "--package", "crucible-tools"]),
                SandboxCommandStage::Requested,
            ),
            SandboxGuardrailDecision::Denied
        );
    }

    #[test]
    fn parent_and_descendant_filters_are_both_required() {
        let parent = SandboxCommandPolicy::new([SandboxCommandRule::prefix(
            SandboxGuardrailEffect::Allow,
            ["/usr/bin/cargo"],
        )
        .expect("parent")])
        .expect("parent policy");
        let child = SandboxCommandPolicy::new([SandboxCommandRule::exact(
            SandboxGuardrailEffect::Allow,
            ["/usr/bin/cargo", "test"],
        )
        .expect("child")])
        .expect("child policy");
        let effective = SandboxCommandPolicy::intersect(&parent, &child).expect("intersection");

        assert_eq!(
            effective.evaluate(&command(&["test"]), SandboxCommandStage::Effective),
            SandboxGuardrailDecision::Allowed
        );
        assert_eq!(
            effective.evaluate(&command(&["build"]), SandboxCommandStage::Effective),
            SandboxGuardrailDecision::Denied
        );
    }
}
