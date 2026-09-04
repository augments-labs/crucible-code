//! The short-lived loopback boundary for browser authorization.
//!
//! Only two origin-form requests exist here: the token-guarded launch path
//! redirects the browser to the complete authorization URI, and
//! `/auth/callback` accepts the code whose state exactly matches this attempt.
//! Headers, targets and query fields are bounded before parsing. Invalid or
//! forged requests receive a fixed response and leave the real attempt
//! waiting.

use std::io::{ErrorKind, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crucible_core::Cancel;

use super::{CANCEL_POLL, OAuthError, random_urlsafe};

/// The loopback ports the provider will redirect a browser back to.
///
/// A redirect address is registered with the provider, not chosen at run time,
/// so a login can only be answered on one of these. There is no ephemeral
/// alternative in production: a port outside this list is a redirect the
/// provider refuses.
pub(crate) const PORTS: [u16; 2] = [1455, 1457];
const MAX_HEADERS: usize = 16 * 1024;
const MAX_FIELDS: usize = 32;
const MAX_VALUE: usize = 8 * 1024;
const REQUEST_LIFETIME: Duration = Duration::from_secs(2);

pub(super) struct Server {
    listener: TcpListener,
    port: u16,
    lifetime: Duration,
    /// The launch path, carrying a token only this attempt was given.
    ///
    /// Loopback is every local account's, not just this one's, and the launch
    /// answer is the authorization URI — state included, which is what lets a
    /// forged callback through. The token travels only where the user does:
    /// the terminal the address is printed to, and the browser it opens.
    launch: String,
}

impl Server {
    /// Listens on the first free port of `ports`, which is `PORTS` in
    /// production and one ephemeral port under test.
    ///
    /// A test that took the registered pair would depend on two fixed ports
    /// being free on whatever host it runs on, which no test controls: another
    /// process holding them, or a second test in the same binary, turns a
    /// working login into a bind failure that reads as a broken one.
    pub(super) fn bind(ports: &[u16], lifetime: Duration) -> Result<Self, OAuthError> {
        for port in ports {
            let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, *port)) else {
                continue;
            };
            listener
                .set_nonblocking(true)
                .map_err(|_| OAuthError::Callback)?;
            let port = listener
                .local_addr()
                .map_err(|_| OAuthError::Callback)?
                .port();
            return Ok(Self {
                listener,
                port,
                lifetime,
                launch: format!("/launch/{}", random_urlsafe::<16>()?),
            });
        }
        Err(OAuthError::Callback)
    }

    pub(super) fn redirect_uri(&self) -> String {
        format!("http://localhost:{}/auth/callback", self.port)
    }

    pub(super) fn launch_uri(&self) -> String {
        format!("http://localhost:{}{}", self.port, self.launch)
    }

    pub(super) fn wait(
        &self,
        authorization: &str,
        expected_state: &str,
        cancel: &Cancel,
        manual: &Receiver<Box<str>>,
    ) -> Result<Box<str>, OAuthError> {
        let started = Instant::now();
        loop {
            if cancel.requested() {
                return Err(OAuthError::Cancelled);
            }
            if started.elapsed() >= self.lifetime {
                return Err(OAuthError::Expired);
            }
            match manual.try_recv() {
                Ok(submitted) => return self.manual(&submitted, expected_state),
                Err(TryRecvError::Disconnected) if cancel.requested() => {
                    return Err(OAuthError::Cancelled);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
            }

            match self.listener.accept() {
                Ok((mut stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    match request(&mut stream, self.port, &self.launch, cancel) {
                        Ok(Request::Launch) => {
                            respond_redirect(&mut stream, authorization);
                        }
                        Ok(Request::Callback {
                            code,
                            state,
                            denied,
                        }) => {
                            if state.as_deref() != Some(expected_state) {
                                respond(&mut stream, 400, "This sign-in request is not current.");
                                continue;
                            }
                            if denied {
                                respond(
                                    &mut stream,
                                    200,
                                    "Authorization did not complete. You can return to Crucible.",
                                );
                                return Err(OAuthError::Denied);
                            }
                            let Some(code) = code else {
                                respond(&mut stream, 400, "The authorization code is missing.");
                                continue;
                            };
                            respond(
                                &mut stream,
                                200,
                                "Authorization complete. You can return to Crucible.",
                            );
                            return Ok(code);
                        }
                        Err(()) => respond(&mut stream, 400, "This request is not accepted."),
                    }
                }
                Err(problem) if problem.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(CANCEL_POLL);
                }
                Err(_) => return Err(OAuthError::Callback),
            }
        }
    }

    fn manual(&self, submitted: &str, expected_state: &str) -> Result<Box<str>, OAuthError> {
        let submitted = submitted.trim();
        if let Some(query) = submitted
            .strip_prefix(&self.redirect_uri())
            .and_then(|rest| rest.strip_prefix('?'))
        {
            let fields = fields(query).map_err(|()| OAuthError::Invalid {
                step: "manual callback",
            })?;
            if one(&fields, "state")
                .map_err(|()| OAuthError::State)?
                .as_deref()
                != Some(expected_state)
            {
                return Err(OAuthError::State);
            }
            if one(&fields, "error")
                .map_err(|()| OAuthError::Denied)?
                .is_some()
            {
                return Err(OAuthError::Denied);
            }
            return one(&fields, "code")
                .map_err(|()| OAuthError::Invalid {
                    step: "manual callback",
                })?
                .ok_or(OAuthError::Invalid {
                    step: "manual callback",
                });
        }
        if submitted.len() <= 4096 && submitted.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Ok(submitted.into());
        }
        Err(OAuthError::Invalid {
            step: "manual authorization code",
        })
    }
}

enum Request {
    Launch,
    Callback {
        code: Option<Box<str>>,
        state: Option<Box<str>>,
        denied: bool,
    },
}

fn request(
    stream: &mut TcpStream,
    port: u16,
    launch: &str,
    cancel: &Cancel,
) -> Result<Request, ()> {
    stream.set_read_timeout(Some(CANCEL_POLL)).map_err(|_| ())?;
    let started = Instant::now();
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {
        if cancel.requested() || started.elapsed() >= REQUEST_LIFETIME {
            return Err(());
        }
        match stream.read(&mut buffer) {
            Ok(0) => return Err(()),
            Ok(read) => {
                if bytes.len().saturating_add(read) > MAX_HEADERS {
                    return Err(());
                }
                bytes.extend_from_slice(buffer.get(..read).ok_or(())?);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(problem)
                if matches!(problem.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Err(()),
        }
    }

    let text = std::str::from_utf8(&bytes).map_err(|_| ())?;
    let mut lines = text.split("\r\n");
    let mut words = lines.next().ok_or(())?.split_ascii_whitespace();
    if words.next() != Some("GET") {
        return Err(());
    }
    let target = words.next().ok_or(())?;
    if words.next() != Some("HTTP/1.1") || words.next().is_some() || !target.starts_with('/') {
        return Err(());
    }

    let mut host = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(());
        };
        if name.eq_ignore_ascii_case("host") && host.replace(value.trim()).is_some() {
            return Err(());
        }
    }
    let expected_name = format!("localhost:{port}");
    let expected_ip = format!("127.0.0.1:{port}");
    if !matches!(host, Some(value) if value.eq_ignore_ascii_case(&expected_name) || value == expected_ip)
    {
        return Err(());
    }

    let (path, query) = target.split_once('?').map_or((target, ""), |parts| parts);
    match path {
        _ if path == launch && query.is_empty() => Ok(Request::Launch),
        "/auth/callback" => {
            let fields = fields(query)?;
            Ok(Request::Callback {
                code: one(&fields, "code")?,
                state: one(&fields, "state")?,
                denied: one(&fields, "error")?.is_some(),
            })
        }
        _ => Err(()),
    }
}

type Fields = Vec<(Box<str>, Box<str>)>;

fn fields(query: &str) -> Result<Fields, ()> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<_> = query.split('&').collect();
    if parts.len() > MAX_FIELDS {
        return Err(());
    }
    parts
        .into_iter()
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            Ok((decoded(name)?, decoded(value)?))
        })
        .collect()
}

fn decoded(text: &str) -> Result<Box<str>, ()> {
    if text.len() > MAX_VALUE.saturating_mul(3) {
        return Err(());
    }
    let mut bytes = text.bytes();
    let mut decoded = Vec::with_capacity(text.len().min(MAX_VALUE));
    while let Some(byte) = bytes.next() {
        if decoded.len() >= MAX_VALUE {
            return Err(());
        }
        match byte {
            b'%' => {
                let high = hex(bytes.next().ok_or(())?).ok_or(())?;
                let low = hex(bytes.next().ok_or(())?).ok_or(())?;
                decoded.push((high << 4) | low);
            }
            b'+' => decoded.push(b' '),
            ordinary => decoded.push(ordinary),
        }
    }
    String::from_utf8(decoded)
        .map(String::into_boxed_str)
        .map_err(|_| ())
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn one(fields: &Fields, name: &str) -> Result<Option<Box<str>>, ()> {
    let mut found = fields
        .iter()
        .filter(|(held, _)| held.as_ref() == name)
        .map(|(_, value)| value.clone());
    let first = found.next();
    if found.next().is_some() {
        return Err(());
    }
    Ok(first.filter(|value| !value.is_empty()))
}

fn respond_redirect(stream: &mut TcpStream, location: &str) {
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n"
    );
    let _ = stream.write_all(response.as_bytes());
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use std::sync::mpsc;

    #[test]
    fn percent_decoding_is_strict_and_bounded() {
        assert_eq!(decoded("a%2Fb+c").unwrap().as_ref(), "a/b c");
        assert!(decoded("%2").is_err());
        assert!(decoded("%GG").is_err());
        assert!(decoded(&"x".repeat(MAX_VALUE + 1)).is_err());
    }

    #[test]
    fn a_forged_state_does_not_consume_the_real_callback() {
        let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
        let port = server.port;
        let launch = server.launch_uri();
        let cancel = Cancel::new();
        let (_input, submitted) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            server.wait(
                "https://example.invalid/authorize",
                "right",
                &cancel,
                &submitted,
            )
        });

        let path = launch
            .strip_prefix(&format!("http://localhost:{port}"))
            .expect("the launch address names this server");
        let launched = get(port, path);
        assert!(launched.starts_with("HTTP/1.1 302"));
        assert!(launched.contains("Location: https://example.invalid/authorize"));

        let forged = get(port, "/auth/callback?code=stolen&state=wrong");
        assert!(forged.starts_with("HTTP/1.1 400"));
        let accepted = get(port, "/auth/callback?code=kept%2Fcode&state=right");
        assert!(accepted.starts_with("HTTP/1.1 200"));
        assert_eq!(worker.join().unwrap().unwrap().as_ref(), "kept/code");
    }

    #[test]
    fn a_launch_without_its_token_reveals_nothing() {
        // The launch address is handed to the user's own terminal and browser
        // and nowhere else. Loopback is every local account's, not just this
        // one's, so a bare `/launch` polled by somebody else must not answer
        // with the authorization URI — the state inside it is what lets a
        // forged callback through.
        let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
        let port = server.port;
        let cancel = Cancel::new();
        let (_input, submitted) = mpsc::sync_channel(1);
        let stopping = cancel.clone();
        let worker = std::thread::spawn(move || {
            server.wait(
                "https://example.invalid/authorize",
                "right",
                &stopping,
                &submitted,
            )
        });

        let bare = get(port, "/launch");
        assert!(bare.starts_with("HTTP/1.1 400"), "{bare}");
        assert!(!bare.contains("example.invalid"), "{bare}");
        let guessed = get(port, "/launch/wrong-token");
        assert!(guessed.starts_with("HTTP/1.1 400"), "{guessed}");

        cancel.request();
        assert!(matches!(worker.join().unwrap(), Err(OAuthError::Cancelled)));
    }

    #[test]
    fn cancellation_ends_an_idle_callback_promptly() {
        let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
        let cancel = Cancel::new();
        let stopping = cancel.clone();
        let (_input, submitted) = mpsc::sync_channel(1);
        let (send, done) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(server.wait("https://example.invalid", "state", &stopping, &submitted))
                .unwrap();
        });
        cancel.request();
        assert!(matches!(
            done.recv_timeout(Duration::from_millis(200)).unwrap(),
            Err(OAuthError::Cancelled)
        ));
        worker.join().unwrap();
    }

    #[test]
    fn manual_input_accepts_a_code_or_the_matching_redirect_only() {
        let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
        assert_eq!(
            server.manual("raw-code", "state").unwrap().as_ref(),
            "raw-code"
        );
        let callback = format!("{}?code=kept%2Fcode&state=right", server.redirect_uri());
        assert_eq!(
            server.manual(&callback, "right").unwrap().as_ref(),
            "kept/code"
        );
        assert!(matches!(
            server.manual(&callback, "wrong"),
            Err(OAuthError::State)
        ));
        assert!(server.manual("not a code", "state").is_err());
    }

    fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
}
