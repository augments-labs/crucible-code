//! Turning a refused response into something a user can act on.
//!
//! Shared because a refusal is the one part of both protocols that is already
//! the same: a status, and a sentence under `error.message` explaining it. A
//! wrong model name and a key without access are both diagnosed from that
//! sentence, so losing it costs a user the only clue they get.

use std::io::Read;

use crucible_core::ProviderError;

/// The most of a refusal to read before giving up on it.
///
/// A refusal is a sentence. Anything larger is a proxy's error page, and
/// reading all of it to print a paragraph of HTML helps nobody.
const MAX_REFUSAL: u64 = 8 * 1024;

/// A refusal, with the sentence the provider sent.
pub(crate) fn refused(
    provider: &'static str,
    status: u16,
    body: Box<dyn Read + Send>,
) -> ProviderError {
    let mut said = Vec::new();
    let read = body.take(MAX_REFUSAL).read_to_end(&mut said);

    let message = match read {
        // Lossy on purpose: this is already the failure path, and a message
        // that is not quite text is still better than no message.
        Ok(_) => explain(&String::from_utf8_lossy(&said)),
        Err(problem) => format!("the response could not be read: {problem}"),
    };

    ProviderError::Refused {
        provider,
        status,
        message: message.into(),
    }
}

/// The sentence inside a refusal body.
///
/// Falls back to the body itself, because a proxy or a gateway in front of the
/// API refuses in its own shape and that text is still what a user needs.
fn explain(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .as_ref()
        .and_then(|payload| payload.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| body.trim().to_owned(), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(body: &str) -> Box<dyn Read + Send> {
        Box::new(std::io::Cursor::new(body.to_owned().into_bytes()))
    }

    #[test]
    fn a_refusal_carries_the_status_and_the_sentence_that_explains_it() {
        let problem = refused(
            "test",
            404,
            reading(r#"{"error":{"type":"not_found","message":"model: nope"}}"#),
        );

        assert_eq!(problem.to_string(), "test: HTTP 404: model: nope");
    }

    #[test]
    fn a_refusal_that_is_not_the_api_still_says_what_it_said() {
        let problem = refused("test", 502, reading("  upstream connect error  "));

        assert_eq!(
            problem.to_string(),
            "test: HTTP 502: upstream connect error"
        );
    }

    #[test]
    fn an_error_page_is_read_only_as_far_as_it_is_worth_reading() {
        // A gateway can answer with a whole HTML document, and all of it would
        // otherwise end up on one line in front of a user.
        let long = "x".repeat(64 * 1024);

        let shown = refused("test", 500, reading(&long)).to_string();

        assert!(
            shown.len() < 16 * 1024,
            "the whole page came back: {} bytes",
            shown.len()
        );
    }
}
