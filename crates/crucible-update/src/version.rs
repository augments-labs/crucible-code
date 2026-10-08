//! A release number, and which of two is the later.
//!
//! One grammar, `major.minor.patch`, each part a decimal number written
//! without a leading zero, held by the receipt, by the active-release link and
//! by the name a release source gives its newest release. Nothing else is a
//! release: a prerelease suffix, a fourth part or a `v` in front is refused
//! rather than read past, so a name that does not follow the grammar can never
//! be taken for one later than what is running.

use std::cmp::Ordering;
use std::fmt;

/// A release number, `major.minor.patch`, as the receipt and the layout's
/// directory names spell it.
///
/// Ordered by the numbers it holds, part by part, so `0.47.10` is later than
/// `0.47.9`. Since no part has a leading zero, two that spell the same text
/// are the same release and no two texts are the same release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(Box<str>);

impl Version {
    /// Reads `major.minor.patch`, each a decimal number without a leading
    /// zero; the receipt and the active-release link are held to this one
    /// grammar.
    pub(crate) fn parse(text: &[u8]) -> Option<Self> {
        let mut parts = text.split(|byte| *byte == b'.');
        let parts = [parts.next(), parts.next(), parts.next(), parts.next()];
        let [Some(major), Some(minor), Some(patch), None] = parts else {
            return None;
        };
        if ![major, minor, patch].into_iter().all(number) {
            return None;
        }
        std::str::from_utf8(text).ok().map(|text| Self(text.into()))
    }

    /// The number as the receipt spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Each part, as a length and then its digits, which compare as the
    /// number does because no part has a leading zero.
    fn parts(&self) -> impl Iterator<Item = (usize, &str)> {
        self.0.split('.').map(|part| (part.len(), part))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parts().cmp(other.parts())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `part` is a decimal number written without a leading zero.
fn number(part: &[u8]) -> bool {
    match part {
        b"0" => true,
        [b'1'..=b'9', rest @ ..] => rest.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}
