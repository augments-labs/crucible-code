//! The Meta provider: Muse Spark, as a dialect of Responses.
//!
//! The wire is [`crate::responses`], which writes the request, reads the
//! response and sends one for the other, and `Responses<Muse>` is what ships.
//! What is Meta's is here: its one address, and the places its Responses
//! differs from the plain wire. What the model said before calling a tool goes
//! back marked as commentary, which the vendor answers with a 400 when it is
//! not, and a stream may close with a `[DONE]` line after the event that ended
//! it. And its refusal of a request too large for the model carries no code,
//! so it is told apart by its shape and its words rather than left to end the
//! turn.
//!
//! No `tool_choice` is ever sent: the vendor takes `auto` alone, which is what
//! a request that names none gets. No reasoning is sent back either: the vendor
//! drops what it is not given and refuses nothing for it.
//!
//! It names no HTTP client and no credential kind. A [`crate::Transport`] is
//! handed in and so is a [`crucible_credentials::Credential`], which is what
//! lets the whole protocol be tested against recorded bytes.

use crucible_models::{PromptCacheCapabilities, PromptCacheProvenance};
use crucible_types::{Modalities, Modality};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::responses::{Dialect, Plain, Responses};

/// What this provider is called, in errors and in the status line.
const NAME: &str = "meta";

/// Where a key from Meta's developer platform is served.
const VENDOR: Endpoint = Endpoint::fixed("https://api.meta.ai/v1/responses");

/// Meta's dialect of Responses.
#[derive(Debug)]
pub struct Muse;

impl Dialect for Muse {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "Meta";
    const ADDRESSES: &'static [Endpoint] = &[VENDOR];
    const PROTOCOL: &'static str = "meta-responses";
    const SHAPE: &'static str = "meta-responses-v1";

    // Replayed as an ordinary answer, text in front of a call is refused with
    // a 400 rather than read as commentary.
    const COMMENTARY: bool = true;

    // Whether a stream closes with this line is said only by the vendor's own
    // sample code, which stops on it; it is let pass rather than read.
    const SENTINEL: bool = true;

    type Route = ();
    type Replay = Plain;

    // A request too large for the model is refused with a 400 whose `code` is
    // null, as Meta's error page prints it, beside a sentence giving the
    // counts. With no code to read, it is told by the shape that refusal has
    // and the words in it: see [`outgrew`].
    const OVERLONG: Option<fn(u16, &Value) -> bool> = Some(outgrew);

    fn route(endpoint: &Endpoint) {
        let _ = endpoint;
    }

    fn prompt_cache(_route: (), model: &str) -> PromptCacheCapabilities {
        let revision = match model {
            "muse-spark-1.3" => "muse-spark-1.3",
            "muse-spark-1.3-contributor" => "muse-spark-1.3-contributor",
            "muse-spark-1.2" => "muse-spark-1.2",
            "muse-spark-1.2-contributor" => "muse-spark-1.2-contributor",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        crate::responses::automatic(
            revision,
            PromptCacheProvenance::new(
                "https://dev.meta.ai/docs/prompt-caching",
                "2026-10-01",
                "meta-prompt-cache-2026-10-01",
            ),
            // No smallest cached prefix is published; this assumes no hit
            // below the floor the wire's other vendors share.
            1_024,
        )
    }

    fn spells() -> Modalities {
        // No part for a picture or a file is documented on this route yet, so
        // text alone until one is.
        Modalities::empty().insert(Modality::Text)
    }
}

/// The words of Meta's refusal of a request too large for the model, as its
/// error page prints them, and of none of its other documented refusals.
const OUTGROWN: &str = "the model's context length is only";

/// Whether a refusal is Meta's of a request too large for the model.
///
/// Meta sends no code for it, so the shape is read first and the words only
/// then: a 400 of the type every refused request has, naming no parameter and
/// no code, whose message says, exactly and in this case, [`OUTGROWN`]. Every
/// other refusal its page prints names the parameter it is about, and stays a
/// refusal in Meta's words.
fn outgrew(status: u16, body: &Value) -> bool {
    let Some(error) = body.get("error") else {
        return false;
    };
    status == 400
        && error.get("type").and_then(Value::as_str) == Some("invalid_request_error")
        && error.get("param").is_some_and(Value::is_null)
        && error.get("code").is_some_and(Value::is_null)
        && error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|said| said.contains(OUTGROWN))
}

/// Meta's Responses API.
pub type Meta = Responses<Muse>;

impl Responses<Muse> {
    /// The address this API is served at, for a caller with no reason to send
    /// anywhere else.
    pub const VENDOR: Endpoint = VENDOR;
}

#[cfg(test)]
mod tests;
