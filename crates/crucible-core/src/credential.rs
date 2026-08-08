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

use std::env;
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
    /// Reads a key from the named environment variable.
    ///
    /// Configuration stores the variable *name*, never the value, so this is
    /// how a key enters the process.
    ///
    /// # Errors
    ///
    /// [`CredentialError::NotInEnvironment`] if the variable is unset or empty.
    pub fn from_env(variable: &str) -> Result<Self, CredentialError> {
        Self::from_lookup(variable, |name| env::var(name).ok())
    }

    /// Takes a key that is already in memory. Used by wiring that resolved the
    /// value some other way.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Writes the key into a request under a header the caller names.
    ///
    /// The provider chooses the header and any prefix, because Anthropic sends
    /// `x-api-key: <key>` and OpenAI sends `authorization: Bearer <key>`. The
    /// value is formatted here and never returned, so this stays the only
    /// place the secret is read.
    pub fn apply(&self, request: &mut Outgoing, header: &str, prefix: &str) {
        request.set_header(header, format!("{prefix}{}", self.0));
    }

    /// The environment read, with the lookup passed in.
    ///
    /// Separated so the "unset" and "blank" rules can be tested without
    /// mutating the process environment — which in edition 2024 is `unsafe`,
    /// and this crate forbids that outright.
    fn from_lookup(
        variable: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, CredentialError> {
        match lookup(variable) {
            Some(value) if !value.trim().is_empty() => Ok(Self(value)),
            _ => Err(CredentialError::NotInEnvironment(variable.into())),
        }
    }
}

/// An API key sent as a header.
///
/// One type serves both wire protocols: the provider supplies the header name
/// and the prefix when it builds this, and the key itself never leaves
/// [`ApiKey`].
#[derive(Debug, Clone)]
pub struct HeaderKey {
    key: ApiKey,
    header: Box<str>,
    prefix: Box<str>,
}

impl HeaderKey {
    /// Builds a credential that sends `<header>: <prefix><key>`.
    #[must_use]
    pub fn new(key: ApiKey, header: impl Into<Box<str>>, prefix: impl Into<Box<str>>) -> Self {
        Self {
            key,
            header: header.into(),
            prefix: prefix.into(),
        }
    }
}

impl Credential for HeaderKey {
    fn authorize(&self, request: &mut Outgoing) -> Result<(), CredentialError> {
        self.key.apply(request, &self.header, &self.prefix);
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
    fn the_real_environment_read_reaches_from_lookup() {
        // `from_env` is a one-liner over `from_lookup`, and the tests above
        // exercise `from_lookup`. This is what proves the wiring between them,
        // using a variable no environment sets.
        let variable = "CRUCIBLE_VARIABLE_THAT_IS_NEVER_SET";
        assert!(env::var(variable).is_err(), "the premise of this test");

        let err = ApiKey::from_env(variable).unwrap_err();
        assert_eq!(
            err.to_string(),
            "CRUCIBLE_VARIABLE_THAT_IS_NEVER_SET is not set"
        );
    }

    #[test]
    fn anthropic_and_openai_send_the_same_key_differently() {
        // One credential type, two wire conventions — this is why `apply` takes
        // the header and the prefix instead of hard-coding either.
        let key = ApiKey::new(SECRET);

        let mut anthropic = Outgoing::new();
        HeaderKey::new(key.clone(), "x-api-key", "")
            .authorize(&mut anthropic)
            .unwrap();
        assert_eq!(header(&anthropic, "x-api-key"), SECRET);

        let mut openai = Outgoing::new();
        HeaderKey::new(key, "authorization", "Bearer ")
            .authorize(&mut openai)
            .unwrap();
        assert_eq!(header(&openai, "authorization"), format!("Bearer {SECRET}"));
    }

    #[test]
    fn a_credential_does_not_leak_the_key_through_its_own_debug() {
        // `Credential` requires `Debug`, so every credential is a print away
        // from a log line. This one holds the key directly.
        let credential = HeaderKey::new(ApiKey::new(SECRET), "x-api-key", "");
        let shown = format!("{credential:?}");
        assert!(!shown.contains(SECRET), "the key leaked: {shown}");
    }
}
