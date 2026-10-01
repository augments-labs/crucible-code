//! A response, from the Chat Completions shape.
//!
//! One event in, however many deltas out. Unlike the newer protocol this
//! endpoint narrates almost nothing: nearly every event is a piece of the
//! answer, and one of them can carry text, a tool call and the reason the model
//! stopped at once. The exception is the last one, which carries what the
//! response cost and no piece of the answer at all.
//!
//! Read by field lookup rather than into mirror structs. The payload is
//! consumed once, here, and a struct per shape would be more code to say the
//! same thing while still needing a fallback for what it does not know.
//!
//! Events are unnamed (the wire's first vendor sends no SSE `event:` line), so
//! what an event is is decided by what its payload holds rather than by a word
//! beside it.
//!
//! What the counts in the last event mean is the one thing vendors on this
//! wire disagree about, so the dialect reads them.
//!
//! A model that thinks first sends its reasoning beside the answer. Most
//! vendors want none of it back; a vendor whose dialect asks for it has it kept
//! here, bounded, as the answer's private continuation, which the request
//! writer sends back on the message it came with.

use std::fmt;
use std::marker::PhantomData;

use crucible_models::{Delta, ProviderError, Request};
use crucible_types::{
    CONTINUATION_BYTES, Continuation, ContinuationData, ContinuationPart, ContinuationScope,
    StopReason, ToolId,
};
use serde_json::Value;

use super::Dialect;
use crate::refusal::SILENT;
use crate::sse::SseEvent;
use crate::stream::Wire;

/// What closes a stream on this endpoint.
///
/// Not JSON, and the last thing every response sends. Parsed as a payload it
/// fails every turn this provider ever runs, at the moment a complete answer
/// has just finished arriving.
const DONE: &str = "[DONE]";

/// What reasoning kept by this wire is recorded as, beside the answer it came
/// with. Written into session logs, so it never changes once shipped.
pub(crate) const PROTOCOL: &str = "chat-completions-reasoning-v1";

/// Chat Completions, being narrated, in `D`'s dialect.
pub(crate) struct Completions<D: Dialect> {
    open: Open,
    kept: D::Kept,
    dialect: PhantomData<D>,
}

impl<D: Dialect> Default for Completions<D> {
    fn default() -> Self {
        Self {
            open: Open::default(),
            kept: D::Kept::default(),
            dialect: PhantomData,
        }
    }
}

impl<D: Dialect> Completions<D> {
    /// The reader for one response to `request`, keeping its reasoning under
    /// `scope`, where there is one.
    pub(crate) fn for_request(request: &Request<'_>, scope: Option<ContinuationScope>) -> Self {
        Self {
            kept: scope.map_or_else(D::Kept::default, |scope| {
                D::Kept::begin(request.model, scope)
            }),
            ..Self::default()
        }
    }
}

impl<D: Dialect> fmt::Debug for Completions<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completions")
            .field("provider", &D::NAME)
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}

impl<D: Dialect> Wire for Completions<D> {
    const PROVIDER: &'static str = D::NAME;

    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        let mut deltas = deltas::<D>(event, &mut self.open, &mut self.kept)?;
        if self.kept.keeping() {
            for delta in &deltas {
                self.kept.note(delta);
            }
            // Handed over just before the stop, which is the one point the
            // whole answer it refers to is known.
            if let Some(at) = deltas
                .iter()
                .position(|delta| matches!(delta, Delta::Stopped(_)))
                && let Some(kept) = self.kept.take()
            {
                deltas.insert(at, Delta::Continuation(kept));
            }
        }
        Ok(deltas)
    }
}

/// What a reader keeps of the reasoning a response writes, for a vendor that
/// wants it back. `()` keeps nothing and takes no room, which is what a
/// vendor that wants none of it is read with.
pub trait Keeps: Default + Send + 'static {
    /// Ready to keep `model`'s reasoning under `scope`.
    fn begin(model: &str, scope: ContinuationScope) -> Self;
    /// Whether anything is being kept.
    fn keeping(&self) -> bool;
    /// One more piece of reasoning.
    fn more(&mut self, piece: &str);
    /// One more piece of the answer the reasoning belongs beside.
    fn note(&mut self, delta: &Delta);
    /// The reasoning, as the continuation of the answer it came with.
    fn take(&mut self) -> Option<Continuation>;
}

impl Keeps for () {
    fn begin(_model: &str, _scope: ContinuationScope) -> Self {}
    fn keeping(&self) -> bool {
        false
    }
    fn more(&mut self, _piece: &str) {}
    fn note(&mut self, _delta: &Delta) {}
    fn take(&mut self) -> Option<Continuation> {
        None
    }
}

/// The reasoning one response has written so far, and enough of its answer to
/// say which text and calls it belongs beside.
#[derive(Default)]
pub struct Thought(Option<Box<Thinking>>);

/// By hand: the reasoning is the model's private working, and only how much
/// of it is kept is shown.
impl fmt::Debug for Thought {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thought")
            .field(
                "kept",
                &self.0.as_ref().map(|thinking| thinking.reasoning.len()),
            )
            .finish_non_exhaustive()
    }
}

struct Thinking {
    model: Box<str>,
    scope: ContinuationScope,
    reasoning: String,
    /// Whether all of it fitted. Reasoning past the bound is not kept at all:
    /// part of it sent back would be the vendor reading words its model never
    /// finished, which is worse than reading none.
    whole: bool,
    said: usize,
    calls: usize,
}

impl Keeps for Thought {
    fn begin(model: &str, scope: ContinuationScope) -> Self {
        Self(Some(Box::new(Thinking {
            model: model.into(),
            scope,
            reasoning: String::new(),
            whole: true,
            said: 0,
            calls: 0,
        })))
    }

    fn keeping(&self) -> bool {
        self.0.is_some()
    }

    fn more(&mut self, piece: &str) {
        if let Some(thinking) = &mut self.0 {
            thinking.more(piece);
        }
    }

    fn note(&mut self, delta: &Delta) {
        if let Some(thinking) = &mut self.0 {
            match delta {
                Delta::Text(said) => thinking.said += said.len(),
                Delta::ToolStarted { .. } => thinking.calls += 1,
                _ => {}
            }
        }
    }

    fn take(&mut self) -> Option<Continuation> {
        self.0.as_mut()?.continuation()
    }
}

impl Thinking {
    /// One more piece of reasoning, kept while the whole still fits.
    fn more(&mut self, piece: &str) {
        if !self.whole {
            return;
        }
        if self.reasoning.len().saturating_add(piece.len()) > CONTINUATION_BYTES {
            self.whole = false;
            self.reasoning = String::new();
            return;
        }
        self.reasoning.push_str(piece);
    }

    /// The reasoning, as the continuation of the answer it came with, or
    /// nothing where there was none or it did not fit.
    fn continuation(&mut self) -> Option<Continuation> {
        if !self.whole || self.reasoning.is_empty() {
            return None;
        }
        let reasoning = std::mem::take(&mut self.reasoning);
        let mut state = Continuation::new(PROTOCOL, &self.model, self.scope).ok()?;
        state
            .push(ContinuationPart::Opaque(
                ContinuationData::new(&reasoning).ok()?,
            ))
            .ok()?;
        if self.said > 0 {
            state
                .push(ContinuationPart::Text {
                    start: 0,
                    end: self.said,
                    data: ContinuationData::new("").ok()?,
                })
                .ok()?;
        }
        for index in 0..self.calls {
            state
                .push(ContinuationPart::Call {
                    index,
                    data: ContinuationData::new("").ok()?,
                })
                .ok()?;
        }
        Some(state)
    }
}

/// The tool call the response has open.
///
/// Carried between events because a call is named in the event that opens it
/// and its arguments arrive in the ones after, which carry the index alone. The
/// index is the whole of the identity a fragment has, so this is what a
/// fragment is checked against.
#[derive(Debug, Default)]
struct Open {
    /// The index a fragment belongs to, or `None` where no call is open.
    call: Option<u64>,
}

/// What an event means, or nothing if it means nothing to us.
///
/// # Errors
///
/// [`ProviderError::Upstream`] when the event is the provider reporting a
/// failure inside a response it had already started, and
/// [`ProviderError::Protocol`] when an event does not parse, announces a tool
/// call by part of its identity, or contradicts what is open.
fn deltas<D: Dialect>(
    event: &SseEvent,
    open: &mut Open,
    kept: &mut D::Kept,
) -> Result<Vec<Delta>, ProviderError> {
    // A heartbeat, which a proxy may send with no data line at all, and the
    // sentinel above. Neither is JSON and neither means anything here.
    let data = event.data.trim();
    if data.is_empty() || data == DONE {
        return Ok(Vec::new());
    }

    let payload = parse::<D>(data)?;

    // A failure inside a response already started. It arrives in place of the
    // choices rather than beside them.
    if let Some(error) = payload.get("error").filter(|error| !error.is_null()) {
        return Err(upstream::<D>(error));
    }
    if let Some(failure) = D::failure(&payload) {
        return Err(failure);
    }

    let mut deltas = Vec::new();

    // Read before the choices rather than after them, because the chunk that
    // carries this has none: on this endpoint the counts arrive on their own,
    // after the answer and after the reason the model stopped.
    if let Some(usage) = D::usage(&payload)? {
        deltas.push(usage);
    }

    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let Some(choice) = choice else {
        // The usage chunk above, and whatever else this endpoint sends without
        // a choice in it.
        return Ok(deltas);
    };

    if let Some(delta) = choice.get("delta") {
        // `reasoning_content` sits beside this on a model that thinks first.
        // Nothing displays it, and reading it as the answer would put the
        // model's working in front of the user as though it were one. It is
        // kept only where the vendor wants it back.
        if let Some(piece) = text(delta, "reasoning_content") {
            kept.more(piece);
        }
        if let Some(said) = text(delta, "content").filter(|said| !said.is_empty()) {
            deltas.push(Delta::Text(said.into()));
        }

        calls::<D>(delta, open, &mut deltas)?;
    }

    if let Some(reason) = text(choice, "finish_reason") {
        deltas.push(Delta::Stopped(
            D::stopped(reason).unwrap_or_else(|| stop(reason)),
        ));
    }

    Ok(deltas)
}

/// The tool calls one event carries, opened or continued.
///
/// One event can hold several, which is why this appends rather than returns:
/// a call finishing and the next one opening arrive together.
fn calls<D: Dialect>(
    delta: &Value,
    open: &mut Open,
    deltas: &mut Vec<Delta>,
) -> Result<(), ProviderError> {
    let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else {
        return Ok(());
    };

    for call in calls {
        // The only identity a fragment has. Without it there is nothing to
        // decide which call the arguments below belong to, and guessing at the
        // one in hand is one tool running on another tool's arguments.
        let Some(index) = call.get("index").and_then(Value::as_u64) else {
            return Err(ProviderError::Protocol {
                provider: D::NAME,
                problem: "a tool call arrived without the index its fragments are keyed by".into(),
            });
        };

        let function = call.get("function");
        let name = function
            .and_then(|function| text(function, "name"))
            .filter(|name| !name.is_empty());

        // A name is what opens a call: it is sent once, with the identity a
        // result is answered against, and never again. Everything after it is a
        // fragment of the call that name opened.
        if let Some(name) = name {
            let Some(id) = text(call, "id").filter(|id| !id.is_empty()) else {
                // Skipped instead, nothing opens and the fragments that follow
                // are assembled onto the call before it, while the call that
                // was half announced leaves no trace, so the turn ends looking
                // like a clean finish with a tool the model asked for never run.
                return Err(ProviderError::Protocol {
                    provider: D::NAME,
                    problem: "a tool call was announced without the identity its result is \
                              answered against"
                        .into(),
                });
            };

            open.call = Some(index);
            deltas.push(Delta::ToolStarted {
                id: ToolId::new(id),
                name: name.into(),
            });
        } else if open.call != Some(index) {
            return Err(ProviderError::Protocol {
                provider: D::NAME,
                problem: "arguments arrived for a tool call other than the one open".into(),
            });
        }

        if let Some(arguments) = function
            .and_then(|function| text(function, "arguments"))
            .filter(|arguments| !arguments.is_empty())
        {
            deltas.push(Delta::ToolArgs(arguments.into()));
        }
    }

    Ok(())
}

/// Why the model stopped.
///
/// A word this build has not heard of reads as unfinished rather than as a
/// finish. Wrong that way it is wrong about a turn that was fine; wrong the
/// other way it is an answer that was cut short arriving looking complete,
/// which is the one failure the user cannot see for themselves.
fn stop(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::Yielded,
        "tool_calls" | "function_call" => StopReason::WantsTools,
        "length" | "max_tokens" => StopReason::OutOfTokens,
        "content_filter" => StopReason::Filtered,
        _ => StopReason::Unknown,
    }
}

/// A failure the provider reported mid-response.
fn upstream<D: Dialect>(error: &Value) -> ProviderError {
    ProviderError::Upstream {
        provider: D::NAME,
        kind: text(error, "type")
            .or_else(|| text(error, "code"))
            .unwrap_or("error")
            .into(),
        message: text(error, "message").unwrap_or(SILENT).into(),
    }
}

/// One string field.
fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

/// The payload as JSON.
fn parse<D: Dialect>(data: &str) -> Result<Value, ProviderError> {
    serde_json::from_str(data).map_err(|problem| ProviderError::Protocol {
        provider: D::NAME,
        // The payload itself is not carried: it is up to a whole event long and
        // this message ends up in front of a user.
        problem: format!("an event was not JSON: {problem}").into(),
    })
}
