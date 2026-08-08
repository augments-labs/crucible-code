//! Which provider, and which model of it.
//!
//! `--model` takes one string because that is how a person thinks of the
//! choice: `gpt-5.2` is a model, and which company serves it is a fact about
//! the name rather than a second decision. The qualified form spells it out
//! when the name alone is not enough.
//!
//! Split here and nowhere else. Everything downstream is handed the two halves
//! already separated, so no later code has to know that a slash meant anything.

/// The provider used when the name is not qualified.
const DEFAULT: &str = "anthropic";

/// A provider and a model, as the command line named them.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    /// Which provider serves it.
    pub(crate) provider: Box<str>,
    /// What that provider calls the model.
    pub(crate) model: Box<str>,
}

impl Choice {
    /// Reads `provider/model`, or a bare model name served by the default.
    ///
    /// `None` when either half is missing. An empty model name reaches the API
    /// as a request for nothing, and the refusal that comes back describes the
    /// request rather than the flag that was typed wrong.
    pub(crate) fn parse(named: &str) -> Option<Self> {
        // The first slash, so that a name containing one — which the vendors
        // serving other companies' models use — stays intact.
        let (provider, model) = named
            .split_once('/')
            .map_or((DEFAULT, named.trim()), |(provider, model)| {
                (provider.trim(), model.trim())
            });

        if provider.is_empty() || model.is_empty() {
            return None;
        }

        Some(Self {
            provider: provider.into(),
            model: model.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(named: &str) -> (String, String) {
        let choice = Choice::parse(named).unwrap_or_else(|| panic!("{named:?} did not parse"));

        (choice.provider.to_string(), choice.model.to_string())
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
    fn a_missing_half_is_not_a_choice() {
        // Each of these otherwise reaches the API as a request for a model
        // called nothing, and the refusal describes the request rather than the
        // flag that was typed wrong.
        for named in ["", "   ", "openai/", "/gpt-5.2", "/"] {
            assert_eq!(Choice::parse(named), None, "{named:?} parsed");
        }
    }
}
