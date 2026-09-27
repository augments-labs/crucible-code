//! A loopback vendor that answers each request from a script, holds an answer
//! back until the trial lets it go, and looks at the runtime while it waits.

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crucible_app::runtime::WORKERS;
use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::Provider;
use crucible_provider::{Anthropic, Endpoint, HttpTurns, OpenAi};
use serde_json::{Value, json};
use tokio::runtime::Handle;

/// The key every request is sent under, which no vendor ever sees.
const KEY: &str = "trial-only-api-key";

/// How long anything here waits before it calls the trial broken.
pub(crate) const WAIT: Duration = Duration::from_secs(10);

/// The one wire each agent in the trial is spoken to over.
#[derive(Clone, Copy)]
pub(crate) enum Wire {
    /// Messages, as Anthropic streams them.
    Claude,
    /// Responses, as `OpenAI` streams them.
    OpenAi,
}

impl Wire {
    /// The model an agent on this wire is aimed at.
    pub(crate) const fn model(self) -> &'static str {
        match self {
            Self::Claude => "claude-fable-5-1",
            Self::OpenAi => "gpt-6-astra",
        }
    }

    /// Where on the loopback address a request for this wire is sent.
    const fn path(self) -> &'static str {
        match self {
            Self::Claude => "messages",
            Self::OpenAi => "responses",
        }
    }

    /// A provider speaking this wire to `endpoint`, through the application's
    /// shared client.
    pub(crate) fn provider(self, endpoint: Endpoint, http: HttpTurns) -> Box<dyn Provider> {
        match self {
            Self::Claude => Box::new(Anthropic::at(
                endpoint,
                Box::new(HeaderKey::new(ApiKey::new(KEY), Header::bare("x-api-key"))),
                Box::new(http),
            )),
            Self::OpenAi => Box::new(OpenAi::at(
                endpoint,
                Box::new(HeaderKey::new(ApiKey::new(KEY), Header::bearer())),
                Box::new(http),
            )),
        }
    }

    /// One whole streamed answer: `text`, then every call in `calls`, having
    /// produced `spent` output tokens.
    pub(crate) fn answer(self, text: &str, calls: &[Call], spent: u64) -> String {
        match self {
            Self::Claude => claude(text, calls, spent),
            Self::OpenAi => openai(text, calls, spent),
        }
    }
}

/// One call a scripted answer asks for.
pub(crate) struct Call {
    /// The provider's identity for it.
    pub id: String,
    /// The tool, as the request advertised it.
    pub name: String,
}

impl Call {
    /// A call `id` to the tool called `name`.
    pub(crate) fn to(id: &str, name: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

fn claude(text: &str, calls: &[Call], spent: u64) -> String {
    let mut events = vec![json!({"type":"message_start","message":{"id":"trial-message"}})];
    let mut blocks = vec![json!({"type":"text","text":text})];
    blocks.extend(
        calls
            .iter()
            .map(|call| json!({"type":"tool_use","id":call.id,"name":call.name,"input":{}})),
    );
    for (index, block) in blocks.into_iter().enumerate() {
        events.push(json!({"type":"content_block_start","index":index,"content_block":block}));
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    let stop = if calls.is_empty() {
        "end_turn"
    } else {
        "tool_use"
    };
    events.push(json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":{"output_tokens":spent}}));
    events.push(json!({"type":"message_stop"}));
    streamed(&events)
}

fn openai(text: &str, calls: &[Call], spent: u64) -> String {
    let phase = if calls.is_empty() {
        "final_answer"
    } else {
        "commentary"
    };
    let mut output = vec![json!({
        "type": "message", "id": "msg-trial", "role": "assistant", "status": "completed",
        "phase": phase, "content": [{"type":"output_text","text":text,"annotations":[]}],
    })];
    output.extend(calls.iter().map(|call| {
        json!({
            "type": "function_call", "id": format!("fc-{}", call.id), "call_id": call.id,
            "name": call.name, "arguments": "{}", "status": "completed",
        })
    }));
    let mut events = vec![
        json!({"type":"response.created","response":{"id":"trial-response","status":"in_progress","output":[]}}),
    ];
    for (index, item) in output.iter().enumerate() {
        events.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
        events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
    }
    events.push(json!({"type":"response.completed","response":{
        "id": "trial-response", "status": "completed", "output": output,
        "usage": {"input_tokens": 1, "output_tokens": spent, "total_tokens": spent + 1},
    }}));
    streamed(&events)
}

/// Server-sent events, each named for its own type.
fn streamed(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        let named = event
            .get("type")
            .and_then(Value::as_str)
            .expect("every scripted event is typed");
        write!(body, "event: {named}\ndata: {event}\n\n").expect("writing into a string");
    }
    body
}

/// The names of the tools a captured request advertised.
pub(crate) fn advertised(request: &Value) -> Vec<String> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// A latch one side of the trial opens and the other waits on, boundedly.
#[derive(Clone, Default)]
pub(crate) struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    /// Lets every waiter through, now and from now on.
    pub(crate) fn open(&self) {
        let (open, opened) = &*self.0;
        *open.lock().expect("the gate") = true;
        opened.notify_all();
    }

    /// Whether the gate opened within `within`.
    pub(crate) fn opened(&self, within: Duration) -> bool {
        let (open, opened) = &*self.0;
        let open = open.lock().expect("the gate");
        let (open, _) = opened
            .wait_timeout_while(open, within, |open| !*open)
            .expect("the gate");
        *open
    }
}

/// Whether every worker of the runtime can be had at once.
///
/// The probe puts one task per worker on the runtime and has each wait for
/// all the others. They can only all arrive if no worker is held by anything
/// else, so a tool call, a stream or a status task that held a worker for as
/// long as [`WAIT`] after the probe looked is what makes it fail. One probe runs at a time:
/// two at once would each hold workers the other is waiting for.
#[derive(Clone)]
pub(crate) struct Probe {
    runtime: Handle,
    seen: Arc<Mutex<Vec<bool>>>,
}

/// Holds the probes of both agents' vendors to one at a time.
static ONE_PROBE: Mutex<()> = Mutex::new(());

impl Probe {
    /// A probe of `runtime`'s workers.
    pub(crate) fn of(runtime: Handle) -> Self {
        Self {
            runtime,
            seen: Arc::default(),
        }
    }

    /// Every look it took, in order: `true` where every worker was free.
    pub(crate) fn seen(&self) -> Vec<bool> {
        self.seen.lock().expect("the probe's looks").clone()
    }

    /// Looks once, and remembers what it saw.
    fn look(&self) {
        let _one = ONE_PROBE.lock().expect("the probe's turn");
        let met = Arc::new((Mutex::new(0_usize), Condvar::new()));
        let waiting: Vec<_> = (0..WORKERS)
            .map(|_| {
                let met = Arc::clone(&met);
                self.runtime.spawn(async move {
                    let (arrived, all) = &*met;
                    let mut arrived = arrived.lock().expect("the count");
                    *arrived += 1;
                    all.notify_all();
                    let (arrived, _) = all
                        .wait_timeout_while(arrived, WAIT, |arrived| *arrived < WORKERS)
                        .expect("the count");
                    *arrived >= WORKERS
                })
            })
            .collect();
        let free = waiting
            .into_iter()
            .all(|task| self.runtime.block_on(task).expect("a probe task answers"));
        self.seen.lock().expect("the probe's looks").push(free);
    }
}

/// How one request is answered.
pub(crate) struct Reply {
    answer: Box<dyn FnOnce(&Value) -> String + Send>,
    after: Option<Gate>,
    probe: Option<Probe>,
}

impl Reply {
    /// Answers with what `answer` makes of the request.
    pub(crate) fn with(answer: impl FnOnce(&Value) -> String + Send + 'static) -> Self {
        Self {
            answer: Box::new(answer),
            after: None,
            probe: None,
        }
    }

    /// Always answers `body`, whatever was asked.
    pub(crate) fn saying(body: String) -> Self {
        Self::with(move |_| body)
    }

    /// Holds the answer back until `gate` opens.
    pub(crate) fn after(mut self, gate: &Gate) -> Self {
        self.after = Some(gate.clone());
        self
    }

    /// Looks at the runtime's workers before answering.
    pub(crate) fn probing(mut self, probe: &Probe) -> Self {
        self.probe = Some(probe.clone());
        self
    }
}

/// A vendor on the loopback address, answering exactly its script.
pub(crate) struct Vendor {
    pub endpoint: Endpoint,
    requests: Arc<Mutex<Vec<Value>>>,
    thread: Option<JoinHandle<()>>,
}

impl Vendor {
    /// A vendor speaking `wire` that answers the requests it receives, in
    /// order, with `replies`.
    pub(crate) fn new(wire: Wire, replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
        listener
            .set_nonblocking(true)
            .expect("a listener that polls");
        let endpoint = Endpoint::parse(&format!(
            "http://{}/{}",
            listener.local_addr().expect("the port it took"),
            wire.path()
        ))
        .expect("a loopback endpoint");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let thread = thread::spawn(move || {
            for reply in replies {
                serve(&listener, &captured, reply);
            }
        });
        Self {
            endpoint,
            requests,
            thread: Some(thread),
        }
    }

    /// Every request body it received, in order.
    pub(crate) fn requests(&self) -> Vec<Value> {
        self.requests.lock().expect("the requests").clone()
    }
}

/// Takes one request off `listener` and answers it with `reply`.
fn serve(listener: &TcpListener, captured: &Mutex<Vec<Value>>, reply: Reply) {
    let until = Instant::now() + WAIT;
    let (mut socket, _) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until =>
            {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("a scripted request never arrived: {error}"),
        }
    };
    // Accepted sockets inherit nonblocking mode on some platforms.
    socket.set_nonblocking(false).expect("a blocking socket");
    socket.set_read_timeout(Some(WAIT)).expect("a read timeout");
    socket
        .set_write_timeout(Some(WAIT))
        .expect("a write timeout");
    let mut reader = BufReader::new(&mut socket);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).expect("a header line") > 0);
        assert!(
            headers.len() + line.len() < 32_768,
            "headers past their bound"
        );
        headers.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let length: usize = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .expect("a sized request")
        .1
        .trim()
        .parse()
        .expect("a length");
    assert!(length < 1_048_576, "a request past its bound");
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).expect("the request body");
    let request: Value = serde_json::from_slice(&bytes).expect("a JSON request");
    captured.lock().expect("the requests").push(request.clone());
    if let Some(gate) = &reply.after {
        assert!(gate.opened(WAIT), "a held answer was never let go");
    }
    if let Some(probe) = &reply.probe {
        probe.look();
    }
    let body = (reply.answer)(&request);
    write!(
        socket,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("the answer");
}

impl Drop for Vendor {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let served = thread.join();
            if !thread::panicking() {
                served.expect("the vendor answered its whole script");
            }
        }
    }
}
