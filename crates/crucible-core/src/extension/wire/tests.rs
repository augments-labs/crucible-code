//! Tests for the frames an extension sends.

use std::collections::VecDeque;
use std::io::{BufReader, Cursor, Read, Write};

use super::{FRAME_BYTES, FrameError, Frames, Written};

/// Reads from `bytes` in chunks small enough that no frame arrives whole.
///
/// A pipe hands over whatever has been written so far, which is never the
/// boundaries the writer meant. Four bytes is that, made reproducible.
fn dribbled(bytes: &[u8]) -> Frames<BufReader<Cursor<Vec<u8>>>> {
    Frames::new(BufReader::with_capacity(4, Cursor::new(bytes.to_vec())))
}

/// Everything that arrived, and how the stream ended.
fn drained(mut frames: Frames<BufReader<Cursor<Vec<u8>>>>) -> (Vec<String>, Option<FrameError>) {
    let mut read = Vec::new();
    while let Some(one) = frames.next_frame() {
        match one {
            Ok(frame) => read.push(frame),
            Err(err) => return (read, Some(err)),
        }
    }
    (read, None)
}

#[test]
fn frames_arrive_one_at_a_time_in_the_order_they_were_written() {
    let (read, err) = drained(dribbled(b"{\"a\":1}\n{\"b\":2}\n"));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(read, vec!["{\"a\":1}".to_owned(), "{\"b\":2}".to_owned()]);
}

#[test]
fn a_frame_split_across_reads_is_handed_over_whole() {
    // Longer than the reader's buffer on purpose: a frame that arrives in
    // pieces is the ordinary case on a pipe, not the exceptional one.
    let long = "x".repeat(100);
    let (read, err) = drained(dribbled(format!("{long}\n").as_bytes()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(read, vec![long]);
}

#[test]
fn a_frame_at_the_ceiling_arrives_and_one_byte_more_does_not() {
    let at = "x".repeat(FRAME_BYTES);
    let (read, err) = drained(dribbled(format!("{at}\n").as_bytes()));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(read.len(), 1);
    assert_eq!(read.first().map(String::len), Some(FRAME_BYTES));

    let over = "x".repeat(FRAME_BYTES + 1);
    let (read, err) = drained(dribbled(format!("{over}\n").as_bytes()));
    assert!(read.is_empty(), "{read:?}");
    assert!(
        matches!(err, Some(FrameError::TooLong { maximum }) if maximum == FRAME_BYTES),
        "{err:?}"
    );
}

#[test]
fn a_frame_past_the_ceiling_ends_the_stream_rather_than_being_skipped() {
    // The frame after it is well formed and must not arrive anyway. Crucible
    // stopped reading mid-frame, so every byte after that is the extension's
    // word for where the next one starts — including the newline it planted.
    let over = "x".repeat(FRAME_BYTES + 1);
    let mut frames = dribbled(format!("{over}\n{{\"after\":true}}\n").as_bytes());
    assert!(matches!(
        frames.next_frame(),
        Some(Err(FrameError::TooLong { .. }))
    ));
    assert!(frames.next_frame().is_none());
}

#[test]
fn output_that_stops_partway_through_a_frame_is_not_a_frame() {
    let (read, err) = drained(dribbled(b"{\"whole\":1}\n{\"cut\":"));
    assert_eq!(read, vec!["{\"whole\":1}".to_owned()]);
    assert!(
        matches!(err, Some(FrameError::Truncated { seen }) if seen == 7),
        "{err:?}"
    );
}

#[test]
fn a_frame_that_is_not_text_is_refused_rather_than_repaired() {
    let (read, err) = drained(dribbled(b"\xff\xfe\n"));
    assert!(read.is_empty(), "{read:?}");
    assert!(matches!(err, Some(FrameError::NotText)), "{err:?}");
}

#[test]
fn a_blank_line_is_not_a_frame() {
    let (read, err) = drained(dribbled(b"\n{\"a\":1}\n\n\n{\"b\":2}\n\n"));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(read, vec!["{\"a\":1}".to_owned(), "{\"b\":2}".to_owned()]);
}

/// A pipe that remembers what reached it and when it was pushed.
#[derive(Debug, Default)]
struct Recorded {
    /// Everything written.
    bytes: Vec<u8>,
    /// How many times it was flushed.
    flushed: usize,
}

impl Write for Recorded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flushed = self.flushed.saturating_add(1);
        Ok(())
    }
}

#[test]
fn each_frame_goes_out_ended_by_one_newline_and_is_not_left_in_a_buffer() {
    let mut pipe = Recorded::default();
    let mut sending = Written::new(&mut pipe);
    sending.send("{\"a\":1}").unwrap();
    sending.send("{\"b\":2}").unwrap();

    assert_eq!(pipe.bytes, b"{\"a\":1}\n{\"b\":2}\n");
    // Once per frame. An extension waiting on a request crucible has written
    // but not pushed is a hang that reports nothing.
    assert_eq!(pipe.flushed, 2);
}

#[test]
fn a_frame_carrying_a_newline_is_refused_before_anything_is_written() {
    let mut pipe = Recorded::default();
    let mut sending = Written::new(&mut pipe);
    let err = sending.send("{\"a\":1}\n{\"forged\":true}").unwrap_err();

    assert!(matches!(err, FrameError::Divided), "{err:?}");
    // Not even the part before the newline: a fragment on the wire is a fragment
    // the far end joins to whatever crucible sends next.
    assert!(pipe.bytes.is_empty(), "{:?}", pipe.bytes);
}

#[test]
fn a_frame_past_the_ceiling_is_refused_before_anything_is_written() {
    let mut pipe = Recorded::default();
    let mut sending = Written::new(&mut pipe);
    let err = sending.send(&"x".repeat(FRAME_BYTES + 1)).unwrap_err();

    assert!(
        matches!(err, FrameError::TooLong { maximum } if maximum == FRAME_BYTES),
        "{err:?}"
    );
    assert!(pipe.bytes.is_empty(), "{:?}", pipe.bytes);
}

#[test]
fn what_was_sent_is_what_comes_back() {
    let mut pipe = Recorded::default();
    let mut sending = Written::new(&mut pipe);
    let sent = ["{\"a\":1}", "{\"unicode\":\"héllo → ✓\"}", "{}"];
    for one in sent {
        sending.send(one).unwrap();
    }

    let (read, err) = drained(dribbled(&pipe.bytes));
    assert!(err.is_none(), "{err:?}");
    assert_eq!(read, sent.map(str::to_owned).to_vec());
}

/// A pipe that answers with a written-down sequence, then ends.
///
/// A real pipe fails and is interrupted at moments nothing else reproduces, so
/// the moments are written down here instead.
#[derive(Debug)]
struct Answers(VecDeque<std::io::Result<Vec<u8>>>);

impl Read for Answers {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        match self.0.pop_front() {
            None => Ok(0),
            Some(Err(err)) => Err(err),
            Some(Ok(bytes)) => {
                let room = into
                    .get_mut(..bytes.len())
                    .expect("a written-down answer fits the reader's buffer");
                room.copy_from_slice(&bytes);
                Ok(bytes.len())
            }
        }
    }
}

/// Reads what `said` answers, one answer per read.
fn answering(said: Vec<std::io::Result<Vec<u8>>>) -> Frames<BufReader<Answers>> {
    Frames::new(BufReader::new(Answers(said.into())))
}

/// A pipe that will not take anything.
#[derive(Debug)]
struct Refuses;

impl Write for Refuses {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_pipe_that_fails_is_reported_and_not_read_past() {
    let mut frames = answering(vec![
        Ok(b"{\"a\":1}\n".to_vec()),
        Err(std::io::Error::other("the pipe broke")),
        Ok(b"{\"after\":true}\n".to_vec()),
    ]);
    assert!(matches!(frames.next_frame(), Some(Ok(frame)) if frame == "{\"a\":1}"));

    let err = frames.next_frame().expect("a failure to report");
    let err = err.expect_err("a failure rather than a frame");
    assert!(matches!(err, FrameError::Unreadable { .. }), "{err:?}");
    assert!(err.to_string().contains("the pipe broke"), "{err}");
    // The answer waiting behind the failure never arrives. The reader stopped
    // where it stopped, and what follows is the far end's word for a boundary.
    assert!(frames.next_frame().is_none());
}

#[test]
fn an_interrupted_read_is_resumed_rather_than_reported() {
    // A signal arriving mid-read is the operating system's business and not the
    // extension's; reporting it would fail a run for something that did not go
    // wrong.
    let mut frames = answering(vec![
        Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
        Ok(b"{\"a\":1}\n".to_vec()),
    ]);
    assert!(matches!(frames.next_frame(), Some(Ok(frame)) if frame == "{\"a\":1}"));
    assert!(frames.next_frame().is_none());
}

#[test]
fn a_pipe_that_will_not_take_a_frame_says_so() {
    let err = Written::new(Refuses).send("{\"a\":1}").unwrap_err();
    assert!(matches!(err, FrameError::Unreadable { .. }), "{err:?}");
}
