//! Server-sent events: the framing both wire protocols stream over.
//!
//! Kept apart from either provider because the framing is the same and the
//! payloads are not. What arrives here is `event:` and `data:` lines separated
//! by blank ones; what leaves is one [`SseEvent`] per blank line, with the
//! payload still text. Parsing that text is the provider's job, because only it
//! knows what its vendor puts in there.
//!
//! Bytes throughout, converted to text once per event. One read from the socket
//! can split a character in half, so accumulating into a `String` would have to
//! either lose it or refuse it -- and the payload is JSON, where a lost byte is
//! a silently wrong answer.

use std::io::{self, BufRead};
use std::string::FromUtf8Error;

/// The most bytes one event -- or one line of it -- may accumulate.
///
/// A response is not trusted to be well formed. Without this, a peer that sent
/// no line ending would grow these buffers until the process died, which is a
/// failure this crate can prevent and the caller cannot.
const MAX_EVENT: usize = 8 * 1024 * 1024;

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
    NotUtf8(#[from] FromUtf8Error),
}

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SseEvent {
    /// What the peer called it. Empty when it sent only data.
    pub(crate) name: String,
    /// The payload, with multiple `data:` lines joined by newlines.
    pub(crate) data: String,
}

/// Frames a byte stream into events.
#[derive(Debug)]
pub(crate) struct Events<R> {
    reader: R,
    line: Vec<u8>,
    name: Vec<u8>,
    data: Vec<u8>,
}

impl<R: BufRead> Events<R> {
    /// Frames whatever `reader` produces.
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            line: Vec::new(),
            name: Vec::new(),
            data: Vec::new(),
        }
    }

    /// The next event, or `None` when the stream is finished.
    ///
    /// A stream that ends part-way through an event delivers nothing for it.
    /// That is not silent: every protocol here ends with an event of its own,
    /// so the caller notices the one that never came.
    pub(crate) fn next(&mut self) -> Option<Result<SseEvent, SseError>> {
        loop {
            match self.read_line() {
                Err(problem) => return Some(Err(problem)),
                Ok(false) => return None,
                Ok(true) => {}
            }

            if !self.line.is_empty() {
                if let Err(problem) = self.take_field() {
                    return Some(Err(problem));
                }
                continue;
            }

            // A blank line dispatches. Runs of them, and the one that closes a
            // comment, have nothing to dispatch.
            if !self.name.is_empty() || !self.data.is_empty() {
                return Some(self.dispatch());
            }
        }
    }

    /// Reads one line into `self.line`, without its ending.
    ///
    /// `false` means the stream is finished. Reads through the buffer rather
    /// than with `read_line`, which would take an unbounded line from a peer
    /// that never sent an ending.
    fn read_line(&mut self) -> Result<bool, SseError> {
        self.line.clear();

        loop {
            let available = self.reader.fill_buf()?;
            if available.is_empty() {
                // A last line with no ending is still a line.
                return Ok(!self.line.is_empty());
            }

            let ending = available.iter().position(|byte| *byte == b'\n');
            let keep = ending.unwrap_or(available.len());
            let consume = ending.map_or(available.len(), |at| at + 1);

            if self.line.len() + keep > MAX_EVENT {
                return Err(SseError::TooLarge);
            }

            self.line.extend(available.iter().take(keep).copied());
            self.reader.consume(consume);

            if ending.is_some() {
                // A CRLF peer leaves the carriage return on the line.
                if self.line.last() == Some(&b'\r') {
                    self.line.pop();
                }
                return Ok(true);
            }
        }
    }

    /// Folds one line into the event being built.
    fn take_field(&mut self) -> Result<(), SseError> {
        let (name, value) = split(&self.line);

        // An empty field name is a comment -- including the keep-alive some
        // proxies send, which is a bare colon.
        match name {
            b"event" => {
                self.name.clear();
                self.name.extend_from_slice(value);
            }
            b"data" => {
                if self.data.len() + value.len() > MAX_EVENT {
                    return Err(SseError::TooLarge);
                }
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(value);
            }
            // `id` and `retry` are reconnection machinery. Nothing here
            // reconnects: a dropped turn is the runner's to retry, and it holds
            // the transcript that would be needed to do it.
            _ => {}
        }

        Ok(())
    }

    /// Finishes the event and resets for the next one.
    fn dispatch(&mut self) -> Result<SseEvent, SseError> {
        Ok(SseEvent {
            name: String::from_utf8(std::mem::take(&mut self.name))?,
            data: String::from_utf8(std::mem::take(&mut self.data))?,
        })
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
    fn events(stream: &str) -> Vec<SseEvent> {
        let mut framed = Events::new(stream.as_bytes());
        let mut out = Vec::new();

        while let Some(event) = framed.next() {
            out.push(event.unwrap());
        }

        out
    }

    /// The same, delivered the way a socket delivers one.
    ///
    /// `at_a_time` bytes per read, because that is what a buffer of that size
    /// gives the framing: nothing arrives whole, and every field, line ending
    /// and event boundary falls across a read.
    fn dripped(stream: &str, at_a_time: usize) -> Vec<SseEvent> {
        let reader = io::BufReader::with_capacity(at_a_time, io::Cursor::new(stream.as_bytes()));
        let mut framed = Events::new(reader);
        let mut out = Vec::new();

        while let Some(event) = framed.next() {
            out.push(event.unwrap());
        }

        out
    }

    #[test]
    fn an_event_carries_its_name_and_its_payload() {
        let out = events("event: ping\ndata: {\"ok\":true}\n\n");

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().name, "ping");
        assert_eq!(out.first().unwrap().data, "{\"ok\":true}");
    }

    #[test]
    fn a_blank_line_ends_an_event_and_the_next_one_starts_clean() {
        let out = events("event: one\ndata: a\n\nevent: two\ndata: b\n\n");

        assert_eq!(out.len(), 2);
        assert_eq!(out.first().unwrap().name, "one");
        assert_eq!(out.last().unwrap().name, "two");
        assert_eq!(out.last().unwrap().data, "b");
    }

    #[test]
    fn several_data_lines_join_with_newlines() {
        let out = events("event: e\ndata: one\ndata: two\n\n");

        assert_eq!(out.first().unwrap().data, "one\ntwo");
    }

    #[test]
    fn only_the_first_space_after_the_colon_is_framing() {
        // The rest belongs to the payload, and JSON can begin with a space.
        let out = events("data:  spaced\n\n");

        assert_eq!(out.first().unwrap().data, " spaced");
    }

    #[test]
    fn a_comment_is_not_an_event() {
        // Proxies send a bare colon to hold the connection open.
        let out = events(":\n:keep-alive\n\nevent: real\ndata: x\n\n");

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().name, "real");
    }

    #[test]
    fn carriage_returns_frame_the_same_as_bare_newlines() {
        let out = events("event: e\r\ndata: payload\r\n\r\n");

        assert_eq!(out.first().unwrap().name, "e");
        assert_eq!(out.first().unwrap().data, "payload");
    }

    #[test]
    fn reconnection_fields_are_ignored() {
        let out = events("id: 7\nretry: 3000\nevent: e\ndata: x\n\n");

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().name, "e");
        assert_eq!(out.first().unwrap().data, "x");
    }

    #[test]
    fn a_stream_that_ends_mid_event_delivers_nothing_for_it() {
        // No blank line, so the event never dispatched. Delivering a truncated
        // payload would hand a provider half a JSON object to parse.
        let out = events("event: e\ndata: {\"half\":");

        assert!(out.is_empty(), "a truncated event was delivered: {out:?}");
    }

    #[test]
    fn a_last_event_with_no_trailing_newline_still_arrives() {
        let out = events("event: e\ndata: x\n\n");

        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_stream_arriving_one_byte_at_a_time_frames_the_same_as_one_that_arrives_whole() {
        // What a socket delivers per read has nothing to do with where the
        // peer put its line endings, so a parser that is only right for whole
        // events is a parser that is right until the network is busy.
        let stream = "event: one\r\ndata: {\"a\":1}\r\n\r\n:keep-alive\n\nevent: two\ndata: line\ndata: and another\n\n";

        let out = dripped(stream, 1);

        assert_eq!(out.len(), 2, "a read boundary swallowed an event: {out:?}");
        assert_eq!(out, events(stream));
    }

    #[test]
    fn a_character_split_across_reads_is_put_back_together() {
        // The reason the framing works in bytes and converts once per event.
        // Two of these characters are three bytes each, so read one byte at a
        // time every one of them spans three reads.
        let stream = "data: {\"text\":\"héllo — wörld\"}\n\n";

        let out = dripped(stream, 1);

        assert_eq!(out.len(), 1);
        assert_eq!(
            out.first().unwrap().data,
            "{\"text\":\"héllo — wörld\"}",
            "a character was lost or mangled at a read boundary"
        );
    }

    #[test]
    fn an_event_far_larger_than_one_read_still_arrives_whole() {
        // A tool call's arguments are one event, and a model writing a file
        // puts the whole file in it. Under the ceiling, length is not a reason
        // to refuse or to truncate.
        let payload = "x".repeat(256 * 1024);
        let stream = format!("event: big\ndata: {payload}\n\n");

        let out = dripped(&stream, 7);

        assert_eq!(out.len(), 1);
        assert_eq!(out.first().unwrap().data.len(), payload.len());
        assert_eq!(out.first().unwrap().data, payload);
    }

    #[test]
    fn a_peer_that_never_sends_a_line_ending_cannot_exhaust_memory() {
        // The reason lines are read through the buffer instead of by
        // `read_line`: this reader is infinite and never sends one.
        let endless = io::BufReader::new(io::repeat(b'x'));
        let mut framed = Events::new(endless);

        let problem = framed.next().unwrap().unwrap_err();

        assert!(
            matches!(problem, SseError::TooLarge),
            "expected a bounded read to give up, got {problem:?}"
        );
    }

    #[test]
    fn a_payload_that_is_not_text_is_refused_rather_than_mangled() {
        // Losing a byte from JSON is a silently wrong answer.
        let mut framed = Events::new(&b"data: \xff\xfe\n\n"[..]);

        let problem = framed.next().unwrap().unwrap_err();

        assert!(
            matches!(problem, SseError::NotUtf8(_)),
            "expected the stream to be refused, got {problem:?}"
        );
    }
}
