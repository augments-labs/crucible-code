//! A cancel ends a response that keeps talking without saying anything.
//!
//! Each test is a loopback source that answers once and then, every 100 ms for
//! as long as it is allowed to, sends lines that never make an event, or more
//! of one line that never ends. Every one of those sends is something
//! arriving, so the quiet wait below the stream never runs out; what ends the
//! read is the cancel, or nothing.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crucible_credentials::{ApiKey, Header, HeaderKey, Outgoing};
use crucible_http::{Http, Lookups, Poison, ProxyEnv, Tls};
use crucible_models::{Provider, Request, RequestPurpose};
use crucible_types::{Message, Transcript};

use super::*;
use crate::{Anthropic, Endpoint, HttpTurns, Transport};

/// How long a cancelled read may take to come back.
const PROMPTLY: Duration = Duration::from_secs(1);

/// How long a test waits before calling a read that has not come back stuck.
const STUCK: Duration = Duration::from_secs(5);

/// How often the source repeats its lines.
const BEAT: Duration = Duration::from_millis(100);

/// How long the source talks before the cancel is raised: long enough that
/// the read is waiting under the beat when it is.
const TALKING: Duration = Duration::from_millis(400);

/// Means nothing by any event, which none of these sources sends.
#[derive(Default)]
struct Unheard;

impl Wire for Unheard {
    const PROVIDER: &'static str = "loopback";

    fn deltas(&mut self, _event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        Ok(Vec::new())
    }
}

/// A source that answers a stream and then sends `beat` every [`BEAT`] until
/// the connection is closed or [`STUCK`] has passed twice over.
fn beating(beat: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        heard(&stream);
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
        if stream.write_all(head.as_bytes()).is_err() {
            return;
        }
        let until = Instant::now() + STUCK * 2;
        while Instant::now() < until {
            let chunk = format!("{:x}\r\n{beat}\r\n", beat.len());
            if stream.write_all(chunk.as_bytes()).is_err() || stream.flush().is_err() {
                return;
            }
            thread::sleep(BEAT);
        }
    });
    format!("http://{address}/v1/messages")
}

/// Reads one request head and the body it names.
fn heard(stream: &TcpStream) {
    let mut asked = BufReader::new(stream);
    let mut line = String::new();
    let mut body = 0;
    while asked.read_line(&mut line).unwrap_or(0) > 0 {
        if line.trim().is_empty() {
            break;
        }
        let said = line.to_ascii_lowercase();
        if let Some(length) = said.strip_prefix("content-length:") {
            body = length.trim().parse().unwrap_or(0);
        }
        line.clear();
    }
    let _ = asked.read_exact(&mut vec![0_u8; body]);
}

fn shared() -> HttpTurns {
    let tls = Tls::new().expect("the pinned TLS configuration to build");
    let poison = Poison::default();
    let target = Lookups::poisoned(NonZeroUsize::MIN, &poison);
    let proxy = Lookups::plain(NonZeroUsize::MIN);
    HttpTurns::new(Http::new(&tls, target, proxy, ProxyEnv::capture()))
}

/// The open response at `url`, read the way a turn reads one.
async fn opened(transport: &HttpTurns, url: &str, cancel: &Cancel) -> Box<dyn DeltaStream> {
    let response = transport
        .post(url, &mut Outgoing::new(), "{}".to_owned(), cancel)
        .await
        .unwrap();
    Box::new(Response::<Unheard>::new(
        response.into_reader(),
        cancel.clone(),
        Redactions::default(),
    ))
}

/// Raises `cancel` once the source has been talking for [`TALKING`], and says
/// when it did.
fn raised_later(cancel: &Cancel) -> mpsc::Receiver<Instant> {
    let (raised, when) = mpsc::channel();
    let cancel = cancel.clone();
    thread::spawn(move || {
        thread::sleep(TALKING);
        cancel.request();
        let _ = raised.send(Instant::now());
    });
    when
}

/// Reads `stream` once, and asserts it said it was cancelled within
/// [`PROMPTLY`] of the cancel being raised.
async fn cancelled_promptly(stream: &mut dyn DeltaStream, raised: &mpsc::Receiver<Instant>) {
    let read = tokio::time::timeout(STUCK, stream.next()).await;
    let returned = Instant::now();

    let read = read.expect("the read was still waiting under the beat after the cancel");
    assert!(
        matches!(read, Some(Ok(Delta::Stopped(StopReason::Cancelled)))),
        "the read did not end as cancelled: {read:?}"
    );
    let raised = raised.recv_timeout(STUCK).unwrap();
    let took = returned.saturating_duration_since(raised);
    assert!(took < PROMPTLY, "the read took {took:?} to see the cancel");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_ends_a_stream_of_comments_within_a_second() {
    let url = beating(": heartbeat\n\n");
    let transport = shared();
    let cancel = Cancel::new();
    let mut stream = opened(&transport, &url, &cancel).await;
    let raised = raised_later(&cancel);

    cancelled_promptly(stream.as_mut(), &raised).await;
}

/// `id` is a field nothing here reads, so an event of only that dispatches
/// nothing, the same as a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_ends_a_stream_of_ignored_fields_as_it_does_comments() {
    let url = beating("id: 1\n\n");
    let transport = shared();
    let cancel = Cancel::new();
    let mut stream = opened(&transport, &url, &cancel).await;
    let raised = raised_later(&cancel);

    cancelled_promptly(stream.as_mut(), &raised).await;
}

/// The read a compaction does: a recap asked of a provider, its stream read
/// under the cancel the request was sent with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_ends_a_recap_under_comments_within_a_second() {
    let url = beating(": heartbeat\n\n");
    let provider = Anthropic::at(
        Endpoint::parse(&url).unwrap(),
        Box::new(HeaderKey::new(
            ApiKey::new("loopback-key"),
            Header::bare("x-api-key"),
        )),
        Box::new(shared()),
    );
    let mut transcript = Transcript::new();
    transcript.push(Message::said("summarize")).unwrap();
    let request = Request {
        purpose: RequestPurpose::Recap,
        model: "claude-test",
        transcript: &transcript,
        tools: &[],
        attached: &[],
        max_tokens: 1024,
        system: None,
        effort: None,
        prompt_cache: None,
    };
    let cancel = Cancel::new();
    let mut stream = provider.stream(request, &cancel).await.unwrap();
    let raised = raised_later(&cancel);

    cancelled_promptly(stream.as_mut(), &raised).await;
}

/// Lines that do go into an event, sent forever with no blank line to finish
/// it: nothing is ever dispatched, the same as a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_ends_an_event_that_never_finishes_as_it_does_comments() {
    let url = beating("event: ping\ndata: {}\n");
    let transport = shared();
    let cancel = Cancel::new();
    let mut stream = opened(&transport, &url, &cancel).await;
    let raised = raised_later(&cancel);

    cancelled_promptly(stream.as_mut(), &raised).await;
}

/// One line sent a few bytes at a time and never ended: no line is ever read
/// whole, so nothing goes into an event, the same as a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_ends_a_line_that_never_ends_as_it_does_comments() {
    let url = beating("data: x");
    let transport = shared();
    let cancel = Cancel::new();
    let mut stream = opened(&transport, &url, &cancel).await;
    let raised = raised_later(&cancel);

    cancelled_promptly(stream.as_mut(), &raised).await;
}
