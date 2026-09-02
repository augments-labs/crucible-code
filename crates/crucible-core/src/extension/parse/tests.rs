//! What a manifest file refuses to be read from.

use crate::extension::{
    EXTENSION_MANIFEST_BYTES, ExtensionCapability, ExtensionContribution, ExtensionError,
    ExtensionManifest, ExtensionProtocol,
};
use crate::registry::SourceKind;

/// A manifest file with everything written, as its author would write it.
const WHOLE: &str = r#"{
  "id": "acme.reviewer",
  "version": "1.4.0",
  "protocol": "1.3",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "0.35.0",
  "capabilities": ["registerTools", "readRunContext"],
  "contributions": ["tools"]
}"#;

/// One manifest, read the way a discovery pass would read it.
fn read(text: &str) -> Result<ExtensionManifest, ExtensionError> {
    ExtensionManifest::parse(text, SourceKind::Extension)
}

#[test]
fn a_written_manifest_is_read_into_the_two_halves() {
    let one = read(WHOLE).expect("a manifest its author wrote correctly");

    assert_eq!(one.id(), "acme.reviewer");
    assert_eq!(one.identity().version.as_ref(), "1.4.0");
    assert_eq!(one.identity().entrypoint.as_ref(), "bin/reviewer");
    assert_eq!(one.identity().found, SourceKind::Extension);
    assert_eq!(one.requests().protocol, ExtensionProtocol::new(1, 3));
    assert_eq!(one.requests().minimum.as_ref(), "0.35.0");
    assert_eq!(
        one.requests().capabilities.as_ref(),
        [
            ExtensionCapability::RegisterTools,
            ExtensionCapability::ReadRunContext
        ]
    );
    assert_eq!(
        one.requests().contributions.as_ref(),
        [ExtensionContribution::Tools]
    );
}

#[test]
fn the_digest_is_taken_over_the_bytes_rather_than_read_out_of_them() {
    // A manifest that stated its own digest would be a file asserting it had
    // not changed since somebody trusted it. Two manifests that differ by one
    // byte are two manifests, and a trust decision filed under the first does
    // not answer for the second.
    let one = read(WHOLE).expect("a written manifest");
    let again = read(WHOLE).expect("the same bytes");
    let moved = read(&WHOLE.replace("1.4.0", "1.4.1")).expect("one byte different");

    assert_eq!(one.identity().digest, again.identity().digest);
    assert_ne!(one.identity().digest, moved.identity().digest);
    assert!(
        one.identity().digest.starts_with("sha256:"),
        "{}",
        one.identity().digest
    );
    assert_eq!(one.identity().digest.len(), "sha256:".len() + 64);
}

#[test]
fn a_key_crucible_does_not_have_is_refused_and_says_what_is_accepted() {
    // Accepted and skipped, `capabilties` is an extension that asks for
    // nothing, starts, and is refused its first registration with nothing
    // pointing at the typo.
    let refused =
        read(&WHOLE.replace("\"capabilities\"", "\"capabilties\"")).expect_err("a misspelled key");

    let said = refused.to_string();
    assert!(
        matches!(refused, ExtensionError::UnknownKey { .. }),
        "{said}"
    );
    assert!(said.contains("capabilties"), "{said}");
    assert!(said.contains("capabilities"), "{said}");
}

#[test]
fn a_spelling_this_build_does_not_know_is_refused_rather_than_dropped() {
    // An extension written against a later crucible is told which word this one
    // could not read. Dropped instead, it would be granted less than it asked
    // for and would fail somewhere else entirely.
    let refused = read(&WHOLE.replace("\"readRunContext\"", "\"readTheDisk\""))
        .expect_err("a capability from another build");

    assert_eq!(
        refused.to_string(),
        "capabilities names readTheDisk, which this crucible does not know"
    );
}

#[test]
fn a_required_key_that_is_missing_or_the_wrong_kind_is_refused() {
    let gone = read(&WHOLE.replace("\"version\": \"1.4.0\",", "")).expect_err("no version");
    assert_eq!(gone, ExtensionError::Missing { field: "version" });

    let kind = read(&WHOLE.replace("\"1.4.0\"", "140")).expect_err("a version that is a number");
    assert_eq!(
        kind,
        ExtensionError::WrongType {
            field: "version",
            wanted: "a string",
        }
    );

    let list =
        read(&WHOLE.replace("[\"tools\"]", "\"tools\"")).expect_err("contributions as a string");
    assert_eq!(
        list,
        ExtensionError::WrongType {
            field: "contributions",
            wanted: "a list of strings",
        }
    );
}

#[test]
fn a_protocol_version_that_is_not_two_numbers_is_refused() {
    // A patch level is refused rather than ignored: a wire protocol that needs
    // one has changed its shape, so the third number would be saying something
    // this build cannot act on.
    for written in ["1", "1.3.0", "one.three", "1.", "-1.3"] {
        let refused = read(&WHOLE.replace("\"1.3\"", &format!("\"{written}\"")))
            .expect_err("a version that is not two numbers");

        assert!(
            matches!(refused, ExtensionError::BadProtocol { .. }),
            "{written} was read as a protocol version"
        );
    }
}

#[test]
fn a_list_left_out_is_asked_for_nothing_rather_than_refused() {
    // A discovery pass that lists what is installed needs no capability, and a
    // manifest for one is a legal manifest.
    let quiet = read(
        r#"{"id":"acme.quiet","version":"0.1.0","protocol":"1.0",
            "entrypoint":"bin/quiet","minimumCrucible":"0.35.0"}"#,
    )
    .expect("a manifest that asks for nothing");

    assert!(quiet.requests().capabilities.is_empty());
    assert!(quiet.requests().contributions.is_empty());
}

#[test]
fn text_that_is_not_json_is_refused_where_the_parser_stopped() {
    let refused = read("{\"id\": \"acme.reviewer\",}").expect_err("a trailing comma");

    let said = refused.to_string();
    assert!(
        matches!(refused, ExtensionError::Malformed { .. }),
        "{said}"
    );
    assert!(said.contains("line 1"), "{said}");
    // Crucible states the position itself, so the parser's own copy of it is
    // cut off. Both, punctuated differently, is the reader's first clue that
    // nobody read the message.
    assert!(!said.contains("at line"), "{said}");

    let listed = read("[]").expect_err("a manifest that is a list");
    assert_eq!(
        listed,
        ExtensionError::WrongType {
            field: "the manifest",
            wanted: "an object",
        }
    );
}

#[test]
fn text_over_its_boundary_is_refused_before_it_is_parsed() {
    // A parser handed an arbitrarily large document has already done the work
    // by the time anything could refuse it, and a manifest is a file whoever
    // published the extension wrote.
    let padded = format!(
        r#"{{"id":"acme.large","version":"{}","protocol":"1.0",
            "entrypoint":"bin/large","minimumCrucible":"0.35.0"}}"#,
        "v".repeat(EXTENSION_MANIFEST_BYTES)
    );
    let refused = read(&padded).expect_err("a manifest over its boundary");

    assert_eq!(
        refused,
        ExtensionError::TooLong {
            field: "the manifest",
            maximum: EXTENSION_MANIFEST_BYTES,
            actual: padded.len(),
        }
    );
}

#[test]
fn every_listed_capability_and_contribution_survives_a_manifest() {
    // What this can prove: a manifest naming every spelling this build offers
    // is read into exactly those, in order, and an unknown one is refused
    // rather than answered with something else.
    //
    // What it cannot: whether `EVERY` is complete. `named` reads through
    // `as_str`, so a spelling and its lookup cannot disagree, and both sides of
    // this assertion come from the same list — a capability added to the enum
    // and left out of `EVERY` would shrink both. Rust cannot enumerate a
    // variant, and an assertion that could never go red is worse than none.
    let spelled = |listed: &[&str]| {
        listed
            .iter()
            .map(|one| format!("\"{one}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let capabilities: Vec<_> = ExtensionCapability::EVERY
        .iter()
        .map(|one| one.as_str())
        .collect();
    let contributions: Vec<_> = ExtensionContribution::EVERY
        .iter()
        .map(|one| one.as_str())
        .collect();

    let all = read(&format!(
        r#"{{"id":"acme.everything","version":"1.0.0","protocol":"1.0",
            "entrypoint":"bin/everything","minimumCrucible":"0.35.0",
            "capabilities":[{}],"contributions":[{}]}}"#,
        spelled(&capabilities),
        spelled(&contributions),
    ))
    .expect("a manifest naming everything this build has");

    assert_eq!(
        all.requests().capabilities.as_ref(),
        ExtensionCapability::EVERY
    );
    assert_eq!(
        all.requests().contributions.as_ref(),
        ExtensionContribution::EVERY
    );

    // A lookup that answered with its first entry rather than nothing would
    // grant an extension a capability it never wrote down.
    assert_eq!(ExtensionCapability::named("registerAnything"), None);
    assert_eq!(ExtensionContribution::named("everything"), None);
}
