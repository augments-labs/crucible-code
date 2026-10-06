//! Checks on the declaration itself, rather than on any document.
//!
//! The examples are the part of the shape that is not checked by anything else.
//! A key that stops existing breaks the parser and the schema together; an
//! example that stops being writable breaks nothing, and goes on being served
//! to every editor that resolves the schema. So each one is put through the
//! same read a real document gets, from the same entry point.

use crucible_models::Effort;
use serde_json::{Map, Value, json};

use crate::document::{Document, Origin};

use super::{DOCUMENT, Field, Shape};

/// Every field offering examples, by the route a document takes to reach it.
fn offering(
    shape: &'static Shape,
    path: &mut Vec<&'static str>,
    found: &mut Vec<(Vec<&'static str>, &'static Field)>,
) {
    match shape {
        Shape::Fields(fields) => {
            for field in *fields {
                path.push(field.name);
                found.push((path.clone(), field));
                offering(&field.shape, path, found);
                path.pop();
            }
        }

        // A key the user chooses, so there is no name to walk through. Any one
        // will do to stand a document up, and what is below it is declared the
        // same way as everything else. The names crucible chose here are walked
        // under the names they have, because those are the ones a document
        // would write.
        Shape::Named {
            declared, others, ..
        } => {
            for field in *declared {
                path.push(field.name);
                found.push((path.clone(), field));
                offering(&field.shape, path, found);
                path.pop();
            }

            path.push("whatever");
            offering(others, path, found);
            path.pop();
        }

        // Nothing below to reach. Spelled out rather than closed with a
        // wildcard, so a shape that later does hold fields has to be decided
        // about here instead of dropping out of the walk in silence.
        // Nothing below an `Opaque` either, and for a reason that will not
        // change: what is under it belongs to an extension, so there is no
        // field here for this walk to have an opinion about.
        Shape::Text
        | Shape::Choice(_)
        | Shape::Count
        | Shape::Limit(_)
        | Shape::TextSet { .. }
        | Shape::Flag
        | Shape::Whole(_)
        | Shape::Within(_)
        | Shape::Pattern(_)
        | Shape::List { .. }
        | Shape::Opaque => {}
    }
}

/// One value, spelled the way a document holds it.
fn spelled(shape: &Shape, example: &str) -> Value {
    match shape {
        // Examples are elements, so one goes in a list of its own.
        Shape::List { .. } | Shape::TextSet { .. } => json!([example]),
        // True and false go into a document as themselves, never as the two
        // words that spell them.
        Shape::Flag => json!(
            example
                .parse::<bool>()
                .expect("a flag is written down as true or false")
        ),
        // A count goes into a document as a number, and a `Whole` beside it as
        // the string that one deliberately is.
        Shape::Count | Shape::Limit(_) | Shape::Within(_) => json!(
            example
                .parse::<u64>()
                .expect("a count is written down as a whole number")
        ),
        Shape::Text
        | Shape::Choice(_)
        | Shape::Whole(_)
        | Shape::Pattern(_)
        | Shape::Fields(_)
        | Shape::Named { .. }
        | Shape::Opaque => json!(example),
    }
}

/// One example, as the smallest document that would hold it.
///
/// Smallest, not shortest: a block with a key it cannot do without is written
/// whole, because the walk refuses a document missing one and an example
/// pasted into half a block would fail here for the block rather than for
/// itself.
fn written(path: &[&str], shape: &Shape, example: &str) -> String {
    written_value(path, spelled(shape, example))
}

fn written_value(path: &[&str], mut value: Value) -> String {
    let mut holding = Vec::new();
    let mut at = &DOCUMENT;
    for key in path {
        holding.push(at);
        at = at.field(key).expect("every key on the path is a key");
    }

    for (key, holder) in path.iter().zip(holding).rev() {
        let mut object = Map::from_iter([((*key).to_owned(), value)]);
        for field in holder.needed() {
            if field.name == *key {
                continue;
            }
            let example = field
                .examples
                .first()
                .expect("a key its block cannot do without offers one to write");
            object.insert(field.name.to_owned(), spelled(&field.shape, example));
        }
        value = Value::Object(object);
    }
    value.to_string()
}

#[test]
fn every_default_the_schema_publishes_is_the_kind_of_value_its_key_takes() {
    // What a key falls back to is declared as the text a document would hold,
    // because what goes in the file is what a reader meets. The schema is
    // served to editors from a registry, so a default published as the wrong
    // kind of value is one every editor will insert into somebody's file and
    // the parser will then refuse. The two are one fact and this is where they
    // are held together.
    let published: Value =
        serde_json::from_str(&crate::shape::schema::schema()).expect("the schema is JSON");

    let mut found = Vec::new();
    offering(&DOCUMENT, &mut Vec::new(), &mut found);
    let stating: Vec<_> = found
        .into_iter()
        .filter(|(_, field)| field.usual.is_some())
        .collect();

    // A walk that stopped short would pass for ever, and the place it stops
    // invisibly is a block the user keys: there is no name in the declaration
    // to notice missing from the list.
    assert!(
        stating.iter().any(|(path, _)| path.contains(&"whatever")),
        "the walk did not reach a key the user chooses"
    );

    for (path, _) in stating {
        let mut at = &published;
        for name in &path {
            let next = at
                .get("properties")
                .and_then(|properties| properties.get(name))
                // A key the user chooses is described once, for all of them.
                .or_else(|| at.get("additionalProperties"));
            at = next.unwrap_or_else(|| panic!("{path:?} is described by the schema"));
        }

        let stated = at
            .get("default")
            .unwrap_or_else(|| panic!("{path:?} publishes what it falls back to"));
        // A key that takes more than one kind says each in `anyOf`, and its
        // default has to be one of them.
        let kinds: Vec<&str> = match at.get("type").and_then(Value::as_str) {
            Some(kind) => vec![kind],
            None => at
                .get("anyOf")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|branch| branch.get("type").and_then(Value::as_str))
                .collect(),
        };
        assert!(!kinds.is_empty(), "{path:?} says what it takes");
        let kind = kinds.join(" or ");

        let agrees = kinds.iter().any(|kind| match *kind {
            "string" => stated.is_string(),
            "boolean" => stated.is_boolean(),
            "array" => stated.is_array(),
            "integer" => stated.is_u64(),
            other => panic!("{path:?} takes {other}, which this test has not been taught"),
        });
        assert!(agrees, "{path:?} takes {kind} and falls back to {stated}");
    }
}

#[test]
fn every_example_the_schema_offers_is_one_crucible_accepts() {
    let mut found = Vec::new();
    offering(&DOCUMENT, &mut Vec::new(), &mut found);
    let offered: Vec<_> = found
        .into_iter()
        .filter(|(_, field)| !field.examples.is_empty())
        .collect();

    // A walk that reached nothing would pass for ever. The examples are the
    // subject here, so their absence is the failure rather than the baseline.
    assert!(!offered.is_empty(), "no field offers an example");

    for (path, field) in offered {
        // The fields whose examples are absolute paths, which is a thing
        // spelled differently per platform. Each offers a spelling per platform
        // and only this platform's can be read back here — the others are valid
        // examples for the machines they are meant for, not defects.
        let spellings = path == ["permissions", "extraDirectories"]
            || path == ["mcp", "servers", "whatever", "command"];
        let mut accepted = 0;

        for example in field.examples {
            // Through `Document::parse`, not through a narrower reader: an
            // example has to survive the shape walk, the absolute-path check
            // and the rule read alike, and which of those applies is exactly
            // what the person pasting it does not have to know.
            let text = written(&path, &field.shape, example);
            let read = Document::parse(&text, "~/.crucible/config.json", Origin::User);

            if read.is_ok() {
                accepted += 1;
                continue;
            }

            assert!(
                spellings,
                "{}: {example} — {:?}",
                path.join("."),
                read.err()
            );
        }

        // Without this a field could offer nothing but other platforms'
        // spellings and pass in silence, which is the failure this test exists
        // to catch, arrived at from the other side.
        assert!(
            accepted > 0,
            "{}: no example this platform accepts",
            path.join(".")
        );
    }
}

#[test]
fn every_default_the_schema_states_is_a_value_crucible_would_accept() {
    // The other half of what the tests beside each settings module do. Those
    // bind one declared default to the value that module falls back to; this
    // one puts every default through the walk a document goes through, which is
    // what catches a word spelled for the schema and for nothing else.
    let mut found = Vec::new();
    offering(&DOCUMENT, &mut Vec::new(), &mut found);
    let stated: Vec<_> = found
        .into_iter()
        .filter_map(|(path, field)| field.usual.map(|usual| (path, field, usual)))
        .collect();

    assert!(!stated.is_empty(), "no field states a default");

    for (path, field, usual) in stated {
        let text = if matches!(field.shape, Shape::TextSet { .. }) {
            written_value(&path, serde_json::from_str(usual).unwrap())
        } else {
            written(&path, &field.shape, usual)
        };
        let read = Document::parse(&text, "~/.crucible/config.json", Origin::User);

        assert!(
            read.is_ok(),
            "{}: {usual} — {:?}",
            path.join("."),
            read.err()
        );
    }
}

#[test]
fn every_effort_a_document_may_write_is_a_rung_the_program_holds() {
    // The one `Choice` in this file whose meaning belongs to another crate, so
    // the two lists it spans cannot be tested where the others are. Both
    // directions matter: a word here that no longer parses is a key the schema
    // completes and the program drops on the floor, and a rung added to the
    // ladder and not to this list is one no configuration file can reach.
    for name in super::EFFORT {
        let rung: Effort = name.parse().unwrap_or_else(|_| panic!("no rung: {name}"));
        assert_eq!(rung.as_str(), *name);
    }

    assert_eq!(
        super::EFFORT.len(),
        Effort::LADDER.len(),
        "a rung the ladder holds that no document may write"
    );
}

#[test]
fn no_example_hands_a_program_a_wildcard() {
    // `bash(git *)` reads as the obvious way to allow git, and covers
    // `git push`. An example is where somebody learns which one to write, so
    // an `allow` that ends in a wildcard may not be one — the rule would work
    // exactly as written, and the habit is what costs.
    let mut found = Vec::new();
    offering(&DOCUMENT, &mut Vec::new(), &mut found);

    for (path, field) in found {
        if path != ["permissions", "allow"] {
            continue;
        }
        for example in field.examples {
            assert!(
                !(example.starts_with("bash(") && example.trim_end_matches(')').ends_with('*')),
                "{example} would teach allowing a program every argument"
            );
        }
    }
}

/// Every key that takes a whole number, by the route a document takes to it.
///
/// Below a name the user chooses as well as beside one, unlike [`offering`],
/// because a context window is keyed by the model it describes and is a whole
/// number a document writes all the same.
fn wholes(
    shape: &'static Shape,
    path: &mut Vec<&'static str>,
    found: &mut Vec<(Vec<&'static str>, &'static Shape)>,
) {
    match shape {
        Shape::Fields(fields) => {
            for field in *fields {
                path.push(field.name);
                wholes(&field.shape, path, found);
                path.pop();
            }
        }
        Shape::Named {
            declared, others, ..
        } => {
            for field in *declared {
                path.push(field.name);
                wholes(&field.shape, path, found);
                path.pop();
            }
            path.push("whatever");
            wholes(others, path, found);
            path.pop();
        }
        Shape::Count | Shape::Limit(_) | Shape::Within(_) | Shape::Whole(_) => {
            found.push((path.clone(), shape));
        }
        // No list holds a whole number, and a list element has no key a
        // document could write it under for this walk to name.
        Shape::Text
        | Shape::Choice(_)
        | Shape::TextSet { .. }
        | Shape::Flag
        | Shape::Pattern(_)
        | Shape::List { .. }
        | Shape::Opaque => {}
    }
}

/// The two smallest values a whole-number key takes.
///
/// Two, because the smallest alone is sometimes the default too, and a reader
/// that dropped what it was given would then read back the right answer.
fn smallest(shape: &Shape) -> [u64; 2] {
    let least = match shape {
        Shape::Limit(_) => 1,
        Shape::Within(bounds) | Shape::Whole(bounds) => u64::from(bounds.least),
        _ => 0,
    };
    [least, least + 1]
}

/// The smallest value past a key's bounds, where it has an upper one.
fn past(shape: &Shape) -> Option<u64> {
    match shape {
        Shape::Limit(most) => Some(most + 1),
        Shape::Within(bounds) | Shape::Whole(bounds) => Some(u64::from(bounds.most) + 1),
        _ => None,
    }
}

/// What the settings a key belongs to read back, as one comparable string.
///
/// Through the reader the program uses, so a key whose reader still took only
/// the integer spelling would read back the default for the other one.
fn read_back(path: &str, settings: &crate::settings::Settings) -> Option<String> {
    let said = match path {
        "providers.whatever.defaultContextWindow" => {
            format!("{:?}", settings.context_window("whatever", "unnamed"))
        }
        "providers.whatever.contextWindow.whatever" => {
            format!("{:?}", settings.context_window("whatever", "whatever"))
        }
        "env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED" => {
            format!("{:?}", settings.env().collect::<Vec<_>>())
        }
        "output.pinAfterSeconds" => format!("{:?}", settings.pin_after()),
        "sandbox.limits.commandSeconds"
        | "sandbox.limits.outputBytes"
        | "sandbox.limits.concurrentCommands" => format!("{:?}", settings.sandbox()),
        "promptCaching.requestedRetention.maxSeconds" => {
            format!("{:?}", settings.prompt_cache())
        }
        "compaction.reserve"
        | "compaction.keep"
        | "compaction.recap"
        | "compaction.askOnResume"
        | "compaction.spendCeiling" => format!("{:?}", settings.compaction()),
        "mcp.servers.whatever.handshakeSeconds"
        | "mcp.servers.whatever.requestSeconds"
        | "mcp.servers.whatever.shutdownSeconds"
        | "mcp.servers.whatever.restarts" => format!("{:?}", settings.mcp_servers()),
        _ => return None,
    };
    Some(said)
}

/// One whole-number key set to `value`, as the smallest document that holds
/// it, with the siblings the key cannot be written without.
fn holding(path: &[&str], value: Value) -> String {
    let text = written_value(path, value);
    if path.last() != Some(&"maxSeconds") {
        return text;
    }
    // A ceiling is refused beside the provider's own retention, which is the
    // class the walk fills in when the block names none.
    let mut document: Value = serde_json::from_str(&text).expect("a written document is JSON");
    let (_, block) = path.split_last().expect("a key has a name");
    let parent = format!("/{}", block.join("/"));
    document
        .pointer_mut(&parent)
        .and_then(Value::as_object_mut)
        .expect("the ceiling sits in a block")
        .insert("class".to_owned(), json!("ephemeral"));
    document.to_string()
}

fn load(text: &str) -> Result<crate::settings::Settings, crate::ConfigError> {
    let document = Document::parse(text, "~/.crucible/config.json", Origin::User)?;
    Ok(crate::settings::Settings::resolve(vec![document]))
}

#[test]
fn a_whole_number_written_with_a_zero_fraction_loads_as_that_number() {
    // The schema calls each of these an `integer`, which an editor holds `6.0`
    // to be. A file the editor passed and the program refused is the editor
    // and the program disagreeing about one document, so each key is loaded
    // both ways and read back through what the program reads it with.
    let mut found = Vec::new();
    wholes(&DOCUMENT, &mut Vec::new(), &mut found);
    let names: Vec<String> = found.iter().map(|(path, _)| path.join(".")).collect();
    assert_eq!(names.len(), 17, "the walk lost or gained a key: {names:?}");

    let mut refused = Vec::new();

    for (path, shape) in found {
        let name = path.join(".");
        let [least, next] = smallest(shape);
        let read = |value: Value| {
            let settings = load(&holding(&path, value.clone()))
                .unwrap_or_else(|error| panic!("{name} = {value} is refused: {error}"));
            read_back(&name, &settings)
                .unwrap_or_else(|| panic!("{name} has no reader in this test"))
        };

        let integer = [read(json!(least)), read(json!(next))];
        assert_ne!(integer[0], integer[1], "{name} is not read back at all");

        for (whole, expected) in [least, next].into_iter().zip(&integer) {
            let fraction = format!("{whole}.0").parse::<f64>().expect("a decimal");
            match load(&holding(&path, json!(fraction))) {
                Ok(settings) => {
                    assert_eq!(
                        read_back(&name, &settings).as_ref(),
                        Some(expected),
                        "{name} = {whole}.0"
                    );
                }
                Err(error) => refused.push(format!("{name} = {whole}.0: {error}")),
            }
        }

        let half = format!("{least}.5").parse::<f64>().expect("a decimal");
        assert!(
            load(&holding(&path, json!(half))).is_err(),
            "{name} = {least}.5"
        );

        let huge = holding(&path, json!("HUGE")).replace("\"HUGE\"", "1e400");
        assert!(load(&huge).is_err(), "{name} = 1e400");

        if let Some(beyond) = past(shape) {
            let integer = load(&holding(&path, json!(beyond)))
                .err()
                .unwrap_or_else(|| panic!("{name} = {beyond} loads"));
            let fraction = format!("{beyond}.0").parse::<f64>().expect("a decimal");
            let fraction = load(&holding(&path, json!(fraction)))
                .err()
                .unwrap_or_else(|| panic!("{name} = {beyond}.0 loads"));
            if fraction.to_string() != integer.to_string() {
                refused.push(format!("{name} = {beyond}.0: {fraction}"));
            }
        }
    }
    assert!(refused.is_empty(), "refused:\n{}", refused.join("\n"));
}

#[test]
fn a_variable_written_as_a_decimal_string_is_still_refused() {
    // The string form is the environment's, and the environment holds digits:
    // `"6.0"` is what a shell would hand over, and nothing there reads it.
    let path = ["env", super::MOUSE_SCROLL_SPEED];
    assert!(load(&written_value(&path, json!("6"))).is_ok());
    assert!(load(&written_value(&path, json!("6.0"))).is_err());
}

#[test]
fn a_float_is_a_whole_number_only_where_it_holds_one_exactly() {
    let exact = 9_007_199_254_740_992_u64;
    let read = |text: &str| super::whole(&serde_json::from_str(text).expect("a number"));

    assert_eq!(read("6"), Some(6));
    assert_eq!(read("6.0"), Some(6));
    assert_eq!(read("6e0"), Some(6));
    assert_eq!(read("-0.0"), Some(0));
    assert_eq!(read("9007199254740992.0"), Some(exact));
    // Past 2^53 a float skips whole numbers, so what it holds may not be what
    // was written; the integer spelling is still read exactly.
    assert_eq!(read("9007199254740994.0"), None);
    assert_eq!(read("9007199254740993"), Some(exact + 1));

    for refused in ["6.5", "-1", "-1.0", "0.1", "\"6\"", "true", "null"] {
        assert_eq!(read(refused), None, "{refused}");
    }
}
