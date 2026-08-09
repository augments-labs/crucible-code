//! Rule text, read into the engine's own rules.
//!
//! Read here, while the document is still one document, because this is the
//! only place a bad rule can be reported the way a configuration error has to
//! be: with the file it is in and the position it is at. By the time the layers
//! are resolved a rule is one line among the lines every file contributed, and
//! nothing left could say which file it came from.
//!
//! So a `Rules` travels with the document that stated it, and resolving the
//! layers concatenates them. Nothing re-reads the text afterwards.

use crucible_core::{Disposition, Rules};
use serde_json::Value;

use super::check::Reader;
use crate::error::{At, ConfigError};

/// The keys holding rules, and what a rule under each one means.
///
/// The kinds are separate keys rather than one list of kind-and-pattern
/// entries: the kind is what decides which rule wins, and a `deny` list that
/// can be read on its own as the list of things that cannot happen is the whole
/// return on that.
const KINDS: [(&str, Disposition); 3] = [
    ("deny", Disposition::Deny),
    ("ask", Disposition::Ask),
    ("allow", Disposition::Allow),
];

/// Reads every rule one document states.
///
/// The value has already been walked against the shape, so anything found here
/// is a string in a list under a key crucible has. What is left to fail is the
/// text itself.
pub(super) fn read(reader: &Reader<'_>, value: &Value) -> Result<Rules, ConfigError> {
    let mut rules = Rules::new();

    let Some(block) = value.get("permissions") else {
        return Ok(rules);
    };

    for (key, kind) in KINDS {
        for (index, text) in written(block, key) {
            rules
                .add(kind, text)
                .map_err(|problem| ConfigError::BadRule {
                    file: reader.file.into(),
                    path: format!("permissions.{key}[{index}]").into(),
                    at: At::of(key, reader.text),
                    problem: problem.to_string().into(),
                })?;
        }
    }

    Ok(rules)
}

/// The strings written in one of the lists, with the index each sits at.
fn written<'a>(block: &'a Value, key: &str) -> impl Iterator<Item = (usize, &'a str)> {
    block
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, held)| Some((index, held.as_str()?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a document's rules, or the error that stopped it.
    fn rules(text: &str) -> Result<Rules, ConfigError> {
        let value: Value = serde_json::from_str(text).expect("the test wrote JSON");
        read(
            &Reader {
                file: ".crucible/config.json",
                text,
            },
            &value,
        )
    }

    #[test]
    fn a_document_with_no_permissions_block_states_no_rules() {
        assert!(rules(r#"{"output":{"color":"never"}}"#).unwrap().is_empty());
    }

    // Which key states which kind is settled where the rules meet the engine,
    // in `settings/permissions.rs`. Driving a `Permission` from here too would
    // be the same fixture written twice for the sake of a shorter path to it.

    #[test]
    fn a_rule_that_does_not_read_names_the_file_the_key_and_the_position() {
        // Somebody has this file open. "invalid configuration" sends them
        // looking; this sends them to the line.
        let err = rules(
            "{\n  \"permissions\": {\n    \"allow\": [\"read(src/**)\", \"read(src\"]\n  }\n}",
        )
        .unwrap_err();

        let said = err.to_string();
        assert!(matches!(err, ConfigError::BadRule { .. }), "got {err:?}");
        assert!(said.contains(".crucible/config.json"), "got {said}");
        assert!(said.contains("permissions.allow[1]"), "got {said}");
        assert!(said.contains("line 3"), "got {said}");
    }
}
