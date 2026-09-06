//! The provider a machine with nothing set up gets.
//!
//! It speaks no wire protocol, which is what it is for. Every other provider
//! here is built from a credential, and a machine holding none has no provider
//! to build — but the session is the place the user is going to fix that, so
//! ending the process instead would take away the screen the answer is typed
//! on. This stands in its place and refuses every turn with the same sentence
//! that was already drawn under the welcome.
//!
//! The sentence is handed in rather than written here. What to tell somebody
//! about setting crucible up is the wiring's to say — it is the one layer that
//! knows which commands exist — and saying it in two places is how the two come
//! to disagree.

use crucible_core::{
    Cancel, CredentialScopeId, DeltaStream, Modalities, Modality, PromptCacheCapabilities,
    PromptCacheRoute, Provider, ProviderError, Request,
};

/// What this provider is called, in the session log and in the status line.
///
/// A word rather than a vendor, because there is no vendor: the record of a
/// turn that went nowhere should not name a company that was never written to.
const NAME: &str = "none";

/// A provider that answers nothing, and says why.
#[derive(Debug)]
pub struct Unavailable {
    said: Box<str>,
    credential_scope: CredentialScopeId,
}

impl Unavailable {
    /// A provider that refuses every turn with `said`.
    #[must_use]
    pub fn new(said: &str) -> Self {
        Self {
            said: said.into(),
            credential_scope: CredentialScopeId::new(),
        }
    }
}

impl Provider for Unavailable {
    fn name(&self) -> &'static str {
        NAME
    }

    fn spells(&self) -> Modalities {
        // No protocol at all, so nothing beyond the text of a turn it will
        // refuse anyway.
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unsupported(
            "unavailable-v1",
            crucible_core::PromptCacheProvenance::new(
                "https://github.com/augments-labs/crucible-code",
                "2026-08-31",
                "unavailable-v1",
            ),
            crucible_core::StatefulTransportCapability::Unsupported,
        )
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "none",
            endpoint: "none",
            custom_endpoint: false,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "unavailable-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> crucible_core::PromptCacheEncoding {
        crucible_core::PromptCacheEncoding::NoControlIntended
    }

    fn stream(
        &self,
        _request: Request<'_>,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        Err(ProviderError::Unconfigured(self.said.clone()))
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{Message, Transcript};

    use super::*;

    fn asking() -> Request<'static> {
        let mut transcript = Transcript::new();
        transcript
            .push(Message::said("hello"))
            .expect("valid fixture transcript");

        Request {
            purpose: crucible_core::RequestPurpose::Turn,
            model: "",
            transcript: Box::leak(Box::new(transcript)),
            tools: &[],
            attached: &[],
            max_tokens: 1024,
            system: None,
            effort: None,
            prompt_cache: None,
        }
    }

    #[test]
    fn every_turn_comes_back_with_the_sentence_it_was_built_with() {
        let provider = Unavailable::new("nothing is set up");

        let problem = provider.stream(asking(), &Cancel::new()).unwrap_err();

        assert_eq!(problem.to_string(), "nothing is set up");
    }

    #[test]
    fn a_turn_that_went_nowhere_names_no_vendor() {
        // The session log records which provider answered. A machine with no
        // key never wrote to one, and a log saying otherwise is a record of a
        // request that was never made.
        assert_eq!(Unavailable::new("nothing is set up").name(), "none");
    }

    /// A provider that speaks no protocol still spells text: what it refuses is
    /// the turn, not the modality, and an empty answer here would have the
    /// binary telling somebody their model cannot read a picture when the real
    /// answer is that nothing is set up yet.
    #[test]
    fn a_provider_that_speaks_no_protocol_still_spells_text() {
        let provider = Unavailable::new("nothing is set up");

        assert_eq!(
            provider.spells(),
            Modalities::empty().insert(Modality::Text),
        );
    }
}
