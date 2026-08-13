//! Which provider, and which model of it.
//!
//! `--model` takes one string because that is how a person thinks of the
//! choice: `gpt-5.2` is a model, and which company serves it is a fact about
//! the name rather than a second decision. The qualified form spells it out
//! when the name alone is not enough.
//!
//! Split here and nowhere else. Everything downstream is handed the two halves
//! already separated, so no later code has to know that a slash meant anything.
//!
//! Both halves are optional, and each is optional for its own reason. A name
//! with no provider in front of it is a name whose provider the machine has to
//! settle, because this file knows nothing about which keys are set and a guess
//! made here is the one that sends a model to the vendor that does not serve
//! it. A provider with no name after it — `--model openai/` — is asking for
//! whichever model was configured for that provider.
//!
//! The flag left off is not read here at all. There is nothing to parse, and
//! what settles it is the caller's to look up.

/// A provider and a model, as the command line named them.
///
/// Either half may be missing, and a missing half is a question for the caller
/// rather than a default to be filled in here.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Choice {
    /// Which provider serves it, when the flag said.
    pub(crate) provider: Option<Box<str>>,
    /// What that provider calls the model, when the flag said.
    pub(crate) model: Option<Box<str>>,
}

impl Choice {
    /// Reads `provider/model`, a bare model name whose provider is still open,
    /// or a provider with no model after it.
    ///
    /// `None` only when the provider half is written and empty, which is the one
    /// thing nothing downstream can fill in: `/gpt-5.2` names a company called
    /// nothing, and the refusal that comes back from resolving it would describe
    /// the request rather than the flag that was typed wrong.
    ///
    /// An empty string parses as neither half. That is `--model ""` and nothing
    /// else — a flag left off never reaches here.
    pub(crate) fn parse(named: &str) -> Option<Self> {
        // The first slash, so that a name containing one — which the vendors
        // serving other companies' models use — stays intact.
        let Some((provider, model)) = named.split_once('/') else {
            let model = named.trim();

            return Some(Self {
                provider: None,
                model: (!model.is_empty()).then(|| model.into()),
            });
        };

        let (provider, model) = (provider.trim(), model.trim());
        if provider.is_empty() {
            return None;
        }

        Some(Self {
            provider: Some(provider.into()),
            model: (!model.is_empty()).then(|| model.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(named: &str) -> (Option<String>, Option<String>) {
        let choice = Choice::parse(named).unwrap_or_else(|| panic!("{named:?} did not parse"));

        (
            choice.provider.map(|half| half.to_string()),
            choice.model.map(|half| half.to_string()),
        )
    }

    #[test]
    fn a_bare_name_leaves_the_provider_for_the_machine_to_settle() {
        // No provider is guessed here. Which one serves `claude-sonnet-5` is a
        // fact about which key this machine holds, and a fallback written into
        // the parser is what sends one vendor another vendor's model name.
        assert_eq!(
            parsed("claude-sonnet-5"),
            (None, Some("claude-sonnet-5".to_owned()))
        );
    }

    #[test]
    fn a_qualified_name_says_who_serves_it() {
        assert_eq!(
            parsed("openai/gpt-5.2"),
            (Some("openai".to_owned()), Some("gpt-5.2".to_owned()))
        );
    }

    #[test]
    fn only_the_first_slash_divides() {
        // Model names carry slashes of their own once a provider serves someone
        // else's models. Splitting on the last one would take the provider from
        // the middle of the name.
        assert_eq!(
            parsed("openai/meta/llama-4"),
            (Some("openai".to_owned()), Some("meta/llama-4".to_owned()))
        );
    }

    #[test]
    fn the_halves_are_taken_without_the_spaces_around_them() {
        assert_eq!(
            parsed(" openai / gpt-5.2 "),
            (Some("openai".to_owned()), Some("gpt-5.2".to_owned()))
        );
    }

    #[test]
    fn a_provider_with_no_model_after_it_asks_for_the_configured_one() {
        // The whole point of the trailing slash: `providers.openai.model` is
        // otherwise unreachable when the flag is present at all.
        assert_eq!(parsed("openai/"), (Some("openai".to_owned()), None));
    }

    #[test]
    fn a_flag_naming_nothing_at_all_names_neither_half() {
        // `--model ""`, which is a flag that was typed. A flag left off does not
        // come through here: nothing was named, so there is nothing to read.
        for named in ["", "   "] {
            assert_eq!(parsed(named), (None, None), "{named:?}");
        }
    }

    #[test]
    fn a_model_served_by_nobody_is_not_a_choice() {
        // The provider is the half nothing downstream can fill in. Written and
        // left empty, it reaches the wiring as a request to a company called
        // nothing.
        for named in ["/gpt-5.2", "/", " / "] {
            assert_eq!(Choice::parse(named), None, "{named:?} parsed");
        }
    }
}
