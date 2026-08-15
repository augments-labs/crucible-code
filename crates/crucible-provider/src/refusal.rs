//! Turning a refused response into something a user can act on.
//!
//! Shared because a refusal is the one part of both protocols that is already
//! the same: a status, and a sentence under `error.message` explaining it. A
//! wrong model name and a key without access are both diagnosed from that
//! sentence, so losing it costs a user the only clue they get.
//!
//! Every refusal ends the turn, and that includes the one saying the credential
//! was not accepted. Renewing a credential happens earlier than here:
//! [`authorize`] runs on the way into every request, which is where a token's
//! age can still be answered about, where no part of a response has been read
//! yet, and where failing costs no round trip. Sending again from this side
//! would repeat a request the vendor has already answered — with a refusal, but
//! answered — and would need a bound of its own so the second refusal could not
//! ask for a third.
//!
//! [`authorize`]: crucible_core::Credential::authorize

use std::io::{self, Read};
use std::time::{Duration, Instant};

use crucible_core::ProviderError;

/// What is said where a failure names no reason at all.
///
/// It says what to check rather than only that something went wrong, because
/// the reader of this sentence is holding a turn that stopped and has to decide
/// what to change. A response that fails with nothing under `error` is most
/// often one whose request asked for something this model does not serve, and
/// that is the one thing crucible can point at without being told.
///
/// One sentence for all three vendors, because it is the one part of a failure
/// that is not the vendor's: it is what crucible says when the vendor said
/// nothing.
pub(crate) const SILENT: &str = "gave up on the response and named no reason; check that this model serves what was asked of it";

/// The most of a refusal to read before giving up on it.
///
/// A refusal is a sentence. Anything larger is a proxy's error page, and
/// reading all of it to print a paragraph of HTML helps nobody.
const MAX_REFUSAL: u64 = 8 * 1024;

/// The longest reading one may take altogether.
///
/// A different failure from the one above, and the reason [`read_to_end`] is not
/// what reads this body. Bodies arrive through a reader that reports a wait that
/// expired as an interruption — the kind the `Read` contract says to retry — so
/// a gateway that answers 429 and then stalls without closing is a reader that
/// neither ends nor errors, and `read_to_end` retries it for ever. That leaves
/// the turn wedged here, which is the one place in it with no cancel to consult:
/// the runner's is not reachable from this side, because by now the turn is
/// already failing and what is being assembled is the sentence saying why.
///
/// The whole read rather than the gaps in it, which is the stronger of the two
/// bounds: a peer trickling one byte per gap satisfies every gap and still holds
/// the thread for as long as it likes.
///
/// Ten seconds because what this competes with is the user learning nothing. A
/// refusal is a sentence, and one that has not finished arriving in that long is
/// not going to.
///
/// [`read_to_end`]: Read::read_to_end
const MAX_WAIT: Duration = Duration::from_secs(10);

/// A refusal, with the sentence the provider sent.
pub(crate) fn refused(
    provider: &'static str,
    status: u16,
    body: Box<dyn Read + Send>,
) -> ProviderError {
    said(provider, status, body, MAX_WAIT)
}

/// The same, with a wait a test can hand over as none.
///
/// The wait rather than the deadline it makes, so nothing here adds to an
/// `Instant` — that addition panics where it overflows, and a bound against
/// hanging is a poor place to put a new way to fail.
fn said(
    provider: &'static str,
    status: u16,
    body: Box<dyn Read + Send>,
    wait: Duration,
) -> ProviderError {
    let mut said = Vec::new();
    let read = fill(&mut body.take(MAX_REFUSAL), &mut said, wait);

    let message = match read {
        // Lossy on purpose: this is already the failure path, and a message
        // that is not quite text is still better than no message.
        Ok(()) => explain(&String::from_utf8_lossy(&said)),
        Err(problem) => format!("the response could not be read: {problem}"),
    };

    ProviderError::Refused {
        provider,
        status,
        message: message.into(),
    }
}

/// Reads until the body ends, its bytes run out, or `wait` does.
fn fill(body: &mut dyn Read, said: &mut Vec<u8>, wait: Duration) -> io::Result<()> {
    let since = Instant::now();
    let mut into = [0_u8; 1024];

    loop {
        if since.elapsed() >= wait {
            return Err(timed_out());
        }

        let read = body.read(&mut into);
        if since.elapsed() >= wait {
            return Err(timed_out());
        }

        match read {
            Ok(0) => return Ok(()),
            Ok(read) => said.extend_from_slice(into.get(..read).unwrap_or_default()),
            Err(problem) if problem.kind() == io::ErrorKind::Interrupted => {}
            Err(problem) => return Err(problem),
        }
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "it stopped part-way through")
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
    use crate::transport::{Paused, Said};

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

    /// A gateway that answered and then stalled without closing: every read is
    /// a wait that expired, spelled the way the transport spells one.
    struct Stalled;

    impl Read for Stalled {
        fn read(&mut self, _into: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::Interrupted.into())
        }
    }

    /// A peer that stays just active enough never to report an interrupted read.
    struct Trickle;

    impl Read for Trickle {
        fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(2));
            if let Some(first) = into.first_mut() {
                *first = b'x';
                return Ok(1);
            }
            Ok(0)
        }
    }

    #[test]
    fn a_body_that_stalls_and_never_closes_gives_up_rather_than_holding_the_turn() {
        // The turn thread is inside this function with no cancel to look at, so
        // a reader that only ever says "retry" is a session that never comes
        // back and never says why. `read_to_end` is what used to read this, and
        // it retries that answer for ever.
        let problem = said("test", 429, Box::new(Stalled), Duration::ZERO);

        assert_eq!(
            problem.to_string(),
            "test: HTTP 429: the response could not be read: it stopped part-way through"
        );
    }

    #[test]
    fn a_body_that_keeps_producing_bytes_cannot_outlive_the_elapsed_deadline() {
        let problem = said("test", 429, Box::new(Trickle), Duration::from_millis(1));

        assert!(problem.to_string().contains("it stopped part-way through"));
    }

    #[test]
    fn a_refusal_that_pauses_before_the_rest_of_it_is_still_read_whole() {
        // The other half, and why the deadline is looked at only where a read
        // said to retry: a refusal that arrives in two pieces is the ordinary
        // case, and giving up on it costs the user the sentence naming what went
        // wrong. The pause here is a read that expired, which is exactly what
        // the arm above gives up on once the wait has run out.
        let body = Paused::saying([
            Said::Bytes(br#"{"error":{"message":"upstream"#.to_vec()),
            Said::Nothing,
            Said::Bytes(br#" is unwell"}}"#.to_vec()),
        ]);

        let problem = said("test", 502, Box::new(body), MAX_WAIT);

        assert_eq!(problem.to_string(), "test: HTTP 502: upstream is unwell");
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
