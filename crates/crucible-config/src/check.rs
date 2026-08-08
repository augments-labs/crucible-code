//! Walking a parsed document against the shape.
//!
//! Everything here is about the sentence the reader gets back. The walk knows
//! the dotted path it is standing at and the source text it came from, so a
//! refusal can name the key, say where it is, and say what was accepted
//! instead — rather than reporting that a file is invalid and leaving the
//! search to the person who has it open.

use serde_json::Value;

use crate::document::Origin;
use crate::env;
use crate::error::{Accepted, At, ConfigError};
use crate::shape::Shape;

/// Where in a document the walk is standing.
///
/// The two travel together everywhere — a refusal names the path and points at
/// the position — so they are carried as one thing rather than as a pair of
/// parameters threaded through every arm.
#[derive(Clone, Copy)]
pub(crate) struct Spot<'a> {
    /// The dotted route to this value.
    path: &'a str,
    /// Where the key holding it was found.
    at: At,
}

impl Spot<'_> {
    /// The root of a document, which no key holds and which has no path.
    pub(crate) const ROOT: Self = Self {
        path: "",
        at: At::Ambiguous,
    };
}

/// One document being checked: the text it came from and what to call it.
pub(crate) struct Reader<'a> {
    /// The file as the user would name it, for the message.
    pub(crate) file: &'a str,
    /// The source, so a key can be located in it.
    pub(crate) text: &'a str,
}

impl Reader<'_> {
    /// Checks one value against the shape it is standing in.
    pub(crate) fn check(
        &self,
        value: &Value,
        shape: &'static Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        match shape {
            Shape::Text => self.text_at(value, shape, spot),
            Shape::Choice(allowed) => self.choice(value, allowed, shape, spot),
            Shape::Fields(_) => self.fields(value, shape, spot),
            Shape::Named(inner) => self.named(value, inner, shape, spot),
        }
    }

    /// A string, and nothing else. A number where a model name goes is a
    /// mistake worth stopping for rather than coercing.
    fn text_at(&self, value: &Value, shape: &Shape, spot: Spot<'_>) -> Result<(), ConfigError> {
        if value.is_string() {
            return Ok(());
        }
        Err(self.wrong_type(shape, spot))
    }

    /// A string from the fixed set.
    fn choice(
        &self,
        value: &Value,
        allowed: &'static [&'static str],
        shape: &Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        let Some(found) = value.as_str() else {
            return Err(self.wrong_type(shape, spot));
        };
        if allowed.contains(&found) {
            return Ok(());
        }
        Err(ConfigError::NotAChoice {
            file: self.file.into(),
            path: spot.path.into(),
            found: found.into(),
            at: spot.at,
            accepted: Accepted::new(allowed.to_vec()),
        })
    }

    /// An object whose keys crucible chose. An unrecognised one stops the read.
    fn fields(
        &self,
        value: &Value,
        shape: &'static Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        let Some(object) = value.as_object() else {
            return Err(self.wrong_type(shape, spot));
        };

        for (key, held) in object {
            if reserved(key) {
                continue;
            }
            let path = join(spot.path, key);
            let at = At::of(key, self.text);

            let inner = shape.field(key).ok_or_else(|| ConfigError::UnknownKey {
                file: self.file.into(),
                path: path.as_str().into(),
                at,
                accepted: Accepted::new(shape.keys()),
            })?;
            self.check(held, inner, Spot { path: &path, at })?;
        }
        Ok(())
    }

    /// An object whose keys the user chose — a provider name, a variable name.
    /// There is no unknown key here, only a value of the wrong kind.
    fn named(
        &self,
        value: &Value,
        inner: &'static Shape,
        shape: &Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        let Some(object) = value.as_object() else {
            return Err(self.wrong_type(shape, spot));
        };

        for (key, held) in object {
            if reserved(key) {
                continue;
            }
            let path = join(spot.path, key);
            let at = At::of(key, self.text);
            self.check(held, inner, Spot { path: &path, at })?;
        }
        Ok(())
    }

    /// Refuses a variable that is not crucible's own in the file that travels
    /// with a clone.
    ///
    /// Structural rather than advisory: the one layer that reaches everyone who
    /// clones a repository can hold no value whose meaning crucible does not
    /// already fix, so a key cannot be leaked by a configuration file somebody
    /// committed without reading it. Crucible's own namespace is exempt because
    /// those names are settings rather than secrets — see [`crate::env`].
    ///
    /// Checked by origin rather than by filename, because the filename is the
    /// wiring's business and this rule is not.
    pub(crate) fn secrets(&self, value: &Value, origin: Origin) -> Result<(), ConfigError> {
        if origin != Origin::Project {
            return Ok(());
        }
        let Some(vars) = value.get("env").and_then(Value::as_object) else {
            return Ok(());
        };

        for name in vars.keys() {
            if env::ours(name) {
                continue;
            }
            return Err(ConfigError::SecretLayer {
                file: self.file.into(),
                // The name, never the value beside it. Naming the variable is
                // what makes the message actionable; quoting what was set there
                // would put a possible secret into an error string.
                name: name.as_str().into(),
                at: At::of(name, self.text),
                namespace: env::NAMESPACE,
            });
        }
        Ok(())
    }

    /// The same refusal from three places, built once.
    fn wrong_type(&self, shape: &Shape, spot: Spot<'_>) -> ConfigError {
        ConfigError::WrongType {
            file: self.file.into(),
            path: if spot.path.is_empty() {
                "the document".into()
            } else {
                spot.path.into()
            },
            wanted: shape.wanted(),
            at: spot.at,
        }
    }
}

/// Keys reserved by JSON Schema itself, which a document may carry anywhere and
/// which mean nothing to crucible.
///
/// `$schema` is how an editor knows which schema to complete against, so a
/// document that could not carry it would not get the completion this format
/// was chosen for. `$comment` is the specification's own answer to JSON having
/// no comments — the one real cost of the format, paid from the standard rather
/// than by inventing a dialect that standard parsers would reject.
///
/// These two by name rather than every key starting with a dollar, so that this
/// and the generated schema accept the same documents.
fn reserved(key: &str) -> bool {
    crate::schema::RESERVED.contains(&key)
}

/// The dotted path to a key, for a message that has to say where it is.
fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use crate::document::Document;

    use super::*;

    /// Reads a document as the layer that travels with a clone.
    fn shared(text: &str) -> Result<Document, ConfigError> {
        Document::parse(text, ".crucible/config.json", Origin::Project)
    }

    /// Reads a document as the layer git ignores.
    fn local(text: &str) -> Result<Document, ConfigError> {
        Document::parse(text, ".crucible/config.local.json", Origin::ProjectLocal)
    }

    #[test]
    fn a_setting_that_wants_a_string_refuses_a_number() {
        let err = shared(r#"{"output": {"color": 1}}"#).unwrap_err();
        let said = err.to_string();
        assert!(matches!(err, ConfigError::WrongType { .. }), "got {err:?}");
        assert!(said.contains("output.color"), "got {said}");
    }

    #[test]
    fn a_choice_names_what_it_accepts_rather_than_only_refusing() {
        let err = shared(r#"{"output": {"color": "beige"}}"#).unwrap_err();

        // Someone who wrote "beige" does not know the set. Listing it is both
        // the shortest thing to compute and more use than one guess at what
        // they meant.
        let said = err.to_string();
        assert!(matches!(err, ConfigError::NotAChoice { .. }), "got {err:?}");
        assert!(said.contains("auto"), "got {said}");
        assert!(said.contains("always"), "got {said}");
        assert!(said.contains("never"), "got {said}");
    }

    #[test]
    fn a_key_the_user_chose_is_not_checked_against_a_list() {
        // `providers` and `env` are keyed by names crucible cannot know. Only
        // the values inside them have a shape.
        local(r#"{"providers": {"anthropic": {"model": "claude-sonnet-5"}}}"#).unwrap();
        local(r#"{"env": {"RUST_LOG": "warn"}}"#).unwrap();
    }

    #[test]
    fn a_wrong_value_inside_a_user_named_key_still_names_its_full_path() {
        let err = shared(r#"{"providers": {"openai": {"model": []}}}"#).unwrap_err();
        let said = err.to_string();
        assert!(said.contains("providers.openai.model"), "got {said}");
    }

    #[test]
    fn the_schema_keys_json_reserves_are_carried_rather_than_refused() {
        // `$schema` is what makes an editor complete this file at all, and
        // `$comment` is the standard's answer to JSON having no comments. A
        // document that could not hold them would lose the reason the format
        // was chosen.
        local(
            r#"{
                 "$schema": "https://example.invalid/crucible-code-schema.json",
                 "$comment": "0.0.x is unstable",
                 "output": {"$comment": "dim the prompt", "color": "never"}
               }"#,
        )
        .unwrap();
    }

    #[test]
    fn someone_elses_variable_is_refused_in_the_file_that_travels_with_a_clone() {
        let err = shared(r#"{"env": {"TOKEN": "hunter2"}}"#).unwrap_err();

        // The refusal has to say where to put it instead, or the next move is
        // to delete the setting rather than to move it.
        let said = err.to_string();
        assert!(
            matches!(err, ConfigError::SecretLayer { .. }),
            "got {err:?}"
        );
        assert!(said.contains("config.local.json"), "got {said}");

        // And it must not quote what it refused. The whole point of refusing is
        // that the value might be a secret; echoing it into an error string
        // would put it exactly where G3 says it may never go.
        assert!(!said.contains("hunter2"), "got {said}");
    }

    #[test]
    fn crucibles_own_setting_is_allowed_even_in_the_file_that_travels() {
        // The namespace is what makes this safe to check in. A name crucible
        // owns is a knob crucible declares — it is read by this program and
        // means what this program says it means, so a project can set one for
        // everybody who clones it without that being a way to ship a secret.
        // An arbitrary name is where a key would hide, and only those are
        // refused above.
        shared(r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "12"}}"#).unwrap();
    }

    #[test]
    fn env_takes_anybodys_variable_in_the_layers_that_do_not_travel() {
        local(r#"{"env": {"RUST_LOG": "warn", "PAGER": "cat"}}"#).unwrap();
        Document::parse(
            r#"{"env": {"RUST_LOG": "warn"}}"#,
            "~/.crucible/config.json",
            Origin::User,
        )
        .unwrap();
    }

    #[test]
    fn a_dollar_key_the_standard_does_not_reserve_is_still_an_unknown_key() {
        // Two reserved names, not any name beginning with a dollar. The schema
        // generated from the shape names exactly these two, so accepting more
        // here would let through a document the reader's editor marks red —
        // and would swallow `$schemas` as a typo nobody is ever told about.
        let err = local(r#"{"$schemas": "x"}"#).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownKey { .. }), "got {err:?}");
    }

    #[test]
    fn a_refusal_points_at_the_line_the_key_is_on() {
        let err = shared("{\n  \"output\": {\n    \"colour\": \"never\"\n  }\n}").unwrap_err();
        let said = err.to_string();
        assert!(said.contains("line 3"), "got {said}");
    }

    #[test]
    fn a_key_that_appears_twice_is_reported_without_a_position() {
        // Two providers both setting `model` means two places the token is
        // found, and naming one of them sends the reader to a line that is
        // correct. No position is better than the wrong position.
        let err = shared(
            r#"{"providers": {"a": {"model": "x", "nope": 1}, "b": {"model": "y", "nope": 2}}}"#,
        )
        .unwrap_err();
        let said = err.to_string();
        assert!(matches!(err, ConfigError::UnknownKey { .. }), "got {err:?}");
        assert!(!said.contains("line"), "got {said}");
    }

    #[test]
    fn a_file_that_is_not_json_says_where_it_stopped_being_json() {
        let err = shared("{\n  \"output\": {,\n}").unwrap_err();
        let said = err.to_string();
        assert!(matches!(err, ConfigError::Malformed { .. }), "got {err:?}");
        assert!(said.contains("line 2"), "got {said}");
    }
}
