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
                if !field.examples.is_empty() {
                    found.push((path.clone(), field));
                }
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
                if !field.examples.is_empty() {
                    found.push((path.clone(), field));
                }
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
        Shape::Text | Shape::Choice(_) | Shape::Count | Shape::Whole(_) | Shape::List(_) => {}
    }
}

/// One example, as the smallest document that would hold it.
fn written(path: &[&str], shape: &Shape, example: &str) -> String {
    let mut value = match shape {
        // Examples are elements, so one goes in a list of its own.
        Shape::List(_) => json!([example]),
        Shape::Text
        | Shape::Choice(_)
        | Shape::Count
        | Shape::Whole(_)
        | Shape::Fields(_)
        | Shape::Named { .. } => json!(example),
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
