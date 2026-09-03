//! Checks on the declaration itself, rather than on any document.
//!
//! The examples are the part of the shape that is not checked by anything else.
//! A key that stops existing breaks the parser and the schema together; an
//! example that stops being writable breaks nothing, and goes on being served
//! to every editor that resolves the schema. So each one is put through the
//! same read a real document gets, from the same entry point.

use crucible_core::Effort;
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
        Shape::Named { declared, others } => {
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
        | Shape::Flag
        | Shape::Whole(_)
        | Shape::List(_)
        | Shape::Opaque => {}
    }
}

/// One example, as the smallest document that would hold it.
fn written(path: &[&str], shape: &Shape, example: &str) -> String {
    let mut value = match shape {
        // Examples are elements, so one goes in a list of its own.
        Shape::List(_) => json!([example]),
        // True and false go into a document as themselves, never as the two
        // words that spell them.
        Shape::Flag => json!(
            example
                .parse::<bool>()
                .expect("a flag is written down as true or false")
        ),
        Shape::Text
        | Shape::Choice(_)
        | Shape::Count
        | Shape::Whole(_)
        | Shape::Fields(_)
        | Shape::Named { .. }
        | Shape::Opaque => json!(example),
    };

    for key in path.iter().rev() {
        value = Value::Object(Map::from_iter([((*key).to_owned(), value)]));
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
        let kind = at
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{path:?} says what it takes"));

        let agrees = match kind {
            "string" => stated.is_string(),
            "boolean" => stated.is_boolean(),
            "integer" => stated.is_u64(),
            other => panic!("{path:?} takes {other}, which this test has not been taught"),
        };
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
        // The one field whose examples are absolute paths, which is a thing
        // spelled differently per platform. It offers a spelling each and only
        // this platform's can be read back here — the other is a valid example
        // for the machine it is meant for, not a defect.
        let spellings = path == ["permissions", "extraDirectories"];
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
        let text = written(&path, &field.shape, usual);
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
