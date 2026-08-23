//! What each model crucible offers accepts and produces.
//!
//! Generated. Do not edit: `scripts/models.sh` writes this file, and a test
//! rewrites it and then fails wherever a row disagrees with `models.json`
//! beside it, which is the slice of the database this was read from — so a
//! row changed by hand is a row the next run of the suite discards. What that
//! slice is a slice *of* is a public database of model limits, read over the
//! network by a `curl` in that script rather than by anything here.
//!
//! Keyed on the model name exactly as crucible asks for it. A name not in this
//! table has no answer here at all, which is deliberate: a window guessed from a
//! name that merely resembles one is wrong by a factor nobody would notice until
//! a session had already thrown half of itself away.

use crucible_core::{Modalities, Modality};

/// What one model accepts and produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The provider it is asked of.
    pub(crate) provider: &'static str,
    /// The model, spelled the way crucible asks for it.
    pub(crate) model: &'static str,
    /// The most one request may carry, in tokens.
    pub(crate) window: u32,
    /// The most one answer may produce, in tokens.
    pub(crate) output: u32,
    /// What the model reads. Half of what may be attached; the
    /// other half is what the provider can spell.
    pub(crate) accepts: Modalities,
}

/// Every model this build knows the limits of, sorted so a diff reads.
pub(crate) const FACTS: &[Facts] = &[
    Facts {
        provider: "anthropic",
        model: "claude-fable-5",
        window: 1_000_000,
        output: 128_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
    Facts {
        provider: "anthropic",
        model: "claude-haiku-4-5",
        window: 200_000,
        output: 64_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
    Facts {
        provider: "anthropic",
        model: "claude-opus-5",
        window: 1_000_000,
        output: 128_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
    Facts {
        provider: "anthropic",
        model: "claude-sonnet-5",
        window: 1_000_000,
        output: 128_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
    Facts {
        provider: "moonshot",
        model: "k3",
        window: 1_048_576,
        output: 131_072,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video),
    },
    Facts {
        provider: "moonshot",
        model: "k3-256k",
        window: 262_144,
        output: 131_072,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video),
    },
    Facts {
        provider: "moonshot",
        model: "kimi-for-coding",
        window: 262_144,
        output: 262_144,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video),
    },
    Facts {
        provider: "moonshot",
        model: "kimi-for-coding-highspeed",
        window: 262_144,
        output: 262_144,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video),
    },
    Facts {
        provider: "openai",
        model: "gpt-5.5",
        window: 922_000,
        output: 128_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
    Facts {
        provider: "openai",
        model: "gpt-5.6-luna",
        window: 922_000,
        output: 128_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
    Facts {
        provider: "openai",
        model: "gpt-5.6-sol",
        window: 922_000,
        output: 128_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
    Facts {
        provider: "openai",
        model: "gpt-5.6-terra",
        window: 922_000,
        output: 128_000,
        accepts: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    },
];
