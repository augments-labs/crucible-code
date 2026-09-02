//! What a manifest refuses to be read from, and what it agrees to speak.

use super::{
    EXTENSION_ID_BYTES, EXTENSION_REQUESTS, EXTENSION_TEXT_BYTES, ExtensionCapability,
    ExtensionContribution, ExtensionError, ExtensionIdentity, ExtensionManifest, ExtensionProtocol,
    ExtensionRequests,
};
use crate::SourceKind;

/// The half a trust decision is made against, for one plausible extension.
fn identity() -> ExtensionIdentity {
    ExtensionIdentity {
        id: "acme.reviewer".into(),
        version: "1.4.0".into(),
        entrypoint: "bin/reviewer".into(),
        digest: "sha256:0f1e".into(),
        found: SourceKind::Extension,
    }
}

/// The half a capability grant is made against, asking for what it promises.
fn requests() -> ExtensionRequests {
    ExtensionRequests {
        protocol: ExtensionProtocol::new(1, 3),
        minimum: "0.35.0".into(),
        capabilities: Box::new([
            ExtensionCapability::RegisterTools,
            ExtensionCapability::ReadRunContext,
        ]),
        contributions: Box::new([ExtensionContribution::Tools]),
    }
}

#[test]
fn a_manifest_answers_with_the_two_halves_it_was_read_from() {
    let one = ExtensionManifest::read(identity(), requests()).expect("a manifest that agrees");

    assert_eq!(one.id(), "acme.reviewer");
    assert_eq!(one.identity().version.as_ref(), "1.4.0");
    assert_eq!(one.identity().entrypoint.as_ref(), "bin/reviewer");
    assert_eq!(one.identity().found, SourceKind::Extension);
    assert_eq!(one.requests().protocol, ExtensionProtocol::new(1, 3));
    assert!(one.asked_for(ExtensionCapability::RegisterTools));
    assert!(!one.asked_for(ExtensionCapability::AskTheOperator));
}

#[test]
fn a_contribution_promised_without_its_capability_is_refused() {
    // The two halves are read by different parties: trust decides the first,
    // and a capability grant decides the second. A manifest that promises a
    // command while asking for nothing that registers one would be trusted,
    // started, and only then found to be staging something nobody agreed to —
    // so it is refused before anything of it runs.
    let refused = ExtensionManifest::read(
        identity(),
        ExtensionRequests {
            contributions: Box::new([
                ExtensionContribution::Tools,
                ExtensionContribution::Commands,
            ]),
            ..requests()
        },
    )
    .expect_err("a promise with nothing asked for");

    assert_eq!(
        refused.to_string(),
        "acme.reviewer contributes commands without requesting registerCommands"
    );
}

#[test]
fn an_identifier_that_names_only_what_it_is_is_refused() {
    // A bare name collides across vendors, and the collision is settled by
    // whichever was registered first — a silent way for one author's extension
    // to answer for another's.
    for bare in ["reviewer", ".reviewer", "acme.", "."] {
        let refused = ExtensionManifest::read(
            ExtensionIdentity {
                id: bare.into(),
                ..identity()
            },
            requests(),
        )
        .expect_err("an identifier naming nobody");

        assert!(
            matches!(refused, ExtensionError::Unqualified { .. }),
            "{bare} was read as source-qualified"
        );
    }
}

#[test]
fn a_spelling_that_is_empty_or_over_its_boundary_is_refused() {
    // Every spelling here arrives from a file this build did not write, and is
    // retained for as long as the extension is known.
    let empty = ExtensionManifest::read(
        ExtensionIdentity {
            version: "".into(),
            ..identity()
        },
        requests(),
    )
    .expect_err("an empty version");
    assert_eq!(
        empty,
        ExtensionError::Empty {
            field: "extension version"
        }
    );

    let long = "e".repeat(EXTENSION_TEXT_BYTES + 1);
    let over = ExtensionManifest::read(
        ExtensionIdentity {
            entrypoint: long.clone().into(),
            ..identity()
        },
        requests(),
    )
    .expect_err("an entrypoint over its boundary");
    assert_eq!(
        over,
        ExtensionError::TooLong {
            field: "entrypoint",
            maximum: EXTENSION_TEXT_BYTES,
            actual: long.len(),
        }
    );

    let id = format!("acme.{}", "r".repeat(EXTENSION_ID_BYTES));
    let named = ExtensionManifest::read(
        ExtensionIdentity {
            id: id.clone().into(),
            ..identity()
        },
        requests(),
    )
    .expect_err("an identifier over its own boundary");
    assert_eq!(
        named,
        ExtensionError::TooLong {
            field: "extension id",
            maximum: EXTENSION_ID_BYTES,
            actual: id.len(),
        }
    );
}

#[test]
fn a_list_over_its_boundary_or_naming_one_thing_twice_is_refused() {
    let many = vec![ExtensionCapability::RegisterTools; EXTENSION_REQUESTS + 1];
    let over = ExtensionManifest::read(
        identity(),
        ExtensionRequests {
            capabilities: many.clone().into(),
            contributions: Box::new([]),
            ..requests()
        },
    )
    .expect_err("more capabilities than the boundary");
    assert_eq!(
        over,
        ExtensionError::TooMany {
            field: "capabilities",
            maximum: EXTENSION_REQUESTS,
            actual: many.len(),
        }
    );

    // A repeat is not harmless bookkeeping: a grant is answered per capability,
    // so the same one asked twice is one question the operator gets asked twice
    // and two answers that may disagree.
    let twice = ExtensionManifest::read(
        identity(),
        ExtensionRequests {
            capabilities: Box::new([
                ExtensionCapability::RegisterTools,
                ExtensionCapability::ReadRunContext,
                ExtensionCapability::RegisterTools,
            ]),
            ..requests()
        },
    )
    .expect_err("a capability named twice");
    assert_eq!(
        twice.to_string(),
        "acme.reviewer names registerTools twice in capabilities"
    );
}

#[test]
fn an_extension_that_asks_for_nothing_is_read_rather_than_refused() {
    // Asking for nothing is a legal manifest and a useful one: a discovery pass
    // that lists what is installed does not need any of it to run. Empty is
    // that fact, and it is not the same as a manifest that could not be read.
    let quiet = ExtensionManifest::read(
        identity(),
        ExtensionRequests {
            capabilities: Box::new([]),
            contributions: Box::new([]),
            ..requests()
        },
    )
    .expect("a manifest that asks for nothing");

    assert!(quiet.requests().capabilities.is_empty());
    assert!(!quiet.asked_for(ExtensionCapability::RegisterTools));
}

#[test]
fn two_versions_agree_on_the_smaller_vocabulary_they_both_know() {
    let older = ExtensionProtocol::new(1, 2);
    let newer = ExtensionProtocol::new(1, 7);

    assert_eq!(older.agreed(newer), Some(older));
    assert_eq!(newer.agreed(older), Some(older));
    assert_eq!(older.agreed(older), Some(older));
}

#[test]
fn a_different_major_agrees_on_nothing_rather_than_on_its_lower_half() {
    // Two programs disagreeing about the shape of a frame have no smaller
    // vocabulary in common to fall back to. Answering with the lower minor
    // would start a plugin that cannot be spoken to and only find out on the
    // first frame.
    let host = ExtensionProtocol::new(2, 0);

    assert_eq!(ExtensionProtocol::new(1, 9).agreed(host), None);
    assert_eq!(ExtensionProtocol::new(3, 0).agreed(host), None);
    assert_eq!(host.major(), 2);
    assert_eq!(host.minor(), 0);
}

#[test]
fn a_manifest_retains_its_spellings_and_what_it_asked_for() {
    // What a registry adds up to decide whether one more extension fits.
    let one = ExtensionManifest::read(identity(), requests()).expect("a manifest that agrees");

    assert_eq!(
        one.retained_bytes(),
        "acme.reviewer".len()
            + "1.4.0".len()
            + "bin/reviewer".len()
            + "sha256:0f1e".len()
            + "0.35.0".len()
            + 2 * size_of::<ExtensionCapability>()
            + size_of::<ExtensionContribution>()
    );
}
