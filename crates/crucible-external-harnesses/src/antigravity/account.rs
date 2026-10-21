//! What the agent's account steps come to, in the words the user is shown.
//!
//! The agent signs in by printing a link and waiting for the browser to come
//! back to it. It is started with a browser that opens nothing, so the link
//! is read from the line it prints, checked to be the sign-in page of the
//! account it signs in to and nothing longer than a link, and shown as text
//! for the user to open.
//!
//! Each operation ends in a sentence that claims no more than its steps
//! established: a sign-in is verified by a status step after it, a sign-out
//! by the agent's own logout and a status step after it reading signed out,
//! and a disconnect calls the agent for nothing,
//! so it says what it knew of the account beforehand and never more.

use crucible_execution::{Authentication, Failure, Text};

use crate::Reading;

/// What the agent prints before its sign-in link.
pub(crate) const LINK_PREFIX: &str = "Open the following link to authenticate the ACP server: ";

/// The page every sign-in link the agent prints opens.
const SIGN_IN_PAGE: &str = "https://accounts.google.com/o/oauth2/v2/auth?";

/// The most bytes a sign-in link may have.
const LINK_BYTES: usize = 16 * 1024;

/// The sign-in link on `line`, where the line carries one.
///
/// # Errors
///
/// A [`Failure`] where the line announces a link that is not the sign-in
/// page, is longer than a link, or holds anything but printable ASCII.
pub(crate) fn link(line: &str) -> Option<Result<Text, Failure>> {
    let printed = line.strip_prefix(LINK_PREFIX)?.trim_end();
    let sound = printed.len() <= LINK_BYTES
        && printed.starts_with(SIGN_IN_PAGE)
        && !printed.contains('#')
        && printed.chars().all(|letter| letter.is_ascii_graphic());
    Some(if sound {
        Ok(Text::new(printed))
    } else {
        Err(Failure::new(
            "The agent printed a sign-in link that is not its sign-in page, so it was not shown.",
        ))
    })
}

/// How an account operation ended, as the user is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// A sign-in a status step then read as signed in.
    SignedIn,
    /// A sign-in no status step could confirm.
    Unverified(Failure),
    /// The agent's own logout signed the profile out, and a status step after
    /// it read signed out.
    SignedOut,
    /// The instance was taken out of use and the agent's sign-out could not
    /// be confirmed, for this reason.
    SignOutUnverified(Failure),
    /// The instance was taken out of use, the account untouched, with what was
    /// known of it beforehand.
    Disconnected(Authentication),
}

impl Ended {
    /// A sign-in that the status step after it read as `status`.
    #[must_use]
    pub fn signed_in(status: &Reading) -> Self {
        match status {
            Reading::Signed(Authentication::SignedIn) => Self::SignedIn,
            Reading::Unreadable(failure) => Self::Unverified(failure.clone()),
            Reading::Signed(_) | Reading::Version(_) => Self::Unverified(Failure::new(
                "The agent did not report a sign-in afterwards.",
            )),
        }
    }

    /// A sign-out whose logout step read `logout`, and the status step after
    /// it `status`.
    #[must_use]
    pub fn signed_out(logout: &Reading, status: &Reading) -> Self {
        match (logout, status) {
            (
                Reading::Signed(Authentication::SignedOut),
                Reading::Signed(Authentication::SignedOut),
            ) => Self::SignedOut,
            (Reading::Unreadable(failure), _)
            | (Reading::Signed(Authentication::SignedOut), Reading::Unreadable(failure)) => {
                Self::SignOutUnverified(failure.clone())
            }
            (Reading::Signed(_) | Reading::Version(_), _) => Self::SignOutUnverified(Failure::new(
                "The agent did not report being signed out afterwards.",
            )),
        }
    }

    /// A disconnect, the account last known as `known`.
    #[must_use]
    pub const fn disconnected(known: Authentication) -> Self {
        Self::Disconnected(known)
    }

    /// What the user is told.
    #[must_use]
    pub fn says(&self) -> &'static str {
        match self {
            Self::SignedIn => "Signed in",
            Self::Unverified(_) => "Could not verify sign-in",
            Self::SignedOut => "Signed out of this agent profile",
            Self::SignOutUnverified(_) => "Disconnected; external sign-out unverified",
            Self::Disconnected(Authentication::SignedIn) => {
                "Disconnected from Crucible. The external account remains signed in."
            }
            Self::Disconnected(_) => {
                "Disconnected from Crucible. External credentials were not changed."
            }
        }
    }
}
