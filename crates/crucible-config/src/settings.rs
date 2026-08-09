//! The layers, resolved into the settings a turn actually runs with.

use serde_json::{Map, Value};

use crate::document::Document;
use crate::shape::{DOCUMENT, Shape};

/// What every layer together says a setting is.
///
/// Built once at startup and read from there on, so nothing on the turn path
/// touches a file.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    value: Value,
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

        Self { value }
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

    /// Whether to write colour, when the command line does not say.
    #[must_use]
    pub fn color(&self) -> Option<Color> {
        Color::read(self.output("color")?)
    }

    /// How much of a tool call to show, when the command line does not say.
    #[must_use]
    pub fn tool_detail(&self) -> Option<ToolDetail> {
        ToolDetail::read(self.output("toolDetail")?)
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

    /// One string out of the `output` block.
    fn output(&self, key: &str) -> Option<&str> {
        self.value.get("output")?.get(key)?.as_str()
    }
}

/// Whether the terminal is written to in colour.
///
/// `Auto` is not the absence of an answer — it is the answer "decide from the
/// terminal", which a layer may state to override a nearer one that did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Follow the terminal and `NO_COLOR`.
    Auto,
    /// Colour even when the output is not a terminal.
    Always,
    /// Never, even when it is.
    Never,
}

impl Color {
    /// Reads one of [`shape::COLOR`](crate::shape::COLOR).
    ///
    /// `None` for anything else, which the shape refused before this could be
    /// reached. There is no fourth answer to fall back to and no call for a
    /// panic over a string that cannot arrive; the test below is what keeps
    /// "cannot arrive" true as the set changes.
    fn read(found: &str) -> Option<Self> {
        match found {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// How much of a tool call and its result one line shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDetail {
    /// Truncated to fit a line.
    Compact,
    /// Whatever the terminal is wide enough for.
    Full,
}

impl ToolDetail {
    /// Reads one of [`shape::TOOL_DETAIL`](crate::shape::TOOL_DETAIL).
    fn read(found: &str) -> Option<Self> {
        match found {
            "compact" => Some(Self::Compact),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// Lays a nearer layer over what the farther ones already said.
///
/// The shape decides how, which is what keeps the merge rule from becoming a
/// second description of the document. Anything that is not an object at this
/// position — every scalar, so `output.color` — is replaced outright by the
/// nearer layer. An object is merged key by key, so `providers` and `env` take
/// the nearest layer that mentioned each *name* rather than the nearest layer
/// that mentioned the block. Nothing in this release is a list; when the first
/// one arrives it will land in the first arm and be replaced wholesale, which
/// is the rule `docs/configuration/configuration.md` states, so adding one does not silently
/// get a rule nobody chose.
fn merge(base: &mut Value, near: &Value, shape: &'static Shape) {
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
    use crate::shape;

    use super::*;

    fn document(text: &str, origin: Origin) -> Document {
        Document::parse(text, "config.json", origin).unwrap()
    }

    #[test]
    fn a_project_naming_one_provider_leaves_the_users_other_one_alone() {
        let user = document(
            r#"{"providers": {"anthropic": {"model": "from-home"},
                              "openai": {"model": "also-from-home"}}}"#,
            Origin::User,
        );
        let project = document(
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
    fn the_nearest_layer_that_set_a_scalar_wins_it_outright() {
        let user = document(
            r#"{"output": {"color": "always", "toolDetail": "full"}}"#,
            Origin::User,
        );
        let local = document(r#"{"output": {"color": "never"}}"#, Origin::ProjectLocal);

        let settings = Settings::resolve(vec![user, local]);

        assert_eq!(settings.color(), Some(Color::Never));

        // `output` is still an object, so it merges key by key like any other.
        // Only the scalar inside it is replaced, and a layer that said nothing
        // about `toolDetail` has not thereby unset it.
        assert_eq!(settings.tool_detail(), Some(ToolDetail::Full));
    }

    #[test]
    fn a_provider_can_be_told_which_variable_holds_its_key() {
        let user = document(
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
    fn a_setting_no_layer_mentioned_is_left_for_the_command_line_to_decide() {
        // None is "the files did not say", not a default. The default lives
        // where it already lives, and the wiring lays the command line over
        // this.
        let settings = Settings::resolve(Vec::new());

        assert_eq!(settings.color(), None);
        assert_eq!(settings.tool_detail(), None);
        assert_eq!(settings.model("anthropic"), None);
    }

    #[test]
    fn env_takes_the_nearest_layer_that_named_each_variable() {
        let user = document(
            r#"{"env": {"RUST_LOG": "warn", "PAGER": "cat"}}"#,
            Origin::User,
        );
        let local = document(r#"{"env": {"RUST_LOG": "debug"}}"#, Origin::ProjectLocal);

        let settings = Settings::resolve(vec![user, local]);
        let mut found: Vec<_> = settings.env().collect();
        found.sort_unstable();

        // Per name, not per block: overriding one variable in a checkout does
        // not turn off the rest of what the user set at home.
        assert_eq!(found, vec![("PAGER", "cat"), ("RUST_LOG", "debug")]);
    }

    #[test]
    fn every_answer_the_document_accepts_reads_back_as_a_value() {
        // The shape decides what a document may say and this module decides
        // what each answer means, so the two lists have to agree. Without this,
        // renaming an answer in the shape leaves the reader matching a string
        // nobody can write any more, and the setting stops working with no
        // error anywhere — the schema would accept the file and the value would
        // be dropped on the floor.
        for name in shape::COLOR {
            assert!(Color::read(name).is_some(), "color: {name}");
        }
        for name in shape::TOOL_DETAIL {
            assert!(ToolDetail::read(name).is_some(), "toolDetail: {name}");
        }
    }
}
