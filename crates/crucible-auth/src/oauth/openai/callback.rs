//! The short-lived loopback boundary for browser authorization.
//!
//! Only two origin-form requests exist here: the token-guarded launch path
//! redirects the browser to the complete authorization URI, and
//! `/auth/callback` accepts the code whose state exactly matches this attempt.
//! Headers, targets and query fields are bounded before parsing. Invalid or
//! forged requests receive a fixed response and leave the real attempt
//! waiting.
//!
//! The listener is a socket on the login's runtime, owned by the login's
//! future: the login awaits the next connection or the next pasted value,
//! whichever comes first, and dropping the login closes the port. One
//! connection is served at a time: its request's head is read within 2 s, and
//! its answer is given 2 s more to be taken.

use std::future::poll_fn;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as Bound};
use std::task::Poll;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::Receiver;

use super::{OAuthError, random_urlsafe};

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
    /// Called on the login's runtime, whose I/O driver the socket is
    /// registered with.
    ///
    /// A test that took the registered pair would depend on two fixed ports
    /// being free on whatever host it runs on, which no test controls: another
    /// process holding them, or a second test in the same binary, turns a
    /// working login into a bind failure that reads as a broken one.
    pub(super) fn bind(ports: &[u16], lifetime: Duration) -> Result<Self, OAuthError> {
        for port in ports {
            let Ok(listener) = Bound::bind((Ipv4Addr::LOCALHOST, *port)) else {
                continue;
            };
            listener
                .set_nonblocking(true)
                .map_err(|_| OAuthError::Callback)?;
            let port = listener
                .local_addr()
                .map_err(|_| OAuthError::Callback)?
                .port();
            let listener = TcpListener::from_std(listener).map_err(|_| OAuthError::Callback)?;
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

    /// Waits for the browser's callback, or a pasted one, whose state is
    /// `expected_state`, until this server's lifetime runs out.
    ///
    /// # Errors
    ///
    /// [`OAuthError::Expired`] once the lifetime has run out,
    /// [`OAuthError::Cancelled`] once nothing can be pasted any more because
    /// the attempt is gone, [`OAuthError::Callback`] where the listener fails,
    /// and a pasted value's or an authorized callback's own refusal.
    pub(super) async fn wait(
        &self,
        authorization: &str,
        expected_state: &str,
        manual: &mut Receiver<Box<str>>,
    ) -> Result<Box<str>, OAuthError> {
        tokio::time::timeout(
            self.lifetime,
            self.answered(authorization, expected_state, manual),
        )
        .await
        .map_err(|_| OAuthError::Expired)?
    }

    async fn answered(
        &self,
        authorization: &str,
        expected_state: &str,
        manual: &mut Receiver<Box<str>>,
    ) -> Result<Box<str>, OAuthError> {
        loop {
            let (mut stream, peer) = match self.next(manual).await {
                Next::Pasted(Some(submitted)) => return self.manual(&submitted, expected_state),
                Next::Pasted(None) => return Err(OAuthError::Cancelled),
                Next::Connected(Ok(accepted)) => accepted,
                Next::Connected(Err(_)) => return Err(OAuthError::Callback),
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            match request(&mut stream, self.port, &self.launch).await {
                Ok(Request::Launch) => {
                    respond_redirect(&mut stream, authorization).await;
                }
                Ok(Request::Callback {
                    code,
                    state,
                    denied,
                }) => {
                    if state.as_deref() != Some(expected_state) {
                        respond(&mut stream, 400, "This sign-in request is not current.").await;
                        continue;
                    }
                    if denied {
                        respond(
                            &mut stream,
                            200,
                            "Authorization did not complete. You can return to Crucible.",
                        )
                        .await;
                        return Err(OAuthError::Denied);
                    }
                    let Some(code) = code else {
                        respond(&mut stream, 400, "The authorization code is missing.").await;
                        continue;
                    };
                    respond(
                        &mut stream,
                        200,
                        "Authorization complete. You can return to Crucible.",
                    )
                    .await;
                    return Ok(code);
                }
                Err(()) => respond(&mut stream, 400, "This request is not accepted.").await,
            }
        }
    }

    /// Whichever comes first: a value pasted into the terminal, or a
    /// connection to the listener. A pasted value is looked at first.
    async fn next(&self, manual: &mut Receiver<Box<str>>) -> Next {
        poll_fn(|context| {
            if let Poll::Ready(pasted) = manual.poll_recv(context) {
                return Poll::Ready(Next::Pasted(pasted));
            }
            self.listener.poll_accept(context).map(Next::Connected)
        })
        .await
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

enum Next {
    /// A value pasted into the terminal, or `None` once the attempt that
    /// could paste one is gone.
    Pasted(Option<Box<str>>),
    Connected(std::io::Result<(TcpStream, SocketAddr)>),
}

enum Request {
    Launch,
    Callback {
        code: Option<Box<str>>,
        state: Option<Box<str>>,
        denied: bool,
    },
}

/// Reads and parses one request's head, within 2 s of the connection.
async fn request(stream: &mut TcpStream, port: u16, launch: &str) -> Result<Request, ()> {
    let bytes = tokio::time::timeout(REQUEST_LIFETIME, head(stream))
        .await
        .map_err(|_| ())??;
    parse(&bytes, port, launch)
}

/// The bytes of a request up to the blank line ending its head, refused past
/// [`MAX_HEADERS`].
async fn head(stream: &mut TcpStream) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).await.map_err(|_| ())?;
        if read == 0 || bytes.len().saturating_add(read) > MAX_HEADERS {
            return Err(());
        }
        bytes.extend_from_slice(buffer.get(..read).ok_or(())?);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(bytes);
        }
    }
}

fn parse(bytes: &[u8], port: u16, launch: &str) -> Result<Request, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
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

async fn respond_redirect(stream: &mut TcpStream, location: &str) {
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n"
    );
    written(stream, response.as_bytes()).await;
}

async fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    written(stream, response.as_bytes()).await;
}

/// Writes an answer, giving a client that will not take it 2 s.
async fn written(stream: &mut TcpStream, answer: &[u8]) {
    let _ = tokio::time::timeout(REQUEST_LIFETIME, stream.write_all(answer)).await;
}

#[cfg(test)]
mod tests;
