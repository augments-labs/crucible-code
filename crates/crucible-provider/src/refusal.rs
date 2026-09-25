//! Turning a refused response into something a user can act on.
//!
//! Shared because a refusal is the one part of both protocols that is already
//! the same: a status, and a sentence explaining it. A wrong model name and a
//! key without access are both diagnosed from that sentence, so losing it costs
//! a user the only clue they get.
//!
//! Where the sentence sits is the part that is not quite shared. The vendors'
//! published APIs put it under `error.message`; the backend a `ChatGPT` plan is
//! served by answers a bare `detail`. Both are read here rather than in either
//! provider, because which of the two shapes arrives is a fact about the
//! service that answered rather than about the protocol it speaks.
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
//! [`authorize`]: crucible_credentials::Credential::authorize

use std::borrow::Cow;
use std::io;
#[cfg(test)]
use std::io::Read;
use std::time::{Duration, Instant};

use crucible_credentials::Redactions;
use crucible_models::ProviderError;
use crucible_runtime::Cancel;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::transport::PostResponse;

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
///
/// One byte past it is read and never kept, the way the web sources read the
/// bodies they bound: a body that ends here and one that was cut here are
/// otherwise the same bytes. Telling them apart is what lets the message say
/// it is only the beginning of the reply, and what says the last bytes kept
/// may be the beginning of a credential rather than the end of a sentence.
///
/// That byte settles it where the reading then ends, and so does a body that
/// ends before it. Where the reading fails once the bound is full, the reply
/// is reported as cut without a length, whether or not that byte had arrived
/// — see [`STOPPED`].
const MAX_REFUSAL: u64 = 8 * 1024;

/// What a reply ends with when the byte past [`MAX_REFUSAL`] arrived and the
/// reading then ended.
///
/// A user diagnoses a stopped turn from this sentence, and one that stops
/// mid-word reads as the whole of what the service said. Saying the rest was
/// never read costs a clause and is the difference between an answer that is
/// puzzling and one that is bounded.
///
/// It names a length because a length was measured: the byte past the bound
/// arrived, so there is more of this reply than is shown. Where the bound
/// filled and the reading then failed instead of ending, the message ends in
/// [`STOPPED`], whether or not that byte came.
///
/// It goes after the sentence rather than in front of it, because what the
/// service said is what the reader came for.
///
/// Only a caller that passes the service's own words on shows this or the
/// other. Google answers every refusal with a sentence of its own instead, and
/// Anthropic and OpenAI do the same for the one model each whose replies are
/// private, so for those a clause is appended to a message that is then
/// replaced. The half of this that protects a credential is unaffected either
/// way: those three show no part of a body at all.
const LONGER: &str = " [cut: the reply was longer than crucible reads]";

/// What a reply that filled the bound and then did not end ends with.
///
/// Usually the byte past the bound never came, and then a reply that stopped
/// exactly there and one the bound cut are the same bytes from here. This
/// says what is true whether or not that byte came — crucible read no
/// further — where [`LONGER`] would most often name a length nobody
/// measured, about a reply that may have been whole.
///
/// It is still a cut, which is the harmless way to be wrong where the reply
/// was whole: a reader looks for a rest that is not there, instead of reading
/// a fragment as the whole. The end it marks is crucible's own either way, so
/// what the guard in [`kept`] protects across a cut it protects across this.
const STOPPED: &str = " [cut: crucible stopped reading here]";

/// The longest reading one may take altogether.
///
/// A different failure from the one above, and the reason [`read_to_end`] is not
/// what reads this body. Bodies arrive through a reader that reports a wait that
/// expired as an interruption — the kind the `Read` contract says to retry — so
/// a gateway that answers 429 and then stalls without closing is a reader that
/// neither ends nor errors, and `read_to_end` retries it for ever. That leaves
/// the turn wedged here. The caller's cancel remains reachable while the
/// refusal is read, so a user need not wait for this deadline when they have
/// already left the turn.
///
/// The whole read rather than the gaps in it, which is the stronger of the two
/// bounds: a peer trickling one byte per gap satisfies every gap and still holds
/// the thread for as long as it likes.
///
/// Ten seconds because what this competes with is the user learning nothing. A
/// refusal is a sentence, and one that has not finished arriving in that long is
/// not going to.
///
/// Ten seconds per attempt rather than per turn, and a peer can make a user meet
/// it more than once. A refusal whose status says the service is busy or was
/// itself kept waiting is transient, so the runner sends the request again — the
/// shipped policy allows two further attempts — and a turn makes a request of
/// its own for every tool pass. A gateway that answers 429, hands over the whole
/// bound and then holds the connection open costs this wait on each of them, on
/// the caller's runtime. The cancel is looked at before and after
/// every read, so Esc ends all of it at once; what a turn may spend altogether
/// is bounded at the turn, and is not this constant's to shorten.
///
/// [`read_to_end`]: std::io::Read::read_to_end
const MAX_WAIT: Duration = Duration::from_secs(10);

/// A refusal, with the sentence the provider sent.
pub(crate) async fn refused(
    provider: &'static str,
    status: u16,
    body: PostResponse,
    redactions: &Redactions,
    cancel: &Cancel,
) -> ProviderError {
    said_async(
        Refusal {
            provider,
            status,
            redactions,
            cancel,
        },
        body,
        MAX_WAIT,
    )
    .await
}

/// The request facts needed while its refused body is read.
#[derive(Clone, Copy)]
struct Refusal<'a> {
    provider: &'static str,
    status: u16,
    redactions: &'a Redactions,
    cancel: &'a Cancel,
}

/// The synchronous reader seam retained for recorded transport tests.
#[cfg(test)]
fn said(refusal: Refusal<'_>, body: Box<dyn Read + Send>, wait: Duration) -> ProviderError {
    let mut said = Vec::new();
    let read = fill(
        &mut body.take(MAX_REFUSAL.saturating_add(1)),
        &mut said,
        wait,
        refusal.cancel,
    );
    resolved(refusal, said, read)
}

/// Reads a live post body on the caller's runtime.
async fn said_async(refusal: Refusal<'_>, body: PostResponse, wait: Duration) -> ProviderError {
    let mut said = Vec::new();
    let mut body = body.into_reader().take(MAX_REFUSAL.saturating_add(1));
    let read = fill_async(&mut body, &mut said, wait, refusal.cancel).await;
    resolved(refusal, said, read)
}

/// Interprets the bounded read identically for live and recorded bodies.
fn resolved(refusal: Refusal<'_>, mut said: Vec<u8>, read: Result<(), ReadError>) -> ProviderError {
    let most = usize::try_from(MAX_REFUSAL).unwrap_or(usize::MAX);
    let longer = said.len() > most;

    // The byte past the bound is one a peer that has sent its whole reply and
    // not yet said so never hands over, and waiting for it is the deadline
    // above. Once the bound is full that byte is one this function would
    // discard anyway, so a read that fails after it is still answered from:
    // what is in hand is already everything that would have been kept, and
    // giving it up would cost a user the reply for the want of a byte nobody
    // wanted.
    //
    // What such a read usually cannot say is whether there was more. The
    // bound filled and the reading did not end cleanly — a stall that reached
    // the deadline, a connection that broke, a body that ended without saying
    // so — and where the byte past the bound never came, a reply that stopped
    // at the bound looks the same from here as one the bound cut. Every such
    // read is reported as cut, because a reader told something may be missing
    // when nothing is loses less than one told nothing is missing when
    // something is — but as `End::Stopped`, whether or not that byte came,
    // which says where crucible stopped rather than claiming a length only
    // the arm above is sure of. The cost is that the guard below then runs
    // over text that may be whole: a complete reply whose last characters
    // happen to begin a protected value loses them.
    let filled = said.len() >= most;
    said.truncate(most);

    let problem = match read {
        Ok(()) if longer => kept(refusal, &said, End::Longer),
        Ok(()) => kept(refusal, &said, End::Whole),
        Err(ReadError::Cancelled) => ProviderError::Cancelled(refusal.provider),
        Err(ReadError::Body(_)) if filled => kept(refusal, &said, End::Stopped),
        Err(ReadError::Body(problem)) => ProviderError::Refused {
            provider: refusal.provider,
            status: refusal.status,
            message: format!("the response could not be read: {problem}").into(),
        },
    };

    problem.redacted(refusal.redactions)
}

/// How the bytes in hand ended, which is what the message may say about them.
///
/// Three states rather than one flag, because the two that end in a clause are
/// known differently and the sentence a user reads is the difference. One was
/// measured: the byte past the bound arrived and the reading ended. The other
/// need not have been: the bound filled and the reading then failed, whether
/// or not that byte came, and where it never came the reply may have been
/// whole. A flag made the second borrow the first's sentence, so a connection
/// reset after eight kibibytes told a user the reply was longer than crucible
/// reads when nobody had counted.
#[derive(Clone, Copy)]
enum End {
    /// The body ended inside the bound: what was kept is all of it.
    Whole,

    /// More of the reply arrived than the bound keeps, and the reading ended.
    Longer,

    /// The bound filled and the reading did not end.
    Stopped,
}

impl End {
    /// What the message ends with, and nothing where the reply was whole.
    ///
    /// A clause is also what says the end of the body is crucible's boundary,
    /// whether or not it is the service's too, which is what [`kept`] guards
    /// and tidies that end for.
    const fn clause(self) -> Option<&'static str> {
        match self {
            Self::Whole => None,
            Self::Longer => Some(LONGER),
            Self::Stopped => Some(STOPPED),
        }
    }
}

/// The refusal that what was read makes, whether it is the whole reply or the
/// beginning of one.
fn kept(refusal: Refusal<'_>, said: &[u8], end: End) -> ProviderError {
    let clause = end.clause();

    // A cut ends the body wherever the bytes ran out, which can be the middle
    // of a credential the gateway echoed back. The filter at the end of `said`
    // matches whole values and would leave the first half of that one on the
    // screen, so what was kept is filtered here instead, while it is still
    // known that its end may be a cut rather than the end of a sentence.
    //
    // The bytes rather than text made from them, because the peer picks what
    // the end of a lossy conversion looks like: bytes that spell no character
    // become stand-in ones, as many as the peer sends, and a search for a
    // credential's beginning that has to reach behind them is a search the
    // peer can blind. `redact_cut` converts what it keeps.
    //
    // Lossy either way, on purpose: this is already the failure path, and a
    // message that is not quite text is still better than no message.
    let body = if clause.is_some() {
        let mut text = refusal.redactions.redact_cut(said);

        // A cut lands on a byte and a character can be several, so the last
        // one kept can be half of one, and what the conversion leaves for it
        // says only where crucible stopped reading. The clause below says that
        // in words, so every stand-in at that end goes — afterwards, never
        // instead: the guard has answered by now, and a peer can no longer
        // move the end it searches by choosing the bytes there.
        //
        // Every one rather than the one the cut split, because how many the
        // cut leaves is the peer's to choose: two bytes that spell nothing
        // leave two, and removing a fixed count would leave a stray on the
        // line. One the service itself sent goes with them — the conversion
        // spells both the same character — and that costs a reader nothing:
        // a stand-in spells no word, whoever sent it.
        text.truncate(text.trim_end_matches(char::REPLACEMENT_CHARACTER).len());
        Cow::Owned(text)
    } else {
        // Where the body ended, the end is the service's own, and its last
        // character stays whatever the service made it.
        String::from_utf8_lossy(said)
    };

    // A request that did not fit is a refusal with a remedy no other refusal
    // has, so it is told apart before the rest are given their sentence.
    // Decided from the vendor's own code rather than from the prose beside it:
    // a code is a value the vendor enumerates, and the prose is a sentence they
    // rewrite whenever they like.
    if outgrew(&body) {
        return ProviderError::WindowExceeded {
            provider: refusal.provider,
        };
    }

    // What the guard above protected is the end of the body, and this lifts a
    // sentence out of the middle of it. A body that was whole JSON up to the
    // cut has its closing bytes protected and its sentence untouched, so the
    // beginning of a credential the cut left anywhere but at the very end
    // still reaches the message. Whole values are gone from either.
    let mut message = explain(&body);
    if let Some(clause) = clause {
        message.push_str(clause);
    }
    ProviderError::Refused {
        provider: refusal.provider,
        status: refusal.status,
        message: message.into(),
    }
}

/// Every code a vendor uses to say the request did not fit its model's window.
///
/// Read off the published APIs as they stood when this was written, the way the
/// stop reasons beside them are, and they go stale the same way. A code that has
/// been renamed costs a compaction that would have recovered the turn — the
/// refusal still reaches the user, saying what the vendor said.
///
/// One list rather than one per provider because a code is not ambiguous: no
/// vendor uses another's code to mean something else, and a wire that never
/// sends any of these is unaffected by all of them.
const OUTGREW: &[&str] = &[
    "context_length_exceeded",
    "string_above_max_length",
    "invalid_request_too_large",
];

/// Whether this refusal is the model saying the request was too large for it.
///
/// The **code**, never the sentence. Matching prose would be reading three
/// vendors' phrasing, in whatever language they answered in, and getting it
/// wrong in the direction that compacts a session for a refusal about something
/// else entirely.
fn outgrew(body: &str) -> bool {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };

    payload
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|code| OUTGREW.contains(&code))
}

/// Why reading the bounded refusal stopped.
#[derive(Debug, thiserror::Error)]
enum ReadError {
    /// The turn was cancelled while its refusal was still arriving.
    #[error("the turn was cancelled")]
    Cancelled,

    /// The response body itself failed.
    #[error("{0}")]
    Body(#[from] io::Error),
}

/// Reads until the body ends, its bytes run out, or `wait` does.
///
/// The wait is checked once per pass, before a read is attempted, never
/// after one returns: what a read has already handed over is kept, and a
/// clean end it reports is honoured, whatever the clock reads by then. The
/// wait's own job is only to stop a *further* attempt — a stall or a
/// trickle that has yet to say anything more — from holding the turn; it
/// has nothing to take back from an attempt that already answered.
#[cfg(test)]
fn fill(
    body: &mut dyn Read,
    said: &mut Vec<u8>,
    wait: Duration,
    cancel: &Cancel,
) -> Result<(), ReadError> {
    let since = Instant::now();
    let mut into = [0_u8; 1024];

    loop {
        if cancel.requested() {
            return Err(ReadError::Cancelled);
        }
        if since.elapsed() >= wait {
            return Err(timed_out().into());
        }

        let read = body.read(&mut into);
        // The cancel is looked at again here, and it still wins: an Esc that
        // lands the instant this read returns drops whatever it just handed
        // over. That costs nothing, because a cancelled turn is reported as
        // `ProviderError::Cancelled` and never shows a byte of this body —
        // the bytes below are worth keeping only where the wait, not a
        // cancel, is what ends the reading.
        if cancel.requested() {
            return Err(ReadError::Cancelled);
        }

        match read {
            Ok(0) => return Ok(()),
            Ok(read) => said.extend_from_slice(into.get(..read).unwrap_or_default()),
            Err(problem) if problem.kind() == io::ErrorKind::Interrupted => {}
            Err(problem) => return Err(problem.into()),
        }
    }
}

/// The asynchronous counterpart of `fill`, with the same whole-read and
/// cancellation rules.
async fn fill_async(
    body: &mut (impl AsyncRead + Unpin + ?Sized),
    said: &mut Vec<u8>,
    wait: Duration,
    cancel: &Cancel,
) -> Result<(), ReadError> {
    let since = Instant::now();
    let mut into = [0_u8; 1024];

    loop {
        if cancel.requested() {
            return Err(ReadError::Cancelled);
        }
        if since.elapsed() >= wait {
            return Err(timed_out().into());
        }

        let read = body.read(&mut into).await;
        if cancel.requested() {
            return Err(ReadError::Cancelled);
        }

        match read {
            Ok(0) => return Ok(()),
            Ok(read) => said.extend_from_slice(into.get(..read).unwrap_or_default()),
            Err(problem) if problem.kind() == io::ErrorKind::Interrupted => {}
            Err(problem) => return Err(problem.into()),
        }
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "it stopped part-way through")
}

/// The sentence inside a refusal body.
///
/// Falls back to the body itself, because a proxy or a gateway in front of the
/// API refuses in its own shape and that text is still what a user needs. What
/// that fallback costs is a line of JSON where a sentence belongs, which is why
/// the shapes actually in front of a user are read rather than left to it.
fn explain(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .as_ref()
        .and_then(sentence)
        .map_or_else(|| body.trim().to_owned(), ToOwned::to_owned)
}

/// The reason a refusal states, in whichever of the two shapes it arrived in.
fn sentence(payload: &serde_json::Value) -> Option<&str> {
    payload
        .get("error")
        .and_then(|error| error.get("message"))
        .or_else(|| payload.get("detail"))
        .and_then(serde_json::Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Paused, Said};

    fn refused_from_read(
        provider: &'static str,
        status: u16,
        body: Box<dyn Read + Send>,
        redactions: &Redactions,
        cancel: &Cancel,
    ) -> ProviderError {
        said(
            Refusal {
                provider,
                status,
                redactions,
                cancel,
            },
            body,
            MAX_WAIT,
        )
    }

    fn reading(body: &str) -> Box<dyn Read + Send> {
        Box::new(std::io::Cursor::new(body.to_owned().into_bytes()))
    }

    /// The same, for a body that is not text. A peer picks the bytes it sends,
    /// and some of the ones it can pick are no character at all.
    fn reading_raw(body: Vec<u8>) -> Box<dyn Read + Send> {
        Box::new(std::io::Cursor::new(body))
    }

    fn plain_refused(status: u16, body: Box<dyn Read + Send>) -> ProviderError {
        refused_from_read("test", status, body, &Redactions::default(), &Cancel::new())
    }

    fn plain_said(status: u16, body: Box<dyn Read + Send>, wait: Duration) -> ProviderError {
        said(
            Refusal {
                provider: "test",
                status,
                redactions: &Redactions::default(),
                cancel: &Cancel::new(),
            },
            body,
            wait,
        )
    }

    #[test]
    fn a_refusal_carries_the_status_and_the_sentence_that_explains_it() {
        let problem = plain_refused(
            404,
            reading(r#"{"error":{"type":"not_found","message":"model: nope"}}"#),
        );

        assert_eq!(problem.to_string(), "test: HTTP 404: model: nope");
    }

    #[test]
    fn a_refusal_that_states_its_reason_as_a_bare_detail_is_read_too() {
        // The shape the backend a plan is served by answers in. Read only as
        // `error.message`, this reached the user as the line of JSON around the
        // sentence rather than as the sentence.
        let problem = plain_refused(400, reading(r#"{"detail":"Unsupported parameter: x"}"#));

        assert_eq!(
            problem.to_string(),
            "test: HTTP 400: Unsupported parameter: x"
        );
    }

    #[test]
    fn a_refusal_that_is_not_the_api_still_says_what_it_said() {
        let problem = plain_refused(502, reading("  upstream connect error  "));

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
        let problem = plain_said(429, Box::new(Stalled), Duration::ZERO);

        assert_eq!(
            problem.to_string(),
            "test: HTTP 429: the response could not be read: it stopped part-way through"
        );
    }

    #[test]
    fn a_body_that_keeps_producing_bytes_cannot_outlive_the_elapsed_deadline() {
        let problem = plain_said(429, Box::new(Trickle), Duration::from_millis(1));

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

        let problem = plain_said(502, Box::new(body), MAX_WAIT);

        assert_eq!(problem.to_string(), "test: HTTP 502: upstream is unwell");
    }

    /// A body whose one read sleeps past `wait` and then answers with `then`.
    ///
    /// Drives the timing deterministically: the sleep is what spends the
    /// wait, not a hope that a fast test outraces a real deadline, so it is
    /// the read's own answer — not a check that ran ahead of it — that
    /// decides what `fill` does with it.
    struct Late {
        wait: Duration,
        then: Vec<u8>,
    }

    impl Read for Late {
        fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(self.wait.saturating_add(Duration::from_millis(20)));
            let took = self.then.len().min(into.len());
            into.get_mut(..took)
                .unwrap_or_default()
                .copy_from_slice(self.then.get(..took).unwrap_or_default());
            self.then.drain(..took);
            Ok(took)
        }
    }

    #[test]
    fn a_clean_end_a_read_hands_over_after_the_wait_runs_out_is_not_a_stall() {
        // The bug this proves against checked the wait right after this
        // read, before looking at what it returned, so a body that closed
        // cleanly exactly there was reported as a stall instead.
        let wait = Duration::from_millis(5);
        let mut reading = Late {
            wait,
            then: Vec::new(),
        };
        let mut said = Vec::new();

        let read = fill(&mut reading, &mut said, wait, &Cancel::new());

        assert!(
            read.is_ok(),
            "a late clean end was reported as a stall: {read:?}"
        );
        assert!(said.is_empty());
    }

    #[test]
    fn a_chunk_a_read_hands_over_as_the_wait_runs_out_is_kept() {
        // The other half: a read that cannot be shown to have ended, or to
        // be the whole reply, still handed bytes over before the wait ran
        // out finding that out, and those bytes must survive it.
        let wait = Duration::from_millis(5);
        let mut reading = Late {
            wait,
            then: b"partial".to_vec(),
        };
        let mut said = Vec::new();

        let read = fill(&mut reading, &mut said, wait, &Cancel::new());

        assert!(
            matches!(read, Err(ReadError::Body(_))),
            "expected the spent wait to end this read: {read:?}"
        );
        assert_eq!(said, b"partial");
    }

    /// A body that hands `first` over at once, then sleeps past `wait`
    /// before answering that nothing more is coming.
    ///
    /// Two pieces because the read this proves is the one that discovers the
    /// clean end: the reply already sat whole in `fill`'s buffer, and the
    /// read confirming that arrives only once the wait has already run out.
    struct WholeThenLateEnd {
        first: Option<Vec<u8>>,
        wait: Duration,
    }

    impl Read for WholeThenLateEnd {
        fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
            let Some(body) = self.first.take() else {
                std::thread::sleep(self.wait.saturating_add(Duration::from_millis(20)));
                return Ok(0);
            };

            let took = body.len().min(into.len());
            into.get_mut(..took)
                .unwrap_or_default()
                .copy_from_slice(body.get(..took).unwrap_or_default());
            Ok(took)
        }
    }

    #[test]
    fn a_reply_read_whole_is_shown_when_the_close_confirming_it_arrives_late() {
        // The same bug as the two above, seen through the whole pipeline:
        // the reply was already complete in hand, and only the read saying
        // so arrived once the wait had run out. Losing it turned a reply
        // into "the response could not be read", and keeping it says
        // `End::Whole` — no cut clause, because none of it was lost.
        let wait = Duration::from_millis(5);
        let body = r#"{"error":{"message":"upstream is unwell"}}"#;
        let reading = WholeThenLateEnd {
            first: Some(body.as_bytes().to_vec()),
            wait,
        };

        let problem = plain_said(502, Box::new(reading), wait);

        assert_eq!(problem.to_string(), "test: HTTP 502: upstream is unwell");
    }

    #[test]
    fn an_error_page_is_read_only_as_far_as_it_is_worth_reading() {
        // A gateway can answer with a whole HTML document, and all of it would
        // otherwise end up on one line in front of a user.
        let long = "x".repeat(64 * 1024);

        let shown = plain_refused(500, reading(&long)).to_string();

        assert!(
            shown.len() < 16 * 1024,
            "the whole page came back: {} bytes",
            shown.len()
        );
    }

    /// A value a gateway could echo back, invented here and nowhere else.
    const CANARY: &str = "canary-9f3c-cut-in-half-do-not-log";

    /// The clause a reply ends with when more of it arrived than the bound
    /// keeps and the reading ended, spelled out rather than read off the
    /// constant these tests are about.
    const SAYS_LONGER: &str = " [cut: the reply was longer than crucible reads]";

    /// The other one, spelled out the same way: what a reply ends with where
    /// the bound filled and the reading then failed rather than ended.
    const SAYS_STOPPED: &str = " [cut: crucible stopped reading here]";

    fn protecting(value: &str) -> Redactions {
        let mut outgoing = crucible_credentials::Outgoing::new();
        outgoing.protect(value);
        outgoing.redactions()
    }

    #[test]
    fn a_credential_the_bound_cut_in_half_does_not_reach_the_message() {
        // The echo starts inside the bound and ends past it, so what is kept
        // is the first part of the credential and no whole-value filter can
        // see it. It is the last thing on the line a user is shown.
        let body = format!("{}{CANARY}tail", "x".repeat(8 * 1024 - 10));

        let shown = refused_from_read(
            "test",
            502,
            reading(&body),
            &protecting(CANARY),
            &Cancel::new(),
        )
        .to_string();

        let start = CANARY.get(..10).unwrap_or_default();
        assert!(
            !shown.contains(start),
            "the first bytes of the credential survived the cut"
        );
        assert!(
            shown.ends_with(SAYS_LONGER),
            "nothing said the reply was cut"
        );
    }

    #[test]
    fn a_reply_longer_than_the_bound_says_so_and_one_that_exactly_fills_it_does_not() {
        // One byte decides it here, which is why the read goes one byte past
        // the bound: a body stopped at the bound and one cut by it are
        // otherwise the same bytes.
        let exact = "x".repeat(8 * 1024);
        let over = format!("{exact}x");

        let fits = plain_refused(500, reading(&exact)).to_string();
        let cut = plain_refused(500, reading(&over)).to_string();

        assert_eq!(fits, format!("test: HTTP 500: {exact}"));
        assert_eq!(
            cut.len(),
            fits.len().saturating_add(SAYS_LONGER.len()),
            "the clause is the whole difference one byte makes"
        );
        assert_eq!(cut.strip_suffix(SAYS_LONGER), Some(fits.as_str()));
    }

    /// A peer that hands over as many bytes as it was made with and then
    /// breaks rather than ending. Made with the bound, the byte that would
    /// tell a reply stopping there from one cut there never arrives and the
    /// reading fails instead; made with one more, that byte has arrived and
    /// the break is behind the bound the body is read through.
    ///
    /// It fails outright rather than stalling until the deadline, so what this
    /// asserts is about the message and not about how fast the machine running
    /// it hands over eight kibibytes. Both reach the same arm: a stall that
    /// runs out the wait is a failed read too, spelled `TimedOut`.
    struct Filled(usize);

    impl Read for Filled {
        fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
            let giving = self.0.min(into.len());
            if giving == 0 {
                return Err(io::ErrorKind::ConnectionReset.into());
            }
            if let Some(slot) = into.get_mut(..giving) {
                slot.fill(b'x');
            }
            self.0 = self.0.saturating_sub(giving);
            Ok(giving)
        }
    }

    #[test]
    fn a_reply_that_fills_the_bound_and_then_does_not_end_is_still_shown() {
        // The read that tells a reply ending at the bound from one cut by it
        // is a read a peer that has sent everything and not closed will never
        // answer. Asking for it must not cost the reply already in hand, which
        // is everything that would have been kept either way.
        let problem = plain_said(500, Box::new(Filled(8 * 1024)), MAX_WAIT);

        let shown = problem.to_string();

        assert!(
            shown.contains(&"x".repeat(8 * 1024)),
            "the reply was thrown away for the want of a byte: {shown}"
        );
        assert!(
            shown.ends_with(SAYS_STOPPED),
            "nothing said crucible read no further"
        );
        assert!(
            !shown.contains(SAYS_LONGER),
            "the message named a length nobody measured"
        );
    }

    #[test]
    fn a_length_the_bound_measured_survives_a_peer_that_breaks_after_it() {
        // What keeps a peer's break from reaching the clause once the byte
        // past the bound has arrived is the bound the body is read through.
        // That byte is the last one that can reach `said`, and once it has,
        // the peer is never read from again: the reset it answers with here
        // is never seen, the reading ends cleanly, and the arm that names a
        // length is the one that fires.
        //
        // So nothing the peer does after the byte that measured a length can
        // have it reported as a reply nobody measured. Pinned because the two
        // sides of that live apart — the bound is applied where the body is
        // taken, and read off where the message is chosen.
        let problem = plain_said(500, Box::new(Filled(8 * 1024 + 1)), MAX_WAIT);

        let shown = problem.to_string();

        assert!(
            shown.ends_with(SAYS_LONGER),
            "the measured length went unnamed: {}",
            shown
                .get(shown.len().saturating_sub(64)..)
                .unwrap_or(&shown)
        );
        assert!(
            !shown.contains(SAYS_STOPPED),
            "a reply measured as longer was reported as merely stopped"
        );
    }

    /// The same canary, with a character too wide for one byte, so a cut can
    /// land inside one rather than between two.
    const WIDE_CANARY: &str = "canary-6b21-coupé-en-deux-do-not-log";

    #[test]
    fn a_credential_the_bound_cut_inside_a_character_does_not_reach_the_message() {
        // The bound counts bytes and a character can be several, so the last
        // thing kept can be half of one. Half a character is no character, so
        // turning the body into text first would stand a replacement one in
        // for it and the filter would be asked about a tail the conversion
        // invented rather than about the bytes that arrived.
        let wide = WIDE_CANARY.find('é').unwrap_or_default();
        let filler = (8 * 1024_usize).saturating_sub(wide).saturating_sub(1);
        let body = format!("{}{WIDE_CANARY}tail", "x".repeat(filler));

        let shown = refused_from_read(
            "test",
            502,
            reading(&body),
            &protecting(WIDE_CANARY),
            &Cancel::new(),
        )
        .to_string();

        let start = WIDE_CANARY.get(..wide).unwrap_or_default();
        assert!(
            !shown.contains(start),
            "the credential before the split character survived the cut"
        );
        assert!(
            shown.ends_with(SAYS_LONGER),
            "nothing said the reply was cut"
        );
    }

    #[test]
    fn a_cut_inside_the_last_character_leaves_no_stand_in_in_the_message() {
        // Nothing protected anywhere in this: what the bound split is an
        // ordinary word. Half a character is no character, so the conversion
        // stands a replacement one in for it, and that stand-in marks where
        // crucible stopped reading rather than anything the service said. The
        // clause after it already says that, in words.
        let filler = (8 * 1024_usize)
            .saturating_sub("caf".len())
            .saturating_sub(1);
        let mut body = "x".repeat(filler).into_bytes();
        body.extend_from_slice("café".as_bytes());

        let shown = plain_refused(502, reading_raw(body)).to_string();

        assert!(
            shown.ends_with(&format!("caf{SAYS_LONGER}")),
            "the half character the cut left is on the line: {}",
            shown
                .get(shown.len().saturating_sub(64)..)
                .unwrap_or(&shown)
        );
    }

    #[test]
    fn a_credential_the_bound_cut_in_front_of_stray_bytes_does_not_reach_the_message() {
        // How many characters the conversion invents at the end is the peer's
        // to choose: two bytes that are no character at all are two of them,
        // and a filter that answers about what the conversion left answers
        // about a tail the peer arranged. All but the last character of the
        // credential is kept, and the stray bytes sit behind it.
        let head = CANARY.get(..CANARY.len().saturating_sub(1)).unwrap_or("");
        let stray = [0xFF_u8, 0xFF];
        let filler = (8 * 1024_usize)
            .saturating_sub(head.len())
            .saturating_sub(stray.len());

        let mut body = "x".repeat(filler).into_bytes();
        body.extend_from_slice(head.as_bytes());
        body.extend_from_slice(&stray);
        // The byte that makes it a cut, and the only one never kept.
        body.push(b'!');

        let shown = refused_from_read(
            "test",
            502,
            reading_raw(body),
            &protecting(CANARY),
            &Cancel::new(),
        )
        .to_string();

        assert!(
            !shown.contains(head),
            "the credential in front of the stray bytes survived the cut"
        );
        assert!(
            shown.ends_with(SAYS_LONGER),
            "nothing said the reply was cut"
        );
    }

    #[test]
    fn cancelling_while_a_refusal_is_read_stays_a_cancel() {
        let cancel = Cancel::new();
        cancel.request();

        let problem = refused_from_read(
            "test",
            429,
            Box::new(Stalled),
            &Redactions::default(),
            &cancel,
        );

        assert!(matches!(problem, ProviderError::Cancelled("test")));
    }
    #[test]
    fn a_request_too_large_for_the_model_is_told_apart_from_every_other_refusal() {
        // The one refusal with a remedy: make the session smaller and ask the
        // same question again. Every other 400 is about the request itself.
        let said = r#"{"error":{"code":"context_length_exceeded",
            "message":"This model's maximum context length is 272000 tokens"}}"#;

        assert!(matches!(
            plain_refused(400, reading(said)),
            ProviderError::WindowExceeded { .. }
        ));
    }

    #[test]
    fn a_refusal_whose_prose_mentions_the_context_is_still_only_a_refusal() {
        // Decided from the code, never the sentence. A vendor apologising in
        // prose about context length, with a code that says something else, is
        // not a session to compact.
        let said = r#"{"error":{"code":"invalid_api_key",
            "message":"maximum context length is not your problem here"}}"#;

        assert!(matches!(
            plain_refused(400, reading(said)),
            ProviderError::Refused { .. }
        ));
    }
}
