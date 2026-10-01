//! Meta's hosted web search, reached in a side request to its Responses API.
//!
//! Search only. Meta's Responses serves `web_search` and no tool that opens
//! one page, so a session on Meta is given no fetch rather than one that would
//! answer every call with an account of a page instead of the page.
//!
//! The request is the one OpenAI's search sends, less `tool_choice`: Meta
//! answers any choice but `auto` with a 400, so the model is asked to search
//! in words rather than made to. An answer written without searching is
//! refused here as it is for OpenAI, and results are the addresses the answer
//! cites. A search call Meta reports as failed, with none beside it that
//! completed, is this search failing in Meta's words; the tool reports it and
//! the turn goes on.

use std::sync::Arc;

use crucible_credentials::{Credential, Outgoing};
use crucible_runtime::{BoxFuture, Cancel};
use crucible_tools::{Host, Search, SearchResponse, SourceError};

use super::{Sending, host_of, openai_input, posted_openai, searched, sent};
use crate::endpoint::Endpoint;
use crate::json::Json;
use crate::transport::Transport;

/// What this source is called, in errors and in what a rule is written about.
const NAME: &str = "meta";

/// Meta's hosted web search, on the session's model and credential.
#[derive(Debug)]
pub struct MetaWeb {
    credential: Box<dyn Credential>,
    transport: Arc<dyn Transport>,
    endpoint: Endpoint,
    model: Box<str>,
}

impl MetaWeb {
    /// A source reaching `endpoint` with `credential`, asking `model`.
    #[must_use]
    pub fn new(
        endpoint: Endpoint,
        credential: Box<dyn Credential>,
        transport: Box<dyn Transport>,
        model: impl Into<Box<str>>,
    ) -> Self {
        Self {
            credential,
            transport: Arc::from(transport),
            endpoint,
            model: model.into(),
        }
    }

    /// The headers Meta's Responses takes, including the secret.
    ///
    /// Raced against `cancel`, as the request itself is: a credential
    /// renewing its token waits for a renewal that is the renewal's own work.
    async fn headers(&self, cancel: &Cancel) -> Result<Outgoing, SourceError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", "text/event-stream");
        cancel
            .race(self.credential.authorize(&mut outgoing))
            .await
            .ok_or(SourceError::Cancelled(NAME))?
            .map_err(|problem| SourceError::Transport {
                named: NAME,
                problem: problem.to_string().into(),
            })?;
        Ok(outgoing)
    }
}

impl Search for MetaWeb {
    fn name(&self) -> &'static str {
        NAME
    }

    fn reaches(&self) -> Host {
        host_of(self.endpoint.as_str())
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<SearchResponse, SourceError>> {
        Box::pin(async move {
            if cancel.requested() {
                return Err(SourceError::Cancelled(NAME));
            }

            let mut json = Json::new();
            json.object(|body| {
                body.text("model", &self.model);
                // A request that is not streamed is held to a time limit the
                // vendor answers with a 504; a streamed one is not.
                body.boolean("stream", true);
                // Asked in words, since the vendor refuses being made to.
                openai_input(body, &format!("Search the web for: {query}"));
                // The vendor keeps a response unless told otherwise, and a
                // query is the user's words.
                body.boolean("store", false);
                body.array("tools", |tools| {
                    tools.object(|declared| declared.text("type", "web_search"));
                });
            });

            let outgoing = self.headers(cancel).await?;
            let redactions = outgoing.redactions();
            let body = json.finish();
            let answered = sent(NAME, cancel, |cancel| async move {
                Box::pin(posted_openai(
                    Sending {
                        named: NAME,
                        transport: self.transport.as_ref(),
                        endpoint: self.endpoint.as_str(),
                    },
                    outgoing,
                    body,
                    &cancel,
                    true,
                ))
                .await
            })
            .await?;

            searched(NAME, &answered, &redactions).map(SearchResponse::from)
        })
    }
}
