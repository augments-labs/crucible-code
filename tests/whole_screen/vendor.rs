//! A provider, on this machine, saying what the case told it to say.
//!
//! The whole-screen cases could only ever watch crucible before it had anything
//! to answer, because the address a request goes to was a constant and no test
//! can reach the real one. `providers.<name>.baseUrl` is what changed that, and
//! `http` on a loopback address is exactly the case its parse allows: these
//! bytes reach no network.
//!
//! It speaks the smallest part of HTTP/1.1 that gets an event stream back to a
//! client: read a request, ignore every word of it, write a status line and the
//! body. Nothing here is a general server, and nothing here should grow into
//! one — what it exists for is that crucible's own reader is on the other end,
//! and the thing under test is the screen rather than the protocol.
//!
//! The whole request, though, and not only its headers. Nothing here reads the
//! body, but a socket closed with bytes still unread in it is reset rather than
//! ended, and the reset throws away whatever of the response the client had not
//! taken yet. That is a truncated answer on a busy machine and a whole answer on
//! an idle one, which is the shape of a case that fails once a fortnight in
//! somebody else's pull request.
//!
//! The deltas go out one at a time with a pause between them. Not for the frame
//! count — crucible draws once per delta however the bytes were chunked getting
//! here — but so that the reader on the other end is doing across a stream what
//! it does in a real turn, rather than meeting a whole answer already arrived.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How long between two deltas.
///
/// An answer arrives here the way one arrives from a vendor: in pieces, over
/// time, rather than as a single write that crucible could have drawn in one
/// frame. Small, because the case that needs the most deltas needs several
/// hundred of them and a test measured in seconds is one people stop running.
const BETWEEN: Duration = Duration::from_millis(5);

/// The event that opens a message. Nothing on this side reads what it carries.
const STARTED: &str =
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n";

/// The event that closes the one content block each of these answers has.
const ENDED: &str = "event: content_block_stop\ndata: {\"index\":0}\n\n";

/// The event that closes the message.
const STOPPED: &str = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

/// The keep-alive the same API sends, which draws nothing.
const PINGED: &str = "event: ping\ndata: {\"type\":\"ping\"}\n\n";

/// How many keep-alives hold a turn open once its answer has arrived whole.
///
/// Enough at [`BETWEEN`] to outlast the steps a case takes after that, and no
/// more: the thread stops the moment crucible is gone, because the write it is
/// on fails and `answer` returns.
const HOLDING: usize = 2_000;

/// What a case with one call in it names that call.
const ONE: &str = "toolu_1";

/// A provider listening on this machine.
pub(crate) struct Vendor {
    /// Where crucible is told to send its requests.
    address: String,
    /// The thread answering them, joined on the way out.
    serving: Option<JoinHandle<()>>,
}

impl Vendor {
    /// Starts one that answers every request with `text`, a word at a time.
    pub(crate) fn answering(text: &str) -> Self {
        Self::serving(vec![stream(text)])
    }

    /// The same answer, arriving in one delta instead of many.
    ///
    /// How the wire happened to be cut is the vendor's business and none of
    /// the screen's, so a case that holds the text still and varies only that
    /// is asking whether the reader agrees. It is the one arrangement where
    /// nothing can be held across a delta, which makes it the answer the
    /// chunked one has to match rather than merely another sample of it.
    pub(crate) fn answering_whole(text: &str) -> Self {
        Self::serving(vec![whole(text)])
    }

    /// Starts with ordinary answers, then returns a structured recap and the
    /// answer after it.
    pub(crate) fn recapping_after(before: &[&str], note: &str, after: Option<&str>) -> Self {
        let mut bodies: Vec<Vec<String>> = before.iter().map(|text| stream(text)).collect();
        bodies.push(stream(&recap(note)));
        if let Some(after) = after {
            bodies.push(stream(after));
        }
        Self::serving(bodies)
    }

    /// Starts one whose first answer asks for `tool` with `input`, and whose
    /// second is `text`.
    ///
    /// Two answers, because a turn with a call in it takes two requests: the
    /// one that came back asking, and the one sent with what the call
    /// returned. They arrive in that order, each on its own connection.
    pub(crate) fn calling(tool: &str, input: &str, text: &str) -> Self {
        Self::serving(vec![asking(tool, input, ONE), stream(text)])
    }

    /// The same, except the second answer holds the turn open behind `text`.
    ///
    /// A case that needs a turn still running once its answer is on screen
    /// cannot get there by making the answer long enough to still be arriving:
    /// what is drawn then is however far the stream happened to have got, which
    /// is the pace the deltas arrive at rather than anything the case is about,
    /// and the picture moves with the machine. Letting the answer finish and
    /// holding the message open afterwards pins the screen and still leaves the
    /// turn where the case wants it.
    pub(crate) fn calling_then_holding(tool: &str, input: &str, text: &str) -> Self {
        Self::serving(vec![asking(tool, input, ONE), holding(text)])
    }

    /// Starts one that asks for each of `calls` in turn, then answers `text`.
    ///
    /// One body per call and one for the answer, which is what a turn with
    /// several calls in it actually costs in requests. Each call is named
    /// after its place in the sequence, so the results come back told apart.
    pub(crate) fn calling_each(calls: &[(&str, String)], text: &str) -> Self {
        let mut bodies: Vec<Vec<String>> = calls
            .iter()
            .enumerate()
            .map(|(at, (tool, input))| asking(tool, input, &format!("toolu_{}", at + 1)))
            .collect();
        bodies.push(stream(text));

        Self::serving(bodies)
    }

    /// Starts one that answers each request with the next of `bodies`.
    ///
    /// Port zero, so two cases running at once cannot collide on one — which
    /// the address then carries, since it is the port the kernel picked.
    ///
    /// A request past the last body is answered with that last body again. A
    /// run that asked once more than the case wrote for then fails on what is
    /// on its screen, which says what happened, rather than hanging on a socket
    /// nobody is answering.
    fn serving(bodies: Vec<Vec<String>>) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port on this machine");
        let port = listener
            .local_addr()
            .expect("the port that was bound")
            .port();

        let serving = thread::spawn(move || {
            // Every connection, not one: crucible opens a fresh one per
            // request, and a case that takes two would otherwise hang on the
            // second. The loop ends when the listener is dropped below.
            let mut asked = 0;
            while let Ok((connection, _)) = listener.accept() {
                if let Some(body) = bodies.get(asked).or_else(|| bodies.last()) {
                    answer(connection, body);
                }
                asked += 1;
            }
        });

        Self {
            address: format!("http://127.0.0.1:{port}/v1/messages"),
            serving: Some(serving),
        }
    }

    /// What `providers.anthropic.baseUrl` is set to for this case.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for Vendor {
    /// The listener is inside the thread, so dropping this has to end the
    /// thread to close the port — which it does by the thread ending on its
    /// own once the process it was serving is gone. `Watched` kills crucible in
    /// its own `Drop`, and this runs after.
    fn drop(&mut self) {
        // Not joined. The thread is parked in `accept` on a listener nothing
        // else holds, and a test run does not need the port back before it
        // ends. Joining would be waiting for a connection that is never coming.
        drop(self.serving.take());
    }
}

/// Reads one request whole and writes the canned response back.
fn answer(mut connection: TcpStream, body: &[String]) {
    // Read to the end of the headers, keeping the one field that says how much
    // follows them.
    let mut reading = BufReader::new(connection.try_clone().expect("a second handle"));
    let mut line = String::new();
    let mut length = 0;
    while reading.read_line(&mut line).is_ok_and(|read| read > 0) {
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(said) = header(&line, "content-length") {
            length = said.trim().parse().unwrap_or(0);
        }
        line.clear();
    }

    // Then the request itself, which nothing here reads a word of — and which
    // has to be read anyway. Closing a socket with bytes still sitting unread
    // in it resets the connection rather than ending it, and a reset throws
    // away whatever of the response the client had not taken yet. A request
    // left undrained is an answer that arrives cut in half, on a machine busy
    // enough that the client was still writing while this wrote back.
    let mut request = vec![0; length];
    if reading.read_exact(&mut request).is_err() {
        return;
    }

    let sent = connection.write_all(
        b"HTTP/1.1 200 OK\r\n\
          content-type: text/event-stream\r\n\
          connection: close\r\n\
          \r\n",
    );
    if sent.is_err() {
        return;
    }

    for event in body {
        if connection.write_all(event.as_bytes()).is_err() {
            return;
        }
        let _ = connection.flush();
        thread::sleep(BETWEEN);
    }
}

/// The value of the `name` header, where `line` is that header.
///
/// Without regard to case, because which case a field name is written in is the
/// client's business and not something a case here should be pinning.
fn header<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let (said, value) = line.split_once(':')?;
    said.trim().eq_ignore_ascii_case(name).then_some(value)
}

/// `text` as the events Anthropic's Messages API streams for it.
///
/// One delta per word, keeping the spaces, so the answer arrives the way a real
/// one does rather than all at once.
fn stream(text: &str) -> Vec<String> {
    let mut events = opening(text);

    events.push(ENDED.to_owned());
    events.push(stopped("end_turn"));
    events.push(STOPPED.to_owned());

    events
}

/// The same events, with the message left open behind them.
///
/// Keep-alives rather than silence: they are what the API itself sends through
/// a quiet stretch, crucible draws nothing for one, and a connection with bytes
/// still moving on it is not one anything on either end mistakes for a stall.
fn holding(text: &str) -> Vec<String> {
    let mut events = opening(text);

    events.extend((0..HOLDING).map(|_| PINGED.to_owned()));

    events
}

/// The same events as [`stream`], with the answer whole in one delta.
fn whole(text: &str) -> Vec<String> {
    let mut events = carrying([text]);

    events.push(ENDED.to_owned());
    events.push(stopped("end_turn"));
    events.push(STOPPED.to_owned());

    events
}

/// The events that open a message and carry `text` into it, a word at a time.
fn opening(text: &str) -> Vec<String> {
    carrying(text.split_inclusive(' '))
}

/// The events that open a message and carry `pieces` of it in, in order.
fn carrying<'a>(pieces: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut events = vec![
        STARTED.to_owned(),
        "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"
            .to_owned(),
    ];

    events.extend(pieces.into_iter().map(|word| {
        format!(
            "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
            serde_json::Value::from(word)
        )
    }));

    events
}

/// A complete structured recap containing `note`.
fn recap(note: &str) -> String {
    format!(
        "## Goal\n{note}\n\n## Constraints & Preferences\n(none)\n\n## Progress\n### Done\n(none)\n\n### In Progress\n{note}\n\n### Blocked\n(none)\n\n## Decisions\n(none)\n\n## Next Steps\n1. {note}\n\n## Critical Context\n(none)"
    )
}

/// A call for `tool` with `input` under `id`, as the same API streams one.
///
/// The id is the caller's, because a turn tells its results apart by it: two
/// calls sharing one would be one call answered twice everywhere downstream.
///
/// The arguments go out as one delta of the text that spells them rather than
/// as an object, because that is what the wire does: crucible reads them back
/// as text and parses them once, when the block ends.
fn asking(tool: &str, input: &str, id: &str) -> Vec<String> {
    vec![
        STARTED.to_owned(),
        format!(
            "event: content_block_start\ndata: {{\"index\":0,\"content_block\":{{\"type\":\
             \"tool_use\",\"id\":{},\"name\":{},\"input\":{{}}}}}}\n\n",
            serde_json::Value::from(id),
            serde_json::Value::from(tool)
        ),
        format!(
            "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\
             \"input_json_delta\",\"partial_json\":{}}}}}\n\n",
            serde_json::Value::from(input)
        ),
        ENDED.to_owned(),
        stopped("tool_use"),
        STOPPED.to_owned(),
    ]
}

/// The event saying why the model stopped, and what it spent getting there.
fn stopped(reason: &str) -> String {
    format!(
        "event: message_delta\ndata: {{\"delta\":{{\"stop_reason\":\"{reason}\"}},\"usage\":\
         {{\"output_tokens\":4}}}}\n\n"
    )
}
