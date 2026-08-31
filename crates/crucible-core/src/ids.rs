//! Identifiers for the things that need naming: a session, a turn within it, a
//! tool call within a turn, one execution, and the agent an execution runs.
//!
//! Each is a newtype, so a session identifier cannot be passed where a turn
//! ordinal belongs. Text arriving from a file name or a command-line flag is
//! parsed into one of these at the boundary and never re-validated inside.
//!
//! A session, a turn and an execution name three different things and none is
//! derived from another. One session holds many turns; one turn is driven by an
//! execution; and an execution will later be able to start further executions
//! that are no turn of anybody's session. Deriving one from another would make
//! the tree of executions unrepresentable the moment there is more than one.

use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

/// Why a string could not become an identifier.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    /// A session identifier was neither a UUID v7 nor `<millis>-<6 hex>`.
    #[error("not a session id: expected a uuid or <millis>-<6 hex>, got {0:?}")]
    NotASessionId(Box<str>),
}

/// Names one session: a conversation bound to a working directory.
///
/// The text form is a hyphenated lowercase UUID v7. Its first twelve hex
/// digits are the start time in milliseconds, so sorting session file names as
/// text sorts them by start time, and ids minted in the same millisecond of
/// one process stay ordered by the version's tie-break counter.
///
/// Releases before the uuid form named sessions `<unix-millis>-<6 hex>` with
/// the timestamp zero-padded to thirteen digits. Those names still parse,
/// sort among themselves by start time, and yield their start time — they
/// only stop being minted. A directory holding both shapes groups every uuid
/// name before every legacy name (a real legacy timestamp leads with `1`, a
/// uuid with `0`), so ordering across the two families is not time order;
/// listings that need one timeline sort on [`SessionId::started`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(Box<str>);

/// Width of the legacy millisecond field. Thirteen digits covers every instant
/// from 2001 to 2286; the padding is what made text order match time order.
const MILLIS_WIDTH: usize = 13;

/// Hex digits of randomness after a legacy timestamp.
const SUFFIX_WIDTH: usize = 6;

impl SessionId {
    /// Mints an identifier for a session starting now.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string().into())
    }

    /// The identifier as text — also its file name in the session directory.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// When the session it names began.
    ///
    /// Out of the name rather than off the file, because they answer different
    /// questions: a log's modification time is when the session last said
    /// something, and this is when it started. Naming a directory's sessions by
    /// the second is also a read of the directory that never opens anything.
    ///
    /// The epoch stands in for a name whose timestamp cannot become an instant
    /// this machine holds. Nothing crucible writes looks like that, but a file
    /// name can be anything somebody typed, and a date is not worth a panic.
    #[must_use]
    pub fn started(&self) -> SystemTime {
        let millis = if uuid_shaped(&self.0) {
            Uuid::try_parse(&self.0)
                .ok()
                .and_then(|uuid| uuid.get_timestamp())
                .map(|timestamp| {
                    let (seconds, nanos) = timestamp.to_unix();
                    Duration::new(seconds, nanos)
                })
        } else {
            self.0
                .split_once('-')
                .and_then(|(millis, _)| millis.parse().ok())
                .map(Duration::from_millis)
        };
        millis
            .and_then(|since| UNIX_EPOCH.checked_add(since))
            .unwrap_or(UNIX_EPOCH)
    }

    /// Splitting this out is what makes minting testable: the caller supplies
    /// the clock and the tie-break bytes, so a test can pin both.
    #[cfg(test)]
    fn at(started: SystemTime, entropy: [u8; 10]) -> Self {
        let millis = started.duration_since(UNIX_EPOCH).map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
        let uuid = uuid::Builder::from_unix_timestamp_millis(millis, &entropy).into_uuid();
        Self(uuid.to_string().into())
    }
}

/// Whether text is the hyphenated lowercase form of a UUID v7 — the only uuid
/// spelling a session file is ever named with. `Uuid::try_parse` also accepts
/// braced, urn and unhyphenated spellings, and those would name a second file
/// for the same session, so the shape is pinned here before the parse.
fn uuid_shaped(text: &str) -> bool {
    text.len() == 36
        && text
            .bytes()
            .enumerate()
            .all(|(position, byte)| match position {
                8 | 13 | 18 | 23 => byte == b'-',
                14 => byte == b'7',
                19 => matches!(byte, b'8' | b'9' | b'a' | b'b'),
                _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
            })
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

        if uuid_shaped(text) || legacy_shaped(text) {
            Ok(Self(text.into()))
        } else {
            Err(reject())
        }
    }
}

/// Whether text is a legacy `<millis>-<6 hex>` session name.
fn legacy_shaped(text: &str) -> bool {
    text.split_once('-').is_some_and(|(millis, suffix)| {
        millis.len() >= MILLIS_WIDTH
            && millis.bytes().all(|b| b.is_ascii_digit())
            && suffix.len() == SUFFIX_WIDTH
            && suffix
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// Names one execution: an agent driven from a prompt to an ending.
///
/// Minted per execution and never read off a [`SessionId`] or a [`TurnId`].
/// A session holds many executions, and an execution will be able to hold
/// further ones; an identifier computed from either of the others could not
/// tell two runs of the same turn apart.
///
/// A UUID v7, so these sort by start time the way a session name does —
/// ordered here rather than only after `to_string`, because this is what every
/// event is filed under and a reader grouping a transcript by run should not
/// have to render one to key it. `Copy` and pointer-free on purpose: every
/// event a run posts carries one, and an identifier that allocated would put
/// an allocation on the delta path.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(Uuid);

impl RunId {
    /// Mints an identifier for an execution starting now.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

/// Every call is a different id.
///
/// Not the usual `Default`, which answers with one agreed value. Here there is
/// no such value — a run that has not been named is not a run — so this exists
/// only because [`RunId::new`] takes nothing, which the lint set reads as an
/// obligation.
///
/// It leaves a trap for a later type. `#[derive(Default)]` around a field of
/// this one mints a fresh execution rather than leaving a blank, and nothing
/// at the deriving site says so. A struct holding one writes its own `Default`
/// or does without.
impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RunId({})", self.0.as_hyphenated())
    }
}

/// Names one attempt to send a logical provider request.
///
/// Retries mint another value even when the cache identity is unchanged. The
/// distinction is what prevents usage from one accepted attempt being replayed
/// onto another and charged twice. Pointer-free because it travels on streamed
/// usage and cache facts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderAttemptId(Uuid);

impl ProviderAttemptId {
    /// Mints one provider-attempt identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for ProviderAttemptId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProviderAttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for ProviderAttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProviderAttemptId({})", self.0.as_hyphenated())
    }
}

/// Non-secret identity of credential-owned authorization material.
///
/// Credential implementations derive a stable value when they own a durable,
/// verified identity and otherwise mint a fresh fail-closed value. The bytes
/// may participate in private cache-scope derivation but never diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CredentialScopeId(Uuid);

impl CredentialScopeId {
    /// Mints a fresh fail-closed credential scope.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Takes the first 128 bits of a cryptographic, domain-separated digest.
    ///
    /// This constructor deliberately accepts the digest rather than identity
    /// material. The credential implementation owns the material and is the
    /// only layer allowed to decide what remains stable across renewal.
    #[must_use]
    pub fn from_digest(digest: [u8; 32]) -> Self {
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Self(Uuid::from_bytes(bytes))
    }

    /// Fixed bytes for local cache-identity derivation.
    #[must_use]
    pub const fn bytes(self) -> [u8; 16] {
        self.0.into_bytes()
    }
}

impl Default for CredentialScopeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CredentialScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CredentialScopeId([redacted])")
    }
}

/// Names one reusable agent definition.
///
/// Text, because an agent is written down by somebody and referred to by the
/// name they gave it — in a configuration document, on a command line, and
/// from another agent that may delegate to it. Stored as given: what makes a
/// name acceptable belongs to whatever registers definitions, not here. The
/// empty string included — `AgentId::new("")` builds, and there is no registry
/// yet to turn it away.
///
/// Looked up by, like every other name in this module: the thing a definition
/// is filed under is the thing something has to be able to find it by, and a
/// name that cannot be a key would make whatever registers definitions invent
/// a second one. Being a key is not being a *valid* key: two definitions
/// written down under the same word — `""` included — are one entry, and the
/// second silently replaces the first. Whatever registers definitions is where
/// that stops being allowed, at the parse boundary this module describes.
///
/// ```
/// use std::collections::{BTreeMap, HashMap};
/// use crucible_core::AgentId;
///
/// let mut written = BTreeMap::new();
/// written.insert(AgentId::new("reviewing"), "reads");
/// written.insert(AgentId::new("coding"), "writes");
///
/// assert_eq!(written.get(&AgentId::new("coding")), Some(&"writes"));
/// assert_eq!(
///     written.keys().map(AgentId::as_str).collect::<Vec<_>>(),
///     ["coding", "reviewing"]
/// );
///
/// let unordered: HashMap<AgentId, &str> = written.into_iter().collect();
/// assert_eq!(unordered[&AgentId::new("reviewing")], "reads");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(Box<str>);

impl AgentId {
    /// Takes the name an agent definition was written down under.
    #[must_use]
    pub fn new(id: impl Into<Box<str>>) -> Self {
        Self(id.into())
    }

    /// The name as it was written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentId({})", self.0)
    }
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
    use super::*;

    #[test]
    fn a_session_id_sorts_by_start_time() {
        // The earlier session gets the larger tie-break bytes, so only the
        // timestamp field can put this pair in the right order.
        let earlier = SessionId::at(UNIX_EPOCH + Duration::from_millis(999), [0xff; 10]);
        let later = SessionId::at(UNIX_EPOCH + Duration::from_secs(1), [0x00; 10]);

        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn an_id_says_when_the_session_it_names_started() {
        let started = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        let id = SessionId::at(started, [0xab; 10]);

        assert_eq!(id.started(), started);
    }

    #[test]
    fn a_legacy_id_still_parses_sorts_and_says_when_it_started() {
        let earlier: SessionId = "0000000000999-ffffff".parse().expect("the legacy shape");
        let later: SessionId = "0000000001000-000000".parse().expect("the legacy shape");

        assert!(earlier < later, "{earlier} should sort before {later}");
        assert_eq!(earlier.started(), UNIX_EPOCH + Duration::from_millis(999));
    }

    #[test]
    fn uuid_names_group_before_legacy_names() {
        // Not time order across the families — the module doc owns this wart.
        // A real legacy timestamp leads with `1`; a uuid, for centuries, with
        // `0`. Anything wanting one timeline sorts on `started()`.
        let legacy: SessionId = "1756246123456-abcdef".parse().expect("the legacy shape");
        let newer = SessionId::at(legacy.started() + Duration::from_hours(1), [0x00; 10]);

        assert!(newer < legacy, "{newer} should group before {legacy}");
        assert!(newer.started() > legacy.started());
    }

    #[test]
    fn a_name_holding_a_date_no_machine_can_hold_is_not_a_crash() {
        // Nothing crucible writes looks like this: the timestamp is thirteen
        // digits of milliseconds. A file name is whatever somebody typed,
        // though, and the parse that accepts it is deliberately loose about
        // how many digits are in front of the dash.
        let absurd: SessionId = "99999999999999999999-000abc".parse().expect("the shape");

        assert_eq!(absurd.started(), UNIX_EPOCH);
    }

    #[test]
    fn ids_minted_together_differ_and_stay_in_minting_order() {
        let minted: Vec<SessionId> = (0..64).map(|_| SessionId::new()).collect();

        let mut seen = std::collections::HashSet::new();
        for id in &minted {
            assert!(seen.insert(id.clone()), "minted twice: {id}");
        }

        let mut sorted = minted.clone();
        sorted.sort();
        assert_eq!(minted, sorted, "same-millisecond ids left minting order");
    }

    #[test]
    fn a_new_session_id_is_a_uuid_v7() {
        let id = SessionId::new();
        let text = id.as_str();

        let hyphens_where_a_uuid_puts_them = text.len() == 36
            && text.char_indices().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
            });
        assert!(hyphens_where_a_uuid_puts_them, "not uuid-shaped: {text:?}");
        assert_eq!(&text[14..15], "7", "not version 7: {text:?}");
        assert!(
            matches!(&text[19..20], "8" | "9" | "a" | "b"),
            "not an RFC variant: {text:?}"
        );
    }

    #[test]
    fn a_uuid_v7_session_id_parses_and_says_when_it_started() {
        // The first twelve hex digits are 1_700_000_000_123 milliseconds.
        let id: SessionId = "018bcfe5-687b-7abc-8def-0123456789ab"
            .parse()
            .expect("a v7 uuid is a session id");

        assert_eq!(
            id.started(),
            UNIX_EPOCH + Duration::from_millis(1_700_000_000_123)
        );
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
            "018bcfe5-687b-4abc-8def-0123456789ab", // v4: no timestamp to read
            "018bcfe5-687b-7abc-cdef-0123456789ab", // variant outside 8..=b
            "018BCFE5-687B-7ABC-8DEF-0123456789AB", // hex must be lower case
            "018bcfe5687b7abc8def0123456789ab",     // unhyphenated spelling
            "{018bcfe5-687b-7abc-8def-0123456789ab}", // braced spelling
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
        let id = SessionId::at(UNIX_EPOCH + Duration::from_millis(7), [0x00; 10]);
        assert_eq!(
            format!("{id:?}"),
            "SessionId(00000000-0007-7000-8000-000000000000)"
        );
    }

    #[test]
    fn turns_count_from_one_and_advance() {
        assert_eq!(TurnId::FIRST.get(), 1);
        assert_eq!(TurnId::FIRST.next().get(), 2);
        assert!(TurnId::FIRST < TurnId::FIRST.next());
    }

    #[test]
    fn run_ids_minted_together_differ() {
        let minted: Vec<RunId> = (0..64).map(|_| RunId::new()).collect();

        let mut seen = std::collections::HashSet::new();
        for id in &minted {
            assert!(seen.insert(*id), "minted twice: {id}");
        }
    }

    #[test]
    fn a_run_id_reads_as_a_hyphenated_uuid() {
        let shown = RunId::new().to_string();

        let hyphens_where_a_uuid_puts_them = shown.len() == 36
            && shown.char_indices().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
            });
        assert!(hyphens_where_a_uuid_puts_them, "not uuid-shaped: {shown:?}");
        assert_eq!(format!("{:?}", RunId::new()).len(), "RunId()".len() + 36);
    }

    #[test]
    fn a_run_id_is_sixteen_bytes() {
        // Every event carries one, and events are posted per delta. What that
        // costs is the whole of why the id is a `Uuid` rather than something
        // that would have to be looked up or reference-counted.
        assert_eq!(std::mem::size_of::<RunId>(), 16);
    }

    #[test]
    fn an_agent_id_keeps_the_name_it_was_written_down_under() {
        let id = AgentId::new("coding");

        assert_eq!(id.as_str(), "coding");
        assert_eq!(id.to_string(), "coding");
        assert_eq!(format!("{id:?}"), "AgentId(coding)");
    }

    #[test]
    fn a_tool_id_keeps_the_text_the_provider_sent() {
        let id = ToolId::new("toolu_01ABC");
        assert_eq!(id.as_str(), "toolu_01ABC");
        assert_eq!(id.to_string(), "toolu_01ABC");
    }
}
