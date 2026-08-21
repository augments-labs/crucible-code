//! Whether crucible finds out that a newer release exists.
//!
//! Its own module for the same reason `output` has one: the document holds a
//! string and the program holds a value, and the reading of it belongs beside
//! the type it produces. Nothing here reaches a server — this crate says what a
//! document may hold, and the wiring above it is what asks.

use super::Settings;

impl Settings {
    /// Whether to ask which release is newest, when nothing else says.
    #[must_use]
    pub fn updates(&self) -> Option<Updates> {
        Updates::read(self.value.get("updates")?.get("check")?.as_str()?)
    }
}

/// Whether crucible asks which release is newest.
///
/// `Auto` is not the absence of an answer — it is the answer "ask", which a
/// layer may state to override a nearer one that said not to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Updates {
    /// Ask, at most once a day, on a thread of its own.
    #[default]
    Auto,
    /// Never reach the network for this.
    Never,
}

impl Updates {
    /// Whether this answer means the question gets asked.
    #[must_use]
    pub fn wanted(self) -> bool {
        matches!(self, Self::Auto)
    }

    /// Reads one of [`shape::UPDATE_CHECK`](crate::shape::UPDATE_CHECK).
    fn read(found: &str) -> Option<Self> {
        match found {
            "auto" => Some(Self::Auto),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{Document, Origin};
    use crate::shape;

    use super::*;

    #[test]
    fn a_machine_that_says_never_is_not_asked_again_by_a_farther_layer() {
        let user = Document::sample(r#"{"updates": {"check": "never"}}"#, Origin::User);

        assert_eq!(
            Settings::resolve(vec![user]).updates(),
            Some(Updates::Never)
        );
    }

    #[test]
    fn nothing_said_is_left_for_the_wiring_to_decide() {
        assert_eq!(Settings::resolve(Vec::new()).updates(), None);
    }

    #[test]
    fn every_answer_the_document_accepts_reads_back_as_a_value() {
        // The shape decides what a document may say and this module decides
        // what each answer means, so the two lists have to agree.
        for name in shape::UPDATE_CHECK {
            assert!(Updates::read(name).is_some(), "check: {name}");
        }
    }

    #[test]
    fn asking_is_what_auto_means_and_the_only_thing_that_does() {
        assert!(Updates::Auto.wanted());
        assert!(!Updates::Never.wanted());
    }

    #[test]
    fn the_default_the_schema_states_for_check_is_the_one_it_falls_back_to() {
        assert_eq!(
            Updates::read(shape::usual(&["updates", "check"])),
            Some(Updates::default())
        );
    }
}
