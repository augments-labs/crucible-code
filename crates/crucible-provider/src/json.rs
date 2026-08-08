//! The small parts of building a request body that every protocol shares.
//!
//! Not a request builder. Providers differ in shape and that difference is the
//! point of having more than one of them; what they agree on is narrower —
//! where a tool's description comes from, and that nothing here may panic on a
//! payload it did not expect.
//!
//! Fields are inserted rather than assigned by index. Indexing a JSON value
//! panics on anything that is not the container it expected, and nothing that
//! builds a request may be one bad assumption away from taking the process
//! down.

use serde_json::{Map, Value};

/// Adds a field.
pub(crate) fn put(fields: &mut Map<String, Value>, name: &str, value: Value) {
    fields.insert(name.to_owned(), value);
}

/// JSON text as an object, or an empty one.
///
/// A tool that takes no arguments is called with no argument text at all, so
/// the empty case is ordinary rather than a failure. Text that is not an object
/// cannot be repaired here either — the tool that owns the arguments is the
/// only thing that knows what they mean, and it will say so in its own words
/// when it is asked to run.
pub(crate) fn object(json: &str) -> Map<String, Value> {
    match serde_json::from_str(json) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    }
}

/// A tool's argument schema, and the sentence that describes the tool.
///
/// A JSON Schema describes its subject at the root, and that sentence is what
/// every API wants as the tool's own description. Moved rather than copied:
/// leaving it in would describe the argument object instead of the tool, and
/// two providers reading the same schema must not disagree about which.
pub(crate) fn described(schema: &str) -> (Map<String, Value>, String) {
    let mut arguments = object(schema);

    let description = match arguments.remove("description") {
        Some(Value::String(text)) => text,
        _ => String::new(),
    };

    (arguments, description)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_schema_gives_up_its_description_rather_than_sharing_it() {
        let (arguments, description) = described(
            r#"{"description":"Reads a file.","type":"object","properties":{"path":{}}}"#,
        );

        assert_eq!(description, "Reads a file.");
        assert_eq!(arguments.get("type"), Some(&json!("object")));
        assert!(
            arguments.get("description").is_none(),
            "the description belongs to the tool, not to its arguments"
        );
    }

    #[test]
    fn a_schema_without_a_description_still_yields_its_arguments() {
        let (arguments, description) = described(r#"{"type":"object"}"#);

        assert!(description.is_empty());
        assert_eq!(arguments.get("type"), Some(&json!("object")));
    }

    #[test]
    fn text_that_is_not_an_object_is_an_empty_one() {
        // Reached by a tool called with no arguments at all, which is ordinary.
        assert!(object("").is_empty());
        assert!(object("[1,2]").is_empty());
        assert!(object("not json").is_empty());
    }
}
