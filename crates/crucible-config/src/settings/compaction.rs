//! What a session does when the model's window fills.
//!
//! Its own module for the reason `updates` has one: the document holds words
//! and counts, and the program holds a decision. Nothing here decides anything
//! about a turn — this crate says what a document may hold, and the runner
//! above it is what reaches the bound.

use super::Settings;

impl Settings {
    /// What every layer together says about compaction.
    ///
    /// Always answers. A document that says nothing about compaction is a
    /// session that compacts on the derived reserve, which is the answer for
    /// somebody who has never heard of any of this — and the one place a
    /// default belongs, since the alternative is a turn that dies at a vendor.
    #[must_use]
    pub fn compaction(&self) -> Compaction {
        let block = self.value.get("compaction");
        let count = |key: &str| {
            block
                .and_then(|block| block.get(key))
                .and_then(serde_json::Value::as_u64)
        };

        Compaction {
            when: block
                .and_then(|block| block.get("when"))
                .and_then(serde_json::Value::as_str)
                .and_then(When::read)
                .unwrap_or(When::Full),
            reserve: count("reserve"),
            keep: count("keep"),
            spend_ceiling: count("spendCeiling"),
        }
    }
}

impl Settings {
    /// How much this model accepts, where a layer said so.
    ///
    /// Keyed by the model name exactly as it is asked for, so a session that
    /// changes model does not carry the last one's figure with it. `None` is
    /// nobody having said, which is not the same as a window of nothing.
    #[must_use]
    pub fn context_window(&self, provider: &str, model: &str) -> Option<u32> {
        let provider = self.value.get("providers")?.get(provider)?;

        // Named first, then whatever this provider says to assume. crucible
        // ships no figure of its own for either: a window it invented would be
        // wrong by a factor nobody could see until a session had already thrown
        // most of itself away, and the reactive rail is what covers the case
        // where nobody has said.
        let said = provider
            .get("contextWindow")
            .and_then(|windows| windows.get(model))
            .or_else(|| provider.get("defaultContextWindow"))?;

        u32::try_from(said.as_u64()?).ok()
    }
}

/// What a session does when the window fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    /// Whether the window filling is answered by compacting.
    pub when: When,
    /// The room to leave for the next exchange, in tokens, where a layer said.
    ///
    /// `None` is not "no reserve" — it is nobody having said, and the reserve
    /// is derived from the model's own ceilings instead. A zero written here is
    /// a user asking for no room at all, which is a different answer and is
    /// theirs to give.
    pub reserve: Option<u64>,
    /// How many whole turns are kept after the recap, where a layer said.
    pub keep: Option<u64>,
    /// The most one turn may produce before it is stopped, in tokens.
    ///
    /// `None` where nobody said, and then nothing stops a turn for spending.
    /// The bound this replaces counted tool calls, which is a proxy for the
    /// thing a runaway turn actually consumes; this is the thing itself.
    pub spend_ceiling: Option<u64>,
}

/// When crucible compacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Once the window has no room for another exchange.
    Full,
    /// Never on its own. `/compact` still compacts, because that is somebody
    /// asking rather than crucible deciding, and a turn that reaches the bound
    /// fails where it would have recovered.
    Never,
}

impl When {
    /// Whether crucible compacts without being asked.
    #[must_use]
    pub fn automatic(self) -> bool {
        matches!(self, Self::Full)
    }

    /// Reads one of [`shape::COMPACTION_WHEN`](crate::shape::COMPACTION_WHEN).
    fn read(found: &str) -> Option<Self> {
        match found {
            "full" => Some(Self::Full),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{Document, Origin};

    use super::*;

    #[test]
    fn a_document_that_says_nothing_still_compacts_when_the_window_fills() {
        let settings = Settings::resolve(vec![Document::sample("{}", Origin::User)]);

        assert_eq!(
            settings.compaction(),
            Compaction {
                when: When::Full,
                reserve: None,
                keep: None,
                spend_ceiling: None,
            }
        );
    }

    #[test]
    fn every_answer_is_read_back_as_the_value_it_becomes() {
        let said = r#"{"compaction": {"when": "never", "reserve": 25000,
            "keep": 4, "spendCeiling": 500000}}"#;
        let settings = Settings::resolve(vec![Document::sample(said, Origin::User)]);

        assert_eq!(
            settings.compaction(),
            Compaction {
                when: When::Never,
                reserve: Some(25_000),
                keep: Some(4),
                spend_ceiling: Some(500_000),
            }
        );
        assert!(!settings.compaction().when.automatic());
    }

    #[test]
    fn a_reserve_of_zero_is_an_answer_and_not_a_silence() {
        // Nobody having said and somebody asking for no room at all are
        // different, and only the first derives a reserve from the model.
        let said = r#"{"compaction": {"reserve": 0}}"#;
        let settings = Settings::resolve(vec![Document::sample(said, Origin::User)]);

        assert_eq!(settings.compaction().reserve, Some(0));
    }

    #[test]
    fn a_window_is_read_back_under_the_model_it_was_written_for() {
        let said = r#"{"providers": {"openai": {"contextWindow":
            {"gpt-5.6-sol": 272000, "gpt-5.5": 1050000}}}}"#;
        let settings = Settings::resolve(vec![Document::sample(said, Origin::User)]);

        assert_eq!(
            settings.context_window("openai", "gpt-5.6-sol"),
            Some(272_000)
        );
        assert_eq!(
            settings.context_window("openai", "gpt-5.5"),
            Some(1_050_000)
        );

        // A model nobody wrote a figure for, and a provider nobody did either.
        assert_eq!(settings.context_window("openai", "gpt-5.6-luna"), None);
        assert_eq!(settings.context_window("anthropic", "gpt-5.6-sol"), None);
    }

    #[test]
    fn a_provider_wide_figure_covers_the_models_nobody_named() {
        let said = r#"{"providers": {"openai": {"defaultContextWindow": 272000,
            "contextWindow": {"gpt-5.5": 1050000}}}}"#;
        let settings = Settings::resolve(vec![Document::sample(said, Origin::User)]);

        // The named model keeps its own figure, and one nobody named takes the
        // provider's. Both are somebody stating a fact rather than crucible
        // inventing one, which is the only reason either is allowed to exist.
        assert_eq!(
            settings.context_window("openai", "gpt-5.5"),
            Some(1_050_000)
        );
        assert_eq!(
            settings.context_window("openai", "gpt-5.9-unheard-of"),
            Some(272_000)
        );

        // And a provider nobody wrote anything for still answers nothing.
        assert_eq!(settings.context_window("anthropic", "claude-opus-5"), None);
    }

    #[test]
    fn a_nearer_layer_wins_one_answer_without_taking_the_others() {
        let user = Document::sample(r#"{"compaction": {"reserve": 9000}}"#, Origin::User);
        let local = Document::sample(r#"{"compaction": {"when": "never"}}"#, Origin::ProjectLocal);

        let settings = Settings::resolve(vec![user, local]);

        assert_eq!(settings.compaction().when, When::Never);
        assert_eq!(settings.compaction().reserve, Some(9_000));
    }
}
