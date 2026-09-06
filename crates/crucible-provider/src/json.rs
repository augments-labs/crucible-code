//! The small parts of building a request body that every protocol shares.
//!
//! Not a request builder. Providers differ in shape and that difference is the
//! point of having more than one of them; what they agree on is narrower —
//! where a tool's description comes from, and that nothing here may panic on a
//! payload it did not expect.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value};

/// A JSON document written directly into its final allocation.
pub(crate) struct Json(String);

impl Json {
    pub(crate) fn new() -> Self {
        Self(String::new())
    }

    pub(crate) fn finish(self) -> String {
        self.0
    }

    pub(crate) fn object(&mut self, fill: impl FnOnce(&mut Object<'_>)) {
        self.0.push('{');
        let mut object = Object {
            json: self,
            first: true,
        };
        fill(&mut object);
        object.json.0.push('}');
    }

    fn array(&mut self, fill: impl FnOnce(&mut Array<'_>)) {
        self.0.push('[');
        let mut array = Array {
            json: self,
            first: true,
        };
        fill(&mut array);
        array.json.0.push(']');
    }

    fn text(&mut self, text: &str) {
        self.0.push('"');
        self.text_content(text);
        self.0.push('"');
    }

    fn text_content(&mut self, text: &str) {
        for character in text.chars() {
            match character {
                '"' => self.0.push_str("\\\""),
                '\\' => self.0.push_str("\\\\"),
                '\u{08}' => self.0.push_str("\\b"),
                '\u{0c}' => self.0.push_str("\\f"),
                '\n' => self.0.push_str("\\n"),
                '\r' => self.0.push_str("\\r"),
                '\t' => self.0.push_str("\\t"),
                '\u{00}'..='\u{1f}' => {
                    let byte = character as usize;
                    self.0.push_str("\\u00");
                    self.0.push(hex(byte >> 4));
                    self.0.push(hex(byte & 0x0f));
                }
                other => self.0.push(other),
            }
        }
    }

    fn value(&mut self, value: &Value) {
        match value {
            Value::Null => self.0.push_str("null"),
            Value::Bool(value) => self.0.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => self.0.push_str(&value.to_string()),
            Value::String(value) => self.text(value),
            Value::Array(values) => self.array(|array| {
                for value in values {
                    array.item(|json| json.value(value));
                }
            }),
            Value::Object(values) => self.object(|object| {
                for (name, value) in values {
                    object.value(name, value);
                }
            }),
        }
    }
}

fn hex(nibble: usize) -> char {
    match nibble {
        0 => '0',
        1 => '1',
        2 => '2',
        3 => '3',
        4 => '4',
        5 => '5',
        6 => '6',
        7 => '7',
        8 => '8',
        9 => '9',
        10 => 'a',
        11 => 'b',
        12 => 'c',
        13 => 'd',
        14 => 'e',
        _ => 'f',
    }
}

/// Fields within an object currently being written.
pub(crate) struct Object<'a> {
    json: &'a mut Json,
    first: bool,
}

impl Object<'_> {
    fn member(&mut self, name: &str, value: impl FnOnce(&mut Json)) {
        if !self.first {
            self.json.0.push(',');
        }
        self.first = false;
        self.json.text(name);
        self.json.0.push(':');
        value(self.json);
    }

    pub(crate) fn text(&mut self, name: &str, value: &str) {
        self.member(name, |json| json.text(value));
    }

    /// Escapes borrowed pieces directly into one string in the final body.
    pub(crate) fn text_with(&mut self, name: &str, fill: impl FnOnce(&mut dyn FnMut(&str))) {
        self.member(name, |json| {
            json.0.push('"');
            fill(&mut |part| json.text_content(part));
            json.0.push('"');
        });
    }

    pub(crate) fn prefixed_text(&mut self, name: &str, prefix: &str, value: &str) {
        self.member(name, |json| {
            json.0.push('"');
            json.text_content(prefix);
            json.text_content(value);
            json.0.push('"');
        });
    }

    /// One string of bytes, encoded straight into the document.
    ///
    /// Base64's alphabet is `A-Z a-z 0-9 + / =`, and JSON escapes none of them,
    /// so the encoder appends into the buffer being built rather than into a
    /// `String` this would then copy out of. On a request carrying megabytes
    /// that copy is the difference the ceiling above this is derived from.
    pub(crate) fn encoded(&mut self, name: &str, bytes: &[u8]) {
        self.member(name, |json| {
            json.0.push('"');
            STANDARD.encode_string(bytes, &mut json.0);
            json.0.push('"');
        });
    }

    /// The same, behind a prefix that is escaped the way any other string is.
    pub(crate) fn prefixed_encoded(&mut self, name: &str, prefix: &str, bytes: &[u8]) {
        self.member(name, |json| {
            json.0.push('"');
            json.text_content(prefix);
            STANDARD.encode_string(bytes, &mut json.0);
            json.0.push('"');
        });
    }

    pub(crate) fn number(&mut self, name: &str, value: u32) {
        self.member(name, |json| json.0.push_str(&value.to_string()));
    }

    pub(crate) fn boolean(&mut self, name: &str, value: bool) {
        self.member(name, |json| {
            json.0.push_str(if value { "true" } else { "false" });
        });
    }

    pub(crate) fn object(&mut self, name: &str, fill: impl FnOnce(&mut Object<'_>)) {
        self.member(name, |json| json.object(fill));
    }

    pub(crate) fn array(&mut self, name: &str, fill: impl FnOnce(&mut Array<'_>)) {
        self.member(name, |json| json.array(fill));
    }

    pub(crate) fn value(&mut self, name: &str, value: &Value) {
        self.member(name, |json| json.value(value));
    }
}

/// Values within an array currently being written.
pub(crate) struct Array<'a> {
    json: &'a mut Json,
    first: bool,
}

impl Array<'_> {
    fn item(&mut self, value: impl FnOnce(&mut Json)) {
        if !self.first {
            self.json.0.push(',');
        }
        self.first = false;
        value(self.json);
    }

    pub(crate) fn object(&mut self, fill: impl FnOnce(&mut Object<'_>)) {
        self.item(|json| json.object(fill));
    }

    /// One parsed native item without a second history-sized allocation.
    pub(crate) fn value(&mut self, value: &Value) {
        self.item(|json| json.value(value));
    }

    /// One string, escaped the way every other string here is.
    pub(crate) fn text(&mut self, value: &str) {
        self.item(|json| json.text(value));
    }
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

    #[test]
    fn direct_writing_escapes_text_and_preserves_nested_schema_values() {
        let nested = json!({"type": "object", "required": ["path"]});
        let mut document = Json::new();
        document.object(|root| {
            root.text("text", "quote \" slash \\ line\n tab\t nul\0 snowman ☃");
            root.value("schema", &nested);
        });

        let written: Value = serde_json::from_str(&document.finish()).unwrap();

        assert_eq!(
            written.get("text"),
            Some(&json!("quote \" slash \\ line\n tab\t nul\0 snowman ☃"))
        );
        assert_eq!(written.get("schema"), Some(&nested));
    }
}
