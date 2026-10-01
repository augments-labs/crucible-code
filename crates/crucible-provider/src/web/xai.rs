//! xAI's hosted web search, reached in a side request to its Responses API.
//!
//! Search only. xAI's `web_search` browses pages inside the search itself and
//! offers no call that opens one address, and the item a search leaves names
//! no action to tell an opened page from a searched one; a fetch would be a
//! guess at what the model did, so a session on xAI is given none.
//!
//! The request is the one OpenAI's search sends, which uses only fields xAI's
//! reference lists, `tool_choice: "required"` among them. Results are the
//! addresses the answer cites, with one difference: xAI's citation carries its
//! number where a title would be, so a result is titled by its address. A
//! search call xAI reports as failed, with none beside it that completed, is
//! this search failing in xAI's words; the tool reports it and the turn goes
//! on. A refusal arrives as `{error: {code, message}}` or as `{code, error}`,
//! and its sentence is read from either.

use std::sync::Arc;

use crucible_credentials::{Credential, Outgoing};
use crucible_runtime::{BoxFuture, Cancel};
use crucible_tools::{Host, Search, SearchResponse, SearchResult, SourceError};

use super::{Sending, host_of, openai_input, posted_openai, searched, sent};
use crate::endpoint::Endpoint;
use crate::json::Json;
use crate::transport::Transport;

/// What this source is called, in errors and in what a rule is written about.
const NAME: &str = "xai";

/// xAI's hosted web search, on the session's model and credential.
#[derive(Debug)]
pub struct XaiWeb {
    credential: Box<dyn Credential>,
    transport: Arc<dyn Transport>,
    endpoint: Endpoint,
    model: Box<str>,
}

impl XaiWeb {
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

    /// The headers xAI's Responses takes, including the secret.
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

impl Search for XaiWeb {
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
                body.boolean("stream", true);
                openai_input(body, query);
                // A query is the user's words, and the vendor keeps a
                // response for retrieval unless told otherwise.
                body.boolean("store", false);
                // Search is what this method was called to do, not a tool
                // the side model may decline in favour of remembered prose.
                body.text("tool_choice", "required");
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

            let found = searched(NAME, &answered, &redactions)?;
            Ok(found.into_iter().map(titled).collect::<Vec<_>>().into())
        })
    }
}

/// A result titled by its address where the vendor gave its citation's number
/// in the title's place, which reads as a title and names nothing.
fn titled(mut result: SearchResult) -> SearchResult {
    if !result.title.is_empty() && result.title.bytes().all(|byte| byte.is_ascii_digit()) {
        result.title = result.url.clone();
    }
    result
}
