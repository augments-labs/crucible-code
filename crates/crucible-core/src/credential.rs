//! Credentials, and the one place a secret is allowed to exist.
//!
//! Authentication is a separate axis from the wire protocol. A provider is
//! handed something implementing [`Credential`] and never learns whether it
//! came from an environment variable, a file, or something else's signature, so
//! a new way to authenticate is a new implementation here rather than an edit
//! to every provider.
//!
//! The secret is *applied*, never returned. [`Credential::authorize`] takes the
//! outgoing request and writes into it, so no caller ever holds the value and
//! there is no accessor to forget to keep out of a log line.
//!
//! That call runs on the way into every request rather than once at startup,
//! which is the moment a credential holding something perishable can renew it:
//! how old a token is only matters where it is about to be used, and a renewal
//! that fails there ends the turn before a request leaves.

use std::fmt;

/// Why a credential could not be resolved or applied.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// The environment variable naming the key is unset or empty.
    ///
    /// Carries the variable *name*, never its value.
    #[error("{0} is not set")]
    NotInEnvironment(Box<str>),

    /// The credential was found and could not be made ready for this request.
    ///
    /// Looking one up is not the only moment authentication fails. A key read
    /// from the environment is there or it is not, and that is the whole of
    /// what can go wrong with it; a credential that holds a token decides in
    /// [`Credential::authorize`] whether the one it holds is still good, and a
    /// renewal that is refused has to be reported from inside that call.
    /// Without somewhere to say so, the only thing such a credential could
    /// report is a variable that is unset, which is not what happened.
    ///
    /// Carries a sentence written for the user and never the token itself:
    /// this reaches a log line and the screen like every other error here.
    #[error("{0}")]
    NotRenewed(Box<str>),
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
    /// place the secret is read. Registering it with [`Outgoing::protect`]
    /// also gives the provider an opaque filter for a gateway that repeats the
    /// submitted key in a response.
    pub fn apply(&self, request: &mut Outgoing, header: &Header) {
        request.protect(self.0.as_str());
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
    /// Whitespace around the value is taken off rather than carried in. A key
    /// pasted with a trailing space is still the key; sent with one it comes
    /// back as a 401 naming nothing, and a trailing CRLF — what a key file
    /// written on Windows leaves behind — is refused by the transport as an
    /// illegal header value and reads as a network failure. Both are the
    /// puzzling answer this lookup exists to turn into a sentence.
    ///
    /// # Errors
    ///
    /// [`CredentialError::NotInEnvironment`] if `lookup` finds nothing or finds
    /// only whitespace.
    pub fn from_lookup(
        variable: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, CredentialError> {
        match lookup(variable).as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => Ok(Self(value.to_owned())),
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
    redactions: Redactions,
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

    /// Marks an exact secret representation carried by this request.
    ///
    /// A credential calls this while it applies itself. The value can then be
    /// removed from provider-controlled text without an accessor ever handing
    /// it back to the provider. Empty values are ignored.
    pub fn protect(&mut self, value: impl Into<Box<str>>) {
        let value = value.into();
        if value.is_empty() || self.redactions.values.contains(&value) {
            return;
        }
        let at = self
            .redactions
            .values
            .partition_point(|present| present.len() >= value.len());
        self.redactions.values.insert(at, value);
    }

    /// An opaque filter for the secrets a credential applied.
    ///
    /// Cloning this duplicates only credential-sized values, never the request
    /// body. Its fields and formatting expose no value.
    #[must_use]
    pub fn redactions(&self) -> Redactions {
        self.redactions.clone()
    }

    /// The headers, for the transport to send. This is the only way a value
    /// comes back out, and it is called by the code that is about to put the
    /// bytes on the socket.
    #[must_use]
    pub fn headers(&self) -> &[(Box<str>, Box<str>)] {
        &self.headers
    }
}

/// Exact secret representations to remove from untrusted diagnostic text.
///
/// A gateway sees request headers and can repeat one in a response. This type
/// carries those values across the response boundary without exposing an
/// accessor or a formatting path for them.
#[derive(Clone, Default)]
pub struct Redactions {
    values: Vec<Box<str>>,
}

impl Redactions {
    /// Replaces every protected value while leaving other text intact.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        let marker = if self
            .values
            .iter()
            .all(|value| !"<redacted>".contains(&**value))
        {
            "<redacted>"
        } else {
            ""
        };
        let mut remaining = text;
        let mut redacted = String::with_capacity(text.len());

        while let Some((at, value)) = self
            .values
            .iter()
            .filter_map(|value| remaining.find(&**value).map(|at| (at, value.as_ref())))
            .min_by(|(left_at, left), (right_at, right)| {
                left_at
                    .cmp(right_at)
                    .then_with(|| right.len().cmp(&left.len()))
            })
        {
            redacted.push_str(remaining.get(..at).unwrap_or_default());
            redacted.push_str(marker);
            remaining = remaining
                .get(at.saturating_add(value.len())..)
                .unwrap_or_default();
        }
        redacted.push_str(remaining);
        redacted
    }
}

impl fmt::Debug for Redactions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Redactions(<redacted>)")
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
        out.field("redactions", &self.redactions);
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
    /// Every exact secret representation written into a header must also be
    /// passed to [`Outgoing::protect`]. A gateway can echo a header in a
    /// response; protection lets the provider keep the useful sentence while
    /// removing the credential.
    ///
    /// Called on every request, which is what makes this the place a token is
    /// renewed: a credential holding one is deciding here whether what it holds
    /// is still good, at the only moment that can be answered about.
    ///
    /// # Errors
    ///
    /// [`CredentialError::NotRenewed`] where the credential had to produce
    /// something before it could answer and could not. One holding a key has
    /// already applied it by this point and cannot fail.
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
    fn protected_values_can_only_be_applied_as_redactions() {
        let mut request = Outgoing::new();
        request.protect(SECRET);
        let redactions = request.redactions();

        let shown = redactions.redact(&format!("gateway repeated {SECRET} here"));

        assert_eq!(shown, "gateway repeated <redacted> here");
        assert!(!format!("{redactions:?}").contains(SECRET));
    }

    #[test]
    fn overlapping_values_are_removed_longest_first_without_cascading() {
        let mut request = Outgoing::new();
        request.protect("secret");
        request.protect("secret-long");
        request.protect("secret");

        let shown = request.redactions().redact("secret-long and secret");

        assert_eq!(shown, "<redacted> and <redacted>");
    }

    #[test]
    fn redaction_preserves_nuls_that_came_from_the_provider() {
        let mut request = Outgoing::new();
        request.protect(SECRET);

        let shown = request
            .redactions()
            .redact(&format!("before\0{SECRET}\0after"));

        assert_eq!(shown, "before\0<redacted>\0after");
    }

    #[test]
    fn the_redaction_marker_cannot_itself_reveal_a_protected_value() {
        let mut request = Outgoing::new();
        request.protect("redacted");

        let shown = request.redactions().redact("gateway repeated redacted");

        assert_eq!(shown, "gateway repeated ");
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
    fn a_variable_padded_by_a_paste_sends_the_key_and_not_the_padding() {
        // The two shapes a key arrives padded in: a space from a paste, and a
        // CRLF from a key file written on Windows. The header value has to be
        // what a key set correctly would send, since the alternative is a 401
        // or a rejected header that names neither the variable nor the space.
        let mut padded = Outgoing::new();
        let key = ApiKey::from_lookup("KEY", |_| Some(format!(" {SECRET} \r\n"))).unwrap();
        HeaderKey::new(key, Header::bare("x-api-key"))
            .authorize(&mut padded)
            .unwrap();

        assert_eq!(header(&padded, "x-api-key"), SECRET);
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

    /// A credential that has to renew something before it can answer, and
    /// cannot. The shape one takes when what it holds has expired and renewing
    /// it was refused.
    #[derive(Debug)]
    struct Stale;

    impl Credential for Stale {
        fn authorize(&self, _request: &mut Outgoing) -> Result<(), CredentialError> {
            Err(CredentialError::NotRenewed("the login has expired".into()))
        }
    }

    #[test]
    fn a_credential_that_cannot_renew_says_so_and_writes_nothing() {
        // Authentication fails at two moments, not one. This is the second:
        // the credential was found, and what it holds is no longer good. A
        // request half-authorised is worse than one never sent, so a failure
        // here leaves the headers as they were.
        let mut request = Outgoing::new();

        let problem = Stale.authorize(&mut request).unwrap_err();

        assert_eq!(problem.to_string(), "the login has expired");
        assert!(request.headers().is_empty(), "the request was written to");
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
