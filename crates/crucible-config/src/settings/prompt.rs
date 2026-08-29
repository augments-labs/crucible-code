//! What the layers together say the model is asked under.
//!
//! Three keys, and the interesting thing about them is which layer each may
//! come from. `tone` and `append` are ordinary settings and a checkout may hold
//! either. `custom` replaces crucible's own instructions — the ones about
//! reading a file before changing it and asking before building the wrong thing
//! — and a repository is not allowed to take those away from whoever cloned it.
//! That is declared beside the key in `shape`, and the walk refuses it in both
//! project layers before anything here is reached; this module reads what
//! survived.
//!
//! The two hooks stay separate strings rather than one key with a mode. A
//! reader who wants a paragraph added should not have to restate the whole
//! prompt to keep it, and a reader who wants their own prompt should not have
//! it silently concatenated with the one they were replacing.

use crucible_core::Tone;

use super::Settings;

impl Settings {
    /// How much of the reasoning comes back with the answer.
    ///
    /// `None` where no layer said, which the prompt reads as the default rather
    /// than as an instruction to say nothing about how to answer.
    #[must_use]
    pub fn tone(&self) -> Option<Tone> {
        self.prompt("tone")?.parse().ok()
    }

    /// Instructions to ask under in place of crucible's own.
    #[must_use]
    pub fn custom_prompt(&self) -> Option<&str> {
        self.prompt("custom")
    }

    /// Instructions to ask under as well as crucible's own.
    #[must_use]
    pub fn appended_prompt(&self) -> Option<&str> {
        self.prompt("append")
    }

    /// One string out of the `systemPrompt` block.
    ///
    /// A key written as an empty string is nothing said. The other readings of
    /// it are worse: an empty `custom` would replace the whole prompt with
    /// silence, and an empty `append` would add a blank paragraph to it.
    fn prompt(&self, key: &str) -> Option<&str> {
        let said = self.value.get("systemPrompt")?.get(key)?.as_str()?.trim();

        (!said.is_empty()).then_some(said)
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::Tone;

    use crate::document::{Document, Origin};
    use crate::shape;

    use super::super::Settings;

    /// One home-directory layer, which is the only one `custom` survives.
    fn settings(text: &str) -> Settings {
        Settings::resolve(vec![Document::sample(text, Origin::User)])
    }

    #[test]
    fn every_tone_a_document_may_write_is_one_the_program_holds() {
        // The second `Choice` whose meaning belongs to another crate, tested in
        // both directions for the reason the first one is: a word here the
        // program no longer parses is a key the schema completes and the
        // program drops, and a tone the program grew and this list did not is
        // one no configuration file can reach.
        for name in shape::TONE {
            let tone: Tone = name.parse().unwrap_or_else(|_| panic!("no tone: {name}"));
            assert_eq!(tone.as_str(), *name);
        }

        assert_eq!(
            shape::TONE.len(),
            Tone::TONES.len(),
            "a tone the program holds that no document may write"
        );
    }

    #[test]
    fn the_default_the_schema_states_for_tone_is_the_one_it_falls_back_to() {
        // Two statements of one answer: the word an editor fills in and the
        // variant a session with no `systemPrompt` block runs with.
        assert_eq!(
            shape::usual(&["systemPrompt", "tone"]).parse(),
            Ok(Tone::default())
        );
    }

    #[test]
    fn a_prompt_block_nobody_wrote_says_nothing_about_any_of_the_three() {
        let settings = Settings::default();

        assert_eq!(settings.tone(), None);
        assert_eq!(settings.custom_prompt(), None);
        assert_eq!(settings.appended_prompt(), None);
    }

    #[test]
    fn a_hook_written_as_an_empty_string_is_nothing_said() {
        // The alternatives are both worse than ignoring it: an empty `custom`
        // would replace the whole prompt with silence, and an empty `append`
        // would add a blank paragraph to it.
        let settings =
            settings(r#"{"systemPrompt": {"custom": "", "append": "   ", "tone": "learning"}}"#);

        assert_eq!(settings.custom_prompt(), None);
        assert_eq!(settings.appended_prompt(), None);
        assert_eq!(settings.tone(), Some(Tone::Learning));
    }

    #[test]
    fn the_two_hooks_are_read_apart_from_one_another() {
        // A reader who wants a paragraph added should not have to restate the
        // prompt to keep it, which only works if adding and replacing are
        // different keys.
        let settings =
            settings(r#"{"systemPrompt": {"custom": "Review only.", "append": "Never push."}}"#);

        assert_eq!(settings.custom_prompt(), Some("Review only."));
        assert_eq!(settings.appended_prompt(), Some("Never push."));
    }
}
