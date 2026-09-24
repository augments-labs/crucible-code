//! Kimi account authorization and its managed-endpoint credential.
//!
//! `MoonshotAI` publishes an RFC 8628 device flow. Crucible reports its own
//! product, platform and version on every authorization, renewal and model
//! request; it never inherits Kimi Code's identity. A random installation id
//! is kept in the protected auth document and copied into the credential's
//! opaque details so every later request presents the same host identity. The
//! token service and browser authorization page have different fixed origins;
//! both are checked before a response can reach the terminal or browser.
//!
//! Every request goes through the client [`Renewals`] owns, within 30 s end
//! to end. A credential's renewal is a rotation that owner runs for the
//! installation's scope; the credential only awaits it.

use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_core::{
    Authorization, Cancel, Credential, CredentialError, CredentialScopeId, Outgoing,
};

use super::{
    LoginAttempt, LoginMethod, LoginSlot, LoginUpdate, OAuthError, Renewals, SubscriptionLogin,
    Tokens, credential_scope, held, renewal::Due,
};
use crate::{Store, StoredCredentials};

const HOST: &str = "https://auth.kimi.com";
const VERIFY: &str = "https://www.kimi.com";
const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const LOGIN_LIFETIME: Duration = Duration::from_mins(15);
const REQUEST_LIFETIME: Duration = Duration::from_secs(30);
const CANCEL_POLL: Duration = Duration::from_millis(50);
const MINIMUM_REFRESH: u64 = 5 * 60;
const DEVICE_ID: &str = "device_id";
const EXPIRES_IN: &str = "expires_in";

/// `MoonshotAI`'s Kimi account login.
#[derive(Clone)]
pub struct KimiOAuth {
    shared: Arc<Shared>,
}

struct Shared {
    flow: Flow,
    worker: LoginSlot,
}

impl KimiOAuth {
    /// Device authorization in a browser.
    pub const DEVICE: LoginMethod = LoginMethod::new("device");

    /// Production Kimi login, renewing through `renewals`.
    #[must_use]
    pub fn new(renewals: Renewals) -> Self {
        Self {
            shared: Arc::new(Shared {
                flow: Flow::production(renewals),
                worker: LoginSlot::new(),
            }),
        }
    }

    #[cfg(test)]
    fn testing(flow: Flow) -> Self {
        Self {
            shared: Arc::new(Shared {
                flow,
                worker: LoginSlot::new(),
            }),
        }
    }

    fn start_method(&self, method: LoginMethod, store: Store) -> Result<LoginAttempt, OAuthError> {
        if method != Self::DEVICE {
            return Err(OAuthError::Method);
        }
        let flow = self.shared.flow.clone();
        self.shared
            .worker
            .start("crucible-kimi-login", move |cancel, updates| {
                if let Err(problem) = flow.login(&store, &cancel, &updates) {
                    let _ = updates.send(Err(problem));
                }
            })
    }

    fn stored_credential(&self, stored: &StoredCredentials) -> Option<Box<dyn Credential>> {
        let (store, tokens) = stored.subscription(self.provider())?;
        Some(Box::new(KimiCredential::new(
            store,
            tokens,
            self.shared.flow.clone(),
        )))
    }
}

impl fmt::Debug for KimiOAuth {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("KimiOAuth").finish_non_exhaustive()
    }
}

impl SubscriptionLogin for KimiOAuth {
    fn provider(&self) -> &'static str {
        "moonshot"
    }

    fn start(&self, method: LoginMethod, store: Store) -> Result<LoginAttempt, OAuthError> {
        self.start_method(method, store)
    }

    fn credential(&self, stored: &StoredCredentials) -> Option<Box<dyn Credential>> {
        self.stored_credential(stored)
    }
}

/// A renewable Kimi credential carrying Crucible's honest host identity.
pub struct KimiCredential {
    store: Store,
    flow: Flow,
    tokens: Mutex<Tokens>,
    scope: CredentialScopeId,
    identity_bound: bool,
}

impl KimiCredential {
    fn new(store: Store, tokens: Tokens, flow: Flow) -> Self {
        let durable = credential_scope(b"moonshot-device", tokens.detail(DEVICE_ID));
        Self {
            store,
            flow,
            tokens: Mutex::new(tokens),
            scope: durable.unwrap_or_default(),
            identity_bound: durable.is_some(),
        }
    }
}

impl Credential for KimiCredential {
    fn scope(&self) -> CredentialScopeId {
        self.scope
    }

    fn authorize<'a>(&'a self, request: &'a mut Outgoing) -> Authorization<'a> {
        Box::pin(async move {
            {
                let mut tokens = held(&self.tokens);
                if needs_refresh(&tokens, now())
                    && let Some(latest) = self.flow.renewals.latest("moonshot", self.scope)
                    && !needs_refresh(&latest, now())
                {
                    *tokens = latest;
                }
                if !needs_refresh(&tokens, now()) {
                    return apply(&tokens, request);
                }
            }

            // Awaited with nothing held: the rotation is the owner's task, and
            // this future only waits for it, so dropping it stops the wait and
            // not the rotation.
            let flow = self.flow.clone();
            let (scope, bound) = (self.scope, self.identity_bound);
            let fresh = self
                .flow
                .renewals
                .renew(Due {
                    scope,
                    provider: "moonshot",
                    store: self.store.clone(),
                    needs_refresh,
                    refresh: Box::new(move |current| {
                        Box::pin(async move {
                            let refreshed = flow.refresh(&current).await?;
                            if bound
                                && credential_scope(b"moonshot-device", refreshed.detail(DEVICE_ID))
                                    != Some(scope)
                            {
                                return Err(OAuthError::Invalid {
                                    step: "refreshed installation identity",
                                });
                            }
                            Ok(refreshed)
                        })
                    }),
                })
                .await
                .map_err(|problem| CredentialError::NotRenewed(problem.to_string().into()))?;

            let mut tokens = held(&self.tokens);
            *tokens = fresh;
            apply(&tokens, request)
        })
    }
}

impl fmt::Debug for KimiCredential {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("KimiCredential")
            .field("tokens", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Applies the installation's identity and the access token.
fn apply(tokens: &Tokens, request: &mut Outgoing) -> Result<(), CredentialError> {
    let identity = Identity::from_tokens(tokens)
        .map_err(|problem| CredentialError::NotRenewed(problem.to_string().into()))?;
    identity.apply(request);
    request.protect(tokens.access().to_owned());
    request.set_header("authorization", format!("Bearer {}", tokens.access()));
    Ok(())
}

fn needs_refresh(tokens: &Tokens, at: u64) -> bool {
    let lifetime = tokens
        .detail(EXPIRES_IN)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(MINIMUM_REFRESH);
    tokens.needs_refresh(at, MINIMUM_REFRESH.max(lifetime / 2), 0)
}

#[derive(Clone)]
struct Flow {
    /// The owner of this installation's renewals, whose client every request
    /// goes through and whose runtime a login step waits on.
    renewals: Renewals,
    /// How long one request may take, from waiting for a connection to the
    /// last byte of its answer.
    request_lifetime: Duration,
    verification: Box<str>,
    authorize: Box<str>,
    token: Box<str>,
    minimum_interval: Duration,
    login_lifetime: Duration,
}

impl Flow {
    fn production(renewals: Renewals) -> Self {
        Self::at(
            renewals,
            HOST,
            VERIFY,
            REQUEST_LIFETIME,
            LOGIN_LIFETIME,
            Duration::from_secs(1),
        )
    }

    // Six settings of one flow, each a different kind of thing and each named
    // at its two call sites; a struct holding them would only rename them.
    #[allow(clippy::too_many_arguments)]
    fn at(
        renewals: Renewals,
        host: &str,
        verification: &str,
        request: Duration,
        login: Duration,
        interval: Duration,
    ) -> Self {
        let host = host.trim_end_matches('/');
        Self {
            renewals,
            request_lifetime: request,
            verification: verification.trim_end_matches('/').into(),
            authorize: format!("{host}/api/oauth/device_authorization").into(),
            token: format!("{host}/api/oauth/token").into(),
            minimum_interval: interval,
            login_lifetime: login,
        }
    }

    #[cfg(test)]
    fn testing(host: &str, renewals: &Renewals) -> Self {
        Self::at(
            renewals.clone(),
            host,
            host,
            crate::oauth::PATIENCE,
            crate::oauth::PATIENCE,
            Duration::from_millis(1),
        )
    }

    fn login(
        &self,
        store: &Store,
        cancel: &Cancel,
        updates: &mpsc::SyncSender<Result<LoginUpdate, OAuthError>>,
    ) -> Result<(), OAuthError> {
        let identity = Identity::for_login(store)?;
        let started = Instant::now();
        loop {
            if cancel.requested() {
                return Err(OAuthError::Cancelled);
            }
            if started.elapsed() >= self.login_lifetime {
                return Err(OAuthError::Expired);
            }
            let device = self
                .renewals
                .login_step(cancel, self.request_device(&identity))?;
            let issued = Instant::now();
            updates
                .send(Ok(LoginUpdate::Authorize {
                    browser_uri: device.complete.clone(),
                    shown_uri: device.verification.clone(),
                    user_code: Some(device.user_code.clone()),
                    manual: false,
                }))
                .map_err(|_| OAuthError::Cancelled)?;
            let time = LoginTime { started, issued };
            if let Some(tokens) = self.poll(&device, &identity, time, cancel)? {
                store.keep_subscription("moonshot", tokens)?;
                return updates
                    .send(Ok(LoginUpdate::Complete))
                    .map_err(|_| OAuthError::Cancelled);
            }
        }
    }

    async fn request_device(&self, identity: &Identity) -> Result<Device, OAuthError> {
        let (status, response) = self
            .post(&self.authorize, &[("client_id", CLIENT_ID)], identity)
            .await?;
        if status != 200 {
            return Err(OAuthError::Refused { status });
        }
        let value = json(&response, "device authorization")?;
        let complete = required(
            &value,
            "verification_uri_complete",
            4096,
            "device authorization",
        )?;
        let verification = optional(&value, "verification_uri", 4096).unwrap_or(complete);
        if !within(&self.verification, verification) || !within(&self.verification, complete) {
            return Err(OAuthError::Invalid {
                step: "device authorization URI",
            });
        }
        Ok(Device {
            code: required(&value, "device_code", 4096, "device authorization")?.into(),
            user_code: required(&value, "user_code", 128, "device authorization")?.into(),
            verification: verification.into(),
            complete: complete.into(),
            interval: seconds(&value, "interval").unwrap_or(5).clamp(1, 60),
            expires_in: seconds(&value, "expires_in").filter(|seconds| *seconds > 0),
        })
    }

    fn poll(
        &self,
        device: &Device,
        identity: &Identity,
        time: LoginTime,
        cancel: &Cancel,
    ) -> Result<Option<Tokens>, OAuthError> {
        let mut interval = Duration::from_secs(device.interval).max(self.minimum_interval);
        loop {
            if cancel.requested() {
                return Err(OAuthError::Cancelled);
            }
            let expired_by_server = device
                .expires_in
                .is_some_and(|lifetime| time.issued.elapsed() >= Duration::from_secs(lifetime));
            if time.started.elapsed() >= self.login_lifetime || expired_by_server {
                return if time.started.elapsed() >= self.login_lifetime {
                    Err(OAuthError::Expired)
                } else {
                    Ok(None)
                };
            }
            let fields = [
                ("client_id", CLIENT_ID),
                ("device_code", device.code.as_ref()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ];
            let (status, response) = self
                .renewals
                .login_step(cancel, self.post(&self.token, &fields, identity))?;
            let value = json(&response, "device token")?;
            if status == 200 {
                return token_response(&value, identity, None).map(Some);
            }
            match optional(&value, "error", 128) {
                Some("authorization_pending") => {}
                Some("slow_down") => interval = interval.saturating_add(Duration::from_secs(5)),
                Some("expired_token") => return Ok(None),
                Some("access_denied") => return Err(OAuthError::Denied),
                _ => return Err(OAuthError::Refused { status }),
            }
            pause(interval, cancel)?;
        }
    }

    async fn refresh(&self, previous: &Tokens) -> Result<Tokens, OAuthError> {
        let identity = Identity::from_tokens(previous)?;
        let fields = [
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", previous.refresh()),
        ];
        let (status, response) = self.post(&self.token, &fields, &identity).await?;
        if status != 200 {
            return Err(OAuthError::Refused { status });
        }
        token_response(
            &json(&response, "token renewal")?,
            &identity,
            Some(previous),
        )
    }

    /// One form request, with the installation's seven identity headers.
    async fn post(
        &self,
        url: &str,
        fields: &[(&str, &str)],
        identity: &Identity,
    ) -> Result<(u16, String), OAuthError> {
        let mut headers = Outgoing::new();
        headers.set_header("content-type", "application/x-www-form-urlencoded");
        headers.set_header("accept", "application/json");
        for (name, value) in identity.headers() {
            headers.set_header(name, value.to_owned());
        }
        self.renewals
            .post(url, headers, form(fields), self.request_lifetime)
            .await
    }
}

struct Device {
    code: Box<str>,
    user_code: Box<str>,
    verification: Box<str>,
    complete: Box<str>,
    interval: u64,
    expires_in: Option<u64>,
}

#[derive(Clone, Copy)]
struct LoginTime {
    started: Instant,
    issued: Instant,
}

#[derive(Clone)]
struct Identity {
    device_id: Box<str>,
    user_agent: Box<str>,
    model: Box<str>,
}

impl Identity {
    fn for_login(store: &Store) -> Result<Self, OAuthError> {
        let candidate = random_id()?;
        let device_id = store.identity("moonshot", &candidate)?;
        Self::new(device_id)
    }

    fn from_tokens(tokens: &Tokens) -> Result<Self, OAuthError> {
        let device_id = tokens.detail(DEVICE_ID).ok_or(OAuthError::Invalid {
            step: "stored Kimi identity",
        })?;
        Self::new(device_id.to_owned())
    }

    fn new(device_id: String) -> Result<Self, OAuthError> {
        if !valid_id(&device_id) {
            return Err(OAuthError::Invalid {
                step: "stored Kimi identity",
            });
        }
        Ok(Self {
            device_id: device_id.into(),
            user_agent: format!("crucible-code/{}", env!("CARGO_PKG_VERSION")).into(),
            model: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH).into(),
        })
    }

    fn headers(&self) -> [(&'static str, &str); 7] {
        [
            ("user-agent", &self.user_agent),
            ("x-msh-platform", "crucible-code"),
            ("x-msh-version", env!("CARGO_PKG_VERSION")),
            ("x-msh-device-name", "unknown"),
            ("x-msh-device-model", &self.model),
            ("x-msh-os-version", "unknown"),
            ("x-msh-device-id", &self.device_id),
        ]
    }

    fn apply(&self, request: &mut Outgoing) {
        request.protect(self.device_id.clone());
        for (name, value) in self.headers() {
            request.set_header(name, value.to_owned());
        }
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Identity")
            .field("device_id", &"<redacted>")
            .field("user_agent", &self.user_agent)
            .field("model", &self.model)
            .finish()
    }
}

fn token_response(
    value: &serde_json::Value,
    identity: &Identity,
    previous: Option<&Tokens>,
) -> Result<Tokens, OAuthError> {
    let access = required(value, "access_token", 32 * 1024, "token response")?;
    let refresh = optional(value, "refresh_token", 32 * 1024)
        .or_else(|| previous.map(Tokens::refresh))
        .ok_or(OAuthError::Invalid {
            step: "token response",
        })?;
    let expires_in = seconds(value, "expires_in")
        .filter(|seconds| *seconds > 0 && *seconds <= 365 * 24 * 60 * 60)
        .ok_or(OAuthError::Invalid {
            step: "token response",
        })?;
    let at = now();
    Ok(Tokens::new(
        access.into(),
        refresh.into(),
        at.saturating_add(expires_in),
        at,
    )
    .with_detail(DEVICE_ID, identity.device_id.to_string())
    .with_detail(EXPIRES_IN, expires_in.to_string()))
}

fn json(response: &str, step: &'static str) -> Result<serde_json::Value, OAuthError> {
    serde_json::from_str(response).map_err(|_| OAuthError::Invalid { step })
}

fn required<'a>(
    value: &'a serde_json::Value,
    field: &str,
    maximum: usize,
    step: &'static str,
) -> Result<&'a str, OAuthError> {
    optional(value, field, maximum).ok_or(OAuthError::Invalid { step })
}

fn optional<'a>(value: &'a serde_json::Value, field: &str, maximum: usize) -> Option<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= maximum)
}

fn seconds(value: &serde_json::Value, field: &str) -> Option<u64> {
    value.get(field).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

fn within(host: &str, uri: &str) -> bool {
    uri.strip_prefix(host)
        .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('?'))
}

fn pause(duration: Duration, cancel: &Cancel) -> Result<(), OAuthError> {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        if cancel.requested() {
            return Err(OAuthError::Cancelled);
        }
        std::thread::sleep(CANCEL_POLL.min(until.saturating_duration_since(Instant::now())));
    }
    Ok(())
}

fn random_id() -> Result<String, OAuthError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| OAuthError::Random)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn valid_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(at, byte)| {
            if matches!(at, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{}={}", encoded(name), encoded(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn encoded(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests;
