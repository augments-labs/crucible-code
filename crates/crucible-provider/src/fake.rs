//! What a test needs that only the rest of the harness can mint.
//!
//! A tool result carrying files is one of them. The files are admitted by the
//! verdict that let the tool run, so a body test cannot write one down — it has
//! to be issued, by the engine that issues every other one.

/// A completed signed exchange whose recap must carry only descriptive text.
pub(crate) fn recap_history() -> crucible_core::Transcript {
    use crucible_core::{
        Continuation, ContinuationData, ContinuationPart, ContinuationScope, Message, StopReason,
        ToolArgs, ToolCall, ToolId, ToolOutput, ToolResult, Transcript,
    };
    let mut state = Continuation::new(
        "fixture-v1",
        "fixture",
        ContinuationScope::from_digest([0; 32]),
    )
    .unwrap();
    state
        .push(ContinuationPart::Opaque(
            ContinuationData::new("private-signature-canary").unwrap(),
        ))
        .unwrap();
    state
        .push(ContinuationPart::Text {
            start: 0,
            end: 10,
            data: ContinuationData::new("").unwrap(),
        })
        .unwrap();
    state
        .push(ContinuationPart::Call {
            index: 0,
            data: ContinuationData::new("").unwrap(),
        })
        .unwrap();
    let mut transcript = Transcript::new();
    transcript.push(Message::said("old question")).unwrap();
    transcript
        .push(Message::Agent {
            text: "old answer".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call-1"),
                name: "lookup".into(),
                args: ToolArgs::new("{}"),
            }],
            stop: Some(StopReason::WantsTools),
            continuation: Some(
                state
                    .finish("old answer", 1, Some(StopReason::WantsTools))
                    .unwrap(),
            ),
        })
        .unwrap();
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: ToolOutput::ok("old result"),
        }]))
        .unwrap();
    transcript.push(Message::said("summarize")).unwrap();
    transcript
}

use crucible_core::{
    Ask, Attachment, Command, Delta, InputTokenUsage, Modality, Permission, PromptCacheBoundary,
    PromptCacheCapabilities, PromptCacheContent, PromptCacheFingerprint, PromptCacheIdentity,
    PromptCacheKey, PromptCacheMechanism, PromptCacheMechanismCapability, PromptCachePlan,
    PromptCachePolicy, PromptCacheProjection, PromptCacheProvenance, PromptCacheRequest,
    PromptCacheResourceHandle, PromptCacheResourceId, PromptCacheResourceReference,
    PromptCacheRetention, PromptCacheRetentionClass, PromptCacheScopeDigest, PromptCacheSelected,
    PromptCacheSelection, PromptCacheUsageReporting, ProviderAttemptId, ProviderNumericDetail,
    ProviderUsage, Remember, Request, Sensitivity, Settled, StatefulTransportCapability, ToolArgs,
    ToolCall, ToolId, ToolOutput, Verdict,
};

const CACHE_CONTENT: &[PromptCacheContent] = &[
    PromptCacheContent::Text,
    PromptCacheContent::Tools,
    PromptCacheContent::Images,
    PromptCacheContent::Documents,
    PromptCacheContent::Audio,
    PromptCacheContent::Video,
];

/// Attaches a complete, leaked cache fixture to a static body-test request.
///
/// Body tests already leak their transcript so the borrowed request can be a
/// compact literal. Cache fixtures follow the same test-only convention; this
/// helper keeps protocol tests focused on exact wire fields rather than the
/// construction boilerplate for the neutral contract.
pub(crate) fn cached(
    request: Request<'static>,
    mechanism: PromptCacheMechanism,
    retention: PromptCacheRetentionClass,
    routing_key: bool,
) -> Request<'static> {
    cache_fixture(request, Some(mechanism), retention, routing_key)
}

/// Attaches an explicit observe-only attempt that must encode no control.
pub(crate) fn observed(request: Request<'static>) -> Request<'static> {
    cache_fixture(
        request,
        None,
        PromptCacheRetentionClass::ProviderDefault,
        false,
    )
}

fn cache_fixture(
    mut request: Request<'static>,
    selected: Option<PromptCacheMechanism>,
    retention: PromptCacheRetentionClass,
    routing_key: bool,
) -> Request<'static> {
    let mechanism = selected.unwrap_or(PromptCacheMechanism::AutomaticPrefix);
    let capability = match mechanism {
        PromptCacheMechanism::ProviderManagedUsageOnly => {
            PromptCacheMechanismCapability::provider_managed(0, CACHE_CONTENT)
        }
        PromptCacheMechanism::AutomaticPrefix => {
            PromptCacheMechanismCapability::automatic_prefix(0, true, true, CACHE_CONTENT)
                .with_retentions(&[
                    PromptCacheRetentionClass::ProviderDefault,
                    PromptCacheRetentionClass::Ephemeral,
                    PromptCacheRetentionClass::Extended,
                ])
        }
        PromptCacheMechanism::ExplicitBreakpoints => {
            PromptCacheMechanismCapability::explicit_breakpoints(
                0,
                4,
                &[
                    PromptCacheBoundary::AfterSystem,
                    PromptCacheBoundary::AfterTools,
                    PromptCacheBoundary::AfterMessage,
                    PromptCacheBoundary::AfterContent,
                ],
                CACHE_CONTENT,
            )
        }
        PromptCacheMechanism::PersistentContent => {
            PromptCacheMechanismCapability::persistent_content(0, CACHE_CONTENT)
        }
    };
    let capabilities = Box::leak(Box::new(PromptCacheCapabilities::supported(
        "body-fixture-v1",
        Some("body-fixture-model"),
        PromptCacheProvenance::new("https://example.invalid", "2026-08-31", "body-fixture-v1"),
        StatefulTransportCapability::Unsupported,
        &[capability],
        PromptCacheUsageReporting::ReadAndWriteTokens,
    )));
    let projection = PromptCacheProjection::inspect(&request).expect("fixture projection");
    let fingerprint = PromptCacheFingerprint::new([0x22; 32]);
    let plan = Box::leak(Box::new(PromptCachePlan::new(&projection, fingerprint)));
    let mut policy = match retention {
        PromptCacheRetentionClass::ProviderDefault => PromptCachePolicy::default(),
        PromptCacheRetentionClass::Ephemeral => PromptCachePolicy::default().with_retention(
            PromptCacheRetention::ephemeral(30 * 60).expect("bounded fixture retention"),
        ),
        PromptCacheRetentionClass::Extended => PromptCachePolicy::default().with_retention(
            PromptCacheRetention::extended(60 * 60).expect("bounded fixture retention"),
        ),
    };
    if selected.is_none() {
        policy = policy.with_mode(crucible_core::PromptCacheMode::ObserveOnly);
    }
    let identity = PromptCacheIdentity::new(
        PromptCacheScopeDigest::new([0x33; 32]),
        fingerprint,
        "body-fixture-v1",
    );
    let resource = (selected == Some(PromptCacheMechanism::PersistentContent)).then(|| {
        let id = Box::leak(Box::new(PromptCacheResourceId::new()));
        let handle = Box::leak(Box::new(
            PromptCacheResourceHandle::new("body-fixture-resource").unwrap(),
        ));
        PromptCacheResourceReference::new(id, handle)
    });
    let cache = Box::leak(Box::new(PromptCacheRequest {
        attempt: ProviderAttemptId::new(),
        policy,
        capabilities,
        plan,
        identity,
        selection: selected.map_or_else(
            || {
                PromptCacheSelection::ineligible(
                    crucible_core::PromptCacheIneligibleReason::ObserveOnly,
                )
            },
            |mechanism| {
                PromptCacheSelection::eligible(PromptCacheSelected::new(mechanism, retention))
            },
        ),
        routing_key: routing_key.then(|| PromptCacheKey::from_digest([0x44; 32], 64)),
        resource,
    }));
    request.prompt_cache = Some(cache);
    request
}

/// Says yes, once, to whatever it is shown.
struct Allows;

impl Ask for Allows {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Allow, Remember::Never)
    }
}

/// What a tool answered with, and the files it was permitted to show.
pub(crate) fn found(text: &str, attachments: Vec<Attachment>) -> ToolOutput {
    showing(ToolOutput::ok(text), attachments)
}

/// The same, for a call that failed with something to show anyway.
pub(crate) fn failed(text: &str, attachments: Vec<Attachment>) -> ToolOutput {
    showing(ToolOutput::failed(text), attachments)
}

/// The verdict, issued, and the files it admits.
fn showing(output: ToolOutput, attachments: Vec<Attachment>) -> ToolOutput {
    let call = ToolCall {
        id: ToolId::new("call_1"),
        name: "bash".into(),
        args: ToolArgs::new("{}"),
    };
    let settled = Permission::new().decide(
        &call,
        &Sensitivity::SpawnsProcess {
            command: Command::Understood {
                sent: "ls".into(),
                parts: vec!["ls".into()].into(),
            },
        },
        &mut Allows,
    );

    let Settled::Approved(approved) = settled else {
        panic!("the fake said yes")
    };
    output.with_attachments(&approved, attachments)
}

/// One file a tool found, as the transcript records it.
///
/// No bytes: a provider reads what the runner resolved and never a path, so
/// this is only what says a result has a file at all — and how many.
pub(crate) fn picture() -> Attachment {
    Attachment {
        path: "pictures/holiday.png".into(),
        modality: Modality::Image,
        media_type: "image/png".into(),
        hash: [0xab; 32],
    }
}

/// Expected normalized usage for inclusive-total provider fixtures.
pub(crate) fn inclusive_usage(
    input: Option<u64>,
    cached: Option<u64>,
    output: Option<u64>,
) -> Delta {
    let input = InputTokenUsage::inclusive_read(input, cached).expect("valid usage fixture");
    let details = cached
        .map(|value| ProviderNumericDetail::new("cached_tokens", value).unwrap())
        .into_iter()
        .collect::<Vec<_>>();
    Delta::Usage(
        ProviderUsage::new(input, output, None, None, &details).expect("valid usage fixture"),
    )
}

/// Expected normalized Anthropic input usage fixture.
pub(crate) fn disjoint_input_usage(
    uncached: Option<u64>,
    write: Option<u64>,
    read: Option<u64>,
) -> Delta {
    let mut details = Vec::new();
    for (label, value) in [
        ("input_tokens", uncached),
        ("cache_creation_input_tokens", write),
        ("cache_read_input_tokens", read),
    ] {
        if let Some(value) = value {
            details.push(ProviderNumericDetail::new(label, value).unwrap());
        }
    }
    Delta::Usage(
        ProviderUsage::new(
            InputTokenUsage::disjoint(uncached, read, write).expect("valid usage fixture"),
            None,
            None,
            None,
            &details,
        )
        .expect("valid usage fixture"),
    )
}

/// Expected normalized Anthropic output-only usage fixture.
pub(crate) fn output_usage(output: u64) -> Delta {
    let detail = ProviderNumericDetail::new("output_tokens", output).unwrap();
    Delta::Usage(
        ProviderUsage::new(
            InputTokenUsage::UNKNOWN,
            Some(output),
            None,
            None,
            &[detail],
        )
        .expect("valid usage fixture"),
    )
}

#[cfg(test)]
mod cache_conformance {
    use super::*;
    use crucible_core::{ApiKey, Header, HeaderKey, PromptCacheSupport, Provider, Transcript};

    use crate::transport::Replay;
    use crate::{Anthropic, Moonshot, OpenAi};

    #[derive(Clone, Copy)]
    enum Case {
        Automatic,
        Breakpoints,
        Persistent,
        UsageOnly,
        Unsupported,
        Unknown,
        ProxyDelegated(bool),
    }

    fn capability(case: Case) -> PromptCacheCapabilities {
        let mechanism = match case {
            Case::Automatic | Case::ProxyDelegated(true) => Some(
                PromptCacheMechanismCapability::automatic_prefix(1, true, false, CACHE_CONTENT),
            ),
            Case::Breakpoints => Some(PromptCacheMechanismCapability::explicit_breakpoints(
                1,
                4,
                &[PromptCacheBoundary::AfterMessage],
                CACHE_CONTENT,
            )),
            Case::Persistent => Some(PromptCacheMechanismCapability::persistent_content(
                1,
                CACHE_CONTENT,
            )),
            Case::UsageOnly => Some(PromptCacheMechanismCapability::provider_managed(
                1,
                CACHE_CONTENT,
            )),
            Case::Unsupported => {
                return PromptCacheCapabilities::unsupported(
                    "fixture-v1",
                    PromptCacheProvenance::new(
                        "https://provider.invalid/cache",
                        "2026-08-31",
                        "fixture-v1",
                    ),
                    StatefulTransportCapability::Unsupported,
                );
            }
            Case::Unknown | Case::ProxyDelegated(false) => {
                return PromptCacheCapabilities::unknown("fixture-v1");
            }
        };
        PromptCacheCapabilities::supported(
            "fixture-v1",
            Some("model-v1"),
            PromptCacheProvenance::new(
                "https://provider.invalid/cache",
                "2026-08-31",
                "fixture-v1",
            ),
            StatefulTransportCapability::Unsupported,
            &[mechanism.expect("a supported case has one mechanism")],
            PromptCacheUsageReporting::ReadAndWriteTokens,
        )
    }

    #[test]
    fn every_neutral_adapter_shape_has_an_explicit_fixture() {
        let expected = [
            (Case::Automatic, Some(PromptCacheMechanism::AutomaticPrefix)),
            (
                Case::Breakpoints,
                Some(PromptCacheMechanism::ExplicitBreakpoints),
            ),
            (
                Case::Persistent,
                Some(PromptCacheMechanism::PersistentContent),
            ),
            (
                Case::UsageOnly,
                Some(PromptCacheMechanism::ProviderManagedUsageOnly),
            ),
        ];
        for (case, mechanism) in expected {
            let capability = capability(case);
            assert_eq!(capability.support(), PromptCacheSupport::Supported);
            assert_eq!(
                capability
                    .mechanisms()
                    .first()
                    .map(PromptCacheMechanismCapability::mechanism),
                mechanism
            );
        }
        assert_eq!(
            capability(Case::Unsupported).support(),
            PromptCacheSupport::Unsupported
        );
        assert_eq!(
            capability(Case::Unknown).support(),
            PromptCacheSupport::Unknown
        );
    }

    #[test]
    fn a_proxy_is_unknown_until_its_forwarded_upstream_is_resolved() {
        assert_eq!(
            capability(Case::ProxyDelegated(false)).support(),
            PromptCacheSupport::Unknown
        );
        assert_eq!(
            capability(Case::ProxyDelegated(true)).support(),
            PromptCacheSupport::Supported
        );
    }

    #[test]
    fn default_policy_selects_each_shipped_providers_native_mechanism() {
        let providers: Vec<(Box<dyn Provider>, &str, PromptCacheMechanism)> = vec![
            (
                Box::new(OpenAi::at(
                    OpenAi::VENDOR,
                    Box::new(HeaderKey::new(ApiKey::new("test"), Header::bearer())),
                    Box::new(Replay::new(200, "")),
                )),
                "gpt-5.6-sol",
                PromptCacheMechanism::AutomaticPrefix,
            ),
            (
                Box::new(Anthropic::at(
                    Anthropic::VENDOR,
                    Box::new(HeaderKey::new(
                        ApiKey::new("test"),
                        Header::bare("x-api-key"),
                    )),
                    Box::new(Replay::new(200, "")),
                )),
                "claude-opus-5",
                PromptCacheMechanism::AutomaticPrefix,
            ),
            (
                Box::new(Moonshot::at(
                    Moonshot::CODING,
                    Box::new(HeaderKey::new(ApiKey::new("test"), Header::bearer())),
                    Box::new(Replay::new(200, "")),
                )),
                "kimi-for-coding",
                PromptCacheMechanism::AutomaticPrefix,
            ),
        ];
        let transcript = Transcript::new();
        let stable = "x".repeat(20_000);

        for (provider, model, expected) in providers {
            let request = Request {
                purpose: crucible_core::RequestPurpose::Turn,
                model,
                transcript: &transcript,
                tools: &[],
                attached: &[],
                max_tokens: 1_024,
                system: Some(&stable),
                effort: None,
                prompt_cache: None,
            };
            let projection = PromptCacheProjection::inspect(&request).unwrap();
            let plan = PromptCachePlan::new(&projection, PromptCacheFingerprint::new([0x55; 32]));
            let capabilities = provider.prompt_cache_capabilities(model);
            let selection = PromptCacheSelection::prepare(
                PromptCachePolicy::default(),
                &capabilities,
                &plan,
                false,
            )
            .unwrap();

            assert_eq!(
                selection
                    .selected()
                    .map(crucible_core::PromptCacheSelected::mechanism),
                Some(expected),
                "{} default cache mechanism",
                provider.name(),
            );
        }
    }
}
