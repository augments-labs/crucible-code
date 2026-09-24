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
//!
//! [`Credential::authorize`] hands back a future rather than an answer, because
//! a renewal can have to reach a server before it knows whether the token it
//! holds is still good. One holding only a key has nothing to wait for and
//! answers the first time that future is polled; a caller that polls once and
//! treats a future still pending as a failure loses nothing any credential
//! shipped here or in `crucible-auth` ever needed to wait for.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use sha2::{Digest as _, Sha256};

use crucible_types::CredentialScopeId;

/// What [`Credential::authorize`] hands back.
///
/// Spelled with `std` alone rather than named from `crucible-runtime`'s own
/// `BoxFuture`: this crate keeps `crucible-types` as its only internal
/// dependency, so it cannot name a crate above it. `'a` is the shorter of
/// `&self` and the `&mut Outgoing` the call was given, so the future cannot
/// outlive either borrow; `Send` is what lets a caller poll it on whichever
/// thread it is on.
pub type Authorization<'a> = Pin<Box<dyn Future<Output = Result<(), CredentialError>> + Send + 'a>>;

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

    /// This call was polled as a runtime worker task, where it must not wait
    /// for a credential's renewal — whether that renewal is this call's own,
    /// or another poll's already in progress.
    ///
    /// A renewal that is not yet owned work of its own does everything —
    /// taking a cross-process lock, reaching the network — inside
    /// [`Credential::authorize`]'s first poll, so a runtime worker task must
    /// never block there. Nor may it wait for another poll to finish one: a
    /// credential that serializes renewal through an in-process lock cannot
    /// tell, from outside that lock, whether the poll holding it is
    /// renewing or only applying a token already fresh, so a worker refuses
    /// either way rather than risk the wait. Its caller decides what to do
    /// about a worker. No runtime dependency is needed to say so: this
    /// variant carries nothing but its own fixed sentence.
    #[error("this credential's renewal cannot run or be waited for on a runtime worker task")]
    RenewalOnWorker,
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
    /// `unsafe`, and this workspace denies that. Configuration stores the
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
    fn scope(&self) -> CredentialScopeId {
        let mut digest = Sha256::new();
        digest.update(b"crucible.header-credential-scope.v1");
        scope_field(&mut digest, self.header.name.as_bytes());
        scope_field(&mut digest, self.header.scheme.as_bytes());
        scope_field(&mut digest, self.key.0.as_bytes());
        CredentialScopeId::from_digest(digest.finalize().into())
    }

    fn authorize<'a>(&'a self, request: &'a mut Outgoing) -> Authorization<'a> {
        // Nothing here can fail or wait, so the key is applied before the
        // future is even built; what is handed back only carries the answer.
        self.key.apply(request, &self.header);
        Box::pin(std::future::ready(Ok(())))
    }
}

fn scope_field(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
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
        // Text with whole values swapped for an ASCII marker is still text, so
        // the conversion back stands nothing in for anything here.
        String::from_utf8_lossy(&self.without(text.as_bytes())).into_owned()
    }

    /// The same, for the bytes of a response whose end was cut off.
    ///
    /// **For cut text only.** A bound that keeps the beginning of a response
    /// lands wherever the bytes ran out, and that can be the middle of a value
    /// a gateway echoed. [`redact`] matches whole values, so it cannot see
    /// half of one, and half a credential at the end of the text is half a
    /// credential the reader keeps. Everything `redact` replaces is replaced
    /// here, and so is the longest part of the end of the text that begins a
    /// protected value.
    ///
    /// Which is why the end has to be a cut: the last bytes of an ordinary
    /// sentence that happen to begin a value are replaced too, and a sentence
    /// ends where its writer meant it to. Where the end is an arbitrary
    /// boundary, losing a byte or two of it costs nothing anybody can read.
    ///
    /// **Hand over the bytes as they arrived, never text made from them.** A
    /// caller that converts first hands over an end its sender composed: how
    /// many stand-in characters a lossy conversion leaves there is decided by
    /// the bytes the sender chose to put at the end, and a search for a value's
    /// beginning that has to reach behind them is a search the sender can
    /// blind. The search here ends at the last whole character the bytes spell,
    /// since anything after that begins no value, and the conversion happens
    /// afterwards over what survives.
    ///
    /// **The answer is about the text as it is handed back.** A caller that
    /// afterwards parses it, unescapes it or lifts a part of it out is showing
    /// something this never decided about: a body that was whole JSON up to
    /// the cut has its own last bytes protected, and the sentence inside it —
    /// which is what such a caller extracts — has whatever the cut left there.
    /// Redact again over anything taken out, and rely on this for none of it.
    ///
    /// What is decided about is the text as given, once the whole values have
    /// gone: a value ending exactly at the cut is replaced by `redact`, and
    /// what is left behind then begins nothing, so it stays one marker rather
    /// than becoming two.
    ///
    /// The work this adds over `redact` is bounded by the protected values
    /// rather than by the text — only an end shorter than a value can begin
    /// one — so a long body costs no more here than a short one.
    ///
    /// [`redact`]: Self::redact
    #[must_use]
    pub fn redact_cut(&self, said: &[u8]) -> String {
        let mut kept = self.without(said);
        if let Some(from) = self.begun(spelled(&kept)) {
            // A protected value is text, so its first byte opens a character
            // and never continues one: a tail that begins a value begins where
            // a character does, and cutting the bytes there splits none.
            kept.truncate(from);
            kept.extend_from_slice(self.marker().as_bytes());
        }
        String::from_utf8_lossy(&kept).into_owned()
    }

    /// Every protected value replaced, in bytes rather than in text.
    ///
    /// One pass for both callers: a filter that saw whole values differently
    /// from the one the cut guard runs would be a second answer to the same
    /// question. Bytes because [`redact_cut`] must not convert first, and a
    /// value is text whose bytes appear in text only where the text spells it,
    /// so nothing here decides differently for a caller that had text.
    ///
    /// [`redact_cut`]: Self::redact_cut
    fn without(&self, text: &[u8]) -> Vec<u8> {
        let marker = self.marker().as_bytes();
        let mut remaining = text;
        let mut redacted = Vec::with_capacity(text.len());

        while let Some((at, value)) = self
            .values
            .iter()
            .filter_map(|value| {
                let value = value.as_bytes();
                found(remaining, value).map(|at| (at, value))
            })
            .min_by(|(left_at, left), (right_at, right)| {
                left_at
                    .cmp(right_at)
                    .then_with(|| right.len().cmp(&left.len()))
            })
        {
            redacted.extend_from_slice(remaining.get(..at).unwrap_or_default());
            redacted.extend_from_slice(marker);
            remaining = remaining
                .get(at.saturating_add(value.len())..)
                .unwrap_or_default();
        }
        redacted.extend_from_slice(remaining);
        redacted
    }

    /// Where the longest part of the end of `text` that begins a value starts.
    ///
    /// `None` where the end of the text begins none of them. The earliest such
    /// start is the longest such end, and a tail is looked for only from the
    /// point where one could still be shorter than the value it begins, which
    /// leaves a whole value to [`redact`].
    ///
    /// [`redact`]: Self::redact
    fn begun(&self, text: &[u8]) -> Option<usize> {
        self.values
            .iter()
            .filter_map(|value| {
                let value = value.as_bytes();
                let earliest = text.len().saturating_sub(value.len().saturating_sub(1));
                (earliest..text.len())
                    .find(|&at| text.get(at..).is_some_and(|tail| value.starts_with(tail)))
            })
            .min()
    }

    /// What a removed value is replaced with.
    ///
    /// Nothing at all where a protected value is part of the marker itself,
    /// since writing the marker would then be writing the value back into the
    /// text it was taken out of.
    fn marker(&self) -> &'static str {
        if self
            .values
            .iter()
            .all(|value| !"<redacted>".contains(&**value))
        {
            "<redacted>"
        } else {
            ""
        }
    }
}

/// Where `needle` first appears in `haystack`.
///
/// What [`str::find`] answers, for bytes that need not be text. A protected
/// value is never empty — [`Outgoing::protect`] drops an empty one — and the
/// guard says so anyway rather than leaving a window of zero to panic on.
fn found(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|at| at == needle)
}

/// The bytes up to the end of the last whole character they spell.
///
/// A cut lands on a byte rather than on a character, so what sits at the end
/// can be half of one, or bytes that spell no character at all. Neither begins
/// a protected value, and either left in place hides a value's beginning in
/// front of it from the search that looks for one.
///
/// Dropping them moves where that search looks: what they could hide is the
/// start of a credential, and what a search would find in them instead is
/// bytes no reader can make a character of.
///
/// It can also cost a truncation the search would otherwise have made: where
/// the character the cut split is a value's own first one, the bytes that
/// began that value are the dropped ones, so nothing is found and nothing is
/// taken off the end. No byte of the value is shown for it. Those bytes are
/// past the last whole character, which is where the conversion stands a
/// replacement character in — a single one, a split character being one
/// incomplete sequence — so what reaches the reader is that stand-in, and
/// neither a byte of the value nor a count of the ones it stands for.
fn spelled(said: &[u8]) -> &[u8] {
    let mut at = 0_usize;
    let mut end = 0_usize;

    for chunk in said.utf8_chunks() {
        at = at.saturating_add(chunk.valid().len());
        if !chunk.valid().is_empty() {
            end = at;
        }
        at = at.saturating_add(chunk.invalid().len());
    }
    said.get(..end).unwrap_or_default()
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
    /// Opaque identity used to keep provider-side resources credential-bound.
    ///
    /// A durable credential derives this from stable identity material with a
    /// cryptographic one-way function. A credential without such material
    /// returns a freshly minted scope and therefore declines cross-restart
    /// reuse. Implementations must never expose or persist the source material.
    fn scope(&self) -> CredentialScopeId;

    /// Writes whatever this credential needs into the request.
    ///
    /// Every exact secret representation written into a header must also be
    /// passed to [`Outgoing::protect`]. A gateway can echo a header in a
    /// response; protection lets the provider keep the useful sentence while
    /// removing the credential.
    ///
    /// Called on every request, which is what makes this the place a token is
    /// renewed: a credential holding one is deciding here whether what it holds
    /// is still good, at the only moment that can be answered about. One
    /// holding only a key has nothing to wait for and answers the first time
    /// its future is polled; a caller that polls once and treats a future
    /// still pending as a failure loses nothing any credential shipped here
    /// needs to wait for.
    ///
    /// # Errors
    ///
    /// [`CredentialError::NotRenewed`] where the credential had to produce
    /// something before it could answer and could not. One holding a key has
    /// already applied it by this point and cannot fail.
    ///
    /// [`CredentialError::RenewalOnWorker`] where this future was polled as
    /// a runtime worker task and either it would have to renew what the
    /// credential holds, or another poll already renewing (or merely
    /// applying an already-fresh token) holds the credential's own lock —
    /// neither may run or be waited for there yet.
    fn authorize<'a>(&'a self, request: &'a mut Outgoing) -> Authorization<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact string that must never appear anywhere but the header value.
    ///
    /// Tests that need to answer a credential's future — `authorize` cannot
    /// be called synchronously — live in `tests/ready_body.rs` instead: this
    /// crate cannot name `crucible-runtime`'s `Bridge`, and standing in for
    /// its one poll by hand is something `scripts/python/bridge-ledger.py`
    /// only allows outside the crate's shipped source.
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
    fn a_value_the_text_was_cut_in_the_middle_of_goes_with_the_whole_ones() {
        // The end of cut text is wherever the bytes ran out, which can be the
        // middle of a value a gateway echoed. Whole-value redaction cannot see
        // half of one, so half of one is what the reader would keep.
        let mut request = Outgoing::new();
        request.protect(SECRET);
        let redactions = request.redactions();

        for taken in 1..SECRET.len() {
            let half = SECRET.get(..taken).unwrap_or_default();

            assert_eq!(
                redactions.redact_cut(format!("before {half}").as_bytes()),
                "before <redacted>",
                "{taken} bytes of it stayed"
            );
        }

        // A value that ends exactly at the cut is one value, not a whole one
        // followed by the start of another: what is decided about is the text
        // as given, after the whole ones have gone.
        assert_eq!(
            redactions.redact_cut(format!("before {SECRET}").as_bytes()),
            "before <redacted>"
        );

        // Nothing at the end begins a value, so nothing goes beyond what
        // whole-value redaction already takes.
        let ordinary = format!("gateway repeated {SECRET} and then gave up");
        assert_eq!(
            redactions.redact_cut(ordinary.as_bytes()),
            redactions.redact(&ordinary)
        );
    }

    #[test]
    fn a_value_the_cut_left_in_front_of_bytes_that_spell_nothing_goes_too() {
        // How many stand-in characters a conversion leaves at the end is the
        // sender's to arrange: one byte that spells no character is one of
        // them, and two are two. Deciding anything about the end of the text
        // from what the conversion left is deciding it from what was sent, so
        // the decision is made on the bytes and the conversion comes after.
        let mut request = Outgoing::new();
        request.protect(SECRET);
        let redactions = request.redactions();

        let half = SECRET.get(..SECRET.len().saturating_sub(1)).unwrap_or("");
        let mut cut = format!("before {half}").into_bytes();
        cut.extend_from_slice(&[0xFF, 0xFF]);

        let shown = redactions.redact_cut(&cut);

        assert!(
            !shown.contains(half),
            "the value in front of the stray bytes stayed: {shown}"
        );
    }

    #[test]
    fn a_cut_inside_the_first_character_of_a_value_shows_no_byte_of_it() {
        // The bytes past the last whole character are dropped before the end
        // is searched, and where the split character is the value's own first
        // one those bytes are the only ones that began it: the search finds
        // nothing, and the end is not truncated. What the value's first bytes
        // become is the stand-in the conversion puts there for them — one of
        // them, however many bytes the cut left — so the reader is shown
        // neither a byte of the value nor how many of them there were.
        for wide in ['é', '€', '𝄞'] {
            let value = format!("{wide}{SECRET}");
            let mut request = Outgoing::new();
            request.protect(value.as_str());
            let redactions = request.redactions();

            for taken in 1..wide.len_utf8() {
                let mut cut = b"before ".to_vec();
                cut.extend_from_slice(value.as_bytes().get(..taken).unwrap_or_default());

                let shown = redactions.redact_cut(&cut);

                assert_eq!(
                    shown, "before \u{FFFD}",
                    "{taken} of the bytes {wide:?} is spelled with reached the answer"
                );
            }
        }
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
    fn a_header_credential_has_a_restart_stable_redacted_scope() {
        let first = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
        let reconstructed = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
        let other_key = HeaderKey::new(ApiKey::new("sk-other"), Header::bearer());
        let other_header = HeaderKey::new(ApiKey::new(SECRET), Header::bare("x-api-key"));

        assert_eq!(first.scope(), reconstructed.scope());
        assert_ne!(first.scope(), other_key.scope());
        assert_ne!(first.scope(), other_header.scope());

        let shown = format!("{:?}", first.scope());
        assert_eq!(shown, "CredentialScopeId([redacted])");
        assert!(!shown.contains(SECRET));
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
