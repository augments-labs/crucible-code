//! Reaching the web through the credential a session already has.
//!
//! A source here is a **side request**: a fresh one-message call to a vendor's
//! own search-enabled endpoint whose only job is to run one search or one fetch
//! and hand back what it found. It is not the coding turn, nothing from it
//! enters the transcript but the text below, and the model never sees a
//! vendor's tool — it sees crucible's, and crucible is the one making the
//! request. That is what leaves a call for the permission engine to hold a
//! verdict about.
//!
//! The cost of doing it this way is real and is not hidden: a vendor's search
//! is metered on an API key, and running one spends a model's tokens on a search
//! engine's job. What it buys is that there is no second service to sign up
//! with, no second key to store, and nothing that stops working because a key
//! nobody set is missing.
//!
//! Unstreamed, unlike everything else in this crate. A turn is read as it
//! arrives because somebody is watching it; a search is one small answer that is
//! useless in halves, so it is read whole and bounded.

use std::io::Read;

use crucible_core::{
    Cancel, Credential, Fetch, Host, Outgoing, Page, Search, SearchResult, SourceError,
};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::json::Json;
use crate::transport::Transport;

#[cfg(test)]
mod tests;

/// The most a source will read from one answer.
///
/// A side request is answered once and read whole, so nothing bounds it the way
/// a stream's own limits bound a turn. Generous, because a fetched page is the
/// point of one of these, and finite because the body is somebody else's.
const MOST: usize = 8 * 1024 * 1024;

/// What a side request asks for, in tokens.
///
/// It has to cover the model's own words around the results, and nothing more:
/// what is wanted from this call is the addresses it found, and a ceiling that
/// let it write an essay would be spending on prose nobody reads.
const CEILING: u32 = 4096;

/// Reads a whole answer, bounded.
fn read(named: &'static str, body: Box<dyn Read + Send>) -> Result<String, SourceError> {
    let mut text = String::new();
    body.take(MOST as u64)
        .read_to_string(&mut text)
        .map_err(|problem| SourceError::Transport {
            named,
            problem: problem.to_string().into(),
        })?;
    Ok(text)
}

/// The value at `pointer`, as text.
fn text_at(value: &Value, pointer: &str) -> Option<Box<str>> {
    value.pointer(pointer)?.as_str().map(Into::into)
}

/// Anthropic's server-side web search and web fetch, reached in a side request.
#[derive(Debug)]
pub struct AnthropicWeb {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
    model: Box<str>,
}

/// What this source is called, in errors and in what a rule is written about.
const ANTHROPIC: &str = "anthropic";

/// The version of the tool this source asks for.
///
/// A dated identifier is the vendor's way of saying the shape can change under
/// a new date and not under this one, so it is pinned here rather than tracked:
/// a newer one is a change to this file, made deliberately, with the shape read
/// again first.
const SEARCH_TOOL: &str = "web_search_20250305";
const FETCH_TOOL: &str = "web_fetch_20250910";

impl AnthropicWeb {
    /// A source reaching `endpoint` with `credential`, asking `model`.
    ///
    /// The model is the session's own rather than a cheap one named here. A name
    /// written into this file is one that outlives the model behind it, and a
    /// credential that reaches crucible at all reaches whatever it is already
    /// using.
    #[must_use]
    pub fn new(
        endpoint: Endpoint,
        credential: Box<dyn Credential>,
        transport: Box<dyn Transport>,
        model: impl Into<Box<str>>,
    ) -> Self {
        Self {
            credential,
            transport,
            endpoint,
            model: model.into(),
        }
    }

    /// Posts one message carrying one server tool, and reads the whole answer.
    fn ask(&self, said: &str, tool: &str, cancel: &Cancel) -> Result<Value, SourceError> {
        if cancel.requested() {
            return Err(SourceError::Cancelled(ANTHROPIC));
        }

        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("anthropic-version", "2023-06-01");
        outgoing.set_header("accept", "application/json");
        self.credential
            .authorize(&mut outgoing)
            .map_err(|problem| SourceError::Transport {
                named: ANTHROPIC,
                problem: problem.to_string().into(),
            })?;

        let redactions = outgoing.redactions();

        let mut json = Json::new();
        json.object(|body| {
            body.text("model", &self.model);
            body.number("max_tokens", CEILING);
            body.array("messages", |messages| {
                messages.object(|message| {
                    message.text("role", "user");
                    message.text("content", said);
                });
            });
            body.array("tools", |tools| {
                tools.object(|declared| {
                    declared.text("type", tool);
                    declared.text(
                        "name",
                        if tool == SEARCH_TOOL {
                            "web_search"
                        } else {
                            "web_fetch"
                        },
                    );
                });
            });
        });

        let response = self
            .transport
            .post(self.endpoint.as_str(), outgoing, json.finish(), cancel)
            .map_err(|problem| SourceError::Transport {
                named: ANTHROPIC,
                problem: redactions.redact(&problem.to_string()).into(),
            })?;

        let body = read(ANTHROPIC, response.body)?;

        if response.status != 200 {
            return Err(SourceError::Refused {
                named: ANTHROPIC,
                status: response.status,
                message: redactions.redact(&body).into(),
            });
        }

        serde_json::from_str(&body).map_err(|problem| SourceError::Protocol {
            named: ANTHROPIC,
            problem: problem.to_string().into(),
        })
    }
}

/// Where this vendor's own service is, for a verdict and for a rule.
fn anthropic_host(endpoint: &Endpoint) -> Host {
    let address = endpoint.as_str();
    match address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"))
        .and_then(|rest| rest.split('/').next())
        .filter(|host| !host.is_empty())
    {
        Some(host) => Host::Named {
            url: address.into(),
            host: host.to_ascii_lowercase().into(),
        },
        None => Host::Opaque(address.into()),
    }
}

impl Search for AnthropicWeb {
    fn name(&self) -> &'static str {
        ANTHROPIC
    }

    fn reaches(&self) -> Host {
        anthropic_host(&self.endpoint)
    }

    fn search(&self, query: &str, cancel: &Cancel) -> Result<Vec<SearchResult>, SourceError> {
        let answered = self.ask(query, SEARCH_TOOL, cancel)?;
        Ok(results(&answered))
    }
}

/// Every result the answer carries, with whatever was quoted from each.
///
/// A `web_search_result` gives a title and an address and no readable extract —
/// its body arrives as `encrypted_content`, which only the vendor's own model
/// can read. What is readable is the citation the model wrote *from* that
/// result, so the two are matched by address and the quoted line becomes the
/// extract. A result nothing was quoted from keeps its place with an empty one:
/// it is still an address worth fetching.
fn results(answered: &Value) -> Vec<SearchResult> {
    let blocks = answered
        .pointer("/content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut quoted: Vec<(Box<str>, Box<str>)> = Vec::new();
    for block in blocks {
        let citations = block
            .pointer("/citations")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();

        for citation in citations {
            if let (Some(url), Some(said)) =
                (text_at(citation, "/url"), text_at(citation, "/cited_text"))
            {
                quoted.push((url, said));
            }
        }
    }

    let mut found = Vec::new();
    for block in blocks {
        if text_at(block, "/type").as_deref() != Some("web_search_tool_result") {
            continue;
        }

        let inside = block
            .pointer("/content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();

        for result in inside {
            let Some(url) = text_at(result, "/url") else {
                continue;
            };

            found.push(SearchResult {
                title: text_at(result, "/title").unwrap_or_else(|| url.clone()),
                extract: quoted
                    .iter()
                    .find(|(cited, _)| *cited == url)
                    .map(|(_, said)| said.clone())
                    .unwrap_or_default(),
                url,
            });
        }
    }

    found
}

impl Fetch for AnthropicWeb {
    fn name(&self) -> &'static str {
        ANTHROPIC
    }

    /// Where `url` would go, read off the address itself.
    ///
    /// Deliberately strict. Anything carrying user information, anything with no
    /// host, and anything that is not http or https comes back opaque and so
    /// matches no rule but a blanket — `https://docs.rs@evil.example/` is the
    /// reading a lenient parse gets wrong, and the cost of being wrong is a
    /// rule somebody wrote about `docs.rs` reaching somewhere else entirely.
    fn reaches(&self, url: &str) -> Host {
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"));

        let host = rest
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .filter(|authority| {
                !authority.is_empty() && !authority.contains('@') && !authority.contains(':')
            });

        match host {
            Some(host) => Host::Named {
                url: url.into(),
                host: host.to_ascii_lowercase().into(),
            },
            None => Host::Opaque(url.into()),
        }
    }

    fn fetch(&self, url: &str, cancel: &Cancel) -> Result<Page, SourceError> {
        if matches!(Fetch::reaches(self, url), Host::Opaque(_)) {
            return Err(SourceError::Address(
                format!("{url} is not an http or https address naming a host").into(),
            ));
        }

        // The address goes in the message because this vendor's fetch will only
        // reach a URL that already appeared in the conversation — a rule of its
        // own against a model inventing one, and here the conversation is one
        // message long and crucible wrote it.
        let answered = self.ask(&format!("Fetch {url}"), FETCH_TOOL, cancel)?;
        page(&answered, url)
    }
}

/// The page an answer carries.
fn page(answered: &Value, asked: &str) -> Result<Page, SourceError> {
    let blocks = answered
        .pointer("/content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    for block in blocks {
        if text_at(block, "/type").as_deref() != Some("web_fetch_tool_result") {
            continue;
        }

        if let Some(code) = text_at(block, "/content/error_code") {
            return Err(SourceError::Protocol {
                named: ANTHROPIC,
                problem: code,
            });
        }

        let Some(text) = text_at(block, "/content/content/source/data") else {
            continue;
        };

        return Ok(Page {
            // Where it ended up, falling back to where it was pointed. A
            // redirect is the case this carries, and a source that reported the
            // address it was asked for would hide exactly that.
            url: text_at(block, "/content/url").unwrap_or_else(|| asked.into()),
            title: text_at(block, "/content/content/title"),
            text,
        });
    }

    Err(SourceError::Protocol {
        named: ANTHROPIC,
        problem: "the answer carried no fetched page".into(),
    })
}
