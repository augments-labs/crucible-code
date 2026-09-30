//! What the cases about the question a route whose vendor uses what is sent
//! puts are watched through: a proxy that stands in for every host.
//!
//! Every case sends through `HTTPS_PROXY`, pointed at a listener here that
//! writes down each host it is asked to connect to and refuses it. So a request
//! that leaves is seen by name, whatever it was for (a turn, a sign-in, a
//! renewal), and none reaches a vendor.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::watched::{Launch, Watched};

/// How long a request that was let go is given to reach the proxy.
const REACHING: Duration = Duration::from_secs(10);

/// The Google key row's route, served by a key from the environment.
pub(crate) const GOOGLE: &str = concat!(
    "{\n",
    "  \"sandbox\": {\"enabled\": false},\n",
    "  \"updates\": {\"check\": \"never\"},\n",
    "  \"provider\": \"google\",\n",
    "  \"providers\": {\"google\": {\"model\": \"gemini-3.8-flash\"}}\n",
    "}\n"
);

/// The same, the route already said yes to.
pub(crate) const GOOGLE_SAID: &str = concat!(
    "{\n",
    "  \"sandbox\": {\"enabled\": false},\n",
    "  \"updates\": {\"check\": \"never\"},\n",
    "  \"provider\": \"google\",\n",
    "  \"providers\": {\"google\": {\"model\": \"gemini-3.8-flash\"}},\n",
    "  \"contentUse\": {\"accepted\": [\"key:google\"]}\n",
    "}\n"
);

/// Nothing chosen: what a case reaching a route through the command line or
/// `/login` starts from.
pub(crate) const NOTHING_CHOSEN: &str = concat!(
    "{\n",
    "  \"sandbox\": {\"enabled\": false},\n",
    "  \"updates\": {\"check\": \"never\"}\n",
    "}\n"
);

/// Where a Gemini request is sent, as a proxy is asked for it.
pub(crate) const GEMINI_HOST: &str = "generativelanguage.googleapis.com:443";

/// The Gemini key crucible is handed, which reaches nothing but the proxy.
pub(crate) const KEY: &str = "fabricated-gemini-key-never-sent";

/// A listener standing in for every host, which writes down what it is asked
/// to connect to and refuses it.
pub(crate) struct Proxy {
    pub(crate) address: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Proxy {
    pub(crate) fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let address = format!("http://{}", listener.local_addr().expect("its address"));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let hearing = Arc::clone(&asked);
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut head = Vec::new();
                let mut byte = [0; 1];
                while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
                    match stream.read(&mut byte) {
                        Ok(1) => head.push(byte[0]),
                        _ => break,
                    }
                }
                let first = String::from_utf8_lossy(&head)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let host = first
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                hearing.lock().expect("the record").push(host);
                let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n");
            }
        });
        Self { address, asked }
    }

    /// Where it listens, as `HTTPS_PROXY` names it.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// Every host asked for so far.
    pub(crate) fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("the record").clone()
    }

    /// The hosts asked for once `more` of them have been, or once
    /// [`REACHING`] is up.
    pub(crate) fn reached(&self, more: usize) -> Vec<String> {
        let deadline = Instant::now() + REACHING;
        while self.asked().len() < more && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        self.asked()
    }
}

/// crucible over `document`, with a Gemini key and every request through
/// `proxy`.
pub(crate) fn through(
    case: &str,
    (columns, rows): (u16, u16),
    document: &str,
    proxy: &Proxy,
    (args, home): (&[&str], Option<&Path>),
) -> Watched {
    Watched::launched(
        case,
        columns,
        rows,
        &Launch {
            document,
            env: &[
                ("GEMINI_API_KEY", KEY),
                ("MOONSHOT_API_KEY", KEY),
                ("HTTPS_PROXY", &proxy.address),
            ],
            args,
            home,
        },
    )
}

/// The user's own file, as the case left it.
pub(crate) fn said(window: &Watched) -> String {
    std::fs::read_to_string(window.home().join("config.json")).unwrap_or_default()
}

/// The configuration of a `MoonshotAI` key sent to `base`.
pub(crate) fn based(base: &str) -> String {
    format!(
        "{{\n  \"sandbox\": {{\"enabled\": false}},\n  \"updates\": {{\"check\": \"never\"}},\n  \
         \"provider\": \"moonshot\",\n  \"providers\": {{\"moonshot\": {{\"model\": \"kimi-for-coding\", \
         \"baseUrl\": \"{base}\"}}}}\n}}\n"
    )
}
