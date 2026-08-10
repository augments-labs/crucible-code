//! Identifiers for the three things that need naming: a session, a turn within
//! it, and a tool call within a turn.
//!
//! Each is a newtype, so a session identifier cannot be passed where a turn
//! ordinal belongs. Text arriving from a file name or a command-line flag is
//! parsed into one of these at the boundary and never re-validated inside.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Why a string could not become an identifier.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    /// A session identifier was not `<millis>-<6 hex>`.
    #[error("not a session id: expected <millis>-<6 hex>, got {0:?}")]
    NotASessionId(Box<str>),
}

/// Names one session: a conversation bound to a working directory.
///
/// The text form is `<unix-millis>-<6 hex>` with the timestamp zero-padded to a
/// fixed thirteen digits, so sorting session file names as text sorts them by
/// start time. The random suffix breaks ties between two sessions started in
/// the same millisecond; it is not a uniqueness guarantee across machines,
/// which nothing here needs.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(Box<str>);

/// Width of the millisecond field. Thirteen digits covers every instant from
/// 2001 to 2286; the padding is what makes text order match time order.
const MILLIS_WIDTH: usize = 13;

/// Hex digits of randomness after the timestamp.
const SUFFIX_WIDTH: usize = 6;

impl SessionId {
    /// Mints an identifier for a session starting now.
    #[must_use]
    pub fn new() -> Self {
        Self::at(SystemTime::now(), random_suffix())
    }

    /// The identifier as text — also its file name in the session directory.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Splitting this out is what makes minting testable: the caller supplies
    /// the clock and the randomness, so a test can pin both.
    fn at(started: SystemTime, suffix: u32) -> Self {
        let millis = started
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let suffix = suffix & 0x00ff_ffff;
        Self(format!("{millis:0MILLIS_WIDTH$}-{suffix:0SUFFIX_WIDTH$x}").into())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", self.0)
    }
}

impl FromStr for SessionId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let reject = || IdError::NotASessionId(text.into());
        let (millis, suffix) = text.split_once('-').ok_or_else(reject)?;

        let shaped = millis.len() >= MILLIS_WIDTH
            && millis.bytes().all(|b| b.is_ascii_digit())
            && suffix.len() == SUFFIX_WIDTH
            && suffix
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());

        if shaped {
            Ok(Self(text.into()))
        } else {
            Err(reject())
        }
    }
}

/// Twenty-four bits of randomness, without a dependency.
///
/// `RandomState` takes its keys from the operating system once per thread and
/// advances one of them on every construction after that, so two hashers built
/// in the same millisecond disagree. That is the whole requirement: this suffix
/// breaks ties, it does not identify. Within one thread the sequence follows
/// from the two keys it started with, so nothing may come to depend on a
/// session name being unguessable.
fn random_suffix() -> u32 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u8(0);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the mask keeps the low 24 bits; truncation is the point"
    )]
    let low = hasher.finish() as u32;
    low & 0x00ff_ffff
}

/// Position of a turn within a session, counting from one.
///
/// A turn is one prompt plus the exchange until the agent yields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TurnId(u32);

impl TurnId {
    /// The first turn of a session.
    pub const FIRST: Self = Self(1);

    /// The turn after this one.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The ordinal, for display and for the session log.
    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifies one tool call within a turn.
///
/// The provider supplies this string — it is how a result is matched back to
/// the call that asked for it — so it is stored as given rather than parsed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolId(Box<str>);

impl ToolId {
    /// Takes the identifier a provider assigned to a tool call.
    #[must_use]
    pub fn new(id: impl Into<Box<str>>) -> Self {
        Self(id.into())
    }

    /// The identifier as the provider wrote it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_session_id_sorts_by_start_time() {
        // Across a digit boundary, and with the earlier session given the
        // larger random suffix. Unpadded, "999" sorts after "1000" as text and
        // both of those would go the wrong way — which is what the padding is
        // for, and why this pair rather than two neighbouring milliseconds.
        let earlier = SessionId::at(UNIX_EPOCH + Duration::from_millis(999), 0xff_ffff);
        // One second is 1000 ms — a digit wider than 999, which is the point.
        let later = SessionId::at(UNIX_EPOCH + Duration::from_secs(1), 0x00_0000);

        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn the_timestamp_is_padded_so_text_order_is_time_order() {
        let id = SessionId::at(UNIX_EPOCH + Duration::from_millis(7), 0xabc);
        assert_eq!(id.as_str(), "0000000000007-000abc");
    }

    #[test]
    fn two_ids_minted_together_still_differ() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            seen.insert(random_suffix());
        }
        assert!(seen.len() > 1, "the suffix is not random: {seen:?}");
    }

    #[test]
    fn a_session_id_round_trips_through_its_text() {
        let minted = SessionId::new();
        let parsed: SessionId = minted.as_str().parse().unwrap();
        assert_eq!(minted, parsed);
    }

    #[test]
    fn text_that_is_not_a_session_id_is_rejected() {
        for bad in [
            "",
            "nope",
            "0000000000007",
            "0000000000007-",
            "0000000000007-abc",     // suffix too short
            "0000000000007-000abcd", // suffix too long
            "0000000000007-00ABCD",  // hex must be lower case, so names sort
            "000000000000-000abc",   // timestamp too short to pad-sort
            "not-000abc",
        ] {
            assert_eq!(
                bad.parse::<SessionId>(),
                Err(IdError::NotASessionId(bad.into())),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn a_session_id_debug_shows_the_id() {
        let id = SessionId::at(UNIX_EPOCH + Duration::from_millis(7), 0xabc);
        assert_eq!(format!("{id:?}"), "SessionId(0000000000007-000abc)");
    }

    #[test]
    fn turns_count_from_one_and_advance() {
        assert_eq!(TurnId::FIRST.get(), 1);
        assert_eq!(TurnId::FIRST.next().get(), 2);
        assert!(TurnId::FIRST < TurnId::FIRST.next());
    }

    #[test]
    fn a_tool_id_keeps_the_text_the_provider_sent() {
        let id = ToolId::new("toolu_01ABC");
        assert_eq!(id.as_str(), "toolu_01ABC");
        assert_eq!(id.to_string(), "toolu_01ABC");
    }
}
