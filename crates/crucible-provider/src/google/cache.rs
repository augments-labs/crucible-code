//! Reviewed Interactions implicit-cache contract and standard API token prices.
//!
//! No explicit cache handles or opt-out controls exist on this route. Prices are
//! published standard paid-tier estimates, not account invoices. Native-tool
//! allowances and charges are unknown, and custom recipients have no prices.

use super::{PROTOCOL, VENDOR_URL};
use crucible_models::{
    PriceRate, PromptCacheCapabilities, PromptCacheContent, PromptCacheMechanismCapability,
    PromptCachePricing, PromptCacheProvenance, PromptCacheRates, Request,
    StatefulTransportCapability, UsageRate,
};
use crucible_types::{
    PricingCurrency, PricingDate, PricingUnit, PromptCacheEncoding, PromptCacheIneligibleReason,
    PromptCacheMechanism, PromptCacheRetentionClass, PromptCacheUsageReporting,
};

const REVIEWED: PricingDate = PricingDate::new(2026, 9, 6);
const CHANGE: PricingDate = PricingDate::new(2027, 1, 1);
/// The day `gemini-4-argon`'s model, caching and pricing pages were read. Its
/// standard price rises on `ARGON_CHANGE`, so the price record has two steps.
const ARGON: &str = "gemini-4-argon";
const ARGON_REVIEWED: PricingDate = PricingDate::new(2026, 11, 5);
const ARGON_CHANGE: PricingDate = PricingDate::new(2026, 12, 1);
const CONTENT: &[PromptCacheContent] = &[
    PromptCacheContent::Text,
    PromptCacheContent::Tools,
    PromptCacheContent::Images,
    PromptCacheContent::Documents,
    PromptCacheContent::Audio,
    PromptCacheContent::Video,
];

fn reviewed(model: &str) -> Option<&'static str> {
    match model {
        "gemini-3.8-flash" => Some("gemini-3.8-flash"),
        "gemini-3.7-flash" => Some("gemini-3.7-flash"),
        "gemini-3.6-flash" => Some("gemini-3.6-flash"),
        "gemini-3.1-pro-preview" => Some("gemini-3.1-pro-preview"),
        ARGON => Some(ARGON),
        _ => None,
    }
}

pub(super) fn capabilities(model: &str) -> PromptCacheCapabilities {
    let Some(model) = reviewed(model) else {
        return PromptCacheCapabilities::unknown("unreviewed model");
    };
    let automatic = PromptCacheMechanismCapability::automatic_prefix(4096, false, false, CONTENT)
        .with_retentions(&[PromptCacheRetentionClass::ProviderDefault]);
    // Argon's caching page says explicit caching is not supported for it and
    // gives a 4,096-token minimum for implicit caching; read 2026-11-05.
    let (day, version) = if model == ARGON {
        ("2026-11-05", "google-interactions-cache-2026-11-05")
    } else {
        ("2026-09-06", "google-interactions-cache-2026-09-06")
    };
    PromptCacheCapabilities::supported(
        version,
        Some(model),
        PromptCacheProvenance::new(
            "https://ai.google.dev/gemini-api/docs/caching",
            day,
            version,
        ),
        StatefulTransportCapability::Unsupported,
        &[automatic],
        PromptCacheUsageReporting::ReadTokens,
    )
}

pub(super) fn encoding(request: &Request<'_>) -> PromptCacheEncoding {
    let Some(selected) = request
        .prompt_cache
        .and_then(|cache| cache.selection.selected())
    else {
        return PromptCacheEncoding::NoControlIntended;
    };
    match selected.mechanism() {
        PromptCacheMechanism::AutomaticPrefix | PromptCacheMechanism::ProviderManagedUsageOnly => {
            PromptCacheEncoding::NoExtraControlEncoded
        }
        PromptCacheMechanism::ExplicitBreakpoints | PromptCacheMechanism::PersistentContent => {
            PromptCacheEncoding::Failed(PromptCacheIneligibleReason::Unsupported)
        }
    }
}

pub(super) fn pricing(
    model: &str,
    revision: Option<&str>,
    tokens: Option<u64>,
    retention: PromptCacheRetentionClass,
    at: PricingDate,
) -> Option<PromptCachePricing> {
    let model = reviewed(model)?;
    let tokens = tokens?;
    let argon = model == ARGON;
    if revision != Some(model)
        || at < if argon { ARGON_REVIEWED } else { REVIEWED }
        || retention != PromptCacheRetentionClass::ProviderDefault
    {
        return None;
    }
    let pro = model == "gemini-3.1-pro-preview";
    // Argon, per million tokens: $2.00 in, $0.10 read, $10.00 out through
    // 2026-11-30, then $4.00, $0.20 and $20.00 from 2026-12-01.
    let (input, read, output) = if argon {
        if at < ARGON_CHANGE {
            (2_000_000_000, 100_000_000, 10_000_000_000)
        } else {
            (4_000_000_000, 200_000_000, 20_000_000_000)
        }
    } else if pro {
        if tokens > 200_000 {
            (4_000_000_000, 400_000_000, 18_000_000_000)
        } else {
            (2_000_000_000, 200_000_000, 12_000_000_000)
        }
    } else if at < CHANGE {
        (750_000_000, 75_000_000, 3_750_000_000)
    } else {
        (1_500_000_000, 150_000_000, 7_500_000_000)
    };
    let mut price = PromptCachePricing::new(
        PROTOCOL,
        VENDOR_URL,
        model,
        Some(model),
        if argon {
            if at >= ARGON_CHANGE {
                ARGON_CHANGE
            } else {
                ARGON_REVIEWED
            }
        } else if !pro && at >= CHANGE {
            CHANGE
        } else {
            REVIEWED
        },
        if argon {
            "google-standard-2026-11-05"
        } else {
            "google-standard-2026-09-06"
        },
        "https://ai.google.dev/gemini-api/docs/pricing",
        PricingCurrency::new("USD"),
        PricingUnit::MillionTokens,
        PromptCacheRates {
            uncached_input: UsageRate::priced(PriceRate::per_million(input)),
            cache_read: UsageRate::priced(PriceRate::per_million(read)),
            output: UsageRate::priced(PriceRate::per_million(output)),
            cache_write_or_creation: UsageRate::NotApplicable,
            reasoning: UsageRate::NotApplicable,
            storage: UsageRate::NotApplicable,
            other: UsageRate::Unknown,
        },
    );
    if pro {
        price = if tokens > 200_000 {
            price.with_input_band(200_001, None)
        } else {
            price.with_input_band(0, Some(200_000))
        };
    } else if argon {
        if at < ARGON_CHANGE {
            price = price.through(PricingDate::new(2026, 11, 30));
        }
    } else if at < CHANGE {
        price = price.through(PricingDate::new(2026, 12, 31));
    }
    Some(price)
}
