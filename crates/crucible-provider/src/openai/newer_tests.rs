//! GPT-6.1 Sol, GPT-6 Sol and GPT-6 Luna: cached as the vendor's other
//! models that bill a cache write are, and fast at twice the price with a key
//! and at the plan's rate signed in.

use super::*;
use crucible_models::FastForm;

const NEWER: [&str; 3] = ["gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna"];

#[test]
fn the_newer_models_have_a_reviewed_cache_record_that_counts_writes() {
    for model in NEWER {
        let record = prompt_cache(Serving::Api, model);
        assert_eq!(record.model_revision(), Some(model));
        assert_eq!(
            record.usage(),
            PromptCacheUsageReporting::ReadAndWriteTokens,
            "{model}"
        );
        assert!(cache_writes(model), "{model}");
        assert!(
            record
                .mechanisms()
                .iter()
                .all(|one| one.minimum_prefix_tokens() == 1_024),
            "{model}"
        );
    }
}

#[test]
fn the_newer_models_are_fast_at_twice_the_price_with_a_key_and_at_the_plans_rate_signed_in() {
    for model in NEWER {
        let FastForm::Field(keyed) = OpenAi::fast_at_vendor(model) else {
            panic!("{model} has a fast form with a key")
        };
        assert_eq!(keyed.price, "2x the price", "{model}");
        let FastForm::Field(signed) = OpenAi::fast_signed_in(model) else {
            panic!("{model} has a fast form signed in")
        };
        assert!(signed.price.contains("2.5x"), "{model}: {}", signed.price);
    }
}
