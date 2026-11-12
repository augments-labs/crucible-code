//! The 12-hour cache write: offered on the published API to the models the
//! prompt-caching guide names, only under a ceiling of twelve hours or more,
//! and priced at its own rate only when it was asked for.

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{
    PromptCachePlan, PromptCachePreparationError, PromptCachePricedRetention,
    PromptCacheProjection, PromptCacheSelection, Provider,
};
use crucible_types::{
    MAX_PROMPT_CACHE_RETENTION_SECONDS, PromptCacheEligibility, PromptCacheFingerprint,
    PromptCacheIneligibleReason, PromptCacheMechanism, PromptCacheMechanisms, PromptCacheMode,
    PromptCachePolicy, PromptCacheRetention, PromptCacheRetentionClass, Transcript,
};

use super::*;
use crate::transport::Replay;

/// The models the guide's "GPT-5.6 and later" section covers, each with the
/// 12-hour write per million input tokens, up to 272K input tokens and above:
/// 2.5x the band's uncached input rate, as the pricing page's "12h write"
/// column shows for the models it lists.
const GUIDE_NAMED: [(&str, u64, u64); 8] = [
    (ASTRA, 25_000_000_000, 50_000_000_000),
    ("gpt-6.1-sol", 5_000_000_000, 10_000_000_000),
    ("gpt-6.1-terra", 2_500_000_000, 5_000_000_000),
    ("gpt-6-sol", 5_000_000_000, 10_000_000_000),
    ("gpt-6-luna", 250_000_000, 500_000_000),
    ("gpt-5.6-sol", 10_000_000_000, 20_000_000_000),
    ("gpt-5.6-terra", 5_000_000_000, 10_000_000_000),
    ("gpt-5.6-luna", 500_000_000, 1_000_000_000),
];

/// Twelve hours, the minimum the guide's `"12h"` keeps an entry for.
const TWELVE_HOURS: u32 = 12 * 60 * 60;

fn extended(seconds: u32) -> PromptCachePolicy {
    PromptCachePolicy::default().with_retention(PromptCacheRetention::extended(seconds).unwrap())
}

/// What `policy` selects on `model`'s record for `route`, for a stable prefix
/// well above every guide-named model's minimum, so the class is what decides.
fn prepare(
    route: Serving,
    model: &str,
    policy: PromptCachePolicy,
) -> Result<PromptCacheSelection, PromptCachePreparationError> {
    let transcript = Transcript::new();
    let stable = "x".repeat(20_000);
    let request = Request {
        purpose: crucible_models::RequestPurpose::Turn,
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
    PromptCacheSelection::prepare(policy, &prompt_cache(route, model), &plan, false)
}

fn refused(reason: PromptCacheIneligibleReason) -> PromptCacheEligibility {
    PromptCacheEligibility::Ineligible(reason)
}

#[test]
fn a_ceiling_of_twelve_hours_selects_the_automatic_prefix_with_the_extended_class() {
    for (model, ..) in GUIDE_NAMED {
        let selected = prepare(Serving::Api, model, extended(TWELVE_HOURS))
            .unwrap()
            .selected();

        assert_eq!(
            selected.map(crucible_types::PromptCacheSelected::mechanism),
            Some(PromptCacheMechanism::AutomaticPrefix),
            "{model}"
        );
        assert_eq!(
            selected.map(crucible_types::PromptCacheSelected::retention),
            Some(PromptCacheRetentionClass::Extended),
            "{model}"
        );
    }
}

#[test]
fn a_ceiling_a_second_short_of_twelve_hours_is_a_disallowed_retention() {
    for (model, ..) in GUIDE_NAMED {
        let selection = prepare(Serving::Api, model, extended(TWELVE_HOURS - 1)).unwrap();

        assert_eq!(
            selection.eligibility(),
            refused(PromptCacheIneligibleReason::DisallowedRetention),
            "{model}"
        );
    }
}

#[test]
fn explicit_breakpoints_alone_are_never_selected_for_the_extended_class() {
    for (model, ..) in GUIDE_NAMED {
        let policy = extended(TWELVE_HOURS).allowing(PromptCacheMechanisms::one(
            PromptCacheMechanism::ExplicitBreakpoints,
        ));
        let selection = prepare(Serving::Api, model, policy).unwrap();

        assert_eq!(
            selection.eligibility(),
            refused(PromptCacheIneligibleReason::DisallowedRetention),
            "{model}"
        );
    }
}

#[test]
fn no_openai_record_can_opt_out_so_a_prohibited_cache_is_refused_whatever_the_class() {
    for (model, ..) in GUIDE_NAMED {
        let policy = extended(TWELVE_HOURS).with_mode(PromptCacheMode::Prohibit);

        assert_eq!(
            prepare(Serving::Api, model, policy),
            Err(PromptCachePreparationError::ProhibitedWithoutOptOut),
            "{model}"
        );
    }
}

#[test]
fn the_subscription_route_never_selects_the_extended_class_at_any_ceiling() {
    for (model, ..) in GUIDE_NAMED {
        for ceiling in [
            TWELVE_HOURS - 1,
            TWELVE_HOURS,
            2 * TWELVE_HOURS,
            MAX_PROMPT_CACHE_RETENTION_SECONDS,
        ] {
            let selection = prepare(Serving::Subscription, model, extended(ceiling)).unwrap();

            assert_eq!(selection.selected(), None, "{model} at {ceiling}");
        }
    }
}

fn at(endpoint: Endpoint) -> OpenAi {
    OpenAi::at(
        endpoint,
        Box::new(HeaderKey::new(ApiKey::new("sk-test"), Header::bearer())),
        Box::new(Replay::new(200, "")),
    )
}

fn priced(
    provider: &OpenAi,
    model: &str,
    input_tokens: u64,
    retention: PromptCachePricedRetention,
) -> Option<PromptCachePricing> {
    provider
        .prompt_cache_pricing(
            model,
            Some(model),
            Some(input_tokens),
            retention,
            OpenAi::prices_reviewed(),
        )
        .unwrap()
}

#[test]
fn a_selected_extended_class_on_the_published_api_prices_the_longer_write_at_the_twelve_hour_rate()
{
    let vendor = at(OpenAi::VENDOR);
    for (model, short, long) in GUIDE_NAMED {
        for (tokens, twelve_hours) in [(272_000, short), (272_001, long)] {
            let base = priced(
                &vendor,
                model,
                tokens,
                PromptCachePricedRetention::Selected(PromptCacheRetentionClass::ProviderDefault),
            )
            .unwrap();
            let record = priced(
                &vendor,
                model,
                tokens,
                PromptCachePricedRetention::Selected(PromptCacheRetentionClass::Extended),
            )
            .unwrap_or_else(|| panic!("{model} at {tokens} has a 12-hour price"));

            assert_eq!(
                record.rates().cache_write_extended,
                UsageRate::priced(PriceRate::per_million(twelve_hours)),
                "{model} at {tokens}"
            );
            assert_eq!(
                record.rates().cache_write_or_creation,
                base.rates().cache_write_or_creation,
                "{model} at {tokens}"
            );
            assert_eq!(
                record.rates().uncached_input,
                base.rates().uncached_input,
                "{model} at {tokens}"
            );
            assert_eq!(
                base.rates().cache_write_extended,
                UsageRate::Unknown,
                "{model} at {tokens}"
            );
        }
    }
}

#[test]
fn an_extended_class_not_selected_or_off_the_published_api_has_no_price() {
    let vendor = at(OpenAi::VENDOR);
    let subscription = at(OpenAi::SUBSCRIPTION);
    for (model, ..) in GUIDE_NAMED {
        assert!(
            priced(
                &vendor,
                model,
                1_000,
                PromptCachePricedRetention::NotSelected(PromptCacheRetentionClass::Extended),
            )
            .is_none(),
            "{model} not selected"
        );
        assert!(
            priced(
                &subscription,
                model,
                1_000,
                PromptCachePricedRetention::Selected(PromptCacheRetentionClass::Extended),
            )
            .is_none(),
            "{model} on the subscription route"
        );
    }
}

#[test]
fn the_model_the_guide_does_not_name_keeps_its_own_extended_price() {
    let vendor = at(OpenAi::VENDOR);
    for retention in [
        PromptCachePricedRetention::Selected(PromptCacheRetentionClass::Extended),
        PromptCachePricedRetention::NotSelected(PromptCacheRetentionClass::Extended),
    ] {
        let record = priced(&vendor, "gpt-5.5", 1_000, retention).unwrap();

        assert_eq!(record.rates().cache_write_extended, UsageRate::Unknown);
    }
}
