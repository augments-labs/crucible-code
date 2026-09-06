//! Bounded local HTTP fixtures, exact native response shapes and owned sessions.

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crucible_core::{
    AgentId, ApiKey, Approved, Aside, Ask, Cancel, DescribeTool, Effort, EventEnvelope, Header,
    HeaderKey, Host, Post, Provider, Remember, Sensitivity, SessionId, Steer, StopReason, Summary,
    Tool, ToolArgs, ToolCall, ToolContext, ToolError, ToolOutput, TurnError, Verdict, Workspace,
};
use crucible_provider::{Anthropic, Endpoint, Google, Https, OpenAi};
use crucible_runner::{
    AgentSpec, Compaction, ContextInputs, Model, RunPolicy, Runner, Session, Tools,
};
use serde_json::{Value, json};

pub(crate) const MODELS: [&str; 6] = [
    "gemini-3.8-flash",
    "gemini-3.7-flash",
    "gemini-3.6-flash",
    "gemini-3.1-pro-preview",
    "claude-fable-5-1",
    "gpt-6-astra",
];
const KEY: &str = "fixture-only-api-key";
const WAIT: Duration = Duration::from_secs(10);

pub(crate) struct Sample {
    path: PathBuf,
    executed: Arc<Mutex<Vec<String>>>,
    approved: AtomicUsize,
    events: Mutex<Vec<EventEnvelope>>,
    padding: usize,
}

impl Sample {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "crucible-provider-lifecycle-{}",
            SessionId::new().as_str()
        ));
        fs::create_dir(&path).expect("valid fixture");
        Self {
            path,
            executed: Arc::default(),
            approved: AtomicUsize::new(0),
            events: Mutex::default(),
            padding: 0,
        }
    }
    pub(crate) fn padded(mut self) -> Self {
        self.padding = 30_000;
        self
    }
    pub(crate) fn workspace(&self) -> Workspace {
        Workspace::open(&self.path).expect("valid fixture")
    }
    pub(crate) fn logs(&self) -> PathBuf {
        self.path.join("sessions")
    }
    pub(crate) fn executed(&self) -> Vec<String> {
        self.executed.lock().expect("valid fixture").clone()
    }
    pub(crate) fn approved(&self) -> usize {
        self.approved.load(Ordering::SeqCst)
    }
    pub(crate) fn events(&self) -> String {
        format!("{:?}", self.events.lock().expect("valid fixture"))
    }
    pub(crate) fn runner(&self, model: &str, vendor: &Vendor, session: Session) -> Runner {
        let mut tools = Tools::new();
        tools
            .add_builtin(Count(self.executed.clone(), self.padding))
            .expect("valid fixture");
        Runner::new(
            provider(model, vendor.endpoint.clone(), KEY),
            tools,
            AgentSpec::new(
                AgentId::new("fixture"),
                Model {
                    name: model.into(),
                    max_tokens: 4096,
                    window: Some(200_000),
                    accepts: None,
                    effort: Some(Effort::High),
                },
            ),
            ContextInputs::new(&self.path),
            session,
        )
        .under(RunPolicy {
            compaction: Compaction {
                keep_tokens: 1,
                ..Compaction::default()
            },
            ..RunPolicy::default()
        })
    }
}
impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
impl Post for Sample {
    fn post(&self, event: EventEnvelope) {
        self.events.lock().expect("valid fixture").push(event);
    }
}
struct Permit<'a>(&'a Sample);
impl Ask for Permit<'_> {
    fn ask(&mut self, _: &ToolCall, _: &Sensitivity) -> (Verdict, Remember) {
        self.0.approved.fetch_add(1, Ordering::SeqCst);
        (Verdict::Allow, Remember::Never)
    }
}
struct Count(Arc<Mutex<Vec<String>>>, usize);
impl DescribeTool for Count {
    fn name(&self) -> &'static str {
        "fixture"
    }
    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{"step":{"type":"string"}},"required":["step"]}"#
    }
}
impl Tool for Count {
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
        let parsed: Value = serde_json::from_str(args.as_str()).expect("valid fixture");
        assert!(parsed.get("step").and_then(Value::as_str).is_some());
        Ok(())
    }
    fn sensitivity(&self, _: &ToolArgs) -> Sensitivity {
        Sensitivity::ReachesNetwork {
            host: Host::Opaque("fixture makes no network request".into()),
        }
    }
    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }
    fn run(&self, approved: Approved, _: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let parsed: Value = serde_json::from_str(approved.args().as_str()).expect("valid fixture");
        let step = parsed
            .get("step")
            .expect("valid fixture")
            .as_str()
            .expect("valid fixture");
        self.0.lock().expect("valid fixture").push(step.into());
        Ok(ToolOutput::ok(format!(
            "fixture-result-{step}{}",
            "x".repeat(self.1)
        )))
    }
}

pub(crate) fn try_turn(
    run: &mut Runner,
    prompt: &str,
    sample: &Sample,
) -> Result<StopReason, TurnError> {
    let cancel = Cancel::new();
    let steer = Steer::new();
    let aside = Aside::new();
    let context = run.starting(sample, &cancel, &steer, &aside);
    run.turn(prompt, Box::new([]), &mut Permit(sample), &context)
}
pub(crate) fn turn(run: &mut Runner, prompt: &str, sample: &Sample) -> StopReason {
    try_turn(run, prompt, sample).expect("valid fixture")
}

pub(crate) fn provider(model: &str, endpoint: Endpoint, key: &str) -> Box<dyn Provider> {
    through(model, endpoint, key, Box::new(Https::new()))
}

fn through(
    model: &str,
    endpoint: Endpoint,
    key: &str,
    transport: Box<dyn crucible_provider::Transport>,
) -> Box<dyn Provider> {
    let header = if model.starts_with("gemini-") {
        Header::bare("x-goog-api-key")
    } else if model.starts_with("claude-") {
        Header::bare("x-api-key")
    } else {
        Header::bearer()
    };
    let credential = Box::new(HeaderKey::new(ApiKey::new(key), header));
    if model.starts_with("gemini-") {
        Box::new(Google::at(endpoint, credential, transport))
    } else if model.starts_with("claude-") {
        Box::new(Anthropic::at(endpoint, credential, transport))
    } else {
        Box::new(OpenAi::at(endpoint, credential, transport))
    }
}

/// Cancels at the clean EOF read, after every terminal frame was delivered.
pub(crate) fn cancelling(model: &str, endpoint: Endpoint, body: String) -> Box<dyn Provider> {
    #[derive(Debug)]
    struct AtEof(String);
    struct Body {
        bytes: std::io::Cursor<Vec<u8>>,
        cancel: Cancel,
    }
    impl std::io::Read for Body {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let count = self.bytes.read(out)?;
            if count == 0 {
                self.cancel.request();
            }
            Ok(count)
        }
    }
    impl crucible_provider::Transport for AtEof {
        fn post(
            &self,
            _: &str,
            _: crucible_core::Outgoing,
            _: String,
            cancel: &Cancel,
        ) -> Result<crucible_provider::Response, crucible_provider::TransportError> {
            Ok(crucible_provider::Response {
                status: 200,
                body: Box::new(Body {
                    bytes: std::io::Cursor::new(self.0.as_bytes().to_vec()),
                    cancel: cancel.clone(),
                }),
            })
        }
    }
    through(model, endpoint, KEY, Box::new(AtEof(body)))
}

#[derive(Clone, Debug)]
pub(crate) struct Sent {
    pub headers: String,
    pub body: Value,
}
pub(crate) struct Vendor {
    pub endpoint: Endpoint,
    requests: Arc<Mutex<Vec<Sent>>>,
    thread: Option<JoinHandle<()>>,
}
impl Vendor {
    pub(crate) fn new(model: &str, responses: impl IntoIterator<Item = String>) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("valid fixture");
        listener.set_nonblocking(true).expect("valid fixture");
        let endpoint = Endpoint::parse(&format!(
            "http://{}/{}",
            listener.local_addr().expect("valid fixture"),
            if model.starts_with("gemini-") {
                "interactions?alt=sse"
            } else if model.starts_with("claude-") {
                "messages"
            } else {
                "responses"
            }
        ))
        .expect("valid fixture");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let responses: Vec<_> = responses.into_iter().collect();
        let thread = thread::spawn(move || {
            for response in responses {
                let until = Instant::now() + WAIT;
                let (mut socket, _) = loop {
                    match listener.accept() {
                        Ok(accepted) => break accepted,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < until =>
                        {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("fixture request did not arrive: {error}"),
                    }
                };
                // Accepted sockets inherit nonblocking mode on some platforms;
                // these bounded request reads need blocking mode explicitly.
                socket.set_nonblocking(false).expect("valid fixture");
                socket.set_read_timeout(Some(WAIT)).expect("valid fixture");
                socket.set_write_timeout(Some(WAIT)).expect("valid fixture");
                let mut reader = BufReader::new(&mut socket);
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).expect("valid fixture") > 0);
                    assert!(headers.len() + line.len() < 32_768);
                    headers.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let length: usize = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .expect("valid fixture")
                    .1
                    .trim()
                    .parse()
                    .expect("valid fixture");
                assert!(length < 1_048_576);
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).expect("valid fixture");
                captured.lock().expect("valid fixture").push(Sent {
                    headers,
                    body: serde_json::from_slice(&bytes).expect("valid fixture"),
                });
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).expect("valid fixture");
            }
        });
        Self {
            endpoint,
            requests,
            thread: Some(thread),
        }
    }
    pub(crate) fn requests(&self) -> Vec<Sent> {
        self.requests.lock().expect("valid fixture").clone()
    }
}
impl Drop for Vendor {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.expect("valid fixture");
            }
        }
    }
}

pub(crate) fn response(model: &str, call: Option<u8>, text: &str) -> String {
    let mut events = Vec::new();
    if model.starts_with("gemini-") {
        let mut steps = vec![
            json!({"type":"thought","signature":"private-state","summary":[]}),
            json!({"type":"model_output","content":[{"type":"text","text":text}]}),
        ];
        if let Some(step) = call {
            steps.push(json!({"type":"function_call","id":format!("call-{step}"),"name":"fixture","arguments":{"step":step.to_string()}}));
        }
        for (index, step) in steps.into_iter().enumerate() {
            events.push(json!({"event_type":"step.start","index":index,"step":step}));
            events.push(json!({"event_type":"step.stop","index":index}));
        }
        events.push(json!({"event_type":"interaction.completed","interaction":{"status":if call.is_some() {"requires_action"} else {"completed"}}}));
    } else if model.starts_with("claude-") {
        events.push(json!({"type":"message_start","message":{"id":"fixture-message"}}));
        let mut blocks = vec![
            json!({"type":"thinking","thinking":"","signature":"private-state"}),
            json!({"type":"text","text":text}),
        ];
        if let Some(step) = call {
            blocks.push(json!({"type":"tool_use","id":format!("call-{step}"),"name":"fixture","input":{"step":step.to_string()}}));
        }
        for (index, block) in blocks.into_iter().enumerate() {
            events.push(json!({"type":"content_block_start","index":index,"content_block":block}));
            events.push(json!({"type":"content_block_stop","index":index}));
        }
        events.push(json!({"type":"message_delta","delta":{"stop_reason":if call.is_some() {"tool_use"} else {"end_turn"}}}));
        events.push(json!({"type":"message_stop"}));
    } else {
        let mut output = vec![
            json!({"type":"reasoning","id":"rs-fixture","summary":[],"encrypted_content":"private-state"}),
            json!({"type":"message","id":"msg-fixture","role":"assistant","status":"completed","phase":if call.is_some() {"commentary"} else {"final_answer"},"content":[{"type":"output_text","text":text,"annotations":[]}]}),
        ];
        if let Some(step) = call {
            output.push(json!({"type":"function_call","id":format!("fc-{step}"),"call_id":format!("call-{step}"),"name":"fixture","arguments":json!({"step":step.to_string()}).to_string(),"status":"completed"}));
        }
        events.push(json!({"type":"response.created","response":{"id":"fixture-response","status":"in_progress","output":[]}}));
        for (index, item) in output.iter().enumerate() {
            events.push(
                json!({"type":"response.output_item.added","output_index":index,"item":item}),
            );
            events
                .push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
        }
        events.push(json!({"type":"response.completed","response":{"id":"fixture-response","status":"completed","output":output}}));
    }
    let mut body = String::new();
    for event in &events {
        use std::fmt::Write as _;
        write!(
            body,
            "event: {}\ndata: {event}\n\n",
            event
                .get("event_type")
                .or_else(|| event.get("type"))
                .expect("valid fixture")
                .as_str()
                .expect("valid fixture")
        )
        .expect("writing into a string");
    }
    body
}

#[derive(Clone, Copy)]
pub(crate) enum Failure {
    MissingStop,
    LateError,
}
pub(crate) fn broken(model: &str, failure: Failure) -> String {
    let body = response(model, Some(1), "visible partial");
    match failure {
        Failure::MissingStop => {
            let index = body.rfind("event: ").expect("valid fixture");
            body.get(..index)
                .expect("event begins at a UTF-8 boundary")
                .into()
        }
        Failure::LateError => format!(
            "{body}event: error\ndata: {{\"type\":\"error\",\"event_type\":\"error\",\"error\":{{\"message\":\"private-state\"}}}}\n\n"
        ),
    }
}

pub(crate) fn recap() -> String {
    "## Goal\nfixture checkpoint\n## Constraints & Preferences\n(none)\n## Progress\n### Done\ntwo tools\n### In Progress\n(none)\n### Blocked\n(none)\n## Decisions\n(none)\n## Next Steps\ncontinue\n## Critical Context\n(none)".into()
}

pub(crate) fn assert_request(model: &str, sent: &Sent) {
    let body = &sent.body;
    assert_eq!(body.get("model").and_then(Value::as_str), Some(model));
    assert_eq!(body.get("stream"), Some(&json!(true)));
    assert!(!body.to_string().contains(KEY));
    let headers = sent.headers.to_ascii_lowercase();
    let (header, effort) = if model.starts_with("gemini-") {
        ("x-goog-api-key", "/generation_config/thinking_level")
    } else if model.starts_with("claude-") {
        ("x-api-key", "/output_config/effort")
    } else {
        ("authorization", "/reasoning/effort")
    };
    assert!(headers.contains(&format!(
        "{header}: {}{KEY}",
        if header == "authorization" {
            "bearer "
        } else {
            ""
        }
    )));
    if header != "authorization" {
        assert!(!headers.contains("authorization:"));
    }
    assert_eq!(body.pointer(effort).and_then(Value::as_str), Some("high"));
    if !model.starts_with("claude-") {
        assert_eq!(body.get("store"), Some(&json!(false)));
        assert!(body.get("previous_interaction_id").is_none());
        assert!(body.get("previous_response_id").is_none());
    }
}

pub(crate) fn assert_native_history(model: &str, body: &Value, calls: usize) {
    let wire = body.to_string();
    assert!(
        wire.contains("private-state"),
        "{model}: native state missing"
    );
    for step in 1..=calls {
        assert_eq!(
            wire.matches(&format!("fixture-result-{step}")).count(),
            1,
            "{model}: result duplicated/lost"
        );
        let result_type = if model.starts_with("gemini-") {
            "function_result"
        } else if model.starts_with("claude-") {
            "tool_result"
        } else {
            "function_call_output"
        };
        assert!(wire.contains(result_type), "{model}: result is not native");
        assert_eq!(
            wire.matches(&format!("\"call-{step}\"")).count(),
            2,
            "{model}: call/result identity lost"
        );
    }
}
