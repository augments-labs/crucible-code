//! The web sources a session's two web tools answer through, following the
//! provider and model the session is asking now.
//!
//! The tools are registered once, at assembly, and hold a [`Following`]
//! rather than a source: a source names the model it was built for in every
//! request it makes, and is signed with the credential of the provider it
//! reaches. A switch of model or provider builds the sources again, for the
//! provider and model then in force, from that provider's credential resolved
//! the way the switch resolved the provider's own, and puts them here. A
//! source for a model whose route is warned is built held until that route's
//! yes, so the hold follows the model too.
//!
//! Which tools are registered stays what the run started with: the runner's
//! tool set is fixed for the run. Where the provider in force gives the session
//! no source for a registered tool (it serves none, or its credential cannot be
//! used for one), a call is refused as it is checked, before any permission
//! question, saying that nothing was sent; nothing goes to that provider or to
//! the one the run started on.
//!
//! A switch takes the conversation mutably and a turn runs on it the same way,
//! so the source a call was approved against is the one it runs on: nothing
//! is swapped between a call's question and its request.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use crucible_runtime::{BoxFuture, Cancel};
use crucible_tools::{Fetch, Host, Page, Search, SearchResponse, SourceError};

use crate::startup::Reaching;

/// What a tool says it is answered by where nobody is being asked.
const NOBODY: &str = "web";

/// The web sources in force, shared by the two web tools and the conversation
/// that replaces them.
///
/// Cheap to clone: every clone reads and replaces the same sources.
#[derive(Clone)]
pub(crate) struct Following {
    current: Arc<RwLock<Current>>,
}

/// The provider being asked, and what answers the web tools for it.
struct Current {
    named: &'static str,
    reaching: Reaching,
}

impl Default for Following {
    fn default() -> Self {
        Self::new(None, Reaching::nothing())
    }
}

impl fmt::Debug for Following {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let current = self.read();
        out.debug_struct("Following")
            .field("named", &current.named)
            .field("reaching", &current.reaching)
            .finish()
    }
}

impl Following {
    /// The sources `reaching` builds for the provider registered as `named`,
    /// where one is being asked.
    pub(crate) fn new(named: Option<&'static str>, reaching: Reaching) -> Self {
        Self {
            current: Arc::new(RwLock::new(Current {
                named: named.unwrap_or(NOBODY),
                reaching,
            })),
        }
    }

    /// From now on, the sources `reaching` built for the provider `named`.
    pub(crate) fn follow(&self, named: &'static str, reaching: Reaching) {
        let mut current = self.current.write().unwrap_or_else(PoisonError::into_inner);
        *current = Current { named, reaching };
    }

    /// From now on, nothing: nobody is being asked, or for no model.
    pub(crate) fn stop(&self) {
        self.follow(NOBODY, Reaching::nothing());
    }

    /// Whether a search is answered by anything now.
    #[must_use]
    pub(crate) fn searches(&self) -> bool {
        self.read().reaching.searched().is_some()
    }

    /// Whether a fetch is answered by anything now.
    #[must_use]
    pub(crate) fn fetches(&self) -> bool {
        self.read().reaching.fetched().is_some()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Current> {
        self.current.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn searching(&self) -> (&'static str, Option<Arc<dyn Search>>) {
        let current = self.read();
        (current.named, current.reaching.searched())
    }

    fn fetching(&self) -> (&'static str, Option<Arc<dyn Fetch>>) {
        let current = self.read();
        (current.named, current.reaching.fetched())
    }
}

/// What a tool answers where the provider in force gives the session no
/// source for it: one it does not serve, or one its credential cannot be used
/// for.
fn unserved(named: &'static str, tool: &str) -> SourceError {
    SourceError::Transport {
        named,
        problem: format!("nothing was sent: the provider in force gives this session no {tool}")
            .into(),
    }
}

/// Where a call would go where nothing would answer it: a host no rule names.
/// Never asked in practice, since such a call is refused as it is checked.
fn nowhere(tool: &str) -> Host {
    Host::Opaque(format!("nothing: the provider in force gives this session no {tool}").into())
}

impl Search for Following {
    fn name(&self) -> &'static str {
        match self.searching() {
            (_, Some(source)) => source.name(),
            (named, None) => named,
        }
    }

    fn reaches(&self) -> Host {
        self.searching()
            .1
            .map_or_else(|| nowhere("web search"), |source| source.reaches())
    }

    fn restricts(&self) -> Option<&'static str> {
        self.searching().1.and_then(|source| source.restricts())
    }

    fn answering(&self) -> Result<(), SourceError> {
        match self.searching() {
            (_, Some(source)) => source.answering(),
            (named, None) => Err(unserved(named, "web search")),
        }
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<SearchResponse, SourceError>> {
        match self.searching() {
            (_, Some(source)) => Box::pin(async move { source.search(query, cancel).await }),
            (named, None) => Box::pin(std::future::ready(Err(unserved(named, "web search")))),
        }
    }
}

impl Fetch for Following {
    fn name(&self) -> &'static str {
        match self.fetching() {
            (_, Some(source)) => source.name(),
            (named, None) => named,
        }
    }

    fn reaches(&self, url: &str) -> Host {
        self.fetching()
            .1
            .map_or_else(|| nowhere("web fetch"), |source| source.reaches(url))
    }

    fn answering(&self) -> Result<(), SourceError> {
        match self.fetching() {
            (_, Some(source)) => source.answering(),
            (named, None) => Err(unserved(named, "web fetch")),
        }
    }

    fn fetch<'a>(
        &'a self,
        url: &'a str,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Page, SourceError>> {
        match self.fetching() {
            (_, Some(source)) => Box::pin(async move { source.fetch(url, cancel).await }),
            (named, None) => Box::pin(std::future::ready(Err(unserved(named, "web fetch")))),
        }
    }
}
