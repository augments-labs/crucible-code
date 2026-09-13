//! What may be sent to a vendor that did not produce it.
//!
//! A vendor's terms can keep what its own service answered with its own models.
//! That term travels on the result, as its [`ResultProvenance`], so it holds
//! wherever the session goes: a switch to another vendor and a session picked
//! up by a run serving one are the same question asked of the same record. The
//! decision is made here, once, so the runner that applies it never names a
//! vendor or a tool.

use crucible_types::ResultProvenance;

use crate::Provider;

/// What becomes of one recorded result when the conversation is next sent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer<'a> {
    /// It goes as it is.
    Keep,
    /// It may not go; this sentence stands in its place.
    Clear(&'a str),
}

/// Decides whether a result may be sent to `recipient`.
///
/// A result another vendor answered under a term keeping it to that vendor's
/// models goes nowhere else. `leaving` is the provider the conversation is
/// moving away from, and `None` where a session is being picked up rather than
/// moved: it is consulted only for a result recorded before results said who
/// answered them, which is decided the way the build that recorded it decided
/// every search result — taken away when leaving a vendor that restricts its
/// results, and kept otherwise, since nothing says who produced it.
#[must_use]
pub fn transfer<'a>(
    provenance: &'a ResultProvenance,
    recipient: &dyn Provider,
    leaving: Option<&dyn Provider>,
) -> Transfer<'a> {
    match provenance {
        ResultProvenance::Answered {
            vendor,
            restricted: Some(notice),
        } if **vendor != *recipient.name() => Transfer::Clear(notice),
        ResultProvenance::Unrecorded => match leaving {
            Some(left) if left.name() != recipient.name() => left
                .restricts_results()
                .map_or(Transfer::Keep, Transfer::Clear),
            _ => Transfer::Keep,
        },
        ResultProvenance::Unstated | ResultProvenance::Answered { .. } => Transfer::Keep,
    }
}
