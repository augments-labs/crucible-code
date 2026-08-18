//! What answers a web search or a fetch.
//!
//! A source is an open set, like a provider and a tool: adding one must not
//! edit this crate. It is asked a question and hands back what it found, and it
//! knows nothing about the tool that asked or the turn around it.
//!
//! Two traits rather than one, because the vendors do not serve one capability.
//! A source that searches and a source that fetches are asked different
//! questions and one exists without the other — so a session reaching a service
//! that only searches registers one tool rather than holding a second that
//! answers every call with *not served*.
//!
//! Neither trait names a transport, a credential or a vendor. That is what lets
//! the tools live in `crucible-tools`, which has no HTTP and must not gain any:
//! the concrete source is built where every other concrete type is, in the
//! binary's own wiring.
//!
//! **What a source hands back is not trusted.** It is a page somebody else
//! wrote, arriving in the same transcript as the user's words and the model's,
//! and nothing here treats it as instruction. The tools bound it; the docs say
//! what it is and is not trusted to say.

use std::fmt;

use crate::cancel::Cancel;
use crate::permission::Host;

/// Why a source could not answer.
///
/// The field naming the source is `named` rather than `source`, which is the
/// word this module is about: `thiserror` reserves a field of that name for the
/// underlying error and tries to treat it as one. A domain word losing to a
/// derive is worth the line saying so, because `source` is what a reader will
/// try to write here next.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The request never arrived, or the connection broke.
    #[error("{named}: {problem}")]
    Transport {
        /// Which source.
        named: &'static str,
        /// What went wrong, with no credential in it.
        problem: Box<str>,
    },

    /// The service answered with a status this request cannot recover from.
    #[error("{named}: HTTP {status}: {message}")]
    Refused {
        /// Which source.
        named: &'static str,
        /// The status.
        status: u16,
        /// What the service said.
        message: Box<str>,
    },

    /// The answer did not have the shape this source expects.
    #[error("{named}: unexpected answer: {problem}")]
    Protocol {
        /// Which source.
        named: &'static str,
        /// What did not fit.
        problem: Box<str>,
    },

    /// The address could not be read, or names a scheme a source will not take.
    ///
    /// Separate from [`Self::Protocol`] because it is about what was asked
    /// rather than about what came back, and it is the model's to correct.
    #[error("{0}")]
    Address(Box<str>),

    /// The user cancelled while the source was answering.
    #[error("{0}: cancelled")]
    Cancelled(&'static str),
}

/// One thing a search handed back.
///
/// Spelled `SearchResult` rather than `Result`, which is Rust's own — the same
/// concession `chunk` makes to a vendor's word in `crucible-provider`. In
/// prose, in comments and in the docs it is a **result**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// The page's title, as the source gave it.
    pub title: Box<str>,
    /// Where it is.
    pub url: Box<str>,
    /// The source's own extract. Never the whole page: what bounds a search is
    /// that a result is a pointer to something rather than the thing itself.
    pub extract: Box<str>,
}

/// A page a fetch handed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Where it came from, after whatever redirects the source followed. Not
    /// necessarily the address that was asked for, which is the point of
    /// carrying it.
    pub url: Box<str>,
    /// The title, where the page had one.
    pub title: Option<Box<str>>,
    /// The page as text.
    pub text: Box<str>,
}

/// Something that answers a query.
pub trait Search: Send + Sync {
    /// What this source is called, in errors and in what a result says.
    fn name(&self) -> &'static str;

    /// Where a query goes, for the verdict and for the rules written about it.
    ///
    /// Asked of the source rather than worked out by the tool, because the tool
    /// holds a `dyn Search` and the address is the one thing about a source it
    /// cannot see. A search reaches one host — the one the user chose — which
    /// is why this takes no argument and a fetch's does.
    fn reaches(&self) -> Host;

    /// Answers `query`.
    ///
    /// # Errors
    ///
    /// [`SourceError`] where the source could not be reached or did not answer
    /// in a shape this implementation reads. No results is not an error.
    fn search(&self, query: &str, cancel: &Cancel) -> Result<Vec<SearchResult>, SourceError>;
}

/// Something that answers a URL.
pub trait Fetch: Send + Sync {
    /// What this source is called.
    fn name(&self) -> &'static str;

    /// Where `url` would go.
    ///
    /// Takes the address because a fetch reaches wherever it is pointed, which
    /// is the whole difference from a search: the host is attacker-influenced
    /// the moment a URL arrives from a result or from a page already fetched.
    /// An address this source cannot read into a host comes back as
    /// [`Host::Opaque`], which matches no rule but a blanket.
    fn reaches(&self, url: &str) -> Host;

    /// Fetches `url`.
    ///
    /// # Errors
    ///
    /// [`SourceError`] where the page could not be had. A page that came back
    /// empty is not an error.
    fn fetch(&self, url: &str, cancel: &Cancel) -> Result<Page, SourceError>;
}

impl fmt::Debug for dyn Search {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Search({})", self.name())
    }
}

impl fmt::Debug for dyn Fetch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fetch({})", self.name())
    }
}
