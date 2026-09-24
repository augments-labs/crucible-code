//! `ChatGPT` account login and the credential used by the Codex endpoint.
//!
//! Browser PKCE is the ordinary local login. Device authorization is a
//! separate method for remote and headless use, matching the choice exposed by
//! the official Codex client. Both methods produce the same stored credential
//! and share one login slot, running as a task on the runtime [`Renewals`]
//! was given.
//!
//! Every request goes through the client [`Renewals`] owns, within 30 s end
//! to end. A credential's renewal is a rotation that owner runs for the
//! account's scope; the credential only awaits it.

mod callback;

pub(super) use callback::PORTS;

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use crucible_core::{Authorization, Credential, CredentialError, CredentialScopeId, Outgoing};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc::Receiver;

use super::{
    LoginAttempt, LoginMethod, LoginSlot, LoginUpdate, LoginUpdates, OAuthError, Renewals,
    SubscriptionLogin, Tokens, credential_scope, held, renewal::Due,
};
use crate::{Store, StoredCredentials};

const ISSUER: &str = "https://auth.openai.com";
pub(super) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub(super) const VERIFY: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT: &str = "https://auth.openai.com/deviceauth/callback";
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const LOGIN_LIFETIME: Duration = Duration::from_mins(15);
const REQUEST_LIFETIME: Duration = Duration::from_secs(30);
const REFRESH_AFTER: u64 = 8 * 24 * 60 * 60;
const EXPIRY_SKEW: u64 = 5 * 60;
const ACCOUNT: &str = "account_id";

/// OpenAI's `ChatGPT` account login.
#[derive(Clone)]
pub struct OpenAiOAuth {
    shared: Arc<Shared>,
}

struct Shared {
    flow: Flow,
    worker: LoginSlot,
}

impl OpenAiOAuth {
    /// Browser PKCE through a local loopback callback.
    pub const BROWSER: LoginMethod = LoginMethod::new("browser");
    /// Device authorization for remote or headless terminals.
    pub const DEVICE: LoginMethod = LoginMethod::new("device");

    /// Production `ChatGPT` login methods, renewing through `renewals`.
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
    pub(super) fn testing(flow: Flow) -> Self {
        Self {
            shared: Arc::new(Shared {
                flow,
                worker: LoginSlot::new(),
            }),
        }
    }

    fn start_method(&self, method: LoginMethod, store: Store) -> Result<LoginAttempt, OAuthError> {
        if !matches!(method, Self::BROWSER | Self::DEVICE) {
            return Err(OAuthError::Method);
        }
        let flow = self.shared.flow.clone();
        let runtime = flow
            .renewals
            .runtime()
            .ok_or(OAuthError::NotStarted)?
            .clone();
        self.shared
            .worker
            .start_with_input(&runtime, move |updates, mut input| async move {
                if let Err(problem) = flow.login(method, &store, &updates, &mut input).await {
                    let _ = updates.send(Err(problem));
                }
            })
    }

    fn stored_credential(&self, stored: &StoredCredentials) -> Option<Box<dyn Credential>> {
        let (store, tokens) = stored.subscription(self.provider())?;
        Some(Box::new(OpenAiCredential::new(
            store,
            tokens,
            self.shared.flow.clone(),
        )))
    }
}

impl fmt::Debug for OpenAiOAuth {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("OpenAiOAuth").finish_non_exhaustive()
    }
}

impl SubscriptionLogin for OpenAiOAuth {
    fn provider(&self) -> &'static str {
        "openai"
    }

    fn start(&self, method: LoginMethod, store: Store) -> Result<LoginAttempt, OAuthError> {
        self.start_method(method, store)
    }

    fn credential(&self, stored: &StoredCredentials) -> Option<Box<dyn Credential>> {
        self.stored_credential(stored)
    }
}

/// A stored `ChatGPT` credential, refreshed at the request boundary.
pub struct OpenAiCredential {
    store: Store,
    flow: Flow,
    tokens: Mutex<Tokens>,
    scope: CredentialScopeId,
    identity_bound: bool,
}

impl OpenAiCredential {
    pub(crate) fn new(store: Store, tokens: Tokens, flow: Flow) -> Self {
        let durable = credential_scope(b"openai-account", tokens.detail(ACCOUNT));
        Self {
            store,
            flow,
            tokens: Mutex::new(tokens),
            scope: durable.unwrap_or_default(),
            identity_bound: durable.is_some(),
        }
    }
}

impl Credential for OpenAiCredential {
    fn scope(&self) -> CredentialScopeId {
        self.scope
    }

    fn authorize<'a>(&'a self, request: &'a mut Outgoing) -> Authorization<'a> {
        Box::pin(async move {
            {
                let mut tokens = held(&self.tokens);
                if needs_refresh(&tokens, now())
                    && let Some(latest) = self.flow.renewals.latest("openai", self.scope)
                    && !needs_refresh(&latest, now())
                {
                    *tokens = latest;
                }
                if !needs_refresh(&tokens, now()) {
                    apply(&tokens, request);
                    return Ok(());
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
                    provider: "openai",
                    store: self.store.clone(),
                    needs_refresh,
                    refresh: Box::new(move |current| {
                        Box::pin(async move {
                            let refreshed = flow.refresh(&current).await?;
                            if bound
                                && credential_scope(b"openai-account", refreshed.detail(ACCOUNT))
                                    != Some(scope)
                            {
                                return Err(OAuthError::Invalid {
                                    step: "refreshed account identity",
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
            apply(&tokens, request);
            Ok(())
        })
    }
}

impl fmt::Debug for OpenAiCredential {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("OpenAiCredential")
            .field("tokens", &"<redacted>")
            .finish_non_exhaustive()
    }
}

fn apply(tokens: &Tokens, request: &mut Outgoing) {
    request.protect(tokens.access().to_owned());
    request.set_header("authorization", format!("Bearer {}", tokens.access()));
    request.set_header("originator", "crucible-code");
    if let Some(account) = tokens.detail(ACCOUNT) {
        request.protect(account.to_owned());
        request.set_header("chatgpt-account-id", account.to_owned());
    }
}

fn needs_refresh(tokens: &Tokens, at: u64) -> bool {
    tokens.needs_refresh(at, EXPIRY_SKEW, REFRESH_AFTER)
}

#[derive(Clone)]
pub(crate) struct Flow {
    /// The owner of this account's renewals, whose client every request goes
    /// through and whose runtime a login runs on.
    renewals: Renewals,
    /// How long one request may take, from waiting for a connection to the
    /// last byte of its answer.
    request_lifetime: Duration,
    issuer: Box<str>,
    device_code: Box<str>,
    device_token: Box<str>,
    token: Box<str>,
    minimum_interval: Duration,
    login_lifetime: Duration,
    /// The loopback ports a browser login may be answered on.
    callback_ports: &'static [u16],
}

impl Flow {
    pub(super) fn production(renewals: Renewals) -> Self {
        Self {
            renewals,
            request_lifetime: REQUEST_LIFETIME,
            issuer: ISSUER.into(),
            device_code: format!("{ISSUER}/api/accounts/deviceauth/usercode").into(),
            device_token: format!("{ISSUER}/api/accounts/deviceauth/token").into(),
            token: format!("{ISSUER}/oauth/token").into(),
            minimum_interval: Duration::from_secs(1),
            login_lifetime: LOGIN_LIFETIME,
            callback_ports: &PORTS,
        }
    }

    #[cfg(test)]
    pub(super) fn testing(base: &str, renewals: &Renewals) -> Self {
        Self::testing_within(base, renewals, crate::oauth::PATIENCE)
    }

    /// A test flow whose requests are each given `request_lifetime`.
    #[cfg(test)]
    pub(super) fn testing_within(
        base: &str,
        renewals: &Renewals,
        request_lifetime: Duration,
    ) -> Self {
        Self {
            renewals: renewals.clone(),
            request_lifetime,
            issuer: base.into(),
            device_code: format!("{base}/api/accounts/deviceauth/usercode").into(),
            device_token: format!("{base}/api/accounts/deviceauth/token").into(),
            token: format!("{base}/oauth/token").into(),
            minimum_interval: Duration::from_millis(1),
            login_lifetime: crate::oauth::PATIENCE,
            // An ephemeral port: the registered pair belongs to the host, and a
            // test that claimed it would fail whenever something else had it.
            callback_ports: &[0],
        }
    }

    /// The loopback ports this flow will answer a browser login on.
    #[cfg(test)]
    pub(super) fn callback_ports(&self) -> &[u16] {
        self.callback_ports
    }

    async fn login(
        &self,
        method: LoginMethod,
        store: &Store,
        updates: &LoginUpdates,
        input: &mut Receiver<Box<str>>,
    ) -> Result<(), OAuthError> {
        let tokens = if method == OpenAiOAuth::BROWSER {
            self.browser(updates, input).await?
        } else if method == OpenAiOAuth::DEVICE {
            self.device(updates).await?
        } else {
            return Err(OAuthError::Method);
        };
        let store = store.clone();
        self.renewals
            .login_store(move || store.keep_subscription("openai", tokens))
            .await?;
        updates.send(Ok(LoginUpdate::Complete))
    }

    async fn browser(
        &self,
        updates: &LoginUpdates,
        input: &mut Receiver<Box<str>>,
    ) -> Result<Tokens, OAuthError> {
        let pkce = Pkce::new()?;
        let state = random_urlsafe::<32>()?;
        let callback = callback::Server::bind(self.callback_ports, self.login_lifetime)?;
        let redirect = callback.redirect_uri();
        let authorization = authorization_uri(&self.issuer, &redirect, &pkce.challenge, &state);
        updates.send(Ok(LoginUpdate::Authorize {
            browser_uri: authorization.clone().into(),
            shown_uri: callback.launch_uri().into(),
            user_code: None,
            manual: true,
        }))?;
        let code = callback.wait(&authorization, &state, input).await?;
        updates.send(Ok(LoginUpdate::Progress {
            message: "finishing browser authorization…",
        }))?;
        self.renewals
            .login_request(self.exchange(&code, &pkce.verifier, &redirect))
            .await
    }

    async fn device(&self, updates: &LoginUpdates) -> Result<Tokens, OAuthError> {
        let device = self.renewals.login_request(self.request_device()).await?;
        updates.send(Ok(LoginUpdate::Authorize {
            browser_uri: VERIFY.into(),
            shown_uri: VERIFY.into(),
            user_code: Some(device.user_code.clone()),
            manual: false,
        }))?;
        let authorized = self.poll(&device).await?;
        updates.send(Ok(LoginUpdate::Progress {
            message: "finishing device authorization…",
        }))?;
        self.renewals
            .login_request(self.exchange(&authorized.code, &authorized.verifier, DEVICE_REDIRECT))
            .await
    }

    async fn request_device(&self) -> Result<Device, OAuthError> {
        let body = serde_json::json!({ "client_id": CLIENT_ID }).to_string();
        let (status, response) = self
            .post(&self.device_code, "application/json", body)
            .await?;
        if status != 200 {
            return Err(OAuthError::Refused { status });
        }
        let value: serde_json::Value =
            serde_json::from_str(&response).map_err(|_| OAuthError::Invalid {
                step: "device authorization",
            })?;
        Ok(Device {
            id: required(&value, "device_auth_id", 512, "device authorization")?.into(),
            user_code: required(&value, "user_code", 128, "device authorization")?.into(),
            interval: interval(&value).max(self.minimum_interval),
        })
    }

    async fn poll(&self, device: &Device) -> Result<Authorized, OAuthError> {
        let started = Instant::now();
        loop {
            if started.elapsed() >= self.login_lifetime {
                return Err(OAuthError::Expired);
            }
            let body = serde_json::json!({
                "device_auth_id": &device.id,
                "user_code": &device.user_code,
            })
            .to_string();
            let (status, response) = self
                .renewals
                .login_request(self.post(&self.device_token, "application/json", body))
                .await?;
            if status == 200 {
                let value: serde_json::Value =
                    serde_json::from_str(&response).map_err(|_| OAuthError::Invalid {
                        step: "device token",
                    })?;
                return Ok(Authorized {
                    code: required(&value, "authorization_code", 4096, "device token")?.into(),
                    verifier: required(&value, "code_verifier", 4096, "device token")?.into(),
                });
            }
            if !matches!(status, 403 | 404) {
                return Err(OAuthError::Refused { status });
            }
            tokio::time::sleep(device.interval).await;
        }
    }

    async fn exchange(
        &self,
        code: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<Tokens, OAuthError> {
        let body = form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ]);
        let (status, response) = self
            .post(&self.token, "application/x-www-form-urlencoded", body)
            .await?;
        if status != 200 {
            return Err(OAuthError::Refused { status });
        }
        token_response(&response, None)
    }

    pub(crate) async fn refresh(&self, previous: &Tokens) -> Result<Tokens, OAuthError> {
        let body = serde_json::json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": previous.refresh(),
        })
        .to_string();
        let (status, response) = self.post(&self.token, "application/json", body).await?;
        if status != 200 {
            return Err(OAuthError::Refused { status });
        }
        token_response(&response, Some(previous))
    }

    /// One request, with the two headers every one of them carries. Nothing
    /// names a user agent, so the client's default is sent, as before.
    async fn post(
        &self,
        url: &str,
        content_type: &'static str,
        body: String,
    ) -> Result<(u16, String), OAuthError> {
        let mut headers = Outgoing::new();
        headers.set_header("content-type", content_type);
        headers.set_header("accept", "application/json");
        self.renewals
            .post(url, headers, body, self.request_lifetime)
            .await
    }
}

struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    fn new() -> Result<Self, OAuthError> {
        let verifier = random_urlsafe::<64>()?;
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Ok(Self {
            verifier,
            challenge,
        })
    }
}

fn random_urlsafe<const N: usize>() -> Result<String, OAuthError> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|_| OAuthError::Random)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn authorization_uri(issuer: &str, redirect: &str, challenge: &str, state: &str) -> String {
    let query = form(&[
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect),
        ("scope", SCOPE),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "crucible-code"),
    ]);
    format!("{}/oauth/authorize?{query}", issuer.trim_end_matches('/'))
}

struct Device {
    id: Box<str>,
    user_code: Box<str>,
    interval: Duration,
}

struct Authorized {
    code: Box<str>,
    verifier: Box<str>,
}

fn token_response(response: &str, previous: Option<&Tokens>) -> Result<Tokens, OAuthError> {
    let value: serde_json::Value =
        serde_json::from_str(response).map_err(|_| OAuthError::Invalid {
            step: "token exchange",
        })?;
    let access = optional(&value, "access_token", 32 * 1024)
        .or_else(|| previous.map(Tokens::access))
        .ok_or(OAuthError::Invalid {
            step: "token exchange",
        })?;
    let refresh = optional(&value, "refresh_token", 32 * 1024)
        .or_else(|| previous.map(Tokens::refresh))
        .ok_or(OAuthError::Invalid {
            step: "token exchange",
        })?;
    let identity = optional(&value, "id_token", 32 * 1024);
    let at = now();
    let expires_at = expiration(access).unwrap_or_else(|| at.saturating_add(60 * 60));
    let mut tokens = Tokens::new(access.into(), refresh.into(), expires_at, at);
    if let Some(previous) = previous {
        tokens.replace_details(previous.details().clone());
    }
    if let Some(account) = identity.and_then(account).or_else(|| account(access)) {
        tokens = tokens.with_detail(ACCOUNT, account);
    }
    Ok(tokens)
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

fn interval(value: &serde_json::Value) -> Duration {
    let seconds = value
        .get("interval")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(5)
        .clamp(1, 60);
    Duration::from_secs(seconds)
}

fn account(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|account| {
            !account.is_empty()
                && account.len() <= 512
                && account.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .map(str::to_owned)
}

fn expiration(jwt: &str) -> Option<u64> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp")?.as_u64()
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
