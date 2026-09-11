//! A tool's schema, generated from the shape its parser walks.
//!
//! Generated rather than written, because a hand-written schema beside a
//! hand-written parser is the same document declared twice, and the two drift
//! in the direction nobody notices: the schema keeps offering an argument the
//! parser stopped reading, or a bound the code stopped holding. Deriving the
//! schema from a declaration that names the same constants the parser reads
//! leaves nothing to disagree — a renamed field or a moved ceiling changes
//! both in the same edit, and cannot change one.
//!
//! `crucible-config` reached the same conclusion about the configuration
//! schema first; this module is that idea again, sized to what a tool needs.
//! What comes out is the string a `Tool` answers `schema()` with, and every
//! keyword in it is one the providers carry through: `type`, `description`,
//! `enum`, `minimum`, `maximum`, `items`, `minItems`, `maxItems`, `required`.
//!
//! One thing the schema deliberately understates: a bound written as
//! `maximum` is usually *clamped* by the tool rather than refused, and a byte
//! ceiling on a piece of text is prose rather than `maxLength`, because that
//! keyword counts characters and the tools count bytes. The schema is
//! therefore never stricter than the code in a way that refuses a call the
//! code would take — the safe direction for the two to differ in.

/// One tool's schema: the sentence the providers lift out as the tool's own
/// description, and the arguments below it.
pub(crate) struct Schema {
    /// What the tool does, and what it will not do — the root `description`.
    pub(crate) about: String,
    /// The arguments, in the order a reader should meet them.
    pub(crate) fields: Vec<Field>,
}

/// One argument: its name, its sentence, whether a call must send it, and the
/// shape of what it holds.
pub(crate) struct Field {
    /// The key, spelled by the same constant the parser reads it with.
    pub(crate) name: &'static str,
    /// The sentence the model reads before deciding what to send.
    pub(crate) about: String,
    /// Whether the name goes into `required`.
    pub(crate) needed: bool,
    /// What kind of value the argument holds.
    pub(crate) shape: Shape,
}

/// What kind of value one position holds.
pub(crate) enum Shape {
    /// A string.
    Text,
    /// A boolean.
    Flag,
    /// An integer between two bounds.
    Count(Whole),
    /// A string drawn from a closed list of words.
    Choice(&'static [&'static str]),
    /// A list, bounded or not, of whatever `of` describes.
    List {
        /// The shape of one element.
        of: Box<Shape>,
        /// The fewest elements a call may send, where there is a floor.
        fewest: Option<usize>,
        /// The most elements a call may send, where there is a ceiling.
        most: Option<usize>,
    },
    /// An object whose keys are all named here.
    Fields(Vec<Field>),
}

/// The bounds on an integer argument.
pub(crate) struct Whole {
    /// The smallest accepted value.
    pub(crate) least: usize,
    /// The largest, where the tool owns a ceiling. The tool usually clamps
    /// rather than refuses, and the field's sentence says which.
    pub(crate) most: Option<usize>,
}

impl Schema {
    /// The schema as the string a `Tool` hands the provider: the root object,
    /// pretty-printed the way the hand-written ones were, fields in
    /// declaration order.
    #[must_use]
    pub(crate) fn text(&self) -> String {
        let mut out = String::from("{\n");
        line(
            &mut out,
            1,
            &format!("\"description\": {},", quoted(&self.about)),
        );
        object(&mut out, &self.fields, 1);
        out.push_str("\n}");
        out
    }
}

/// Writes one indented line, newline included.
fn line(out: &mut String, depth: usize, text: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(text);
    out.push('\n');
}

/// Writes `"type": "object"`, the properties and the `required` list at
/// `depth`, starting indented and ending unterminated — the caller decides
/// what follows, because the root object ends there and a nested one goes on
/// to its own description.
fn object(out: &mut String, fields: &[Field], depth: usize) {
    line(out, depth, "\"type\": \"object\",");
    line(out, depth, "\"properties\": {");
    let mut ahead = fields.iter().peekable();
    while let Some(field) = ahead.next() {
        line(out, depth + 1, &format!("{}: {{", quoted(field.name)));
        shaped(out, &field.shape, depth + 2);
        line(
            out,
            depth + 2,
            &format!("\"description\": {}", quoted(&field.about)),
        );
        let comma = if ahead.peek().is_some() { "}," } else { "}" };
        line(out, depth + 1, comma);
    }
    let needed: Vec<String> = fields
        .iter()
        .filter(|field| field.needed)
        .map(|field| quoted(field.name))
        .collect();
    if needed.is_empty() {
        indented(out, depth, "}");
    } else {
        line(out, depth, "},");
        indented(
            out,
            depth,
            &format!("\"required\": [{}]", needed.join(", ")),
        );
    }
}

/// Writes one indented line without the newline — how everything that might
/// be followed by a comma ends.
fn indented(out: &mut String, depth: usize, text: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(text);
}

/// Writes the keywords one shape amounts to, each on its own line ending in a
/// comma — the caller writes the description after them, so there is always a
/// next line.
fn shaped(out: &mut String, shape: &Shape, depth: usize) {
    match shape {
        Shape::Text => line(out, depth, "\"type\": \"string\","),
        Shape::Flag => line(out, depth, "\"type\": \"boolean\","),
        Shape::Count(bounds) => {
            line(out, depth, "\"type\": \"integer\",");
            line(out, depth, &format!("\"minimum\": {},", bounds.least));
            if let Some(most) = bounds.most {
                line(out, depth, &format!("\"maximum\": {most},"));
            }
        }
        Shape::Choice(words) => {
            let spelled: Vec<String> = words.iter().map(|word| quoted(word)).collect();
            line(out, depth, "\"type\": \"string\",");
            line(out, depth, &format!("\"enum\": [{}],", spelled.join(", ")));
        }
        Shape::List { of, fewest, most } => {
            line(out, depth, "\"type\": \"array\",");
            if let Some(fewest) = fewest {
                line(out, depth, &format!("\"minItems\": {fewest},"));
            }
            if let Some(most) = most {
                line(out, depth, &format!("\"maxItems\": {most},"));
            }
            line(out, depth, "\"items\": {");
            element(out, of, depth + 1);
            line(out, depth, "},");
        }
        Shape::Fields(fields) => {
            object(out, fields, depth);
            out.push_str(",\n");
        }
    }
}

/// Writes the whole of an element's schema. An `items` value stands alone, so
/// unlike `shaped` it closes its last keyword rather than leaving a comma for
/// a description that is not coming.
fn element(out: &mut String, shape: &Shape, depth: usize) {
    match shape {
        Shape::Fields(fields) => {
            object(out, fields, depth);
            out.push('\n');
        }
        Shape::Text => line(out, depth, "\"type\": \"string\""),
        Shape::Flag => line(out, depth, "\"type\": \"boolean\""),
        Shape::Count(bounds) => {
            line(out, depth, "\"type\": \"integer\",");
            if let Some(most) = bounds.most {
                line(out, depth, &format!("\"minimum\": {},", bounds.least));
                line(out, depth, &format!("\"maximum\": {most}"));
            } else {
                line(out, depth, &format!("\"minimum\": {}", bounds.least));
            }
        }
        Shape::Choice(words) => {
            let spelled: Vec<String> = words.iter().map(|word| quoted(word)).collect();
            line(out, depth, "\"type\": \"string\",");
            line(out, depth, &format!("\"enum\": [{}]", spelled.join(", ")));
        }
        Shape::List { of, fewest, most } => {
            line(out, depth, "\"type\": \"array\",");
            if let Some(fewest) = fewest {
                line(out, depth, &format!("\"minItems\": {fewest},"));
            }
            if let Some(most) = most {
                line(out, depth, &format!("\"maxItems\": {most},"));
            }
            line(out, depth, "\"items\": {");
            element(out, of, depth + 1);
            line(out, depth, "}");
        }
    }
}

/// A JSON string literal, quotes and all.
///
/// Written here rather than borrowed from a serializer so the path cannot
/// fail: every character either passes through or becomes an escape, and the
/// escapes are the ones the standard requires.
fn quoted(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for piece in text.chars() {
        match piece {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            control if control < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            plain => out.push(plain),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parsed(schema: &Schema) -> Value {
        serde_json::from_str(&schema.text()).expect("the rendered schema is valid JSON")
    }

    fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
        value.pointer(path).expect(path)
    }

    fn sample() -> Schema {
        Schema {
            about: "Does a \"thing\".\nCarefully.".into(),
            fields: vec![
                Field {
                    name: "path",
                    about: "Where.".into(),
                    needed: true,
                    shape: Shape::Text,
                },
                Field {
                    name: "limit",
                    about: "How many.".into(),
                    needed: false,
                    shape: Shape::Count(Whole {
                        least: 1,
                        most: Some(10),
                    }),
                },
                Field {
                    name: "mode",
                    about: "Which way.".into(),
                    needed: false,
                    shape: Shape::Choice(&["fast", "slow"]),
                },
                Field {
                    name: "steps",
                    about: "Each one.".into(),
                    needed: true,
                    shape: Shape::List {
                        of: Box::new(Shape::Fields(vec![
                            Field {
                                name: "say",
                                about: "The words.".into(),
                                needed: true,
                                shape: Shape::Text,
                            },
                            Field {
                                name: "loud",
                                about: "Or not.".into(),
                                needed: false,
                                shape: Shape::Flag,
                            },
                            Field {
                                name: "tags",
                                about: "Plain words.".into(),
                                needed: false,
                                shape: Shape::List {
                                    of: Box::new(Shape::Text),
                                    fewest: None,
                                    most: Some(3),
                                },
                            },
                        ])),
                        fewest: Some(1),
                        most: Some(4),
                    },
                },
            ],
        }
    }

    #[test]
    fn the_rendered_schema_is_valid_json_with_the_envelope_providers_lift() {
        let value = parsed(&sample());
        assert_eq!(at(&value, "/type"), "object");
        assert_eq!(at(&value, "/description"), "Does a \"thing\".\nCarefully.");
        assert_eq!(
            at(&value, "/required"),
            &serde_json::json!(["path", "steps"])
        );
    }

    #[test]
    fn every_field_lands_under_properties_with_its_sentence() {
        let value = parsed(&sample());
        let properties = at(&value, "/properties").as_object().expect("properties");
        assert_eq!(properties.len(), 4);
        for (name, field) in properties {
            assert!(
                field
                    .pointer("/description")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty()),
                "{name} has no description"
            );
        }
    }

    #[test]
    fn bounds_words_and_element_counts_come_out_as_keywords() {
        let value = parsed(&sample());
        assert_eq!(at(&value, "/properties/limit/minimum"), 1);
        assert_eq!(at(&value, "/properties/limit/maximum"), 10);
        assert_eq!(
            at(&value, "/properties/mode/enum"),
            &serde_json::json!(["fast", "slow"])
        );
        assert_eq!(at(&value, "/properties/steps/minItems"), 1);
        assert_eq!(at(&value, "/properties/steps/maxItems"), 4);
    }

    #[test]
    fn a_list_of_objects_declares_its_elements_fields_and_their_own_required() {
        let value = parsed(&sample());
        assert_eq!(at(&value, "/properties/steps/items/type"), "object");
        assert_eq!(
            at(&value, "/properties/steps/items/required"),
            &serde_json::json!(["say"])
        );
        assert_eq!(
            at(&value, "/properties/steps/items/properties/loud/type"),
            "boolean"
        );
        assert_eq!(
            at(&value, "/properties/steps/items/properties/tags/type"),
            "array"
        );
        assert_eq!(
            at(&value, "/properties/steps/items/properties/tags/maxItems"),
            3
        );
        assert_eq!(
            at(&value, "/properties/steps/items/properties/tags/items/type"),
            "string"
        );
    }

    #[test]
    fn an_unbounded_count_carries_no_maximum() {
        let schema = Schema {
            about: "Counts.".into(),
            fields: vec![Field {
                name: "offset",
                about: "From where.".into(),
                needed: false,
                shape: Shape::Count(Whole {
                    least: 1,
                    most: None,
                }),
            }],
        };
        let value = parsed(&schema);
        assert_eq!(at(&value, "/properties/offset/minimum"), 1);
        assert!(value.pointer("/properties/offset/maximum").is_none());
    }
}
