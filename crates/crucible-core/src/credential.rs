//! Credentials, and the one place a secret is allowed to exist.
//!
//! Authentication is a separate axis from the wire protocol. A provider is
//! handed something implementing [`Credential`] and never learns whether it
//! came from an environment variable, a keyring or a subscription login, so a
//! new way to authenticate is a new implementation here rather than an edit to
//! every provider.
//!
//! The secret is *applied*, never returned. [`Credential::authorize`] takes the
//! outgoing request and writes into it, so no caller ever holds the value and
//! there is no accessor to forget to keep out of a log line.

use std::fmt;

/// Why a credential could not be resolved or applied.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// The environment variable naming the key is unset or empty.
    ///
    /// Carries the variable *name*, never its value.
    #[error("{0} is not set")]
    NotInEnvironment(Box<str>),
}

/// An API key.
///
/// `Debug` is written by hand and redacts; there is deliberately no `Display`,
/// no `as_str`, and no `Serialize`. The only thing that can be done with one is
/// to apply it to a request.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    /// Takes a key that is already in memory. Used by wiring that resolved the
    /// value some other way.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Writes the key into a request the way `header` says to.
    ///
    /// The value is formatted here and never returned, so this stays the only
    /// place the secret is read.
    pub fn apply(&self, request: &mut Outgoing, header: &Header) {
        request.set_header(&*header.name, format!("{}{}", header.scheme, self.0));
    }

    /// A key from the environment, with the lookup passed in.
    ///
    /// The lookup is a parameter so the "unset" and "blank" rules can be tested
    /// without mutating the process environment — which in edition 2024 is
    /// `unsafe`, and this crate forbids that outright. Configuration stores the
    /// variable *name*, never the value, so this is how a key enters the
    /// process.
    ///
    /// # Errors
    ///
    /// [`CredentialError::NotInEnvironment`] if `lookup` finds nothing or finds
    /// only whitespace.
    pub fn from_lookup(
        variable: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, CredentialError> {
        match lookup(variable) {
            Some(value) if !value.trim().is_empty() => Ok(Self(value)),
            _ => Err(CredentialError::NotInEnvironment(variable.into())),
        }
    }
}

/// Where a key goes in a request, and what has to sit in front of it there.
///
/// One value rather than two strings side by side. Anthropic sends
/// `x-api-key: <key>` and OpenAI sends `authorization: Bearer <key>`, so both
/// halves would otherwise be `&str` arguments in a row — and a call site that
/// puts them the wrong way round compiles, then sends
/// `Bearer : authorization<key>` and gets back an authentication failure that
/// names neither argument.
#[derive(Debug, Clone)]
pub struct Header {
    name: Box<str>,
    /// Written in front of the key inside the header value.
    scheme: &'static str,
}

impl Header {
    /// `<name>: <key>` — the header value is the key and nothing else.
    #[must_use]
    pub fn bare(name: impl Into<Box<str>>) -> Self {
        Self {
            name: name.into(),
            scheme: "",
        }
    }

    /// `authorization: Bearer <key>`, the scheme RFC 6750 defines.
    ///
    /// The header name is not a parameter because the scheme is only defined
    /// for that one, and a bearer token sent anywhere else is a mistake rather
    /// than a variation.
    #[must_use]
    pub fn bearer() -> Self {
        Self {
            name: "authorization".into(),
            scheme: "Bearer ",
        }
    }
}

/// An API key sent as a header.
///
/// One type serves both wire protocols: the provider supplies the [`Header`]
/// when it builds this, and the key itself never leaves [`ApiKey`].
#[derive(Debug, Clone)]
pub struct HeaderKey {
    key: ApiKey,
    header: Header,
}

impl HeaderKey {
    /// Builds a credential that sends `key` the way `header` says to.
    #[must_use]
    pub fn new(key: ApiKey, header: Header) -> Self {
        Self { key, header }
    }
}

impl Credential for HeaderKey {
    fn authorize(&self, request: &mut Outgoing) -> Result<(), CredentialError> {
        self.key.apply(request, &self.header);
        Ok(())
    }
}

impl fmt::Debug for ApiKey {
    /// Redacts. A key must not reach a log, an error, a session file or a
    /// panic payload, and derived `Debug` is how it would reach all four.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// A request on its way out, before a provider hands it to the transport.
///
/// Providers build one of these and pass it to a credential. Its `Debug`
/// redacts every header value, because the credential's whole job is to put a
/// secret into one of them.
#[derive(Default, Clone)]
pub struct Outgoing {
    headers: Vec<(Box<str>, Box<str>)>,
}

impl Outgoing {
    /// An outgoing request with no headers yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a header, replacing any previous value for the same name.
    pub fn set_header(&mut self, name: impl Into<Box<str>>, value: impl Into<Box<str>>) {
        let name = name.into();
        let value = value.into();
        match self
            .headers
            .iter_mut()
            .find(|(existing, _)| *existing == name)
        {
            Some(slot) => slot.1 = value,
            None => self.headers.push((name, value)),
        }
    }

    /// The headers, for the transport to send. This is the only way a value
    /// comes back out, and it is called by the code that is about to put the
    /// bytes on the socket.
    #[must_use]
    pub fn headers(&self) -> &[(Box<str>, Box<str>)] {
        &self.headers
    }
}

impl fmt::Debug for Outgoing {
    /// Names every header and shows no value. One of them is a secret and this
    /// type cannot tell which, so it treats all of them as one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = f.debug_struct("Outgoing");
        for (name, _) in &self.headers {
            out.field(name, &"<redacted>");
        }
        out.finish()
    }
}

/// Applies authentication to an outgoing request.
///
/// Takes the request rather than returning the secret, so the secret never
/// becomes a value a caller can hold, format or store.
pub trait Credential: Send + Sync + fmt::Debug {
    /// Writes whatever this credential needs into the request.
    ///
    /// # Errors
    ///
    /// Implementation-defined; a credential that must be fetched or refreshed
    /// can fail here.
    fn authorize(&self, request: &mut Outgoing) -> Result<(), CredentialError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact string that must never appear anywhere but the header value.
    const SECRET: &str = "sk-ant-do-not-log-me";

    /// One header's value, by name.
    ///
    /// Looking it up rather than indexing keeps the assertion about the header
    /// the test means, not about the order the headers happen to be in.
    fn header<'a>(request: &'a Outgoing, name: &str) -> &'a str {
        request
            .headers()
            .iter()
            .find(|(present, _)| &**present == name)
            .map_or("<no such header>", |(_, value)| value)
    }

    #[test]
    fn an_api_key_does_not_appear_in_its_debug() {
        let key = ApiKey::new(SECRET);
        let shown = format!("{key:?}");
        assert!(!shown.contains(SECRET), "the key leaked: {shown}");
        assert_eq!(shown, "ApiKey(<redacted>)");
    }

    #[test]
    fn an_api_key_does_not_leak_through_a_container() {
        // The realistic leak is not `{:?}` on the key itself — it is the key
        // sitting inside something else that derived `Debug`.
        let held = Some(vec![ApiKey::new(SECRET)]);
        let shown = format!("{held:?}");
        assert!(!shown.contains(SECRET), "the key leaked: {shown}");
    }

    #[test]
    fn a_header_value_does_not_appear_in_the_request_debug() {
        let mut request = Outgoing::new();
        request.set_header("x-api-key", SECRET);
        let shown = format!("{request:?}");
        assert!(!shown.contains(SECRET), "the key leaked: {shown}");
        assert!(
            shown.contains("x-api-key"),
            "the header name is useful: {shown}"
        );
    }

    #[test]
    fn a_header_set_twice_keeps_the_last_value() {
        let mut request = Outgoing::new();
        request.set_header("x-api-key", "first");
        request.set_header("x-api-key", "second");
        assert_eq!(request.headers().len(), 1, "it replaced, not appended");
        assert_eq!(header(&request, "x-api-key"), "second");
    }

    #[test]
    fn a_missing_variable_names_the_variable_and_not_a_value() {
        let err = ApiKey::from_lookup("ANTHROPIC_API_KEY", |_| None).unwrap_err();

        // The name is the whole point: it is what tells a user what to set,
        // and it is safe to print precisely because it is not the value.
        assert_eq!(err.to_string(), "ANTHROPIC_API_KEY is not set");
    }

    #[test]
    fn a_blank_variable_counts_as_unset() {
        // An exported-but-empty variable is the usual shape of a broken shell
        // profile. Treating it as set produces a puzzling 401 instead of the
        // sentence that says what to fix.
        for blank in ["", "   ", "\n", "\t "] {
            let err = ApiKey::from_lookup("KEY", |_| Some(blank.to_owned())).unwrap_err();
            assert!(matches!(err, CredentialError::NotInEnvironment(_)));
        }
    }

    #[test]
    fn a_present_variable_becomes_a_key_that_still_redacts() {
        let key = ApiKey::from_lookup("KEY", |_| Some(SECRET.to_owned())).unwrap();
        assert!(!format!("{key:?}").contains(SECRET));
    }

    #[test]
    fn anthropic_and_openai_send_the_same_key_differently() {
        // One credential type, two wire conventions — this is why a credential
        // is handed a `Header` instead of hard-coding either.
        let key = ApiKey::new(SECRET);

        let mut anthropic = Outgoing::new();
        HeaderKey::new(key.clone(), Header::bare("x-api-key"))
            .authorize(&mut anthropic)
            .unwrap();
        assert_eq!(header(&anthropic, "x-api-key"), SECRET);

        let mut openai = Outgoing::new();
        HeaderKey::new(key, Header::bearer())
            .authorize(&mut openai)
            .unwrap();
        assert_eq!(header(&openai, "authorization"), format!("Bearer {SECRET}"));
    }

    #[test]
    fn a_credential_does_not_leak_the_key_through_its_own_debug() {
        // `Credential` requires `Debug`, so every credential is a print away
        // from a log line. This one holds the key directly.
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bare("x-api-key"));
        let shown = format!("{credential:?}");
        assert!(!shown.contains(SECRET), "the key leaked: {shown}");
    }
}
