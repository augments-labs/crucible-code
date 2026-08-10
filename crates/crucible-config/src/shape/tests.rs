//! Checks on the declaration itself, rather than on any document.
//!
//! The examples are the part of the shape that is not checked by anything else.
//! A key that stops existing breaks the parser and the schema together; an
//! example that stops being writable breaks nothing, and goes on being served
//! to every editor that resolves the schema. So each one is put through the
//! same read a real document gets, from the same entry point.

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
                if !field.examples.is_empty() {
                    found.push((path.clone(), field));
                }
                offering(&field.shape, path, found);
                path.pop();
            }
        }

        // A key the user chooses, so there is no name to walk through. Any one
        // will do to stand a document up, and what is below it is declared the
        // same way as everything else.
        Shape::Named(inner) => {
            path.push("whatever");
            offering(inner, path, found);
            path.pop();
        }

        // Nothing below to reach. Spelled out rather than closed with a
        // wildcard, so a shape that later does hold fields has to be decided
        // about here instead of dropping out of the walk in silence.
        Shape::Text | Shape::Choice(_) | Shape::List(_) => {}
    }
}

/// One example, as the smallest document that would hold it.
fn written(path: &[&str], shape: &Shape, example: &str) -> String {
    let mut value = match shape {
        // Examples are elements, so one goes in a list of its own.
        Shape::List(_) => json!([example]),
        Shape::Text | Shape::Choice(_) | Shape::Fields(_) | Shape::Named(_) => json!(example),
    };

    for key in path.iter().rev() {
        value = Value::Object(Map::from_iter([((*key).to_owned(), value)]));
    }
    value.to_string()
}

#[test]
fn every_example_the_schema_offers_is_one_crucible_accepts() {
    let mut found = Vec::new();
    offering(&DOCUMENT, &mut Vec::new(), &mut found);

    // A walk that reached nothing would pass for ever. The examples are the
    // subject here, so their absence is the failure rather than the baseline.
    assert!(!found.is_empty(), "no field offers an example");

    for (path, field) in found {
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
            let read = Document::parse(&text, "config.json", Origin::ProjectLocal);

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
