//! Tests for the frames a hosted program sends.

use std::collections::VecDeque;
use std::io::{BufReader, Cursor, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncWriteExt as _, ReadBuf};

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
    // stopped reading mid-frame, so every byte after that is the program's
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
    // Once per frame. A program waiting on a request crucible has written
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
    // program's; reporting it would fail a run for something that did not go
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

// The asynchronous reader and sender. Every hostile stream above is refused
// the same way whichever kind of stream it arrives on, so the refusals are one
// table, read through both.

/// Whether a refusal is the kind a case means.
type Kind = fn(&FrameError) -> bool;

/// What reading a stream has to come to.
struct Reading {
    /// The frames that arrive, in order.
    frames: Vec<String>,
    /// How the stream was refused, if it was: whether the refusal is the kind
    /// meant, and its words exactly.
    refused: Option<(Kind, String)>,
}

/// A stream, named for the failure it would be.
struct Case {
    /// What the stream is.
    name: &'static str,
    /// Its bytes, in order.
    sent: Vec<u8>,
    /// What reading them has to come to.
    reading: Reading,
}

fn arrives(frames: &[&str]) -> Reading {
    Reading {
        frames: frames.iter().map(|frame| (*frame).to_owned()).collect(),
        refused: None,
    }
}

fn refused(frames: &[&str], kind: Kind, words: &str) -> Reading {
    Reading {
        frames: frames.iter().map(|frame| (*frame).to_owned()).collect(),
        refused: Some((kind, words.to_owned())),
    }
}

/// The streams whose outcome has to be the same however they are read.
///
/// The words are written out rather than taken from either reader, so this is
/// a statement of what a refusal says, not a comparison of two readers that
/// could drift together.
fn cases() -> Vec<Case> {
    let too_long = |err: &FrameError| matches!(err, FrameError::TooLong { maximum } if *maximum == FRAME_BYTES);
    let past = "the program on the other end sent more than 1048576 bytes without ending a frame";
    let at = "x".repeat(FRAME_BYTES);
    let over = "x".repeat(FRAME_BYTES + 1);
    vec![
        Case {
            name: "two frames",
            sent: b"{\"a\":1}\n{\"b\":2}\n".to_vec(),
            reading: arrives(&["{\"a\":1}", "{\"b\":2}"]),
        },
        Case {
            name: "nothing at all",
            sent: Vec::new(),
            reading: arrives(&[]),
        },
        Case {
            name: "blank lines between and after",
            sent: b"\n{\"a\":1}\n\n\n{\"b\":2}\n\n".to_vec(),
            reading: arrives(&["{\"a\":1}", "{\"b\":2}"]),
        },
        Case {
            name: "a frame at the ceiling",
            sent: format!("{at}\n").into_bytes(),
            reading: arrives(&[at.as_str()]),
        },
        Case {
            name: "a frame one byte past the ceiling",
            sent: format!("{over}\n").into_bytes(),
            reading: refused(&[], too_long, past),
        },
        Case {
            name: "a frame past the ceiling, then a well-formed one",
            sent: format!("{over}\n{{\"after\":true}}\n").into_bytes(),
            reading: refused(&[], too_long, past),
        },
        Case {
            name: "past the ceiling with no newline ever",
            sent: over.clone().into_bytes(),
            reading: refused(&[], too_long, past),
        },
        Case {
            // Within the ceiling, so what ends it is the stream: a frame that
            // never ended is not one, however much of it was allowed.
            name: "at the ceiling with no newline ever",
            sent: at.clone().into_bytes(),
            reading: refused(
                &[],
                |err| matches!(err, FrameError::Truncated { seen } if *seen == FRAME_BYTES),
                "the program on the other end stopped 1048576 bytes into an unfinished frame",
            ),
        },
        Case {
            name: "a frame, then output that stops partway through one",
            sent: b"{\"whole\":1}\n{\"cut\":".to_vec(),
            reading: refused(
                &["{\"whole\":1}"],
                |err| matches!(err, FrameError::Truncated { seen: 7 }),
                "the program on the other end stopped 7 bytes into an unfinished frame",
            ),
        },
        Case {
            name: "a frame that is not text",
            sent: b"\xff\xfe\n".to_vec(),
            reading: refused(
                &[],
                |err| matches!(err, FrameError::NotText),
                "the program on the other end sent a frame that is not UTF-8",
            ),
        },
        Case {
            name: "a frame that is not text, then a well-formed one",
            sent: b"{\"a\":1}\n\xff\n{\"b\":2}\n".to_vec(),
            reading: refused(
                &["{\"a\":1}"],
                |err| matches!(err, FrameError::NotText),
                "the program on the other end sent a frame that is not UTF-8",
            ),
        },
    ]
}

/// Everything that arrived, how the stream ended, and whether anything was
/// read past that ending.
type Outcome = (Vec<String>, Option<FrameError>, bool);

/// Reads `sent` blocking, `size` bytes a read at most.
fn read_blocking(sent: &[u8], size: usize) -> Outcome {
    let mut frames = Frames::new(BufReader::with_capacity(size, Cursor::new(sent.to_vec())));
    let mut read = Vec::new();
    while let Some(one) = frames.next_frame() {
        match one {
            Ok(frame) => read.push(frame),
            Err(err) => return (read, Some(err), frames.next_frame().is_some()),
        }
    }
    (read, None, frames.next_frame().is_some())
}

/// Reads `sent` over an in-memory pipe that carries `size` bytes at a time
/// into a reader that takes `size` bytes a read at most.
async fn read_asynchronously(sent: Vec<u8>, size: usize) -> Outcome {
    let (mut near, far) = tokio::io::duplex(size);
    let writing = async move {
        // Refused frames end the reading, and with it the far end of the
        // pipe; what could not be written then is what nobody was reading.
        let _ = near.write_all(&sent).await;
        let _ = near.shutdown().await;
    };
    let reading = async move {
        let mut frames = Frames::new(tokio::io::BufReader::with_capacity(size, far));
        let mut read = Vec::new();
        while let Some(one) = frames.next_frame_async().await {
            match one {
                Ok(frame) => read.push(frame),
                Err(err) => {
                    let past = frames.next_frame_async().await.is_some();
                    return (read, Some(err), past);
                }
            }
        }
        let past = frames.next_frame_async().await.is_some();
        (read, None, past)
    };
    tokio::join!(writing, reading).1
}

fn holds(case: &Case, how: &str, outcome: &Outcome) {
    let (read, err, past) = outcome;
    let name = case.name;
    assert_eq!(read, &case.reading.frames, "{name}, read {how}");
    match (&case.reading.refused, err) {
        (None, None) => {}
        (Some((kind, words)), Some(err)) => {
            assert!(kind(err), "{name}, read {how}: refused as {err:?}");
            assert_eq!(&err.to_string(), words, "{name}, read {how}");
        }
        (expected, got) => panic!(
            "{name}, read {how}: expected a refusal saying {:?}, got {got:?}",
            expected.as_ref().map(|(_, words)| words)
        ),
    }
    assert!(
        !past,
        "{name}, read {how}: a frame arrived after the stream ended"
    );
}

#[tokio::test]
async fn every_stream_comes_to_the_same_outcome_read_blocking_or_asynchronously() {
    for case in cases() {
        holds(&case, "blocking", &read_blocking(&case.sent, 4));
        holds(
            &case,
            "asynchronously",
            &read_asynchronously(case.sent.clone(), 4).await,
        );
    }
}

#[tokio::test]
async fn every_short_stream_comes_to_the_same_outcome_split_at_every_byte() {
    // Every read size from one byte to the whole stream, so every boundary a
    // pipe could put between two reads falls somewhere inside a frame, on a
    // newline, or just after one, in one run or another. The streams past the
    // ceiling are left to the test above: a megabyte at every read size is a
    // million runs of the same arithmetic.
    for case in cases().into_iter().filter(|case| case.sent.len() <= 64) {
        for size in 1..=case.sent.len().max(1) {
            holds(
                &case,
                &format!("blocking, {size} bytes a read"),
                &read_blocking(&case.sent, size),
            );
            holds(
                &case,
                &format!("asynchronously, {size} bytes a read"),
                &read_asynchronously(case.sent.clone(), size).await,
            );
        }
    }
}

/// Reads what `said` answers, one answer per read, asynchronously.
fn answering_asynchronously(
    said: Vec<std::io::Result<Vec<u8>>>,
) -> Frames<tokio::io::BufReader<Answers>> {
    Frames::new(tokio::io::BufReader::new(Answers(said.into())))
}

impl tokio::io::AsyncRead for Answers {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        into: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(match self.get_mut().0.pop_front() {
            None => Ok(()),
            Some(Err(err)) => Err(err),
            Some(Ok(bytes)) => {
                assert!(
                    bytes.len() <= into.remaining(),
                    "a written-down answer fits the reader's buffer"
                );
                into.put_slice(&bytes);
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn a_pipe_that_fails_asynchronously_is_reported_and_not_read_past() {
    let mut frames = answering_asynchronously(vec![
        Ok(b"{\"a\":1}\n".to_vec()),
        Err(std::io::Error::other("the pipe broke")),
        Ok(b"{\"after\":true}\n".to_vec()),
    ]);
    assert!(matches!(frames.next_frame_async().await, Some(Ok(frame)) if frame == "{\"a\":1}"));

    let err = frames
        .next_frame_async()
        .await
        .expect("a failure to report");
    let err = err.expect_err("a failure rather than a frame");
    assert!(matches!(err, FrameError::Unreadable { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        "the program on the other end could not be read: the pipe broke"
    );
    assert!(frames.next_frame_async().await.is_none());
}

#[tokio::test]
async fn an_interrupted_asynchronous_read_is_resumed_rather_than_reported() {
    let mut frames = answering_asynchronously(vec![
        Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
        Ok(b"{\"a\":1}\n".to_vec()),
    ]);
    assert!(matches!(frames.next_frame_async().await, Some(Ok(frame)) if frame == "{\"a\":1}"));
    assert!(frames.next_frame_async().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_read_given_up_on_partway_through_a_frame_loses_none_of_it() {
    // A caller that stops waiting for a frame drops the read, and the next read
    // has to find the frame whole: the part that had arrived stays with the
    // reader rather than going with the read that was abandoned.
    let (mut near, far) = tokio::io::duplex(64);
    let mut frames = Frames::new(tokio::io::BufReader::new(far));
    near.write_all(b"{\"half\":").await.unwrap();

    let waited = tokio::time::timeout(Duration::from_secs(1), frames.next_frame_async()).await;
    assert!(waited.is_err(), "no frame had ended: {waited:?}");

    near.write_all(b"true}\n").await.unwrap();
    assert!(
        matches!(frames.next_frame_async().await, Some(Ok(frame)) if frame == "{\"half\":true}")
    );
}

/// An asynchronous pipe that remembers what reached it and when it was pushed.
#[derive(Debug, Default)]
struct RecordedAsynchronously {
    /// Everything written.
    bytes: Vec<u8>,
    /// How many times it was flushed.
    flushed: usize,
}

impl tokio::io::AsyncWrite for RecordedAsynchronously {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().bytes.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.flushed = this.flushed.saturating_add(1);
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl tokio::io::AsyncWrite for Refuses {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn each_frame_sent_asynchronously_goes_out_ended_by_one_newline_and_pushed() {
    let mut pipe = RecordedAsynchronously::default();
    let mut sending = Written::new(&mut pipe);
    sending.send_async("{\"a\":1}").await.unwrap();
    sending.send_async("{\"b\":2}").await.unwrap();

    assert_eq!(pipe.bytes, b"{\"a\":1}\n{\"b\":2}\n");
    assert_eq!(pipe.flushed, 2);
}

#[tokio::test]
async fn every_frame_is_refused_or_sent_the_same_way_blocking_or_asynchronously() {
    // What is refused is refused before a byte is written, whichever kind of
    // pipe is underneath; what is sent arrives as the same bytes.
    let over = "x".repeat(FRAME_BYTES + 1);
    let at = "x".repeat(FRAME_BYTES);
    let cases: [(&str, &str, Option<&str>); 4] = [
        ("a frame", "{\"a\":1}", None),
        ("a frame at the ceiling", &at, None),
        (
            "a frame carrying a newline",
            "{\"a\":1}\n{\"forged\":true}",
            Some("a frame crucible was about to send contains a newline"),
        ),
        (
            "a frame past the ceiling",
            &over,
            Some(
                "the program on the other end sent more than 1048576 bytes without ending a frame",
            ),
        ),
    ];
    for (name, frame, refusal) in cases {
        let mut blocking = Recorded::default();
        let sent_blocking = Written::new(&mut blocking).send(frame);
        let mut asynchronous = RecordedAsynchronously::default();
        let sent_asynchronously = Written::new(&mut asynchronous).send_async(frame).await;

        for (how, sent, bytes) in [
            ("blocking", sent_blocking, &blocking.bytes),
            ("asynchronously", sent_asynchronously, &asynchronous.bytes),
        ] {
            match (refusal, sent) {
                (None, Ok(())) => assert_eq!(
                    bytes.as_slice(),
                    format!("{frame}\n").as_bytes(),
                    "{name}, sent {how}"
                ),
                (Some(words), Err(err)) => {
                    assert_eq!(err.to_string(), words, "{name}, sent {how}");
                    assert!(err.never_left(), "{name}, sent {how}: {err:?}");
                    assert!(
                        bytes.is_empty(),
                        "{name}, sent {how}: {} bytes",
                        bytes.len()
                    );
                }
                (refusal, sent) => {
                    panic!("{name}, sent {how}: expected {refusal:?}, got {sent:?}")
                }
            }
        }
    }
}

#[tokio::test]
async fn a_pipe_that_will_not_take_a_frame_asynchronously_says_so() {
    let err = Written::new(Refuses)
        .send_async("{\"a\":1}")
        .await
        .unwrap_err();
    assert!(matches!(err, FrameError::Unreadable { .. }), "{err:?}");
    assert!(err.never_left(), "{err:?}");
}

#[tokio::test]
async fn what_was_sent_asynchronously_is_what_comes_back() {
    let (near, far) = tokio::io::duplex(4);
    let sent = ["{\"a\":1}", "{\"unicode\":\"héllo → ✓\"}", "{}"];
    let writing = async move {
        let mut sending = Written::new(near);
        for one in sent {
            sending.send_async(one).await.unwrap();
        }
    };
    let reading = async move {
        let mut frames = Frames::new(tokio::io::BufReader::new(far));
        let mut read = Vec::new();
        while let Some(one) = frames.next_frame_async().await {
            read.push(one.unwrap());
        }
        read
    };
    let ((), read) = tokio::join!(writing, reading);
    assert_eq!(read, sent.map(str::to_owned).to_vec());
}

#[test]
fn an_asynchronous_read_and_send_can_be_awaited_on_a_task_of_their_own() {
    // A runtime moves a spawned task between its threads, so a read or a send
    // that is not `Send` could only be awaited where it was made.
    fn sendable<T: Send>(_: &T) {}
    let (near, far) = tokio::io::duplex(4);
    let mut frames = Frames::new(tokio::io::BufReader::new(far));
    let mut sending = Written::new(near);
    sendable(&frames.next_frame_async());
    sendable(&sending.send_async("{}"));
}
