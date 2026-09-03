//! Which decisions let an extension run, and which of them stopped applying.

use super::{ExtensionDecision, ExtensionUntrusted};
use crate::SourceKind;
use crate::extension::{
    ExtensionCapability, ExtensionContribution, ExtensionIdentity, ExtensionManifest,
    ExtensionProtocol, ExtensionRequests,
};

/// The digest of the manifest every test here decides about.
const INSTALLED: &str = "sha256:0f1e";

/// One plausible extension, installed and asking for nothing unusual.
fn manifest() -> ExtensionManifest {
    ExtensionManifest::read(
        ExtensionIdentity {
            id: "acme.reviewer".into(),
            version: "1.4.0".into(),
            entrypoint: "bin/reviewer".into(),
            digest: INSTALLED.into(),
            found: SourceKind::Extension,
        },
        ExtensionRequests {
            protocol: ExtensionProtocol::new(1, 0),
            minimum: "0.35.0".into(),
            capabilities: Box::new([ExtensionCapability::RegisterTools]),
            contributions: Box::new([ExtensionContribution::Tools]),
        },
    )
    .expect("a manifest that agrees")
}

#[test]
fn an_extension_nobody_decided_about_may_not_run() {
    let decided = ExtensionDecision {
        enabled: false,
        digest: Some(INSTALLED),
    };

    assert_eq!(
        manifest().trusted(decided),
        Err(ExtensionUntrusted::Undecided),
    );
}

#[test]
fn a_decision_that_names_no_manifest_permits_nothing() {
    let decided = ExtensionDecision {
        enabled: true,
        digest: None,
    };

    assert_eq!(
        manifest().trusted(decided),
        Err(ExtensionUntrusted::Unpinned)
    );
}

#[test]
fn a_manifest_that_changed_since_it_was_agreed_to_is_asked_about_again() {
    let decided = ExtensionDecision {
        enabled: true,
        digest: Some("sha256:beef"),
    };

    assert_eq!(
        manifest().trusted(decided),
        Err(ExtensionUntrusted::Changed {
            decided: "sha256:beef".into(),
        }),
    );
}

#[test]
fn a_decision_about_these_exact_bytes_permits_this_exact_extension() {
    let decided = ExtensionDecision {
        enabled: true,
        digest: Some(INSTALLED),
    };

    let trusted = manifest().trusted(decided).expect("agreed to");

    assert_eq!(trusted.id(), "acme.reviewer");
    assert_eq!(trusted.digest(), INSTALLED);
}

#[test]
fn an_extension_that_was_turned_off_is_told_that_and_not_told_about_its_digest() {
    // Off outranks a stale pin. Reporting the discrepancy would be crucible
    // describing a disagreement inside an agreement nobody reached.
    let decided = ExtensionDecision {
        enabled: false,
        digest: Some("sha256:beef"),
    };

    assert_eq!(
        manifest().trusted(decided),
        Err(ExtensionUntrusted::Undecided),
    );
}

#[test]
fn each_refusal_says_which_of_the_three_it_is() {
    let said = |why: ExtensionUntrusted| why.to_string();

    assert!(said(ExtensionUntrusted::Undecided).contains("nobody has said"));
    assert!(said(ExtensionUntrusted::Unpinned).contains("no digest says which"));
    assert!(
        said(ExtensionUntrusted::Changed {
            decided: "sha256:beef".into(),
        })
        .contains("sha256:beef")
    );
}
