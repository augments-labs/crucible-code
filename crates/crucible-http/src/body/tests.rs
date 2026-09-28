use std::time::Duration;

use hyper::Request;
use hyper::body::Incoming;
use hyper::client::conn::http1::{SendRequest, handshake};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
use tokio::time::Instant;

use super::{
    BodyError, Chunk, Chunks, End, MAX_REFUSAL, QUIET, REFUSAL_WAIT, read_limited, read_refusal,
};

/// The server's end of one exchange, over an in-memory pipe, so a test says
/// exactly when each byte of the body arrives. The client's handle is kept so
/// the connection lives as long as the test holds this.
struct Peer {
    far: DuplexStream,
    _asking: SendRequest<String>,
}

impl Peer {
    async fn send(&mut self, bytes: &[u8]) {
        self.far.write_all(bytes).await.unwrap();
    }
}

/// Answers one request with `head`, read by hyper's own client connection,
/// and hands back the response's body and the server's end.
async fn answered(head: &str) -> (Incoming, Peer) {
    let (near, mut far) = duplex(1 << 20);
    let (mut asking, connection) = handshake(TokioIo::new(near)).await.unwrap();
    tokio::spawn(connection);
    let answer = asking.send_request(Request::new(String::new()));
    let mut seen = Vec::new();
    while !seen.ends_with(b"\r\n\r\n") {
        let mut byte = [0; 1];
        far.read_exact(&mut byte).await.unwrap();
        seen.extend_from_slice(&byte);
    }
    far.write_all(head.as_bytes()).await.unwrap();
    let body = answer.await.unwrap().into_body();
    (
        body,
        Peer {
            far,
            _asking: asking,
        },
    )
}

/// A head promising a body of `length` bytes.
fn promising(length: usize) -> String {
    format!("HTTP/1.1 200 OK\r\ncontent-length: {length}\r\n\r\n")
}

/// A body of exactly `length` bytes, already sent whole.
async fn sent_whole(length: usize) -> (Incoming, Peer) {
    let (body, mut peer) = answered(&promising(length)).await;
    peer.send(&body_of(length)).await;
    (body, peer)
}

fn body_of(length: usize) -> Vec<u8> {
    b"abcdefghijklmnopqrstuvwxyz"
        .iter()
        .cycle()
        .take(length)
        .copied()
        .collect()
}

const LIMIT: usize = 1024;
const WITHIN: Duration = Duration::from_mins(2);

/// What `reading` gave, if it ended within an hour, which is far past every
/// deadline here.
async fn ended<T>(reading: impl Future<Output = T>) -> T {
    let ended = tokio::time::timeout(Duration::from_hours(1), reading).await;
    ended.expect("the read never ended")
}

/// The next of `chunks`, which is to come within two quiet intervals.
async fn next(chunks: &mut Chunks) -> Option<Result<Chunk, BodyError>> {
    let next = tokio::time::timeout(QUIET * 2, chunks.next()).await;
    next.expect("neither bytes nor a tick came")
}

#[tokio::test]
async fn a_limited_read_accepts_exactly_its_limit() {
    let (body, _peer) = sent_whole(LIMIT).await;
    let read = ended(read_limited(body, LIMIT, WITHIN)).await.unwrap();
    assert_eq!(read, body_of(LIMIT));
}

#[tokio::test]
async fn a_limited_read_refuses_one_byte_over_its_limit() {
    let (body, _peer) = sent_whole(LIMIT + 1).await;
    let read = ended(read_limited(body, LIMIT, WITHIN)).await;
    assert!(matches!(read, Err(BodyError::TooLarge)), "{read:?}");
}

/// The byte past the limit is enough to refuse: the read does not wait for a
/// rest that never comes, nor for its deadline.
#[tokio::test(start_paused = true)]
async fn a_limited_read_refuses_as_soon_as_the_byte_over_arrives() {
    let (body, mut peer) = answered(&promising(LIMIT + 100)).await;
    peer.send(&body_of(LIMIT + 1)).await;
    let started = Instant::now();
    let read = ended(read_limited(body, LIMIT, WITHIN)).await;
    assert!(matches!(read, Err(BodyError::TooLarge)), "{read:?}");
    assert_eq!(started.elapsed(), Duration::ZERO);
}

/// Sends `peer` one byte a second, for ever.
fn trickle(mut peer: Peer) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if peer.far.write_all(b"x").await.is_err() {
                return;
            }
        }
    })
}

/// The deadline is the whole read's: a peer that sends a byte a second
/// never waits out a gap, and is given up on all the same.
#[tokio::test(start_paused = true)]
async fn a_limited_read_gives_up_at_its_deadline_however_the_bytes_trickle() {
    let (body, peer) = answered(&promising(LIMIT)).await;
    let trickling = trickle(peer);
    let started = Instant::now();
    let read = ended(read_limited(body, LIMIT, Duration::from_secs(10))).await;
    assert!(matches!(read, Err(BodyError::Deadline)), "{read:?}");
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    trickling.abort();
}

/// A body whose peer closes before the length it promised is incomplete, not
/// a short body.
#[tokio::test]
async fn a_body_its_peer_closes_before_its_end_is_incomplete() {
    let (body, mut peer) = answered(&promising(100)).await;
    peer.send(b"0123456789").await;
    drop(peer);
    let read = ended(read_limited(body, LIMIT, WITHIN)).await;
    assert!(matches!(read, Err(BodyError::Incomplete(_))), "{read:?}");
}

#[tokio::test]
async fn a_refusal_of_exactly_its_bound_is_whole() {
    let (body, _peer) = sent_whole(MAX_REFUSAL).await;
    let refusal = ended(read_refusal(body)).await;
    assert!(matches!(refusal.end, End::Whole), "{:?}", refusal.end);
    assert_eq!(refusal.said, body_of(MAX_REFUSAL));
}

/// One byte past the bound is kept from nobody but marked: what is kept is
/// the bound's worth, and the read says there was more.
#[tokio::test]
async fn a_refusal_one_byte_over_its_bound_is_cut() {
    let (body, _peer) = sent_whole(MAX_REFUSAL + 1).await;
    let refusal = ended(read_refusal(body)).await;
    assert!(matches!(refusal.end, End::Cut), "{:?}", refusal.end);
    assert_eq!(refusal.said, body_of(MAX_REFUSAL));
}

/// A refusal whose bound filled and whose peer then held on is given up at
/// the deadline, keeping what arrived, and says it failed rather than ended.
#[tokio::test(start_paused = true)]
async fn a_refusal_that_fills_its_bound_and_stalls_keeps_what_came() {
    let (body, mut peer) = answered(&promising(MAX_REFUSAL + 1)).await;
    peer.send(&body_of(MAX_REFUSAL)).await;
    let started = Instant::now();
    let refusal = ended(read_refusal(body)).await;
    let stalled = matches!(refusal.end, End::Failed(BodyError::Deadline));
    assert!(stalled, "{:?}", refusal.end);
    assert_eq!(started.elapsed(), REFUSAL_WAIT);
    assert_eq!(refusal.said, body_of(MAX_REFUSAL));
}

#[tokio::test(start_paused = true)]
async fn a_refusal_that_trickles_is_given_up_at_ten_seconds() {
    let (body, peer) = answered(&promising(MAX_REFUSAL)).await;
    let trickling = trickle(peer);
    let started = Instant::now();
    let refusal = ended(read_refusal(body)).await;
    let stalled = matches!(refusal.end, End::Failed(BodyError::Deadline));
    assert!(stalled, "{:?}", refusal.end);
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    assert!(
        !refusal.said.is_empty() && refusal.said.len() <= 10,
        "{}",
        refusal.said.len()
    );
    trickling.abort();
}

/// A pause inside the deadline is only a pause.
#[tokio::test(start_paused = true)]
async fn a_refusal_that_pauses_before_its_rest_is_read_whole() {
    let (body, mut peer) = answered(&promising(10)).await;
    peer.send(b"01234").await;
    let rest = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(9)).await;
        peer.send(b"56789").await;
        peer
    });
    let refusal = ended(read_refusal(body)).await;
    assert!(matches!(refusal.end, End::Whole), "{:?}", refusal.end);
    assert_eq!(refusal.said, b"0123456789");
    drop(rest.await.unwrap());
}

/// Each wait that finds nothing is a tick of the paused clock, a quarter of a
/// second of it, with no real wait behind it; bytes that come after the ticks
/// arrive whole, and the end of the body after them.
#[tokio::test(start_paused = true)]
async fn a_quiet_body_ticks_on_the_paused_clock() {
    let (body, mut peer) = answered(&promising(5)).await;
    let mut chunks = Chunks::new(body);
    let started = Instant::now();
    let wall = std::time::Instant::now();
    for tick in 1..=40 {
        let next = next(&mut chunks).await;
        assert!(matches!(next, Some(Ok(Chunk::Quiet))), "{tick}: {next:?}");
        assert_eq!(started.elapsed(), QUIET * tick);
    }
    assert!(
        wall.elapsed() < Duration::from_secs(5),
        "{:?}",
        wall.elapsed()
    );
    peer.send(b"hello").await;
    let Some(Ok(Chunk::Data(data))) = next(&mut chunks).await else {
        panic!("no data after the ticks");
    };
    assert_eq!(&data[..], b"hello");
    assert_eq!(format!("{:?}", Chunk::Data(data)), "Data(\"5 bytes\")");
    assert!(next(&mut chunks).await.is_none());
}

/// A stream has no deadline: a peer quiet for ten minutes is still waited
/// for.
#[tokio::test(start_paused = true)]
async fn a_stream_quiet_for_ten_minutes_is_still_read() {
    let (body, mut peer) = answered(&promising(5)).await;
    let mut chunks = Chunks::new(body);
    let started = Instant::now();
    while started.elapsed() < Duration::from_mins(10) {
        let next = next(&mut chunks).await;
        assert!(matches!(next, Some(Ok(Chunk::Quiet))), "{next:?}");
    }
    peer.send(b"hello").await;
    let next = next(&mut chunks).await;
    assert!(
        matches!(&next, Some(Ok(Chunk::Data(data))) if &data[..] == b"hello"),
        "{next:?}"
    );
}

#[tokio::test]
async fn a_stream_its_peer_closes_before_its_end_is_incomplete() {
    let (body, mut peer) = answered(&promising(100)).await;
    peer.send(b"0123456789").await;
    let mut chunks = Chunks::new(body);
    let first = next(&mut chunks).await;
    assert!(
        matches!(&first, Some(Ok(Chunk::Data(data))) if &data[..] == b"0123456789"),
        "{first:?}"
    );
    drop(peer);
    let next = next(&mut chunks).await;
    assert!(
        matches!(next, Some(Err(BodyError::Incomplete(_)))),
        "{next:?}"
    );
}

/// A refusal may echo a credential it was sent, so what it says is never in
/// its `Debug`: only how much was kept and how the reading ended.
#[tokio::test]
async fn a_refusal_shows_how_much_it_kept_but_never_what() {
    let echoed = "Bearer not-a-real-token";
    let (body, mut peer) = answered(&promising(echoed.len())).await;
    peer.send(echoed.as_bytes()).await;
    let refusal = ended(read_refusal(body)).await;
    assert_eq!(refusal.said, echoed.as_bytes());
    let shown = format!("{refusal:?}");
    assert!(!shown.contains("not-a-real-token"), "{shown}");
    let bytes = format!("{:?}", echoed.as_bytes());
    assert!(!shown.contains(&bytes[1..bytes.len() - 1]), "{shown}");
    assert_eq!(
        shown,
        format!("Refusal {{ said: \"{} bytes\", end: Whole }}", echoed.len())
    );
}
