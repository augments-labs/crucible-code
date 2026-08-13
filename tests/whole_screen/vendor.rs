//! A provider, on this machine, saying what the case told it to say.
//!
//! The whole-screen cases could only ever watch crucible before it had anything
//! to answer, because the address a request goes to was a constant and no test
//! can reach the real one. `providers.<name>.baseUrl` is what changed that, and
//! `http` on a loopback address is exactly the case its parse allows: these
//! bytes reach no network.
//!
//! It speaks the smallest part of HTTP/1.1 that gets an event stream back to a
//! client: read a request until the headers end, ignore every word of it, write
//! a status line and the body. Nothing here is a general server, and nothing
//! here should grow into one — what it exists for is that crucible's own reader
//! is on the other end, and the thing under test is the screen rather than the
//! protocol.
//!
//! The deltas go out one at a time with a pause between them. Not for the frame
//! count — crucible draws once per delta however the bytes were chunked getting
//! here — but so that the reader on the other end is doing across a stream what
//! it does in a real turn, rather than meeting a whole answer already arrived.

use std::io::{BufRead as _, BufReader, Write as _};
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

/// A provider listening on this machine.
pub(crate) struct Vendor {
    /// Where crucible is told to send its requests.
    address: String,
    /// The thread answering them, joined on the way out.
    serving: Option<JoinHandle<()>>,
}

impl Vendor {
    /// Starts one that answers every request with `text`, a word at a time.
    ///
    /// Port zero, so two cases running at once cannot collide on one — which
    /// the address then carries, since it is the port the kernel picked.
    pub(crate) fn answering(text: &str) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port on this machine");
        let port = listener
            .local_addr()
            .expect("the port that was bound")
            .port();

        let body = stream(text);
        let serving = thread::spawn(move || {
            // Every connection, not one: crucible opens a fresh one per turn,
            // and a case that takes two turns would otherwise hang on the
            // second. The loop ends when the listener is dropped below.
            while let Ok((connection, _)) = listener.accept() {
                answer(connection, &body);
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
    /// own once the process it was serving is gone. `Window` kills crucible in
    /// its own `Drop`, and this runs after.
    fn drop(&mut self) {
        // Not joined. The thread is parked in `accept` on a listener nothing
        // else holds, and a test run does not need the port back before it
        // ends. Joining would be waiting for a connection that is never coming.
        drop(self.serving.take());
    }
}

/// Reads one request and writes the canned response back.
fn answer(mut connection: TcpStream, body: &[String]) {
    // Read to the end of the headers and no further. There is a body after
    // them, and none of it changes what this says back.
    let mut reading = BufReader::new(connection.try_clone().expect("a second handle"));
    let mut line = String::new();
    while reading.read_line(&mut line).is_ok_and(|read| read > 0) {
        if line == "\r\n" || line == "\n" {
            break;
        }
        line.clear();
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

/// `text` as the events Anthropic's Messages API streams for it.
///
/// One delta per word, keeping the spaces, so the answer arrives the way a real
/// one does rather than all at once.
fn stream(text: &str) -> Vec<String> {
    let mut events = vec![
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n"
            .to_owned(),
        "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"
            .to_owned(),
    ];

    events.extend(text.split_inclusive(' ').map(|word| {
        format!(
            "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
            serde_json::Value::from(word)
        )
    }));

    events.push("event: content_block_stop\ndata: {\"index\":0}\n\n".to_owned());
    events.push(
        "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n"
            .to_owned(),
    );
    events.push("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_owned());

    events
}
