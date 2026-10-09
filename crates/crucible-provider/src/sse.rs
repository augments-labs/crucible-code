//! Server-sent events: the framing both wire protocols stream over.
//!
//! Kept apart from either provider because the framing is the same and the
//! payloads are not. What arrives here is `event:` and `data:` lines separated
//! by blank ones; what leaves is one [`SseEvent`] per blank line, with the
//! payload still text. Parsing that text is the provider's job, because only it
//! knows what its vendor puts in there.
//!
//! A read that gave up waiting, that brought only part of a line, or that read
//! a line which finishes no event, leaves as [`Framed::Quiet`] rather than as
//! either an event or an ending, which is what lets a caller holding a cancel
//! act on it while a response is open. Both providers stream through here, so
//! that is one behaviour rather than two that have to be kept the same.
//!
//! Lines are read as bytes and converted to text whole. One read from the
//! socket can split a character in half, so converting what each read brings
//! would have to either lose it or refuse it -- and the payload is JSON, where a
//! lost byte is a silently wrong answer. A line ending is never part of a
//! character, so a whole line is text or it is not.

use std::io;
use std::str::Utf8Error;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// The most bytes one event -- or one line of it -- may accumulate.
///
/// A response is not trusted to be well formed. Without this, a peer that sent
/// no line ending would grow these buffers until the process died, which is a
/// failure this crate can prevent and the caller cannot. One MiB still carries
/// a large generated edit in one event while bounding the unavoidable
/// allocation before the runner can apply cumulative response limits.
const MAX_EVENT: usize = 1024 * 1024;

/// Why a stream stopped framing.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SseError {
    /// The connection broke.
    #[error("the connection broke: {0}")]
    Io(#[from] io::Error),

    /// One event was larger than [`MAX_EVENT`].
    #[error("an event was longer than {MAX_EVENT} bytes")]
    TooLarge,

    /// The payload was not text.
    #[error("the stream was not UTF-8: {0}")]
    NotUtf8(#[from] Utf8Error),
}

/// One dispatched event.
///
/// Public in name only, for a dialect of a shared wire to be handed one; the
/// module that holds it is the crate's own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    /// What the peer called it. Empty when it sent only data.
    pub(crate) name: String,
    /// The payload, with multiple `data:` lines joined by newlines.
    pub(crate) data: String,
}

/// What one call to [`Events::next`] came back with.
#[derive(Debug)]
pub(crate) enum Framed {
    /// One event, whole.
    Event(SseEvent),

    /// No event yet. The read waited as long as it waits and the peer said
    /// nothing, the peer sent only part of a line, or it sent a line that
    /// finishes no event: a comment, a field, or a blank line with nothing to
    /// dispatch. None is a failure or an ending: the response is still open
    /// and the model is still thinking.
    ///
    /// It exists so that waiting is the caller's to do. A provider that goes
    /// silent would otherwise hold the caller inside this call for as long as
    /// it stayed silent, one that sends a line every few hundred milliseconds
    /// for as long as it kept sending them, and one that sends a line a few
    /// bytes at a time until it reached [`MAX_EVENT`], and a user who asked to
    /// stop would be waiting on the same socket. Handed back the turn, the
    /// caller looks at its cancel and asks again.
    Quiet,
}

/// What one read of a line came back with.
enum Line {
    /// A line, in `self.line`, without its ending.
    Read,
    /// No whole line yet: the wait expired, or part of the line arrived and
    /// the rest has not. What had arrived of the line stays where it is, so
    /// the read that follows carries on from the middle of it.
    Quiet,
    /// The stream is finished.
    Ended,
}

/// Frames a byte stream into events.
#[derive(Debug)]
pub(crate) struct Events<R> {
    reader: R,
    line: Vec<u8>,
    name: String,
    data: String,
}

impl<R: AsyncBufRead + Unpin> Events<R> {
    /// Frames whatever `reader` produces.
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            line: Vec::new(),
            name: String::new(),
            data: String::new(),
        }
    }

    /// The next event, [`Framed::Quiet`] if no whole line has arrived yet or
    /// the line read finishes none, or `None` when the stream is finished.
    ///
    /// A stream that ends part-way through an event delivers nothing for it.
    /// That is not silent: every protocol here ends with an event of its own,
    /// so the caller notices the one that never came. A line of it that was
    /// not text has already been refused, as it was read.
    pub(crate) async fn next(&mut self) -> Option<Result<Framed, SseError>> {
        match self.read_line().await {
            Err(problem) => return Some(Err(problem)),
            Ok(Line::Quiet) => return Some(Ok(Framed::Quiet)),
            Ok(Line::Ended) => return None,
            Ok(Line::Read) => {}
        }

        if !self.line.is_empty() {
            let taken = self.take_field();
            // Here rather than where the next line starts: a line half of
            // which has arrived is held in the same place, and clearing on
            // the way in would drop that half every time the peer paused
            // mid-line.
            self.line.clear();
            // Handed back rather than read past, whatever the line was: a peer
            // can send lines for as long as it likes without ever finishing an
            // event, and the caller cannot look at its cancel in here.
            return Some(taken.map(|()| Framed::Quiet));
        }

        // A blank line dispatches. Runs of them, and the one that closes a
        // comment, have nothing to dispatch.
        if self.name.is_empty() && self.data.is_empty() {
            return Some(Ok(Framed::Quiet));
        }
        Some(Ok(Framed::Event(self.dispatch())))
    }

    /// Reads what one read brings of a line into `self.line`, without its
    /// ending, and says whether the line is whole.
    ///
    /// Reads through the buffer rather than with `read_line`, which would take
    /// an unbounded line from a peer that never sent an ending.
    async fn read_line(&mut self) -> Result<Line, SseError> {
        let available = match self.reader.fill_buf().await {
            Ok(available) => available,
            // A wait that expired, not a stream that broke. The transport
            // spells it this way on purpose; see [`Framed::Quiet`].
            Err(problem) if problem.kind() == io::ErrorKind::Interrupted => {
                return Ok(Line::Quiet);
            }
            Err(problem) => return Err(problem.into()),
        };

        if available.is_empty() {
            // A last line with no ending is still a line.
            return Ok(if self.line.is_empty() {
                Line::Ended
            } else {
                Line::Read
            });
        }

        let ending = available.iter().position(|byte| *byte == b'\n');
        let keep = ending.unwrap_or(available.len());
        let consume = ending.map_or(available.len(), |at| at + 1);

        if self.line.len() + keep > MAX_EVENT {
            return Err(SseError::TooLarge);
        }

        self.line.extend(available.iter().take(keep).copied());
        self.reader.consume(consume);

        if ending.is_none() {
            // Handed back rather than read on: a peer can send one line a few
            // bytes at a time for as long as [`MAX_EVENT`] lets it, and the
            // caller cannot look at its cancel in here.
            return Ok(Line::Quiet);
        }

        // A CRLF peer leaves the carriage return on the line.
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        Ok(Line::Read)
    }

    /// Folds one line into the event being built.
    fn take_field(&mut self) -> Result<(), SseError> {
        let (name, value) = split(&self.line);

        // An empty field name is a comment -- including the keep-alive some
        // proxies send, which is a bare colon.
        match name {
            b"event" => {
                let value = std::str::from_utf8(value)?;
                self.name.clear();
                self.name.push_str(value);
            }
            b"data" => {
                if self.data.len() + value.len() > MAX_EVENT {
                    return Err(SseError::TooLarge);
                }
                let value = std::str::from_utf8(value)?;
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value);
            }
            // `id` and `retry` are reconnection machinery. Nothing here
            // reconnects: a dropped turn is the runner's to retry, and it holds
            // the transcript that would be needed to do it.
            _ => {}
        }
        Ok(())
    }

    /// Finishes the event and resets for the next one.
    fn dispatch(&mut self) -> SseEvent {
        SseEvent {
            name: std::mem::take(&mut self.name),
            data: std::mem::take(&mut self.data),
        }
    }
}

/// Splits a line into its field name and value.
///
/// A line with no colon is a field name with no value, and one optional space
/// after the colon belongs to the framing rather than the payload.
fn split(line: &[u8]) -> (&[u8], &[u8]) {
    let Some(colon) = line.iter().position(|byte| *byte == b':') else {
        return (line, b"");
    };

    let name = line.get(..colon).unwrap_or_default();
    let value = line.get(colon + 1..).unwrap_or_default();

    match value.first() {
        Some(b' ') => (name, value.get(1..).unwrap_or_default()),
        _ => (name, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames a whole stream, so a test reads as one call.
    async fn events(stream: &str) -> Vec<SseEvent> {
        framed(Events::new(stream.as_bytes())).await
    }

    /// The same, delivered the way a socket delivers one.
    ///
    /// `at_a_time` bytes per read, because that is what a buffer of that size
    /// gives the framing: nothing arrives whole, and every field, line ending
    /// and event boundary falls across a read.
    async fn dripped(stream: &str, at_a_time: usize) -> Vec<SseEvent> {
        let reader =
            tokio::io::BufReader::with_capacity(at_a_time, io::Cursor::new(stream.as_bytes()));
        framed(Events::new(reader)).await
    }

    /// Everything a reader framed.
    async fn framed<R: AsyncBufRead + Unpin>(mut events: Events<R>) -> Vec<SseEvent> {
        let mut out = Vec::new();

        while let Some(next) = events.next().await {
            if let Framed::Event(event) = next.unwrap() {
                out.push(event);
            }
        }

        out
    }

    #[tokio::test]
    async fn an_event_carries_its_name_and_its_payload() {
        let out = events("event: ping\ndata: {\"ok\":true}\n\n").await;

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().name, "ping");
        assert_eq!(out.first().unwrap().data, "{\"ok\":true}");
    }

    #[tokio::test]
    async fn a_blank_line_ends_an_event_and_the_next_one_starts_clean() {
        let out = events("event: one\ndata: a\n\nevent: two\ndata: b\n\n").await;

        assert_eq!(out.len(), 2);
        assert_eq!(out.first().unwrap().name, "one");
        assert_eq!(out.last().unwrap().name, "two");
        assert_eq!(out.last().unwrap().data, "b");
    }

    #[tokio::test]
    async fn several_data_lines_join_with_newlines() {
        let out = events("event: e\ndata: one\ndata: two\n\n").await;

        assert_eq!(out.first().unwrap().data, "one\ntwo");
    }

    #[tokio::test]
    async fn only_the_first_space_after_the_colon_is_framing() {
        // The rest belongs to the payload, and JSON can begin with a space.
        let out = events("data:  spaced\n\n").await;

        assert_eq!(out.first().unwrap().data, " spaced");
    }

    #[tokio::test]
    async fn a_comment_is_not_an_event() {
        // Proxies send a bare colon to hold the connection open.
        let out = events(":\n:keep-alive\n\nevent: real\ndata: x\n\n").await;

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().name, "real");
    }

    #[tokio::test]
    async fn carriage_returns_frame_the_same_as_bare_newlines() {
        let out = events("event: e\r\ndata: payload\r\n\r\n").await;

        assert_eq!(out.first().unwrap().name, "e");
        assert_eq!(out.first().unwrap().data, "payload");
    }

    #[tokio::test]
    async fn reconnection_fields_are_ignored() {
        let out = events("id: 7\nretry: 3000\nevent: e\ndata: x\n\n").await;

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().name, "e");
        assert_eq!(out.first().unwrap().data, "x");
    }

    #[tokio::test]
    async fn a_stream_that_ends_mid_event_delivers_nothing_for_it() {
        // No blank line, so the event never dispatched. Delivering a truncated
        // payload would hand a provider half a JSON object to parse.
        let out = events("event: e\ndata: {\"half\":").await;

        assert!(out.is_empty(), "a truncated event was delivered: {out:?}");
    }

    #[tokio::test]
    async fn a_last_event_with_no_trailing_newline_still_arrives() {
        let out = events("event: e\ndata: x\n\n").await;

        assert_eq!(out.len(), 1);
    }

    #[tokio::test]
    async fn a_stream_arriving_one_byte_at_a_time_frames_the_same_as_one_that_arrives_whole() {
        // What a socket delivers per read has nothing to do with where the
        // peer put its line endings, so a parser that is only right for whole
        // events is a parser that is right until the network is busy.
        let stream = "event: one\r\ndata: {\"a\":1}\r\n\r\n:keep-alive\n\nevent: two\ndata: line\ndata: and another\n\n";

        let out = dripped(stream, 1).await;

        assert_eq!(out.len(), 2, "a read boundary swallowed an event: {out:?}");
        assert_eq!(out, events(stream).await);
    }

    #[tokio::test]
    async fn a_character_split_across_reads_is_put_back_together() {
        // The reason the framing works in bytes and converts once per event.
        // Two of these characters are three bytes each, so read one byte at a
        // time every one of them spans three reads.
        let stream = "data: {\"text\":\"héllo — wörld\"}\n\n";

        let out = dripped(stream, 1).await;

        assert_eq!(out.len(), 1);
        assert_eq!(
            out.first().unwrap().data,
            "{\"text\":\"héllo — wörld\"}",
            "a character was lost or mangled at a read boundary"
        );
    }

    #[tokio::test]
    async fn an_event_far_larger_than_one_read_still_arrives_whole() {
        // A tool call's arguments are one event, and a model writing a file
        // puts the whole file in it. Under the ceiling, length is not a reason
        // to refuse or to truncate.
        let payload = "x".repeat(256 * 1024);
        let stream = format!("event: big\ndata: {payload}\n\n");

        let out = dripped(&stream, 7).await;

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().data.len(), payload.len());
        assert_eq!(out.first().unwrap().data, payload);
    }

    #[tokio::test]
    async fn a_pause_anywhere_in_a_stream_frames_the_same_as_one_that_arrives_whole() {
        // What a model thinking mid-sentence does to the socket, and it can
        // fall anywhere: the half of a line that had arrived has to survive the
        // pause. The pause reaching the caller is the other half of this: it is
        // waited out there, where the cancel is, and not in here. Other lines
        // come back quiet too, so what is counted is the pauses themselves,
        // and each has to come back before anything after it is read.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let stream = "event: one\r\ndata: {\"a\":1}\r\n\r\n:keep-alive\n\nevent: two\ndata: line\ndata: and another\n\n";
        let pauses = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&pauses);
        let source = tokio::io::BufReader::new(crate::transport::SyncReader::new(
            crate::transport::Paused::dawdling(stream, 3).meanwhile(move || {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
        ));
        let mut framing = Events::new(source);
        let mut out = Vec::new();
        let mut paused_in_all = 0;

        while let Some(next) = framing.next().await {
            let paused_in_this = pauses.swap(0, Ordering::Relaxed);
            paused_in_all += paused_in_this;
            match next.unwrap() {
                Framed::Event(event) => {
                    assert_eq!(paused_in_this, 0, "the framing waited a pause out itself");
                    out.push(event);
                }
                Framed::Quiet => assert!(
                    paused_in_this <= 1,
                    "the framing waited {paused_in_this} pauses out itself"
                ),
            }
        }

        assert!(paused_in_all > 0, "the stream never paused");
        assert_eq!(out, events(stream).await);
    }

    #[tokio::test]
    async fn a_peer_that_never_sends_a_line_ending_cannot_exhaust_memory() {
        // The reason lines are read through the buffer instead of by
        // `read_line`: this reader is infinite and never sends one.
        // Each read hands back what it brought of the line as a quiet, so the
        // refusal comes after many of them; every read brings at least a byte,
        // which bounds how many.
        let endless = tokio::io::BufReader::new(tokio::io::repeat(b'x'));
        let mut framed = Events::new(endless);

        let mut reads = 0;
        let problem = loop {
            match framed.next().await.unwrap() {
                Ok(Framed::Quiet) if reads < MAX_EVENT => reads += 1,
                Ok(other) => panic!("expected the line to be refused, got {other:?}"),
                Err(problem) => break problem,
            }
            assert!(framed.line.len() <= MAX_EVENT, "the line outgrew the bound");
        };

        assert!(
            matches!(problem, SseError::TooLarge),
            "expected a bounded read to give up, got {problem:?}"
        );
    }

    #[tokio::test]
    async fn an_event_over_the_bound_is_refused_before_it_is_dispatched() {
        let payload = "x".repeat(MAX_EVENT + 1);
        let stream = format!("data: {payload}\n\n");
        let mut framed = Events::new(io::Cursor::new(stream));

        let problem = framed.next().await.unwrap().unwrap_err();

        assert!(matches!(problem, SseError::TooLarge));
    }

    #[tokio::test]
    async fn a_payload_that_is_not_text_is_refused_rather_than_mangled() {
        // Losing a byte from JSON is a silently wrong answer.
        let mut framed = Events::new(&b"data: \xff\xfe\n\n"[..]);

        let problem = framed.next().await.unwrap().unwrap_err();

        assert!(
            matches!(problem, SseError::NotUtf8(_)),
            "expected the stream to be refused, got {problem:?}"
        );
    }
}
