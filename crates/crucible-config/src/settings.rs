//! The layers, resolved into the settings a turn actually runs with.
//!
//! This file holds what every block shares — the layering, and the merge that
//! decides which layer wins a position. A block that carries meaning of its own
//! has a module beside it under `settings/`, so its keys, the types they become
//! and the tests for both sit in one place. `providers` and `env` stay here:
//! they are read out as the strings they were written as, and there is nothing
//! to interpret.

use std::fmt;

use crucible_core::Rules;
use serde_json::{Map, Value};

use crate::document::Document;
use crate::env;
use crate::shape::{DOCUMENT, Shape};

mod layers;
mod output;
mod permissions;
mod variables;

pub use layers::local;
pub use output::{Color, Glyphs, ToolDetail};
pub use variables::ClearScreen;

pub(crate) use variables::refused;

/// What every layer together says a setting is.
///
/// Built once at startup and read from there on, so nothing on the turn path
/// touches a file.
#[derive(Clone, Default)]
pub struct Settings {
    value: Value,

    /// Every layer's rules together. Held apart from the value because a rule
    /// is read where it is written — see [`Document::parse`] — and what survives
    /// the layering is the rule rather than its text.
    rules: Rules,
}

impl fmt::Debug for Settings {
    /// Written by hand so the `env` block is redacted. This type is what the
    /// wiring above holds for the whole session, so it is the one most likely
    /// to end up inside somebody's diagnostic — and it holds every variable
    /// the two private layers set, values and all.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("value", &env::Redacted(&self.value))
            .field("rules", &self.rules)
            .finish()
    }
}

impl Settings {
    /// Resolves the documents that were found, in any order.
    ///
    /// Order is taken from each document's origin rather than from the sequence
    /// they arrive in, so a caller that reads the files in a different order
    /// than it lists them still gets the same answer.
    ///
    /// Not public: [`Settings::read`] is the one way in, so there is no second
    /// path that could find a different set of files than the first one does.
    pub(crate) fn resolve(mut documents: Vec<Document>) -> Self {
        documents.sort_by_key(|document| document.origin().nearness());

        let mut value = Value::Object(Map::new());
        for document in &documents {
            merge(&mut value, document.value(), &DOCUMENT);
        }

        // Concatenated, like every list in the document and for the reason
        // `Rules::absorb` gives: a nearer layer may add to what is allowed and
        // may not take away what a farther one denied.
        let mut rules = Rules::new();
        for document in documents {
            rules.absorb(document.rules());
        }

        Self { value, rules }
    }

    /// The model to ask this provider for, when the command line names none.
    #[must_use]
    pub fn model(&self, provider: &str) -> Option<&str> {
        self.value
            .get("providers")?
            .get(provider)?
            .get("model")?
            .as_str()
    }

    /// The name of the variable this provider's key is read from.
    ///
    /// The name only. A key has no path into a configuration file: this is the
    /// setting for somebody who keeps a work key and a personal key in two
    /// variables, and the value still comes from the environment.
    #[must_use]
    pub fn api_key_env(&self, provider: &str) -> Option<&str> {
        self.value
            .get("providers")?
            .get(provider)?
            .get("apiKeyEnv")?
            .as_str()
    }

    /// The variables the commands crucible runs are started with.
    ///
    /// Crucible's own environment is not touched and cannot be: writing to it
    /// is `unsafe` in edition 2024 and this workspace forbids that. So this
    /// block says what `cargo test` or `git` sees, not what crucible sees —
    /// crucible's own settings have keys of their own, and the one variable it
    /// reads before opening a file is refused here outright rather than left to
    /// look applied.
    pub fn env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.value
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|vars| {
                vars.iter()
                    .filter_map(|(name, value)| Some((name.as_str(), value.as_str()?)))
            })
    }
}

/// Lays a nearer layer over what the farther ones already said.
///
/// The shape decides how, which is what keeps the merge rule from becoming a
/// second description of the document. A scalar — `output.color` — is replaced
/// outright by the nearer layer. An object is merged key by key, so `providers`
/// and `env` take the nearest layer that mentioned each *name* rather than the
/// nearest layer that mentioned the block. A list is concatenated: a nearer
/// layer adds entries and removes none, which is the only rule that leaves a
/// `deny` written at home standing when a checked-out repository states rules
/// of its own. All three are in `docs/configuration/configuration.md`.
fn merge(base: &mut Value, near: &Value, shape: &'static Shape) {
    if shape.element().is_some()
        && let (Some(into), Some(from)) = (base.as_array_mut(), near.as_array())
    {
        into.extend(from.iter().cloned());
        return;
    }

    let (Some(into), Some(from)) = (base.as_object_mut(), near.as_object()) else {
        *base = near.clone();
        return;
    };

    for (key, value) in from {
        // A key with no shape is one JSON reserves — `$schema`, `$comment`.
        // Nothing merges into it, so the nearer layer's copy stands.
        match (into.get_mut(key), shape.field(key)) {
            (Some(held), Some(inner)) => merge(held, value, inner),
            _ => {
                into.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::Origin;

    use super::*;

    #[test]
    fn a_project_naming_one_provider_leaves_the_users_other_one_alone() {
        let user = Document::sample(
            r#"{"providers": {"anthropic": {"model": "from-home"},
                              "openai": {"model": "also-from-home"}}}"#,
            Origin::User,
        );
        let project = Document::sample(
            r#"{"providers": {"anthropic": {"model": "from-project"}}}"#,
            Origin::Project,
        );

        // Handed over nearest-first, to prove the order they arrive in is not
        // what decides precedence.
        let settings = Settings::resolve(vec![project, user]);

        assert_eq!(settings.model("anthropic"), Some("from-project"));

        // The one the project said nothing about. A map is merged key by key,
        // so naming one provider is not a way to delete the others — a project
        // that pins its own model must not silently take away the model the
        // user set for a provider it never mentioned.
        assert_eq!(settings.model("openai"), Some("also-from-home"));
    }

    #[test]
    fn a_provider_can_be_told_which_variable_holds_its_key() {
        let user = Document::sample(
            r#"{"providers": {"anthropic": {"apiKeyEnv": "WORK_ANTHROPIC_KEY"}}}"#,
            Origin::User,
        );

        let settings = Settings::resolve(vec![user]);

        // The name. Nothing in this crate ever reads the variable, and nothing
        // in a document ever carries the value.
        assert_eq!(
            settings.api_key_env("anthropic"),
            Some("WORK_ANTHROPIC_KEY")
        );
        assert_eq!(settings.api_key_env("openai"), None);
    }

    #[test]
    fn a_provider_no_layer_mentioned_is_left_for_the_command_line_to_decide() {
        // None is "the files did not say", not a default. The default lives
        // where it already lives, and the wiring lays the command line over
        // this.
        assert_eq!(Settings::resolve(Vec::new()).model("anthropic"), None);
    }

    #[test]
    fn env_takes_the_nearest_layer_that_named_each_variable() {
        let user = Document::sample(
            r#"{"env": {"RUST_LOG": "warn", "PAGER": "cat"}}"#,
            Origin::User,
        );
        let local = Document::sample(r#"{"env": {"RUST_LOG": "debug"}}"#, Origin::ProjectLocal);

        let settings = Settings::resolve(vec![user, local]);
        let mut found: Vec<_> = settings.env().collect();
        found.sort_unstable();

        // Per name, not per block: overriding one variable in a checkout does
        // not turn off the rest of what the user set at home.
        assert_eq!(found, vec![("PAGER", "cat"), ("RUST_LOG", "debug")]);
    }

    #[test]
    fn printing_the_settings_names_a_variable_and_shows_nothing_of_its_value() {
        // The layers git ignores may hold anything the user's commands need, so
        // this type holds secrets by design. What it must not do is print one:
        // a `{settings:?}` in a diagnostic somebody adds later is a leak nobody
        // reviewed, which is why the redaction lives in the type rather than in
        // the call sites.
        let local = Document::sample(r#"{"env": {"TOKEN": "hunter2"}}"#, Origin::ProjectLocal);
        let settings = Settings::resolve(vec![local]);

        let printed = format!("{settings:?}");
        assert!(printed.contains("TOKEN"), "got {printed}");
        assert!(!printed.contains("hunter2"), "got {printed}");

        // And the value is still there for the commands that need it.
        assert_eq!(
            settings.env().collect::<Vec<_>>(),
            vec![("TOKEN", "hunter2")]
        );
    }

    #[test]
    fn printing_the_settings_shows_everything_that_is_not_a_variable() {
        // Redacting is not a reason to print nothing. A diagnostic about which
        // model a turn asked for is exactly what this would be read for.
        let user = Document::sample(
            r#"{"providers": {"anthropic": {"model": "from-home"}}}"#,
            Origin::User,
        );

        let printed = format!("{:?}", Settings::resolve(vec![user]));
        assert!(printed.contains("from-home"), "got {printed}");
    }
}
