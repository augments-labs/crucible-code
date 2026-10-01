//! A request, in OpenAI's shape: Responses as [`super::Gpt`] writes it.
//!
//! The writer is [`crate::responses::body`]. What is OpenAI's own here is the
//! one model whose earlier turns go back as the items it answered with:
//! [`effort`] and [`replay`] write those, and [`replayed`] is where the shared
//! writer hands such a request over. The rest is for OpenAI's tests of what a
//! request carries, which stay where they were and reach the shared writer
//! through OpenAI's dialect.

mod effort;
mod replay;

use crucible_models::{Attached, ProviderError, Request};
use crucible_types::{ContinuationScope, Message};
#[cfg(test)]
use crucible_types::{
    PromptCacheEncoding, PromptCacheIneligibleReason, PromptCacheMechanism,
    PromptCacheRetentionClass, StopReason, ToolCall, ToolResult, ToolSchema,
};
#[cfg(test)]
use serde_json::Value;

use super::Gpt;
#[cfg(test)]
use super::Serving;
use crate::json::{Array, Object};

/// How hard to think and the input, for the model whose turns go back as the
/// items it answered with. `explicit` is the message an explicit cache
/// breakpoint marks.
pub(super) fn replayed(
    body: &mut Object<'_>,
    request: &Request<'_>,
    scope: ContinuationScope,
    explicit: Option<usize>,
) -> Result<(), ProviderError> {
    let mut efforts = effort::Efforts::new(request, scope)?;
    if let Some(effort) = efforts
        .as_ref()
        .map_or(request.effort, effort::Efforts::initial)
    {
        body.object("reasoning", |reasoning| {
            reasoning.text("effort", effort.as_str());
        });
    }
    let mut outcome = Ok(());
    body.array("input", |input| {
        outcome = replay::write(input, request, scope, explicit, efforts.as_mut());
    });
    outcome
}

/// One message, as however many items this wire needs, in OpenAI's dialect.
fn append(
    items: &mut Array<'_>,
    message: &Message,
    nth: usize,
    attached: &[Attached<'_>],
    breakpoint: bool,
) {
    crate::responses::body::append::<Gpt>(items, message, nth, attached, breakpoint);
}

/// The whole request body at standard speed, as `serving` accepts it, and as
/// the tests written before speed was asked for build it.
#[cfg(test)]
pub(super) fn serialize(
    request: &Request<'_>,
    serving: Serving,
    scope: Option<ContinuationScope>,
) -> Result<String, ProviderError> {
    crate::responses::body::serialize::<Gpt>(request, serving, scope, false)
}

/// The cache metadata the body adds for this exact request.
#[cfg(test)]
pub(super) fn prompt_cache_encoding(
    request: &Request<'_>,
    serving: Serving,
) -> PromptCacheEncoding {
    crate::responses::body::prompt_cache_encoding::<Gpt>(request, serving)
}

/// The body the published API receives, which is the whole of it.
#[cfg(test)]
fn build(request: &Request<'_>) -> Value {
    served(request, Serving::Api)
}

/// The body one of the two services receives.
#[cfg(test)]
fn served(request: &Request<'_>, serving: Serving) -> Value {
    serde_json::from_str(&serialize(request, serving, None).unwrap()).expect("request body is JSON")
}

#[cfg(test)]
mod tests;
