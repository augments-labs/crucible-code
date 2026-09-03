//! The JSON Schema, generated from the shape.
//!
//! Generated rather than written, because a hand-written schema beside a
//! hand-written parser is the same document declared twice, and the two drift
//! in the direction nobody notices: the schema keeps completing a key the
//! parser stopped accepting, or accepts a value the parser refuses. A gate
//! comparing them can only test the documents somebody thought to write down.
//! Deriving one from the other leaves nothing to disagree.
//!
//! The test at the foot of this file regenerates the schema and compares it
//! byte for byte with the copy checked in at `schema/crucible-code-schema.json`,
//! which is the file SchemaStore serves — so `cargo test` is what says whether
//! the two agree. The checked-in schema is therefore not maintained; it is
//! output, and a stale one fails rather than misleading an editor.

use serde_json::{Map, Value, json};

use super::{DOCUMENT, Field, Shape, Whole};

/// The dialect this schema is written in.
///
/// draft-07 rather than 2020-12 because SchemaStore asks for it: later drafts
/// are not recommended there until editor support catches up, and a schema
/// nobody's editor reads is a schema that does nothing. Nothing here is newer
/// than draft-07 anyway — `type`, `properties`, `propertyNames`, `enum` and
/// `additionalProperties` are all in it — so this costs a version string and
/// buys the registry that serves the file.
const DIALECT: &str = "http://json-schema.org/draft-07/schema#";

/// Where SchemaStore serves it, which is what a document's `$schema` points at.
///
/// `www` rather than `json`: both hostnames resolve, and the registry requires
/// the `$id` of a schema it hosts to be `https://www.schemastore.org/<file>`.
const ID: &str = "https://www.schemastore.org/crucible-code-schema.json";

/// The keys of the standard's own that a document may carry at any level.
///
/// `$schema` is what makes an editor complete the file at all, and `$comment`
/// is how a reader leaves a note in a format with no comments. Both are strings
/// wherever they appear, and neither means anything to crucible.
pub(crate) const RESERVED: &[&str] = &["$schema", "$comment"];

/// The schema for a crucible configuration document.
///
/// Pretty-printed with a trailing newline, so the checked-in copy is a file a
/// person can read in a diff rather than one line.
#[must_use]
pub fn schema() -> String {
    let mut root = object(&DOCUMENT);
    if let Some(fields) = root.as_object_mut() {
        fields.insert("$schema".into(), DIALECT.into());
        fields.insert("$id".into(), ID.into());
        fields.insert("title".into(), "crucible configuration".into());
        fields.insert(
            "description".into(),
            // Said in the schema itself, because an editor showing this file's
            // documentation is exactly where somebody needs to know it.
            "Settings for the crucible coding agent. Formats are unstable while \
             the version is 0.x: any key here may be renamed or removed in \
             any release, with no deprecation period."
                .into(),
        );
    }

    let mut text = serde_json::to_string_pretty(&root).unwrap_or_default();
    text.push('\n');
    text
}

/// One position in the document, as a schema.
fn of(shape: &Shape) -> Value {
    match shape {
        Shape::Text => json!({ "type": "string" }),
        Shape::Choice(allowed) => json!({ "type": "string", "enum": allowed }),

        // `minimum` rather than an upper bound as well: how large a count may
        // be is a fact about somebody's model, which this crate does not have.
        Shape::Count => json!({ "type": "integer", "minimum": 0 }),

        Shape::Flag => json!({ "type": "boolean" }),

        // `minimum` and `maximum` would say nothing here: they hold for a
        // number, and this is a string, which is what the environment has.
        // `pattern` is what an editor checks a string against, so the bounds
        // are spelled as one.
        Shape::Whole(bounds) => json!({ "type": "string", "pattern": pattern(bounds) }),
        Shape::Fields(_) | Shape::Named { .. } => object(shape),
        // An object and nothing more. No `properties`, because the names are
        // the extension's; and deliberately no `additionalProperties: false`,
        // which everywhere else in this file is what turns a misspelling into a
        // squiggle. Here it would turn every correctly spelled setting into
        // one, in every editor that resolves this schema.
        Shape::Opaque => json!({ "type": "object" }),

        // `uniqueItems` because no list here means anything by a repeat: the
        // kind decides which rule wins, so a second copy of a rule cannot
        // change an outcome, and a directory named twice is reached once. The
        // editor marking it is how a paste that went in twice is noticed.
        Shape::List(inner) => json!({
            "type": "array",
            "items": of(inner),
            "uniqueItems": true,
        }),
    }
}

/// Every number between two bounds, as a pattern an editor can check.
///
/// One alternative per number rather than a decomposition into digit ranges.
/// The published bounds are then the accepted ones by construction, with no
/// algorithm in between that a test would have to stand behind — and the
/// pattern is read by editors rather than by people, so its length is paid in
/// bytes and not in comprehension.
///
/// A leading `+` and any number of leading zeros are allowed because the reader
/// that turns one of these into a value takes them: `+6` and `06` are spellings
/// of a number somebody meant, and a schema that squiggled what the program
/// accepts is the disagreement this whole file exists to prevent.
fn pattern(bounds: &Whole) -> String {
    let mut written = String::from(r"^\+?0*(?:");
    for number in bounds.least..=bounds.most {
        if number > bounds.least {
            written.push('|');
        }
        written.push_str(&number.to_string());
    }
    written.push_str(r")$");
    written
}

/// One field of an object, as a schema: its shape, its sentence, its examples.
///
/// Examples are *elements* wherever the field holds a list, so they hang off
/// `items` rather than off the array. An editor offering `["read(src/**)"]`
/// where one entry goes would be offering a list inside a list.
fn described(field: &Field) -> Value {
    // Destructured rather than read member by member, so a new member of
    // `Field` stops the build here instead of being quietly left out. That is
    // what caught `examples`, which would otherwise have been documentation no
    // editor ever showed.
    let Field {
        name: _,
        about,
        shape,
        examples,
        usual,
        // Which layers a key may be written in is not something one schema can
        // say: this file is served to all three, and a key refused in one of
        // them is a key everywhere else. The sentence above says it in words
        // instead, which is what an editor shows.
        widens: _,
    } = field;

    let mut described = of(shape);
    if let Some(into) = described.as_object_mut() {
        into.insert("description".into(), (*about).into());
        if let Some(usual) = usual {
            into.insert("default".into(), stated(shape, usual));
        }
    }
    if examples.is_empty() {
        return described;
    }

    let holder = match shape {
        Shape::List(_) => described.get_mut("items"),
        Shape::Text
        | Shape::Choice(_)
        | Shape::Count
        | Shape::Flag
        | Shape::Whole(_)
        | Shape::Fields(_)
        | Shape::Named { .. }
        | Shape::Opaque => Some(&mut described),
    };
    if let Some(into) = holder.and_then(Value::as_object_mut) {
        into.insert("examples".into(), json!(examples));
    }
    described
}

/// A stated default, as the value an editor will write into the file.
///
/// `usual` is spelled the way it would be written in a document, because what
/// goes in the file is what a reader meets. For every key whose value is a
/// string that is the same text either way; for a [`Shape::Flag`] the document
/// holds `true`, and a schema publishing `"true"` beside `"type": "boolean"`
/// would have every editor that resolves it insert the one value the key
/// refuses.
///
/// A spelling that is not the shape's own is left as the string it was written
/// as, so that it fails the agreement test beside this one rather than being
/// quietly turned into whichever value `false` happens to be.
fn stated(shape: &Shape, usual: &str) -> Value {
    match shape {
        Shape::Flag => usual
            .parse::<bool>()
            .map_or_else(|_| Value::from(usual), Value::from),
        Shape::Text
        | Shape::Choice(_)
        | Shape::Count
        | Shape::Whole(_)
        | Shape::Fields(_)
        | Shape::Named { .. }
        | Shape::List(_)
        | Shape::Opaque => Value::from(usual),
    }
}

/// An object, whichever way its keys are decided.
fn object(shape: &Shape) -> Value {
    let mut properties = Map::new();
    for key in RESERVED {
        properties.insert((*key).into(), json!({ "type": "string" }));
    }

    match shape {
        // Keys crucible chose: each is named, and nothing else is allowed. That
        // `false` is what turns a misspelling into a red squiggle in the editor
        // instead of a setting that silently never applies.
        Shape::Fields(fields) => {
            for field in *fields {
                properties.insert(field.name.into(), described(field));
            }
            json!({
                "type": "object",
                "properties": properties,
                "additionalProperties": false,
            })
        }

        // Keys the user chose: a provider name, a variable name. Every name
        // not spoken for above answers to `additionalProperties`, so the ones
        // that are — `$comment`, and the names crucible chose here — keep their
        // own shape instead of having to satisfy the inner one.
        //
        // Which leaves the `$` prefix to guard, since it belongs to the
        // standard and an unrecognised one is a misspelling rather than a
        // variable. `propertyNames` is what says so. A pattern would say it
        // too, and would then also match the names declared just above — legal,
        // and refused by a validator in strict mode, which is what the registry
        // compiles this file with.
        Shape::Named { declared, others } => {
            for field in *declared {
                properties.insert(field.name.into(), described(field));
            }
            json!({
                "type": "object",
                "properties": properties,
                "propertyNames": { "anyOf": [{ "pattern": "^[^$]" }, { "enum": RESERVED }] },
                "additionalProperties": of(others),
            })
        }

        Shape::Text
        | Shape::Choice(_)
        | Shape::Count
        | Shape::Flag
        | Shape::Whole(_)
        | Shape::List(_)
        | Shape::Opaque => of(shape),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated schema, parsed back.
    fn generated() -> Value {
        serde_json::from_str(&schema()).unwrap()
    }

    /// One key the test needs to be there.
    fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
        value.get(key).expect(key)
    }

    /// Walks to a property, by the route a document would take.
    fn property<'a>(schema: &'a Value, path: &[&str]) -> &'a Value {
        let mut here = schema;
        for key in path {
            here = at(at(here, "properties"), key);
        }
        here
    }

    #[test]
    fn every_key_the_shape_declares_is_a_property_with_its_sentence() {
        let schema = generated();

        // The sentence is not decoration: it is what an editor shows in the
        // completion popup, and it is written in `shape.rs` precisely so that
        // adding a key cannot produce one without it.
        for name in DOCUMENT.keys() {
            let found = property(&schema, &[name]);
            assert!(found.is_object(), "{name} is missing");
            assert!(
                at(found, "description")
                    .as_str()
                    .is_some_and(|about| !about.is_empty()),
                "{name} has no description"
            );
        }
    }

    #[test]
    fn a_key_crucible_does_not_have_is_refused_by_the_schema_too() {
        // The same answer as the parser's, from the other reader of the shape.
        // Without this the two disagree in the direction nobody notices: the
        // editor stays quiet and the refusal only arrives at startup.
        let schema = generated();

        assert_eq!(at(&schema, "additionalProperties"), &Value::Bool(false));
        assert_eq!(
            at(property(&schema, &["output"]), "additionalProperties"),
            &Value::Bool(false)
        );
    }

    #[test]
    fn a_choice_offers_exactly_the_answers_the_shape_accepts() {
        let schema = generated();
        let color = property(&schema, &["output", "color"]);

        assert_eq!(at(color, "type"), "string");
        assert_eq!(at(color, "enum"), &json!(crate::shape::COLOR));
    }

    #[test]
    fn a_block_the_user_keys_takes_any_name_and_checks_the_value() {
        let schema = generated();
        let providers = property(&schema, &["providers"]);

        // No property list, because crucible cannot know what a provider will
        // be called — but the value under whatever name is used still has to be
        // a provider.
        let inner = at(providers, "additionalProperties");
        assert_eq!(at(inner, "type"), "object");
        assert!(at(at(inner, "properties"), "apiKeyEnv").is_object());
    }

    #[test]
    fn a_name_crucible_declares_never_sits_beside_a_pattern_that_matches_it() {
        // Legal, and refused by every validator running in strict mode — which
        // includes the registry's own gate, so a schema carrying the overlap is
        // one no editor ever fetches. The blocks a user keys are also the
        // blocks crucible declares names in, so the pattern is what has to go:
        // an unknown name answers to `additionalProperties` instead.
        fn walk(node: &Value, route: &str) {
            let Some(fields) = node.as_object() else {
                return;
            };
            assert!(
                !fields.contains_key("patternProperties"),
                "{route} keys names by pattern"
            );
            for (key, value) in fields {
                walk(value, &format!("{route}.{key}"));
            }
        }

        walk(&generated(), "");
    }

    #[test]
    fn a_dollar_name_the_standard_did_not_reserve_is_refused_by_a_block_the_user_keys() {
        let schema = generated();

        // `additionalProperties` alone would take `$scehma` for an environment
        // variable and say nothing. The `$` prefix belongs to the standard, and
        // a block the user keys still lends it out only to the two names above.
        for name in ["providers", "env"] {
            assert_eq!(
                at(property(&schema, &[name]), "propertyNames"),
                &json!({ "anyOf": [{ "pattern": "^[^$]" }, { "enum": RESERVED }] }),
                "{name}"
            );
        }
    }

    #[test]
    fn the_keys_the_standard_reserves_are_allowed_at_every_level() {
        let schema = generated();

        for key in RESERVED {
            for (where_, level) in [
                ("root", &schema),
                ("output", property(&schema, &["output"])),
                ("providers", property(&schema, &["providers"])),
            ] {
                let held = at(at(level, "properties"), key);
                assert_eq!(at(held, "type"), "string", "{where_} {key}");
            }
        }
    }

    #[test]
    fn the_checked_in_schema_is_what_this_generates() {
        // The gate that makes the checked-in file output rather than a second
        // copy to maintain. It rewrites and then fails, so the fix is to run
        // the tests again and commit — but the failure is what CI sees, so a
        // stale schema cannot be shipped to the editors that fetch it.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schema/crucible-code-schema.json");

        let generated = schema();
        if std::fs::read_to_string(&path).is_ok_and(|checked_in| checked_in == generated) {
            return;
        }

        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(&path, generated).unwrap();
        panic!("schema/crucible-code-schema.json was stale and has been rewritten — commit it");
    }

    #[test]
    fn the_schema_says_which_dialect_it_is_and_where_it_is_served() {
        let schema = generated();

        assert_eq!(at(&schema, "$schema"), DIALECT);

        // The `$id` is what a document's own `$schema` line points at, so it
        // has to be the URL SchemaStore serves rather than a path in this
        // repository.
        assert_eq!(at(&schema, "$id"), ID);
    }
}
