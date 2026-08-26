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
//! A side answer is still consumed whole and bounded. OpenAI requires its
//! Responses transport to be streamed, so that source frames the events but
//! keeps only the terminal response; the result is no more visible in halves
//! than either vendor's unstreamed answer.

use std::io::{self, BufReader, Read};
use std::time::{Duration, Instant};

use crucible_core::{
    Cancel, Credential, Fetch, Host, Outgoing, Page, Redactions, Search, SearchResult, SourceError,
};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::json::Json;
use crate::sse::{Events, Framed};
use crate::transport::{Response, Transport};

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

/// What a *fetch* asks for, in tokens.
///
/// Larger than a search's, because this vendor puts the fetched document into
/// the response content rather than beside it: a ceiling sized for prose stops
/// the answer part-way through the page, and what reaches [`page`] is then no
/// page at all. The tool is separately told what to keep, so the two bounds
/// agree instead of the outer one truncating whatever the inner one allowed.
const FETCH_CEILING: u32 = 32_768;

/// What the fetch tool is told to keep of a page, in tokens.
const FETCH_CONTENT: u32 = 24_000;

/// The longest reading one answer may take altogether.
///
/// Bodies arrive through a reader that reports a wait that expired as an
/// interruption — the kind the `Read` contract says to retry — so a service
/// that answers its headers and then stalls without closing is a reader that
/// neither ends nor errors, and reading it to the end retries for ever. The
/// whole read is bounded rather than the gaps in it, because a peer trickling
/// one byte per gap satisfies every gap and still holds the thread for as long
/// as it likes. Two minutes because the largest honest answer here is a
/// fetched page, and one that has not finished arriving in that long is not
/// going to.
const MAX_WAIT: Duration = Duration::from_mins(2);

/// Reads a whole answer, bounded in bytes and in time.
fn read(
    named: &'static str,
    body: Box<dyn Read + Send>,
    cancel: &Cancel,
) -> Result<String, SourceError> {
    filled(named, body, MAX_WAIT, cancel)
}

/// The same, with a wait a test can hand over as none.
///
/// The wait rather than the deadline it makes, so nothing here adds to an
/// `Instant` — that addition panics where it overflows, and a bound against
/// hanging is a poor place to put a new way to fail.
fn filled(
    named: &'static str,
    body: Box<dyn Read + Send>,
    wait: Duration,
    cancel: &Cancel,
) -> Result<String, SourceError> {
    let since = Instant::now();
    let mut body = body.take(MOST as u64);
    let mut said = Vec::new();
    let mut into = [0_u8; 8 * 1024];

    let transport = |problem: &io::Error| SourceError::Transport {
        named,
        problem: problem.to_string().into(),
    };

    loop {
        if cancel.requested() {
            return Err(SourceError::Cancelled(named));
        }
        if since.elapsed() >= wait {
            return Err(transport(&timed_out()));
        }

        let read = body.read(&mut into);
        if cancel.requested() {
            return Err(SourceError::Cancelled(named));
        }
        if since.elapsed() >= wait {
            return Err(transport(&timed_out()));
        }

        match read {
            Ok(0) => break,
            Ok(read) => said.extend_from_slice(into.get(..read).unwrap_or_default()),
            Err(problem) if problem.kind() == io::ErrorKind::Interrupted => {}
            Err(problem) => return Err(transport(&problem)),
        }
    }

    String::from_utf8(said).map_err(|problem| SourceError::Transport {
        named,
        problem: problem.to_string().into(),
    })
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "it stopped part-way through")
}

/// The value at `pointer`, as text.
fn text_at(value: &Value, pointer: &str) -> Option<Box<str>> {
    value.pointer(pointer)?.as_str().map(Into::into)
}

/// The authority without its port, where the port is one.
///
/// A port is not part of what a rule is about — `example.com:8443` and
/// `example.com` are one host to anybody writing policy — but it cannot simply
/// be ignored either, because `docs.rs:8443@evil.example` is not a port at all.
/// So a single trailing `:digits` is removed and anything else keeps the colon,
/// which then fails the `@` check or reads as no host.
fn port_stripped(authority: &str) -> Option<&str> {
    let Some((host, port)) = authority.rsplit_once(':') else {
        return Some(authority);
    };

    if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) {
        Some(host)
    } else {
        None
    }
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
        let fetching = tool == FETCH_TOOL;
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

        let mut json = Json::new();
        json.object(|body| {
            body.text("model", &self.model);
            body.number("max_tokens", if fetching { FETCH_CEILING } else { CEILING });
            body.array("messages", |messages| {
                messages.object(|message| {
                    message.text("role", "user");
                    message.text("content", said);
                });
            });
            body.array("tools", |tools| {
                tools.object(|declared| {
                    declared.text("type", tool);
                    declared.text("name", if fetching { "web_fetch" } else { "web_search" });
                    // A side request is exactly one source operation. Letting
                    // the model repeat it can multiply metered calls without
                    // adding another result the caller asked for.
                    declared.number("max_uses", 1);
                    if fetching {
                        declared.number("max_content_tokens", FETCH_CONTENT);
                    }
                });
            });
        });

        posted(
            Sending {
                named: ANTHROPIC,
                transport: self.transport.as_ref(),
                endpoint: self.endpoint.as_str(),
            },
            outgoing,
            json.finish(),
            cancel,
        )
    }
}

/// Who is sending, and where.
///
/// One value rather than three arguments, for the reason `ApiAudience` is one
/// in the binary: a name, an address and the transport that reaches it are one
/// fact about a source, and a call site free to pair them by hand is a call
/// site that can post one vendor's body to another's address.
#[derive(Clone, Copy)]
struct Sending<'a> {
    named: &'static str,
    transport: &'a dyn Transport,
    endpoint: &'a str,
}

/// Posts one body and reads the whole answer as JSON.
///
/// Shared because the difference between two vendors here is the body and the
/// headers, and a second copy of the status-and-redaction handling is a second
/// place for a key to escape.
fn posted(
    sending: Sending<'_>,
    outgoing: Outgoing,
    body: String,
    cancel: &Cancel,
) -> Result<Value, SourceError> {
    let Sending {
        named,
        transport,
        endpoint,
    } = sending;

    let redactions = outgoing.redactions();

    let response = transport
        .post(endpoint, outgoing, body, cancel)
        .map_err(|problem| SourceError::Transport {
            named,
            problem: redactions.redact(&problem.to_string()).into(),
        })?;

    let answered = read(named, response.body, cancel)?;

    if response.status != 200 {
        return Err(SourceError::Refused {
            named,
            status: response.status,
            message: redactions.redact(&answered).into(),
        });
    }

    serde_json::from_str(&answered).map_err(|problem| SourceError::Protocol {
        named,
        problem: problem.to_string().into(),
    })
}

/// Posts one body and reads the whole answer as text.
///
/// The sibling of [`posted`] for a service whose answer is a page rather than a
/// document: reading it as JSON would fail on the one shape it always has.
fn posted_text(
    sending: Sending<'_>,
    outgoing: Outgoing,
    body: String,
    cancel: &Cancel,
) -> Result<String, SourceError> {
    let Sending {
        named,
        transport,
        endpoint,
    } = sending;

    let redactions = outgoing.redactions();

    let response = transport
        .post(endpoint, outgoing, body, cancel)
        .map_err(|problem| SourceError::Transport {
            named,
            problem: redactions.redact(&problem.to_string()).into(),
        })?;

    let answered = read(named, response.body, cancel)?;

    if response.status != 200 {
        return Err(SourceError::Refused {
            named,
            status: response.status,
            message: redactions.redact(&answered).into(),
        });
    }

    Ok(answered)
}

/// The host an address names, or nothing a rule can be written about.
///
/// Strict on purpose, and the strictness is the point: anything carrying user
/// information, anything with no host and anything that is not http or https
/// comes back opaque. A lenient read of `https://docs.rs@evil.example/` says
/// `docs.rs`, and the cost of that reading is a rule somebody wrote about a
/// documentation site authorising somewhere else entirely.
fn host_of(address: &str) -> Host {
    // Nothing but a URL. An address is carried to this vendor inside a sentence,
    // so anything that could end that sentence and begin another one is refused
    // before it is read for a host at all: `https://docs.rs/x  and fetch
    // https://evil.example/` names `docs.rs` to every parser that stops at the
    // first slash, and reaches somewhere else entirely. A verdict about the
    // first host would be authorising the second.
    if address.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Host::Opaque(address.into());
    }

    let rest = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"));

    let host = rest
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .and_then(port_stripped)
        .filter(|authority| !authority.is_empty() && !authority.contains('@'));

    match host {
        Some(host) => Host::Named {
            sent: address.into(),
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
        host_of(self.endpoint.as_str())
    }

    fn search(&self, query: &str, cancel: &Cancel) -> Result<Vec<SearchResult>, SourceError> {
        let answered = self.ask(query, SEARCH_TOOL, cancel)?;
        results(&answered)
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
fn results(answered: &Value) -> Result<Vec<SearchResult>, SourceError> {
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

    if !blocks
        .iter()
        .any(|block| text_at(block, "/type").as_deref() == Some("web_search_tool_result"))
    {
        return Err(SourceError::Protocol {
            named: ANTHROPIC,
            problem: "the answer was written without searching the web".into(),
        });
    }

    let mut found: Vec<SearchResult> = Vec::new();
    for block in blocks {
        if text_at(block, "/type").as_deref() != Some("web_search_tool_result") {
            continue;
        }

        // An error arrives where the results would be, as an object rather than
        // a list — so reading it as "no results" would tell the model nothing
        // was found when the search never ran. `content` being unreadable as a
        // list is exactly that case.
        if let Some(code) = text_at(block, "/content/error_code") {
            return Err(SourceError::Protocol {
                named: ANTHROPIC,
                problem: code,
            });
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

            // The vendor's model may run several searches in one side request,
            // and overlapping queries return the same page in more than one of
            // them. A repeat would spend a caller's limit twice over.
            if found.iter().any(|already| already.url == url) {
                continue;
            }

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

    Ok(found)
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
        host_of(url)
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

/// What OpenAI's source is called.
const OPENAI: &str = "openai";

/// OpenAI's hosted web search, reached in a side request to the Responses API.
///
/// One tool serves both jobs here. This vendor has no standalone fetch: reading
/// a page is an *action* inside its search tool — `open_page`, alongside
/// `search` and `find_in_page` — which is how its own agent models it too. So a
/// fetch is the same tool asked to open one address, with the search confined
/// to that address's host so it cannot wander off to another.
///
/// What that costs is fidelity, and it is worth knowing: what comes back is the
/// model's rendering of the page rather than the page. Anthropic's fetch hands
/// over the document; this hands over an account of it.
#[derive(Debug)]
pub struct OpenAiWeb {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
    model: Box<str>,
}

impl OpenAiWeb {
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
            transport,
            endpoint,
            model: model.into(),
        }
    }

    /// The headers both Responses services accept, including the secret.
    fn headers(&self) -> Result<Outgoing, SourceError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", "text/event-stream");
        self.credential
            .authorize(&mut outgoing)
            .map_err(|problem| SourceError::Transport {
                named: OPENAI,
                problem: problem.to_string().into(),
            })?;
        Ok(outgoing)
    }

    /// Posts a streamed Responses request and keeps its terminal response.
    fn ask(&self, body: String, cancel: &Cancel) -> Result<Value, SourceError> {
        posted_openai(
            Sending {
                named: OPENAI,
                transport: self.transport.as_ref(),
                endpoint: self.endpoint.as_str(),
            },
            self.headers()?,
            body,
            cancel,
        )
    }
}

/// Writes one side request as the message list both Responses services accept.
///
/// The published API documents both a bare string and a list. The `ChatGPT`
/// Responses service answers the string with `Input must be a list`, and its own
/// client sends a list, so this takes the common shape rather than branching on
/// which credential selected the otherwise shared protocol.
fn openai_input(body: &mut crate::json::Object<'_>, text: &str) {
    body.array("input", |input| {
        input.object(|message| {
            message.text("role", "user");
            message.text("content", text);
        });
    });
}

/// Posts the one Responses shape accepted by both OpenAI services.
///
/// The public API can answer without streaming, but the `ChatGPT` account endpoint
/// rejects that shape with `Stream must be set to true`. The terminal
/// `response.completed` event carries the same whole response object the
/// unstreamed API would have returned, so this frames the existing bounded body
/// and hands that object to the existing result readers.
fn posted_openai(
    sending: Sending<'_>,
    outgoing: Outgoing,
    body: String,
    cancel: &Cancel,
) -> Result<Value, SourceError> {
    let Sending {
        named,
        transport,
        endpoint,
    } = sending;
    let redactions = outgoing.redactions();
    let Response { status, body } =
        transport
            .post(endpoint, outgoing, body, cancel)
            .map_err(|problem| SourceError::Transport {
                named,
                problem: redactions.redact(&problem.to_string()).into(),
            })?;

    if status != 200 {
        let answered = read(named, body, cancel)?;
        return Err(SourceError::Refused {
            named,
            status,
            message: redactions.redact(&answered).into(),
        });
    }

    openai_response(body, cancel, &redactions)
}

/// Reads a streamed side response through the same bounded SSE framing as turns.
fn openai_response(
    body: Box<dyn Read + Send>,
    cancel: &Cancel,
    redactions: &Redactions,
) -> Result<Value, SourceError> {
    let since = Instant::now();
    let mut events = Events::new(BufReader::new(body.take(MOST as u64)));
    let mut finished = Vec::new();

    while let Some(next) = events.next() {
        if cancel.requested() {
            return Err(SourceError::Cancelled(OPENAI));
        }
        if since.elapsed() >= MAX_WAIT {
            return Err(SourceError::Transport {
                named: OPENAI,
                problem: timed_out().to_string().into(),
            });
        }

        let event = match next {
            Ok(Framed::Quiet) => continue,
            Ok(Framed::Event(event)) => event,
            Err(problem) => {
                return Err(SourceError::Transport {
                    named: OPENAI,
                    problem: problem.to_string().into(),
                });
            }
        };

        let data = event.data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let payload: Value =
            serde_json::from_str(data).map_err(|problem| SourceError::Protocol {
                named: OPENAI,
                problem: format!("an event was not JSON: {problem}").into(),
            })?;

        match text_at(&payload, "/type").as_deref() {
            Some("response.output_item.done") => {
                let item = payload
                    .get("item")
                    .cloned()
                    .ok_or_else(|| SourceError::Protocol {
                        named: OPENAI,
                        problem: "response.output_item.done carried no item".into(),
                    })?;
                finished.push(item);
            }
            Some("response.completed") => {
                let mut response =
                    payload
                        .get("response")
                        .cloned()
                        .ok_or_else(|| SourceError::Protocol {
                            named: OPENAI,
                            problem: "response.completed carried no response".into(),
                        })?;

                // The public endpoint repeats its output here. The ChatGPT plan
                // backend sends an empty list after narrating each finished item,
                // so retain those items only where the terminal object has none.
                let has_output = response
                    .get("output")
                    .and_then(Value::as_array)
                    .is_some_and(|output| !output.is_empty());
                if !has_output && !finished.is_empty() {
                    let object = response
                        .as_object_mut()
                        .ok_or_else(|| SourceError::Protocol {
                            named: OPENAI,
                            problem: "response.completed did not carry an object".into(),
                        })?;
                    object.insert("output".to_owned(), Value::Array(finished));
                }
                return Ok(response);
            }
            Some("response.failed") => return Err(openai_failed(&payload, redactions)),
            Some("response.incomplete") => {
                let reason = text_at(&payload, "/response/incomplete_details/reason")
                    .unwrap_or_else(|| "the response was incomplete".into());
                return Err(SourceError::Protocol {
                    named: OPENAI,
                    problem: redactions.redact(&reason).into(),
                });
            }
            Some("error") => return Err(openai_upstream(&payload, redactions)),
            _ => {}
        }
    }

    Err(SourceError::Protocol {
        named: OPENAI,
        problem: "the stream ended before response.completed".into(),
    })
}

/// A response the provider gave up on after accepting the request.
fn openai_failed(payload: &Value, redactions: &Redactions) -> SourceError {
    let error = payload
        .pointer("/response/error")
        .filter(|error| !error.is_null());
    if let Some(error) = error {
        return openai_upstream(error, redactions);
    }

    let kind = text_at(payload, "/response/status").unwrap_or_else(|| "error".into());
    let message = text_at(payload, "/response/incomplete_details/reason")
        .unwrap_or_else(|| "the provider gave up on the response and named no reason".into());
    SourceError::Protocol {
        named: OPENAI,
        problem: redactions.redact(&format!("{kind}: {message}")).into(),
    }
}

/// A failure event, flat or nested under a failed response.
fn openai_upstream(error: &Value, redactions: &Redactions) -> SourceError {
    let kind = text_at(error, "/code")
        .or_else(|| text_at(error, "/type"))
        .unwrap_or_else(|| "error".into());
    let message = text_at(error, "/message")
        .unwrap_or_else(|| "the provider did not say what went wrong".into());
    SourceError::Protocol {
        named: OPENAI,
        problem: redactions.redact(&format!("{kind}: {message}")).into(),
    }
}

impl Search for OpenAiWeb {
    fn name(&self) -> &'static str {
        OPENAI
    }

    fn reaches(&self) -> Host {
        host_of(self.endpoint.as_str())
    }

    fn search(&self, query: &str, cancel: &Cancel) -> Result<Vec<SearchResult>, SourceError> {
        if cancel.requested() {
            return Err(SourceError::Cancelled(OPENAI));
        }

        let mut json = Json::new();
        json.object(|body| {
            body.text("model", &self.model);
            body.boolean("stream", true);
            openai_input(body, query);
            // This endpoint retains a response for retrieval unless told
            // otherwise, and a query is the user's words.
            body.boolean("store", false);
            // Search is the operation this method was called to perform, not a
            // tool the side model may decline in favour of remembered prose.
            body.text("tool_choice", "required");
            body.array("tools", |tools| {
                tools.object(|declared| {
                    declared.text("type", "web_search");
                });
            });
        });

        let answered = self.ask(json.finish(), cancel)?;
        if !web_called(&answered) {
            return Err(SourceError::Protocol {
                named: OPENAI,
                problem: "the answer was written without searching the web".into(),
            });
        }

        Ok(cited(&answered))
    }
}

/// Every address this answer cited, with the span of prose written off it.
///
/// This vendor reports its results as *annotations* on the text rather than as
/// a block of their own: what comes back is the model's answer with a citation
/// marking the run of characters each address supports. So the extract is that
/// run, sliced out of the very text it annotates.
///
/// The indices are the vendor's and the string is this program's, so they are
/// checked rather than trusted: an index past the end, or one that lands inside
/// a character, yields no extract instead of a panic. A result with no readable
/// span is still an address worth fetching.
fn cited(answered: &Value) -> Vec<SearchResult> {
    let mut found: Vec<SearchResult> = Vec::new();

    let output = answered
        .pointer("/output")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    for item in output {
        let parts = item
            .pointer("/content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();

        for part in parts {
            let said = text_at(part, "/text").unwrap_or_default();
            let annotations = part
                .pointer("/annotations")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();

            for annotation in annotations {
                if text_at(annotation, "/type").as_deref() != Some("url_citation") {
                    continue;
                }

                let Some(url) = text_at(annotation, "/url") else {
                    continue;
                };

                if found.iter().any(|already| already.url == url) {
                    continue;
                }

                found.push(SearchResult {
                    title: text_at(annotation, "/title").unwrap_or_else(|| url.clone()),
                    extract: span(&said, annotation),
                    url,
                });
            }
        }
    }

    found
}

/// The run of `said` an annotation points at, where it points at a real one.
fn span(said: &str, annotation: &Value) -> Box<str> {
    let at = |name: &str| {
        annotation
            .pointer(name)
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
    };

    let (Some(from), Some(to)) = (at("/start_index"), at("/end_index")) else {
        return "".into();
    };

    if from >= to {
        return "".into();
    }

    // Character positions, not byte offsets. For ASCII the two agree, which is
    // why a slice by bytes looks right until the model writes an em dash — then
    // it lands short of the text or inside a character.
    let mut characters = said.char_indices().map(|(at, _)| at);
    let Some(start) = characters.nth(from) else {
        return "".into();
    };

    said.char_indices()
        .map(|(at, _)| at)
        .nth(to)
        .map_or_else(|| said.get(start..), |end| said.get(start..end))
        .unwrap_or_default()
        .into()
}

/// What Moonshot's source is called.
const MOONSHOT: &str = "moonshot";

/// The honest client identity Kimi Code's services receive.
const MOONSHOT_AGENT: &str = concat!("crucible/", env!("CARGO_PKG_VERSION"));

/// Kimi Code's own search and fetch services.
///
/// Not a side request to a model: these are two plain endpoints that take a
/// query or an address and answer with results or a page. That makes this the
/// simplest of the three sources and the one whose answer needs the least
/// reading — the service has already pulled the text out of the page by the
/// time crucible sees it.
///
/// They belong to the Kimi Code platform rather than to the open platform, and
/// a key issued against the latter is refused by them. crucible's Moonshot arm
/// already posts to the coding host unless a setting moves it, so the ordinary
/// session reaches these; one pointed elsewhere gets no web tools rather than a
/// pair that answer every call with somebody else's refusal.
#[derive(Debug)]
pub struct MoonshotWeb {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    searching: Endpoint,
    fetching: Endpoint,
}

impl MoonshotWeb {
    /// Where Kimi Code answers a query.
    pub const SEARCH: Endpoint = Endpoint::fixed("https://api.kimi.com/coding/v1/search");

    /// Where Kimi Code answers an address.
    pub const FETCH: Endpoint = Endpoint::fixed("https://api.kimi.com/coding/v1/fetch");

    /// A source reaching Kimi Code's services with `credential`.
    #[must_use]
    pub fn new(credential: Box<dyn Credential>, transport: Box<dyn Transport>) -> Self {
        Self {
            credential,
            transport,
            searching: Self::SEARCH,
            fetching: Self::FETCH,
        }
    }

    /// The headers both services take, including the secret.
    fn headers(&self, accepting: &str) -> Result<Outgoing, SourceError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", accepting);
        outgoing.set_header("user-agent", MOONSHOT_AGENT);
        self.credential
            .authorize(&mut outgoing)
            .map_err(|problem| SourceError::Transport {
                named: MOONSHOT,
                problem: problem.to_string().into(),
            })?;
        Ok(outgoing)
    }
}

impl Search for MoonshotWeb {
    fn name(&self) -> &'static str {
        MOONSHOT
    }

    fn reaches(&self) -> Host {
        host_of(self.searching.as_str())
    }

    fn search(&self, query: &str, cancel: &Cancel) -> Result<Vec<SearchResult>, SourceError> {
        if cancel.requested() {
            return Err(SourceError::Cancelled(MOONSHOT));
        }

        let mut json = Json::new();
        json.object(|body| {
            body.text("text_query", query);
            // Match Kimi Code's own bounded defaults. Page bodies belong to the
            // fetch tool; carrying five of them through search would spend the
            // caller's result budget on duplicate content.
            body.number("limit", 5);
            body.boolean("enable_page_crawling", false);
            body.number("timeout_seconds", 30);
        });

        let answered = posted(
            Sending {
                named: MOONSHOT,
                transport: self.transport.as_ref(),
                endpoint: self.searching.as_str(),
            },
            self.headers("application/json")?,
            json.finish(),
            cancel,
        )?;

        let found = answered
            .pointer("/search_results")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .ok_or_else(|| SourceError::Protocol {
                named: MOONSHOT,
                problem: "the answer carried no search_results list".into(),
            })?;

        Ok(found
            .iter()
            .filter_map(|result| {
                let url = text_at(result, "/url")?;
                Some(SearchResult {
                    title: text_at(result, "/title").unwrap_or_else(|| url.clone()),
                    extract: text_at(result, "/snippet").unwrap_or_default(),
                    url,
                })
            })
            .collect())
    }
}

impl Fetch for MoonshotWeb {
    fn name(&self) -> &'static str {
        MOONSHOT
    }

    fn reaches(&self, url: &str) -> Host {
        host_of(url)
    }

    fn fetch(&self, url: &str, cancel: &Cancel) -> Result<Page, SourceError> {
        if matches!(Fetch::reaches(self, url), Host::Opaque(_)) {
            return Err(SourceError::Address(
                format!("{url} is not an http or https address naming a host").into(),
            ));
        }

        if cancel.requested() {
            return Err(SourceError::Cancelled(MOONSHOT));
        }

        let mut json = Json::new();
        json.object(|body| body.text("url", url));

        // The service answers with the page's text rather than a document
        // describing it, so there is nothing to read a final address out of.
        // What was asked for is what it fetched, as far as anything here can
        // tell — and the tool compares the two, so saying otherwise would make
        // every fetch look like a redirect.
        let text = posted_text(
            Sending {
                named: MOONSHOT,
                transport: self.transport.as_ref(),
                endpoint: self.fetching.as_str(),
            },
            self.headers("text/markdown")?,
            json.finish(),
            cancel,
        )?;

        Ok(Page {
            url: url.into(),
            title: None,
            text: text.into(),
        })
    }
}

impl Fetch for OpenAiWeb {
    fn name(&self) -> &'static str {
        OPENAI
    }

    fn reaches(&self, url: &str) -> Host {
        host_of(url)
    }

    fn fetch(&self, url: &str, cancel: &Cancel) -> Result<Page, SourceError> {
        let reached = Fetch::reaches(self, url);
        let Host::Named { host, .. } = &reached else {
            return Err(SourceError::Address(
                format!("{url} is not an http or https address naming a host").into(),
            ));
        };

        if cancel.requested() {
            return Err(SourceError::Cancelled(OPENAI));
        }

        let mut json = Json::new();
        json.object(|body| {
            body.text("model", &self.model);
            body.boolean("stream", true);
            openai_input(
                body,
                &format!("Open {url} and reproduce its contents as text."),
            );
            body.boolean("store", false);
            // Opening is the operation this method was called to perform, not
            // a tool the side model may decline in favour of remembered prose.
            body.text("tool_choice", "required");
            body.array("tools", |tools| {
                tools.object(|declared| {
                    declared.text("type", "web_search");
                    // Confined to the host a verdict was reached about. The
                    // tool is free to search as well as open, and a search let
                    // loose would reach hosts nobody approved — this is the
                    // vendor's own control for saying which ones it may touch.
                    declared.object("filters", |filters| {
                        filters.array("allowed_domains", |domains| domains.text(host));
                    });
                });
            });
        });

        let answered = self.ask(json.finish(), cancel)?;

        opened(&answered, url)
    }
}

/// Whether the completed response records a hosted web call.
fn web_called(answered: &Value) -> bool {
    answered
        .pointer("/output")
        .and_then(Value::as_array)
        .is_some_and(|output| {
            output
                .iter()
                .any(|item| text_at(item, "/type").as_deref() == Some("web_search_call"))
        })
}

/// The page an answer accounts for.
///
/// Refused rather than answered where the tool never ran: this vendor will
/// happily write about an address from memory, and a page that was never
/// fetched arriving as though it had been is the one failure the caller cannot
/// see. A `web_search_call` whose action is `open_page` is the evidence that it
/// went; a search for the URL is not the page.
fn opened(answered: &Value, asked: &str) -> Result<Page, SourceError> {
    let output = answered
        .pointer("/output")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    if !output.iter().any(|item| {
        text_at(item, "/type").as_deref() == Some("web_search_call")
            && text_at(item, "/action/type").as_deref() == Some("open_page")
    }) {
        return Err(SourceError::Protocol {
            named: OPENAI,
            problem: "the answer was written without opening the page".into(),
        });
    }

    let mut text = String::new();
    for item in output {
        let parts = item
            .pointer("/content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();

        for part in parts {
            if let Some(said) = text_at(part, "/text") {
                text.push_str(&said);
            }
        }
    }

    if text.is_empty() {
        return Err(SourceError::Protocol {
            named: OPENAI,
            problem: "the answer carried no page".into(),
        });
    }

    Ok(Page {
        url: asked.into(),
        title: None,
        text: text.into(),
    })
}
