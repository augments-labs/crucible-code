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
//! Only the provider half is required. A flag that names no model is asking for
//! whichever model was configured for that provider, which is what makes
//! `--model openai/` a way to say "the one I set at home" — and what lets the
//! flag be left off entirely.

/// The provider used when the name is not qualified.
const DEFAULT: &str = "anthropic";

/// A provider and a model, as the command line named them.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    /// Which provider serves it.
    pub(crate) provider: Box<str>,
    /// What that provider calls the model, when the flag said.
    pub(crate) model: Option<Box<str>>,
}

impl Choice {
    /// Reads `provider/model`, a bare model name served by the default, or a
    /// provider with no model after it.
    ///
    /// `None` only when the provider half is empty, which is the one thing
    /// nothing downstream can fill in: `/gpt-5.2` names a company called
    /// nothing, and the refusal that comes back from resolving it would
    /// describe the request rather than the flag that was typed wrong.
    ///
    /// An empty string is what the flag being left off looks like, and it
    /// parses: the default provider, no model named.
    pub(crate) fn parse(named: &str) -> Option<Self> {
        // The first slash, so that a name containing one — which the vendors
        // serving other companies' models use — stays intact.
        let (provider, model) = named
            .split_once('/')
            .map_or((DEFAULT, named.trim()), |(provider, model)| {
                (provider.trim(), model.trim())
            });

        if provider.is_empty() {
            return None;
        }

        Some(Self {
            provider: provider.into(),
            model: (!model.is_empty()).then(|| model.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(named: &str) -> (String, String) {
        let choice = Choice::parse(named).unwrap_or_else(|| panic!("{named:?} did not parse"));
        let model = choice
            .model
            .unwrap_or_else(|| panic!("{named:?} named no model"));

        (choice.provider.to_string(), model.to_string())
    }

    #[test]
    fn a_bare_name_is_served_by_the_default() {
        assert_eq!(
            parsed("claude-sonnet-5"),
            ("anthropic".to_owned(), "claude-sonnet-5".to_owned())
        );
    }

    #[test]
    fn a_qualified_name_says_who_serves_it() {
        assert_eq!(
            parsed("openai/gpt-5.2"),
            ("openai".to_owned(), "gpt-5.2".to_owned())
        );
    }

    #[test]
    fn only_the_first_slash_divides() {
        // Model names carry slashes of their own once a provider serves someone
        // else's models. Splitting on the last one would take the provider from
        // the middle of the name.
        assert_eq!(
            parsed("openai/meta/llama-4"),
            ("openai".to_owned(), "meta/llama-4".to_owned())
        );
    }

    #[test]
    fn the_halves_are_taken_without_the_spaces_around_them() {
        assert_eq!(
            parsed(" openai / gpt-5.2 "),
            ("openai".to_owned(), "gpt-5.2".to_owned())
        );
    }

    #[test]
    fn a_provider_with_no_model_after_it_asks_for_the_configured_one() {
        // The whole point of the trailing slash: `providers.openai.model` is
        // otherwise unreachable, because every other way of choosing openai
        // names a model in the same breath and the flag is the nearer layer.
        let choice = Choice::parse("openai/").expect("a provider was named");

        assert_eq!(&*choice.provider, "openai");
        assert_eq!(choice.model, None);
    }

    #[test]
    fn the_flag_left_off_is_the_default_provider_and_no_model() {
        // What `run` hands over when there is no `--model` at all, so that one
        // path resolves the choice rather than two.
        for named in ["", "   "] {
            let choice = Choice::parse(named).unwrap_or_else(|| panic!("{named:?} did not parse"));

            assert_eq!(&*choice.provider, DEFAULT, "{named:?}");
            assert_eq!(choice.model, None, "{named:?}");
        }
    }

    #[test]
    fn a_model_served_by_nobody_is_not_a_choice() {
        // The provider is the half nothing downstream can fill in. Left empty,
        // it reaches the wiring as a request to a company called nothing.
        for named in ["/gpt-5.2", "/", " / "] {
            assert_eq!(Choice::parse(named), None, "{named:?} parsed");
        }
    }
}
