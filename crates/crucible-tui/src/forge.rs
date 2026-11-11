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

    /// Whether this forge keeps its pages under `/-/`, as GitLab does.
    fn nests(&self) -> bool {
        self.path.starts_with(NESTING)
    }
}

/// How many digits a number is read as, at most.
///
/// A forge counts in the thousands and this is room for a great deal more than
/// that. The bound is here for the reason every other one in the reader is:
/// what is held comes off a wire, and a run of digits nobody meant to end is
/// not a reason to grow a string without limit.
pub(crate) const DIGITS: usize = 12;

/// What a repository is named with, either side of the slash.
pub(crate) fn named(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '_' | '-')
}

/// What a forge that nests its pages puts between the repository and them.
const NESTING: &str = "/-/";

/// One thing on a forge an address names: what the reader draws as the
/// reference somebody working on the repository would have written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// A pull request, or a merge request on a forge that nests its pages.
    PullRequest(Item),
    /// An issue.
    Issue(Item),
    /// A commit, named by the digits of its hash the address gave.
    Commit(Item),
}

/// Where a [`Reference`] is, and which one it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Scheme and host, with no trailing slash: `https://github.com`.
    origin: Box<str>,
    /// The repository: `owner/repo`, or `group/subgroup/repo`.
    slug: Box<str>,
    /// The number, or the hash.
    id: Box<str>,
    /// Whether the forge keeps its pages under `/-/`.
    nested: bool,
}

impl Reference {
    /// The reference `address` names, read against `forge`, or `None` where
    /// it names anything else.
    ///
    /// Two hosts are known by name, and the checkout's own is known by the
    /// spelling its forge was read with. Only the `https` address of the item
    /// itself: anything after it -- a tab of the page, a query, an anchor --
    /// is somewhere more particular than the reference, and is left the
    /// ordinary link it is.
    #[must_use]
    pub fn read(address: &str, forge: Option<&Forge>) -> Option<Self> {
        let rest = address.strip_prefix(SECURE)?;
        let (host, path) = rest.split_once('/')?;
        let origin = address.get(..SECURE.len() + host.len())?;
        let nested = match host {
            GITHUB => false,
            GITLAB => true,
            _ => forge.filter(|forge| *forge.origin == *origin)?.nests(),
        };

        let path = path.strip_suffix('/').unwrap_or(path);
        let (front, id) = path.rsplit_once('/')?;
        let (front, kind) = front.rsplit_once('/')?;
        let slug = if nested {
            front.strip_suffix(NESTING.trim_end_matches('/'))?
        } else {
            front
        };
        let segments = slug.split('/').count();
        let fits = if nested { segments >= 2 } else { segments == 2 };
        if !fits
            || slug
                .split('/')
                .any(|part| part.is_empty() || !part.chars().all(named))
        {
            return None;
        }

        let item = Item {
            origin: origin.into(),
            slug: slug.into(),
            id: id.into(),
            nested,
        };
        let numbered =
            !id.is_empty() && id.len() <= DIGITS && id.bytes().all(|b| b.is_ascii_digit());
        let hashed = (HASH_SHORT..=HASH_LONG).contains(&id.len())
            && id.bytes().all(|b| b.is_ascii_hexdigit());
        match (kind, nested) {
            ("pull", false) | ("merge_requests", true) if numbered => Some(Self::PullRequest(item)),
            ("issues", _) if numbered => Some(Self::Issue(item)),
            ("commit", _) if hashed => Some(Self::Commit(item)),
            _ => None,
        }
    }

    /// The reference as it is drawn: short against `forge` where the address
    /// is in that repository, and naming its repository everywhere else.
    ///
    /// The same repository on another host is another repository, so it is
    /// named in full. A commit is drawn by as many digits of its hash as a
    /// person would quote.
    #[must_use]
    pub fn short(&self, forge: Option<&Forge>) -> String {
        let (item, mark) = match self {
            Self::PullRequest(item) if item.nested => (item, "!"),
            Self::PullRequest(item) | Self::Issue(item) => (item, "#"),
            Self::Commit(item) => (item, "@"),
        };
        let id = match self {
            Self::Commit(_) => item.id.get(..HASH_SHORT).unwrap_or(&item.id),
            _ => &item.id,
        };
        let home =
            forge.is_some_and(|forge| forge.origin == item.origin && forge.slug == item.slug);
        match (self, home) {
            (Self::Commit(_), true) => id.to_owned(),
            (_, true) => format!("{mark}{id}"),
            (_, false) => format!("{}{mark}{id}", item.slug),
        }
    }
}

/// The scheme a reference is read under.
const SECURE: &str = "https://";

/// The forge that files pull requests beside its issues, by its name.
const GITHUB: &str = "github.com";

/// The forge that keeps its pages under `/-/`, by its name.
const GITLAB: &str = "gitlab.com";

/// The fewest digits of a hash a commit is read by, and how many it is drawn
/// with.
const HASH_SHORT: usize = 7;

/// The most digits a hash has.
const HASH_LONG: usize = 40;

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

    /// The forge this checkout is on, as the composition root would build it.
    fn here(origin: &str, path: &str) -> Forge {
        Forge::new(origin, "augments-labs/crucible-code", path)
    }

    /// `address` drawn against `forge`, or `None` where it is no reference.
    fn drawn(address: &str, forge: Option<&Forge>) -> Option<String> {
        Reference::read(address, forge).map(|reference| reference.short(forge))
    }

    #[test]
    fn a_pull_request_issue_and_commit_in_this_checkout_are_drawn_as_its_own() {
        let forge = here("https://github.com", "/issues/");
        let at = "https://github.com/augments-labs/crucible-code";

        assert_eq!(
            drawn(&format!("{at}/pull/1324"), Some(&forge)).as_deref(),
            Some("#1324")
        );
        assert_eq!(
            drawn(&format!("{at}/issues/1288/"), Some(&forge)).as_deref(),
            Some("#1288")
        );
        assert_eq!(
            drawn(&format!("{at}/commit/58ed227aa01"), Some(&forge)).as_deref(),
            Some("58ed227")
        );
    }

    #[test]
    fn a_reference_elsewhere_or_with_no_forge_names_its_repository() {
        let forge = here("https://github.com", "/issues/");

        assert_eq!(
            drawn("https://github.com/ratatui/ratatui/pull/2101", Some(&forge)).as_deref(),
            Some("ratatui/ratatui#2101")
        );
        assert_eq!(
            drawn(
                "https://github.com/augments-labs/crucible-code/pull/1324",
                None
            )
            .as_deref(),
            Some("augments-labs/crucible-code#1324")
        );
        assert_eq!(
            drawn("https://github.com/o/r/commit/abcdef1234", None).as_deref(),
            Some("o/r@abcdef1")
        );
    }

    #[test]
    fn the_same_repository_on_another_host_is_not_this_one() {
        let forge = here("https://github.com", "/issues/");

        assert_eq!(
            drawn(
                "https://gitlab.com/augments-labs/crucible-code/-/issues/7",
                Some(&forge)
            )
            .as_deref(),
            Some("augments-labs/crucible-code#7")
        );
    }

    #[test]
    fn a_merge_request_is_drawn_with_a_bang_and_an_issue_with_a_hash() {
        let forge = here("https://gitlab.com", "/-/issues/");
        let at = "https://gitlab.com/augments-labs/crucible-code";

        assert_eq!(
            drawn(&format!("{at}/-/merge_requests/12"), Some(&forge)).as_deref(),
            Some("!12")
        );
        assert_eq!(
            drawn(&format!("{at}/-/issues/12"), Some(&forge)).as_deref(),
            Some("#12")
        );
        assert_eq!(
            drawn(
                "https://gitlab.com/group/sub/repo/-/merge_requests/3",
                Some(&forge)
            )
            .as_deref(),
            Some("group/sub/repo!3")
        );
        // GitLab's spelling on GitHub, and GitHub's on GitLab, name nothing.
        assert_eq!(drawn("https://gitlab.com/o/r/pull/3", Some(&forge)), None);
        assert_eq!(
            drawn("https://github.com/o/r/-/merge_requests/3", None),
            None
        );
    }

    #[test]
    fn the_checkouts_own_host_is_read_in_the_spelling_its_forge_uses() {
        let nested = here("https://git.example.test", "/-/issues/");
        let filed = here("https://git.example.test", "/issues/");
        let address = "https://git.example.test/augments-labs/crucible-code/-/merge_requests/9";

        assert_eq!(drawn(address, Some(&nested)).as_deref(), Some("!9"));
        assert_eq!(drawn(address, Some(&filed)), None);
        assert_eq!(drawn(address, None), None);
    }

    #[test]
    fn anything_past_the_item_or_off_the_web_s_encrypted_scheme_is_an_ordinary_link() {
        let forge = here("https://github.com", "/issues/");
        for address in [
            "https://github.com/o/r/pull/12/files",
            "https://github.com/o/r/pull/12?diff=split",
            "https://github.com/o/r/pull/12#issuecomment-1",
            "http://github.com/o/r/pull/12",
            "https://example.com/o/r/pull/12",
            "https://github.com/o/r/pull/",
            "https://github.com/o/r/pull/12a",
            "https://github.com/o/r/pull/1234567890123",
            "https://github.com/o/r/commit/abcdef",
            "https://github.com/o/r/commit/abcdefg",
            "https://github.com/o/r/commit/0123456789012345678901234567890123456789a",
            "https://github.com/o/r/x/pull/12",
            "https://github.com/o//pull/12",
            "https://github.com/o/r/pull/12//",
            "https://github.com/o/r\u{1b}/pull/12",
        ] {
            assert_eq!(drawn(address, Some(&forge)), None, "{address}");
        }
        // The bounds themselves still name something.
        assert!(drawn("https://github.com/o/r/pull/123456789012", None).is_some());
        assert!(drawn("https://github.com/o/r/commit/abcdef1", None).is_some());
        assert!(
            drawn(
                "https://github.com/o/r/commit/0123456789012345678901234567890123456789",
                None
            )
            .is_some()
        );
    }
}
