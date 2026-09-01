//! Where a number the answer wrote points.
//!
//! A model writing about work in progress writes `#487`, because that is what
//! everyone working on the repository calls it. On a page it is a link; in a
//! terminal it has always been four characters the reader has to carry to a
//! browser themselves. What is missing is not the shape — that is unambiguous
//! — but the repository it is counted against, and this is that fact, held for
//! as long as the session is in that checkout.
//!
//! Assembled here rather than read here. Which repository this is comes out of
//! the checkout, which is the composition layer's to look at; what a number
//! means once you know is a rendering decision and belongs beside the reader
//! that will meet one.
//!
//! Nothing is guessed. A session with no forge in hand draws `#487` exactly as
//! it drew it before, because a link to a repository nobody named is a link
//! somewhere wrong, and somewhere wrong is worse than nowhere at all.

/// The repository a bare number is counted against, and how that forge spells
/// the page it is on.
///
/// One value rather than a formatted prefix, because a reference can name its
/// own repository — `owner/other#12` is a number counted somewhere else on the
/// same forge — and a prefix with the repository already in it could not answer
/// that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forge {
    /// Scheme and host, with no trailing slash: `https://github.com`.
    origin: Box<str>,
    /// The repository this checkout is of: `owner/repo`.
    slug: Box<str>,
    /// What the forge puts between the repository and the number, with a slash
    /// at each end: `/issues/`.
    ///
    /// The issue page rather than the pull-request one, in both spellings this
    /// build knows. A number in prose is an issue or a pull request and the
    /// text cannot say which — but a forge that files both in one series
    /// answers the issue address for either, and one that does not is a forge
    /// where the issue page is at least the right repository and the right
    /// number.
    path: Box<str>,
}

impl Forge {
    /// The repository at `origin/slug`, whose numbers live under `path`.
    #[must_use]
    pub fn new(
        origin: impl Into<Box<str>>,
        slug: impl Into<Box<str>>,
        path: impl Into<Box<str>>,
    ) -> Self {
        Self {
            origin: origin.into(),
            slug: slug.into(),
            path: path.into(),
        }
    }

    /// Where `number` points, in `elsewhere` when the reference named a
    /// repository of its own and in this one otherwise.
    #[must_use]
    pub fn address(&self, elsewhere: Option<&str>, number: &str) -> String {
        let slug = elsewhere.unwrap_or(&self.slug);
        format!("{}/{slug}{}{number}", self.origin, self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_number_is_counted_against_the_repository_in_hand() {
        let forge = Forge::new(
            "https://github.com",
            "augments-labs/crucible-code",
            "/issues/",
        );

        assert_eq!(
            forge.address(None, "487"),
            "https://github.com/augments-labs/crucible-code/issues/487"
        );
    }

    #[test]
    fn a_number_that_named_a_repository_is_counted_against_that_one() {
        // Same forge, different repository: what `owner/other#12` means, and
        // the reason the repository is held apart from the address rather than
        // formatted into it once.
        let forge = Forge::new(
            "https://gitlab.com",
            "augments-labs/crucible-code",
            "/-/issues/",
        );

        assert_eq!(
            forge.address(Some("someone/else"), "12"),
            "https://gitlab.com/someone/else/-/issues/12"
        );
    }
}
