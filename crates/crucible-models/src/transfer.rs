//! What may be sent to a vendor that did not produce it.
//!
//! A vendor's terms can keep what its own service answered with its own models.
//! That term travels on the result, as its [`ResultProvenance`], so it holds
//! wherever the session goes: a switch to another vendor and a session picked
//! up by a run serving one are the same question asked of the same record. The
//! decision is made here, once, so the runner that applies it never names a
//! vendor or a tool.

use crucible_types::ResultProvenance;

use crate::Provider;

/// What becomes of one recorded result when the conversation is next sent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer<'a> {
    /// It goes as it is.
    Keep,
    /// It may not go; this sentence stands in its place.
    Clear(&'a str),
}

/// Decides whether a result may be sent to `recipient`.
///
/// A result another vendor answered under a term keeping it to that vendor's
/// models goes nowhere else — unless `recipient` reaches no model at all, the
/// stand-in served while nothing is set up, which is sent nothing and so needs
/// nothing kept from it; clearing for its sake would take the result from the
/// vendor the session goes back to.
///
/// `leaving` is the provider the conversation is moving away from, and `None`
/// where a session is being picked up or a result recorded rather than a
/// provider replaced. It is consulted only for a result recorded before results
/// said who answered them, which is decided the way the build that recorded it
/// decided every search result: taken away when leaving a vendor that restricts
/// its results, whoever is next, and kept otherwise, since nothing says who
/// produced it.
#[must_use]
pub fn transfer<'a>(
    provenance: &'a ResultProvenance,
    recipient: &dyn Provider,
    leaving: Option<&dyn Provider>,
) -> Transfer<'a> {
    match provenance {
        ResultProvenance::Answered(answered) => match answered.restricted() {
            Some(notice)
                if recipient.reaches_a_model() && answered.vendor() != recipient.name() =>
            {
                Transfer::Clear(notice)
            }
            _ => Transfer::Keep,
        },
        ResultProvenance::Unrecorded => match leaving {
            Some(left) if left.name() != recipient.name() => left
                .restricts_results()
                .map_or(Transfer::Keep, Transfer::Clear),
            _ => Transfer::Keep,
        },
        ResultProvenance::Unstated => Transfer::Keep,
    }
}

#[cfg(test)]
mod tests {
    use crucible_runtime::{BoxFuture, Cancel};
    use crucible_types::{CredentialScopeId, Modalities, PromptCacheEncoding, ResultProvenance};

    use super::{Transfer, transfer};
    use crate::{
        DeltaStream, PromptCacheCapabilities, PromptCacheRoute, Provider, ProviderError, Request,
    };

    const KEPT: &str = "[cleared — kept to its vendor]";

    /// A provider that answers only what the decision asks of it.
    struct Named {
        name: &'static str,
        restricts: Option<&'static str>,
        reaches: bool,
        scope: CredentialScopeId,
    }

    fn vendor(name: &'static str) -> Named {
        Named {
            name,
            restricts: None,
            reaches: true,
            scope: CredentialScopeId::new(),
        }
    }

    impl Provider for Named {
        fn name(&self) -> &'static str {
            self.name
        }
        fn spells(&self) -> Modalities {
            Modalities::empty()
        }
        fn restricts_results(&self) -> Option<&'static str> {
            self.restricts
        }
        fn reaches_a_model(&self) -> bool {
            self.reaches
        }
        fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
            PromptCacheCapabilities::unknown("a test provider")
        }
        fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
            PromptCacheRoute {
                protocol: "test",
                endpoint: "test",
                custom_endpoint: true,
                credential_scope: self.scope,
                account: None,
                project: None,
                request_shape_version: "test-v1",
            }
        }
        fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
            PromptCacheEncoding::NoControlIntended
        }
        fn stream<'a>(
            &'a self,
            _request: Request<'a>,
            _cancel: &'a Cancel,
        ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
            Box::pin(async {
                Err(ProviderError::Unconfigured(
                    "a test provider answers nothing".into(),
                ))
            })
        }
    }

    #[test]
    fn a_result_nothing_restricts_goes_anywhere() {
        let anywhere = [
            ResultProvenance::Unstated,
            ResultProvenance::answered("google", None).expect("a bounded vendor"),
        ];
        let restricting = Named {
            restricts: Some(KEPT),
            ..vendor("google")
        };
        for provenance in &anywhere {
            assert_eq!(
                transfer(provenance, &vendor("openai"), None),
                Transfer::Keep
            );
            assert_eq!(
                transfer(provenance, &vendor("openai"), Some(&restricting)),
                Transfer::Keep
            );
        }
    }

    #[test]
    fn a_restricted_result_goes_only_to_its_vendor_or_to_nothing_at_all() {
        let kept = ResultProvenance::answered("google", Some(KEPT)).expect("a bounded term");

        assert_eq!(
            transfer(&kept, &vendor("anthropic"), None),
            Transfer::Clear(KEPT)
        );
        assert_eq!(transfer(&kept, &vendor("google"), None), Transfer::Keep);

        let nothing = Named {
            reaches: false,
            ..vendor("none")
        };
        assert_eq!(transfer(&kept, &nothing, None), Transfer::Keep);
    }

    #[test]
    fn an_unrecorded_result_follows_the_rule_of_the_build_that_recorded_it() {
        let unrecorded = ResultProvenance::Unrecorded;
        let restricting = Named {
            restricts: Some(KEPT),
            ..vendor("google")
        };

        assert_eq!(
            transfer(&unrecorded, &vendor("anthropic"), Some(&restricting)),
            Transfer::Clear(KEPT),
            "leaving a vendor that restricts its results takes them away"
        );
        let nothing = Named {
            reaches: false,
            ..vendor("none")
        };
        assert_eq!(
            transfer(&unrecorded, &nothing, Some(&restricting)),
            Transfer::Clear(KEPT),
            "whoever is next, as that build did"
        );
        assert_eq!(
            transfer(&unrecorded, &vendor("google"), Some(&restricting)),
            Transfer::Keep,
            "staying with the same vendor moves nothing"
        );
        assert_eq!(
            transfer(&unrecorded, &vendor("anthropic"), Some(&vendor("openai"))),
            Transfer::Keep,
            "a vendor that restricts nothing takes nothing away"
        );
        assert_eq!(
            transfer(&unrecorded, &vendor("anthropic"), None),
            Transfer::Keep,
            "picked up rather than moved, nothing says who produced it"
        );
    }
}
