//! A claim says no more than what it was read from.

use crate::{Capabilities, Capability, Claim, Confined, Confinement, Observation, Scope};

/// A model list kept from an earlier reading is no evidence the running
/// program can switch models, whatever it says; a live one is.
#[test]
fn a_cached_observation_is_never_supported() {
    let cached = Capabilities::unknown().observed(
        Capability::ModelSwitch,
        Observation::cached(true, Scope::InstalledVersion),
    );
    assert_eq!(cached.claim(Capability::ModelSwitch), Claim::Unknown);
    assert!(!cached.supports(Capability::ModelSwitch));

    let live = Capabilities::unknown().observed(
        Capability::ModelSwitch,
        Observation::live(true, Scope::Instance),
    );
    assert_eq!(
        live.claim(Capability::ModelSwitch),
        Claim::Supported(Scope::Instance)
    );
}

/// Nothing is claimed until something says so, confinement included.
#[test]
fn a_fresh_set_claims_nothing() {
    let claims = Capabilities::unknown();
    for capability in Capability::ALL {
        assert_eq!(claims.claim(capability), Claim::Unknown, "{capability:?}");
    }
    assert_eq!(claims.confinement(), None);

    let confined = claims.confined(Confined::new(Confinement::Observed, Scope::Instance));
    assert_eq!(
        confined.confinement().map(Confined::level),
        Some(Confinement::Observed)
    );
}

/// Each capability's claim is held apart from every other's.
#[test]
fn each_claim_is_its_own() {
    for capability in Capability::ALL {
        let claims =
            Capabilities::unknown().observed(capability, Observation::live(false, Scope::Session));
        for other in Capability::ALL {
            let expected = if other == capability {
                Claim::Unsupported(Scope::Session)
            } else {
                Claim::Unknown
            };
            assert_eq!(
                claims.claim(other),
                expected,
                "{capability:?} set, {other:?} read"
            );
        }
    }
}
