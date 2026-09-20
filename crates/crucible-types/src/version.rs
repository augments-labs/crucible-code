//! Which of two release names names the later release.
//!
//! Number by number rather than as text, which is the whole reason this is not
//! a comparison of the two strings: `0.0.10` sorts before `0.0.9` as text, and
//! whoever is on the newer of the two would be told for ever that the older one
//! is ahead of them.
//!
//! Two readers ask this and neither wrote what it is comparing. One is the
//! machine deciding whether a release name it heard from GitHub is worth
//! mentioning; the other is an extension's manifest naming the oldest crucible
//! its author says it works with. One rule answers both, so the two cannot
//! drift into disagreeing about which of two names is ahead.

/// Whether `offered` names a later release than `running`.
///
/// Anything after a `-` is dropped, so a release candidate is read as the
/// release it leads to and is not ahead of it. A part that is not a number
/// reads as nothing, which is what makes a name with no version in it behind
/// every real one rather than ahead of it: a spelling this cannot read is not
/// evidence, and one line of somebody else's text should not be able to claim
/// it is later than every crucible there is.
#[must_use]
pub fn later(offered: &str, running: &str) -> bool {
    let numbers = |version: &str| -> Vec<u64> {
        version
            .split('-')
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|part| part.trim().parse().unwrap_or(0))
            .collect()
    };

    numbers(offered) > numbers(running)
}

#[cfg(test)]
mod tests;
