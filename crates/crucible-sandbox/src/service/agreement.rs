//! What a confinement claim and a negotiation each require of a backend.
//!
//! Sibling to `tests`, and not behind the gate `tests` carries: this module's
//! fixtures are absolute on every target, and the two lists it compares are not
//! platform-split, so a Windows build is as able to disagree with itself as a
//! Unix one is.

use super::*;

use crucible_storage::{SandboxCleanup, SandboxFilesystemProvenance, SandboxResourceLimits};

use crate::inspect::inspection;
use crate::policy::SandboxFilesystemRule;
use crate::{
    SandboxBackendId, SandboxBackendProvenance, SandboxDomainPattern, SandboxDomainPolicy,
    SandboxFilesystemAccess, SandboxNetworkProvenance,
};

// The two lists that name the boundaries a confinement depends on, and the fact
// that nothing holds them together.
//
// `SandboxInspection::new` refuses a report claiming an enforcing kernel
// boundary its own fields cannot support, and `capability_requirements` says
// what a peer must offer before it is talked to at all. Each names six
// boundaries, neither reads the other, and the crate boundary between them
// means neither can. A seventh boundary added to one is therefore invisible
// from the other: negotiation admits a backend that cannot confine, and the
// disagreement surfaces at the record — after the peer was negotiated with, and
// as a message about the record rather than about the negotiation that never
// asked for it.
//
// Both are read here where they are written: the negotiation list as the
// function returns it, and the boundary list as what the constructor does with
// one claim withheld. Neither is restated here, because a third copy is the
// disagreement this is here to catch, and `SandboxFeature::ALL` is the
// vocabulary both are stated in. The record's rule is a conjunction — a plan
// that matches the claim, and every boundary enforced — so a set enforcing
// every feature is accepted exactly when nothing is missing, and withholding
// one feature at a time names every boundary the rule reads. That rule reads a
// whole network shape rather than a network feature, so it is read over shapes
// that make each dimension of a mediated shape live instead of over one
// mediated shape that leaves them inert.
//
// The sweep states the implication and not that its left side is non-empty, so
// a `confines_as_claimed` reduced to accepting every claim would pass it
// vacuously. Non-emptiness is not this test's to hold:
// `crucible-storage`'s own
// `a_report_claiming_confinement_is_refused_when_its_own_fields_cannot_support_it`
// refuses `SandboxCapabilities::none()` under `confined = true` and pins the
// membership, and the two would have to break together.
//
// The two copies of the boundary list this deliberately does not add to are
// `ESSENTIAL` in `crucible-storage`'s own tests and `FIXED` in `inspect`'s:
// each states the list apart from the code it checks, so a test reading the
// list it is checking would agree with a wrong one. This adds a comparison
// rather than a copy, and reads both sides where they are written.

/// The one readable root these fixtures grant.
///
/// A rule is refused unless its path is absolute, and absolute is a leading
/// slash on Unix and a drive on Windows, so one literal cannot serve both
/// targets. The crate's own directory is absolute on every one of them, and
/// nothing opens it: a policy keeps an identity for the path, not the path.
const BOUNDARY_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// One policy per network shape, because which network boundary a claim rests
/// on is read from the shape rather than from the list.
///
/// A mediated shape is a `Domains` inspection carrying two counts, a deny count
/// and a local-binding flag, and the record keeps every one of them so a claim
/// can be judged on them, so a fixture that left them at their inert values
/// would not reach a boundary a future `confines_as_claimed` demands for a
/// granted socket or a live listener: it would find nothing essential for
/// either and still pass. Each mediated shape therefore makes one dimension
/// live and holds the rest, so two shapes differ only in what their name states.
fn shapes() -> [(&'static str, SandboxNetworkPolicy); 5] {
    [
        ("a closed network", SandboxNetworkPolicy::Closed),
        ("a mediated network", mediated(false, false)),
        (
            "a mediated network that binds local listeners",
            mediated(true, false),
        ),
        (
            "a mediated network that reaches one host socket",
            mediated(false, true),
        ),
        (
            "a mediated network that binds local listeners and reaches one host socket",
            mediated(true, true),
        ),
    ]
}

/// One mediated shape, with the local dimensions at the values asked for.
///
/// The two host lists carry one grant and one deny on every mediated shape, so
/// each is a non-empty count rather than an inert zero wherever mediation is
/// live at all.
fn mediated(local_binding: bool, unix_socket: bool) -> SandboxNetworkPolicy {
    SandboxNetworkPolicy::Domains(
        SandboxDomainPolicy::new(
            [SandboxDomainPattern::new("private.example").expect("domain pattern")],
            [SandboxDomainPattern::new("blocked.example").expect("domain pattern")],
            local_binding,
            // Absolute on every target for the reason `BOUNDARY_ROOT` is, and
            // joined onto it rather than spelled out, because no one literal
            // is a socket path on both Unix and Windows. Nothing connects to
            // it: a policy keeps a count of authorized sockets, not the sockets.
            if unix_socket {
                vec![PathBuf::from(BOUNDARY_ROOT).join("boundary.sock")]
            } else {
                Vec::new()
            },
            SandboxNetworkProvenance::User,
        )
        .expect("domain policy"),
    )
}

/// A policy carrying nothing but the gate and the network shape.
///
/// The bounds a real policy sets — limits, a manifest, durable sessions,
/// snapshots — are the policy's own demands and not boundaries a confinement
/// rests on, so a policy without them is the one whose negotiation requirements
/// are the boundary list and nothing else.
fn boundary_policy(enabled: bool, network: SandboxNetworkPolicy) -> SandboxPolicy {
    SandboxPolicy::new(
        enabled,
        [SandboxFilesystemRule::new(
            BOUNDARY_ROOT,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("rule")],
        BOUNDARY_ROOT,
        network,
        SandboxResourceLimits::default(),
    )
    .expect("policy")
}

/// A backend claiming every feature but the one named, so two calls differing
/// only in `missing` differ only in the claim under test.
fn enforcing_except(missing: Option<SandboxFeature>) -> SandboxCapabilities {
    SandboxFeature::ALL
        .into_iter()
        .fold(SandboxCapabilities::none(), |claims, feature| {
            if Some(feature) == missing {
                claims
            } else {
                claims.with(feature, SandboxCapability::Enforced)
            }
        })
}

/// A confined report for `capabilities`, built as a live build builds it: the
/// policy reduced to the plan a record keeps, then the record that refuses a
/// claim its own fields cannot support.
fn confined_report(
    policy: &SandboxPolicy,
    capabilities: SandboxCapabilities,
) -> Result<SandboxInspection, SandboxError> {
    inspection(
        SandboxId::new(),
        SandboxBackendIdentity::new(
            SandboxBackendId::new("boundary").expect("backend id"),
            "1",
            SandboxBackendProvenance::System,
            Some([1; 32]),
        )
        .expect("backend identity"),
        capabilities,
        policy,
        &SandboxManifest::empty(),
        true,
        None::<Box<str>>,
        SandboxCleanup::Pending,
    )
}

#[test]
fn every_boundary_a_confined_report_rests_on_is_asked_for_before_a_peer_is_talked_to() {
    for (shape, network) in shapes() {
        let policy = boundary_policy(true, network);
        // The instrument, proved before it is read. If the report is refused
        // with every feature enforced then this fixture is what refuses it, and
        // every refusal below would be about the fixture rather than about the
        // boundary withheld.
        assert!(
            confined_report(&policy, enforcing_except(None)).is_ok(),
            "{shape} refuses a confined report that enforces every feature, so nothing below \
             would measure a boundary"
        );
        // What a peer must offer before it is talked to, read from the list that
        // asks rather than from a copy of it, and at the level the record's own
        // rule demands: enforced, which is what `is_enforced` means there.
        let asked: Vec<SandboxFeature> =
            capability_requirements(&policy, &SandboxManifest::empty())
                .into_iter()
                .filter(|(_, minimum)| minimum.is_enforced())
                .map(|(feature, _)| feature)
                .collect();

        for feature in SandboxFeature::ALL {
            // Accepted while this one claim is withheld means no confinement
            // rests on it; refused means one does, and negotiation has to ask.
            if confined_report(&policy, enforcing_except(Some(feature))).is_ok() {
                continue;
            }
            assert!(
                asked.contains(&feature),
                "{} is a boundary a confinement claim rests on: `SandboxInspection::new` in \
                 crucible-storage refuses a confined report that does not enforce it, and \
                 `capability_requirements` does not ask a peer for it. Negotiation would admit a \
                 backend that cannot confine, and the disagreement would arrive at the record \
                 rather than at the peer that was never asked for it",
                feature.as_str()
            );
        }
    }
}

#[test]
fn a_disabled_policy_can_report_no_confinement_so_it_asks_for_no_boundary() {
    for (shape, network) in shapes() {
        // The gate is the whole of why the two lists need not agree below it.
        // `capability_requirements` drops the fixed boundaries when
        // `policy.enabled()` is false, which is sound only while a disabled plan
        // cannot be reported as confined at all: it cannot, whatever a backend
        // claims, and the refusal is the plan's own rather than a boundary's
        // absence — a set enforcing every feature a claim could rest on is
        // refused too, by `build` and by the record's own constructor alike. So
        // no confinement claim exists on this side of the gate for a boundary to
        // rest on, which is what lets the sweep above be stated for an enabled
        // policy and still cover the whole rule.
        // The negotiation half of the same fact is
        // `disabled_confinement_requires_auditing_and_at_least_observed_usage`:
        // a disabled policy asks for auditing and usage and refuses on neither
        // boundary, because it never asks for one.
        let disabled = boundary_policy(true, network).with_enabled(false);
        assert!(
            confined_report(&disabled, enforcing_except(None)).is_err(),
            "{shape} reports confinement under a disabled policy with every feature enforced, so \
             the boundary list applies with the gate off and `capability_requirements` would have \
             to ask a peer for the boundaries there as well"
        );
    }
}
