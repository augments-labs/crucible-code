//! The one resolver: what each access chooses, and that leaving access off
//! chooses exactly what crucible chose before the setting existed.

use crucible_auth::{Held, Kind};
use crucible_config::{Driver, Settings};

use super::*;
use crate::providers::Served;
use crate::sample::Sample;

fn serving(named: &str) -> Served {
    crate::startup::served(
        &crate::providers::providers()
            .expect("the built-in providers register")
            .snapshot(),
        named,
    )
    .expect("a provider this build has")
}

fn held(kind: Kind, name: &str) -> Held {
    Held {
        kind,
        name: name.to_owned(),
    }
}

/// `exported` is the one variable set, to a key.
fn lookup(exported: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
    move |asked| (Some(asked) == exported).then(|| "a-key".to_owned())
}

const SIGNED_IN: Signing = Signing {
    offered: true,
    usable: true,
};
const NO_SUBSCRIPTION: Signing = Signing {
    offered: false,
    usable: false,
};

// --- Left off: the order crucible has always resolved in -------------------

/// What one provider was answered with before `access` existed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Before {
    Subscription,
    Environment,
    Stored,
    SubscriptionAddress,
    Absent,
}

/// The order provider construction resolved in, copied from it as it stood
/// before the resolver, so the table below holds the resolver to it.
fn construction_before(
    addressed: bool,
    exported: bool,
    held: Option<Kind>,
    usable: bool,
) -> Before {
    let account = held == Some(Kind::Account) && usable;
    if !addressed && account {
        return Before::Subscription;
    }
    if exported {
        return Before::Environment;
    }
    if held == Some(Kind::Key) {
        return Before::Stored;
    }
    if account {
        return Before::SubscriptionAddress;
    }
    Before::Absent
}

/// The order `/login`, `/model` and `doctor` read a provider's source in,
/// copied the same way: an account counted wherever the build offered a
/// subscription for the provider.
fn mirror_before(
    addressed: bool,
    exported: bool,
    held: Option<Kind>,
    offered: bool,
) -> Option<Before> {
    if !addressed && offered && held == Some(Kind::Account) {
        return Some(Before::Subscription);
    }
    if exported {
        return Some(Before::Environment);
    }
    (held == Some(Kind::Key)).then_some(Before::Stored)
}

fn after(resolved: &Result<Route, AccessError>) -> Before {
    match resolved {
        Ok(Route::Subscription { .. }) => Before::Subscription,
        Ok(Route::ApiKey {
            source: KeySource::Environment { .. },
            ..
        }) => Before::Environment,
        Ok(Route::ApiKey {
            source: KeySource::Stored { .. },
            ..
        }) => Before::Stored,
        Err(AccessError::SubscriptionAddress { .. }) => Before::SubscriptionAddress,
        Err(AccessError::Absent(_)) => Before::Absent,
        other => panic!("left-off access resolved to {other:?}"),
    }
}

#[test]
fn leaving_access_off_resolves_every_combination_as_the_old_order_did() {
    let sample = Sample::new("access-left-off");
    let plain = sample.user("{}");
    let addressed =
        sample.user(r#"{"providers": {"openai": {"baseUrl": "https://gateway.example/v1"}}}"#);
    let one = serving("openai");

    let mut rows = 0;
    for (settings, is_addressed) in [(&plain, false), (&addressed, true)] {
        for exported in [false, true] {
            for kind in [None, Some(Kind::Key), Some(Kind::Account)] {
                for usable in [false, true] {
                    let signing = Signing {
                        offered: usable,
                        usable,
                    };
                    let from = lookup(exported.then_some("OPENAI_API_KEY"));
                    let name = if kind == Some(Kind::Key) {
                        "openai@platform"
                    } else {
                        "openai"
                    };
                    let resolved = resolved(
                        one,
                        settings,
                        &from,
                        kind.map(|kind| held(kind, name)).as_ref(),
                        signing,
                    );

                    let expected = construction_before(is_addressed, exported, kind, usable);
                    assert_eq!(
                        after(&resolved),
                        expected,
                        "addressed {is_addressed}, exported {exported}, held {kind:?}, usable {usable}"
                    );
                    let mirrored = mirror_before(is_addressed, exported, kind, usable);
                    let read = match after(&resolved) {
                        Before::SubscriptionAddress | Before::Absent => None,
                        found => Some(found),
                    };
                    assert_eq!(read, mirrored, "the mirror, for the same row");
                    rows += 1;
                }
            }
        }
    }
    assert_eq!(rows, 24, "every combination was asked");
}

#[test]
fn a_left_off_key_is_read_from_the_variable_the_configuration_names() {
    let sample = Sample::new("access-left-off-variable");
    let settings = sample.user(&format!(
        r#"{{"providers": {{"openai": {{{}}}}}}}"#,
        naming("WORK_KEY")
    ));
    let one = serving("openai");

    let route = resolved(
        one,
        &settings,
        &lookup(Some("WORK_KEY")),
        None,
        NO_SUBSCRIPTION,
    )
    .unwrap();

    assert_eq!(
        route,
        Route::ApiKey {
            provider: "openai",
            source: KeySource::Environment {
                variable: "WORK_KEY".into()
            }
        }
    );
    assert_eq!(variable(one, &settings), "WORK_KEY");
    assert_eq!(
        variables(one, &settings).collect::<Vec<_>>(),
        ["OPENAI_API_KEY", "WORK_KEY"]
    );
}

// --- subscription -----------------------------------------------------------

fn written(sample: &Sample, block: &str) -> Settings {
    sample.user(&format!(r#"{{"providers": {{"openai": {block}}}}}"#))
}

#[test]
fn a_subscription_access_is_used_over_an_exported_key() {
    let sample = Sample::new("access-subscription-over-key");
    let settings = written(&sample, r#"{"access": {"type": "subscription"}}"#);

    let route = resolved(
        serving("openai"),
        &settings,
        &lookup(Some("OPENAI_API_KEY")),
        Some(&held(Kind::Account, "openai")),
        SIGNED_IN,
    )
    .unwrap();

    assert_eq!(
        route,
        Route::Subscription {
            provider: "openai",
            account: "openai".into()
        }
    );
}

#[test]
fn a_subscription_access_with_a_configured_address_is_refused() {
    let sample = Sample::new("access-subscription-elsewhere");
    let settings = written(
        &sample,
        r#"{"access": {"type": "subscription"}, "baseUrl": "https://gateway.example/v1"}"#,
    );

    let refused = resolved(
        serving("openai"),
        &settings,
        &lookup(Some("OPENAI_API_KEY")),
        Some(&held(Kind::Account, "openai")),
        SIGNED_IN,
    )
    .unwrap_err();

    assert!(
        matches!(refused, AccessError::SubscriptionElsewhere { .. }),
        "{refused:?}"
    );
}

#[test]
fn a_subscription_access_for_a_provider_with_no_sign_in_is_refused() {
    let sample = Sample::new("access-subscription-unoffered");
    let settings =
        sample.user(r#"{"providers": {"google": {"access": {"type": "subscription"}}}}"#);

    let refused = resolved(
        serving("google"),
        &settings,
        &lookup(Some("GEMINI_API_KEY")),
        None,
        NO_SUBSCRIPTION,
    )
    .unwrap_err();

    assert!(
        matches!(refused, AccessError::NoSubscription { .. }),
        "{refused:?}"
    );
    assert!(!refused.to_string().contains("key"), "{refused}");
}

#[test]
fn a_subscription_access_with_an_account_that_cannot_sign_is_not_signed_in() {
    let sample = Sample::new("access-subscription-unusable");
    let settings = written(&sample, r#"{"access": {"type": "subscription"}}"#);

    let refused = resolved(
        serving("openai"),
        &settings,
        &lookup(None),
        Some(&held(Kind::Account, "openai")),
        Signing {
            offered: true,
            usable: false,
        },
    )
    .unwrap_err();

    assert!(
        matches!(refused, AccessError::NotSignedIn { .. }),
        "{refused:?}"
    );
}

// --- apiKey -----------------------------------------------------------------

#[test]
fn an_api_key_access_is_used_over_a_signed_in_account() {
    let sample = Sample::new("access-key-over-account");
    let settings = written(&sample, r#"{"access": {"type": "apiKey"}}"#);

    let route = resolved(
        serving("openai"),
        &settings,
        &lookup(Some("OPENAI_API_KEY")),
        Some(&held(Kind::Account, "openai")),
        SIGNED_IN,
    )
    .unwrap();

    assert!(
        matches!(
            route,
            Route::ApiKey {
                source: KeySource::Environment { .. },
                ..
            }
        ),
        "{route:?}"
    );
}

#[test]
fn an_api_key_access_reads_the_stored_key_where_no_variable_is_set() {
    let sample = Sample::new("access-key-stored");
    let settings = written(&sample, r#"{"access": {"type": "apiKey"}}"#);

    let route = resolved(
        serving("openai"),
        &settings,
        &lookup(None),
        Some(&held(Kind::Key, "openai")),
        SIGNED_IN,
    )
    .unwrap();

    assert_eq!(
        route,
        Route::ApiKey {
            provider: "openai",
            source: KeySource::Stored {
                name: "openai".into()
            }
        }
    );
}

#[test]
fn an_api_key_access_with_nothing_found_names_its_variable_and_not_the_account() {
    let sample = Sample::new("access-key-none");
    let settings = written(
        &sample,
        &format!(
            r#"{{"access": {{"type": "apiKey"}}, {}}}"#,
            naming("WORK_KEY")
        ),
    );

    let refused = resolved(
        serving("openai"),
        &settings,
        &lookup(None),
        Some(&held(Kind::Account, "openai")),
        SIGNED_IN,
    )
    .unwrap_err();

    let said = refused.to_string();
    assert!(matches!(refused, AccessError::NoKey { .. }), "{refused:?}");
    assert!(said.contains("WORK_KEY"), "{said}");
    assert!(said.contains("/login"), "{said}");
    assert!(
        !said.contains("subscription") && !said.contains("account"),
        "{said}"
    );
}

#[test]
fn an_account_for_a_provider_with_no_subscription_sign_in_is_never_its_route() {
    let route = resolved(
        serving("openai"),
        &Settings::default(),
        &lookup(None),
        Some(&held(Kind::Account, "openai")),
        Signing {
            offered: false,
            usable: true,
        },
    );

    assert!(matches!(route, Err(AccessError::Absent(_))), "{route:?}");
}

// --- externalAgent ----------------------------------------------------------

fn agent_settings(sample: &Sample, beside: &str, enabled: bool) -> Settings {
    sample.user(&format!(
        r#"{{"providers": {{"anthropic": {{"access": {{"type": "externalAgent", "instance": "mine"}}{beside}}}}},
            "externalAgents": {{"mine": {{"enabled": {enabled}, "driver": "claude", "executable": "claude"}}}}}}"#
    ))
}

#[test]
fn an_agent_access_resolves_to_its_instance_as_written() {
    let sample = Sample::new("access-agent");
    for enabled in [true, false] {
        let settings = agent_settings(&sample, "", enabled);

        let route = resolved(
            serving("anthropic"),
            &settings,
            &lookup(Some("ANTHROPIC_API_KEY")),
            Some(&held(Kind::Key, "anthropic")),
            NO_SUBSCRIPTION,
        )
        .unwrap();

        assert_eq!(
            route,
            Route::External {
                provider: "anthropic",
                instance: "mine".into(),
                driver: Driver::Claude,
                executable: "claude".into(),
                profile: None,
                model: None,
                effort: None,
                enabled,
            }
        );
    }
}

#[test]
fn an_agent_access_beside_a_key_only_crucibles_requests_read_is_refused_naming_it() {
    let sample = Sample::new("access-agent-conflict");
    for (beside, key) in [
        (
            r#", "baseUrl": "https://gateway.example""#.to_owned(),
            "baseUrl",
        ),
        (format!(", {}", naming("WORK_KEY")), "apiKeyEnv"),
    ] {
        let settings = agent_settings(&sample, &beside, true);

        let refused = resolved(
            serving("anthropic"),
            &settings,
            &lookup(None),
            None,
            NO_SUBSCRIPTION,
        )
        .unwrap_err();

        let said = refused.to_string();
        assert!(
            matches!(refused, AccessError::AgentConflict { .. }),
            "{refused:?}"
        );
        assert!(
            said.contains(&format!("providers.anthropic.{key}")),
            "{said}"
        );
    }
}

/// An installed agent signs in to its own vendor and asks only it, so a route
/// to another provider through it is refused, naming the provider it serves.
#[test]
fn an_agent_access_through_another_vendor_s_agent_is_refused() {
    let sample = Sample::new("access-agent-foreign");
    let settings = sample.user(
        r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}},
  "providers": {"openai": {"access": {"type": "externalAgent", "instance": "work"}}}
}"#,
    );

    let refused = resolved(
        serving("openai"),
        &settings,
        &lookup(None),
        None,
        NO_SUBSCRIPTION,
    )
    .unwrap_err();

    let said = refused.to_string();
    assert!(
        matches!(refused, AccessError::ForeignAgent { .. }),
        "{refused:?}"
    );
    assert!(said.contains("providers.openai.access"), "{said}");
    assert!(said.contains("anthropic"), "{said}");
}

#[test]
fn an_agent_access_naming_no_instance_written_is_refused() {
    let sample = Sample::new("access-agent-unknown");
    let settings = sample.user(
        r#"{"providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "elsewhere"}}}}"#,
    );

    let refused = resolved(
        serving("anthropic"),
        &settings,
        &lookup(None),
        None,
        NO_SUBSCRIPTION,
    )
    .unwrap_err();

    let said = refused.to_string();
    assert!(
        matches!(refused, AccessError::UnknownInstance { .. }),
        "{refused:?}"
    );
    assert!(said.contains("elsewhere"), "{said}");
}

/// The model an agent's entry names is carried on its route, so the launch
/// that route makes is started with it.
#[test]
fn an_agent_access_carries_the_model_its_entry_names() {
    let sample = Sample::new("access-agent-model");
    let settings = sample.user(
        r#"{"providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "mine"}}},
            "externalAgents": {"mine": {"driver": "claude", "executable": "claude", "model": "opus"}}}"#,
    );

    let route = resolved(
        serving("anthropic"),
        &settings,
        &lookup(None),
        None,
        NO_SUBSCRIPTION,
    )
    .unwrap();

    assert!(
        matches!(&route, Route::External { model: Some(model), .. } if &**model == "opus"),
        "{route:?}"
    );
}
