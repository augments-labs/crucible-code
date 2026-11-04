//! Whether crucible finds out that a newer release exists, and whether an
//! installed agent's update is taken without a question.
//!
//! Its own module for the same reason `output` has one: the document holds a
//! string and the program holds a value, and the reading of it belongs beside
//! the type it produces. Nothing here reaches a server — this crate says what a
//! document may hold, and the wiring above it is what asks.

use serde_json::Value;

use super::Settings;

impl Settings {
    /// Whether to ask which release is newest, when nothing else says.
    #[must_use]
    pub fn updates(&self) -> Option<Updates> {
        Updates::read(self.value.get("updates")?.get("check")?.as_str()?)
    }

    /// Whether an update of an installed agent to a version the reviewed
    /// catalog lists is taken without asking.
    ///
    /// [`AgentUpdate::Ask`] where nothing said. The key widens, so this is
    /// only ever read from the user's own file: a project file that writes it
    /// is refused at the boundary, naming the file.
    #[must_use]
    pub fn agent_update(&self) -> AgentUpdate {
        self.value
            .get("agents")
            .and_then(|block| block.get("update"))
            .and_then(Value::as_str)
            .and_then(AgentUpdate::read)
            .unwrap_or_default()
    }
}

/// Whether an update of an installed agent is put to the user first.
///
/// Only the update question is ever skipped: a first install, a rollback, a
/// removal and a version the reviewed catalog does not list are not governed
/// by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentUpdate {
    /// Put every update to the user.
    #[default]
    Ask,
    /// Take an update to a reviewed, unrevoked version without asking.
    Reviewed,
}

impl AgentUpdate {
    /// Reads one of [`shape::AGENT_UPDATE`](crate::shape::AGENT_UPDATE).
    fn read(found: &str) -> Option<Self> {
        match found {
            "ask" => Some(Self::Ask),
            "reviewed" => Some(Self::Reviewed),
            _ => None,
        }
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

    #[test]
    fn an_update_is_asked_about_unless_the_users_own_file_says_reviewed() {
        assert_eq!(
            Settings::resolve(Vec::new()).agent_update(),
            AgentUpdate::Ask
        );
        let user = Document::sample(r#"{"agents": {"update": "reviewed"}}"#, Origin::User);
        assert_eq!(
            Settings::resolve(vec![user]).agent_update(),
            AgentUpdate::Reviewed
        );
        let user = Document::sample(r#"{"agents": {"update": "ask"}}"#, Origin::User);
        assert_eq!(
            Settings::resolve(vec![user]).agent_update(),
            AgentUpdate::Ask
        );
    }

    #[test]
    fn a_checkout_file_cannot_take_updates_without_asking_and_is_told_which_file() {
        for origin in [Origin::Project, Origin::ProjectLocal] {
            let error = Document::parse(
                r#"{"agents": {"update": "reviewed"}}"#,
                "/work/repo/.crucible/config.json",
                origin,
            )
            .unwrap_err();

            assert!(
                matches!(&error, crate::ConfigError::Widening { path, .. } if &**path == "agents.update"),
                "{error:?}"
            );
            assert!(
                error
                    .to_string()
                    .contains("/work/repo/.crucible/config.json"),
                "{error}"
            );
        }
    }

    #[test]
    fn every_agent_update_answer_the_document_accepts_reads_back_as_a_value() {
        for name in shape::AGENT_UPDATE {
            assert!(AgentUpdate::read(name).is_some(), "update: {name}");
        }
    }

    #[test]
    fn the_default_the_schema_states_for_agent_update_is_the_one_it_falls_back_to() {
        assert_eq!(
            AgentUpdate::read(shape::usual(&["agents", "update"])),
            Some(AgentUpdate::default())
        );
    }
}
