//! A web source built for a model whose own route is warned, asked about that
//! route before each request it makes.
//!
//! A model's route names no origin: its vendor's address also serves models
//! nobody warned about, so the client's hold lets the address through. A turn
//! on the model is asked about where it is sent; a search or a fetch naming
//! it goes from a tool call and is asked about here instead, each time, so a
//! yes given or forgotten later in the run counts as it would for a turn.

use std::sync::Arc;

use crucible_runtime::{BoxFuture, Cancel};
use crucible_tools::{Fetch, Host, Page, Search, SearchResponse, SourceError};

use super::Consent;

/// `source`, sending nothing while `route` has no yes.
pub(crate) struct Asked<S: ?Sized> {
    source: Arc<S>,
    consent: Consent,
    route: String,
}

impl<S: ?Sized> Asked<S> {
    pub(crate) fn new(source: Arc<S>, consent: &Consent, route: String) -> Self {
        Self {
            source,
            consent: consent.clone(),
            route,
        }
    }

    /// What `named` answers in place of a request, while the route waits.
    ///
    /// Worded as the client's hold words a request it keeps back, so a
    /// search held here reads as one held there.
    fn held(&self, named: &'static str) -> Option<SourceError> {
        self.consent
            .asks(&self.route)
            .map(|warned| SourceError::Transport {
                named,
                problem: format!("nothing was sent: {} waits for an answer", warned.route).into(),
            })
    }
}

impl Search for Asked<dyn Search> {
    fn name(&self) -> &'static str {
        self.source.name()
    }

    fn reaches(&self) -> Host {
        self.source.reaches()
    }

    fn restricts(&self) -> Option<&'static str> {
        self.source.restricts()
    }

    fn answering(&self) -> Result<(), SourceError> {
        self.source.answering()
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<SearchResponse, SourceError>> {
        match self.held(self.source.name()) {
            Some(held) => Box::pin(std::future::ready(Err(held))),
            None => self.source.search(query, cancel),
        }
    }
}

impl Fetch for Asked<dyn Fetch> {
    fn name(&self) -> &'static str {
        self.source.name()
    }

    fn reaches(&self, url: &str) -> Host {
        self.source.reaches(url)
    }

    fn answering(&self) -> Result<(), SourceError> {
        self.source.answering()
    }

    fn fetch<'a>(
        &'a self,
        url: &'a str,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Page, SourceError>> {
        match self.held(self.source.name()) {
            Some(held) => Box::pin(std::future::ready(Err(held))),
            None => self.source.fetch(url, cancel),
        }
    }
}
