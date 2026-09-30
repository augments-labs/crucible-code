use std::collections::BTreeMap;

use crucible_auth::{KimiSite, Kind};
use crucible_http::{Hold, Origin};
use crucible_provider::{Google, Moonshot, MoonshotWeb, OpenAi};

use super::*;
use crate::providers::Rows;

fn origin(url: &str) -> Origin {
    Origin::of(url).unwrap()
}

#[test]
fn a_documented_address_is_recognised_however_it_is_spelled() {
    for (spelled, route) in [
        ("https://api.moonshot.ai/v1", "api.moonshot.ai"),
        ("https://API.Moonshot.AI/v1", "api.moonshot.ai"),
        ("https://api.moonshot.ai./v1", "api.moonshot.ai"),
        ("https://api.moonshot.ai:443/v1", "api.moonshot.ai"),
        ("https://api.moonshot.ai/v1/", "api.moonshot.ai"),
        (
            "https://api.moonshot.ai/v1/chat/completions",
            "api.moonshot.ai",
        ),
        ("https://api.moonshot.cn/v1", "api.moonshot.cn"),
        ("https://api.kimi.ai/coding/v1", "key:moonshot@kimi.ai"),
        (
            "https://api.kimi.com/coding/v1/chat/completions",
            "key:moonshot",
        ),
        (
            "https://generativelanguage.googleapis.com/v1beta/interactions",
            "key:google",
        ),
        ("https://api.openai.com/v1/responses", "key:openai"),
        ("https://api.anthropic.com/v1/messages", "key:anthropic"),
    ] {
        assert_eq!(recognised(spelled), Some(route), "{spelled}");
    }
}

#[test]
fn a_near_miss_is_not_recognised() {
    for missed in [
        // Another host, and one that only ends the same way.
        "https://gateway.example/v1",
        "https://moonshot.ai/v1",
        "https://evilapi.moonshot.ai/v1",
        "https://api.moonshot.ai.evil.example/v1",
        // Plain http for https, and another port.
        "http://api.moonshot.ai/v1",
        "https://api.moonshot.ai:8443/v1",
        // A path that only begins with the same letters, or sits above.
        "https://api.moonshot.ai/v10",
        "https://api.moonshot.ai/v1beta",
        "https://api.moonshot.ai",
        "https://api.kimi.com/coding",
        "https://api.kimi.com/v1",
        // The ChatGPT sign-in's own address: no key row stands there.
        "https://chatgpt.com/backend-api/codex/responses",
        "http://localhost:8080/v1",
        "not an address",
    ] {
        assert_eq!(recognised(missed), None, "{missed}");
    }
}

/// Every address a key row sends to lies under the address recognised for
/// it, so a `baseUrl` set to where a key row already goes answers for that
/// row; and every recognised address answers for a key row or for a Kimi open
/// platform route.
#[test]
fn every_key_row_is_recognised_at_the_address_it_sends_to() {
    let rows = Rows::production();
    let vendor = |provider: &str| match provider {
        "google" => Google::VENDOR.as_str().to_owned(),
        "openai" => OpenAi::VENDOR.as_str().to_owned(),
        "anthropic" => crucible_provider::Anthropic::VENDOR.as_str().to_owned(),
        _ => panic!("no default address for {provider}"),
    };
    for row in rows.listed(List::Key) {
        let address = row
            .address
            .as_ref()
            .map_or_else(|| vendor(row.provider), |at| at.as_str().to_owned());
        assert_eq!(
            recognised(&address).map(str::to_owned),
            Some(row_route(row)),
            "{address}"
        );
    }
    for one in RECOGNISED {
        let row = rows.all().iter().find(|row| row_route(row) == one.route);
        assert!(
            row.is_some_and(|row| row.kind == Kind::Key) || one.route.starts_with("api.moonshot."),
            "{one:?}"
        );
    }
}

/// The routes and the rows agree: each warned row route names a row, each
/// sends only to its route's origins, a Kimi row's sign-in, model and web tool
/// hosts are its site's, and route spellings are distinct.
#[test]
fn every_warned_route_names_its_row_and_every_origin_it_is_sent_to() {
    let rows = Rows::production();
    let routes = Routes::production();
    let mut spelled = BTreeSet::new();
    for warned in routes.all() {
        assert!(spelled.insert(warned.route), "{} twice", warned.route);
        if warned.route.starts_with("api.moonshot.") {
            assert_eq!(warned.origins, [format!("https://{}", warned.route)]);
            continue;
        }
        let row = rows
            .all()
            .iter()
            .find(|row| row_route(row) == warned.route)
            .unwrap_or_else(|| panic!("{} names no row", warned.route));
        assert_eq!(warned.shown, row.shown);
        let has = |url: &str| warned.origins.contains(&origin(url).to_string().as_str());
        if let Some(address) = &row.address {
            assert!(has(address.as_str()), "{}: {address:?}", warned.route);
        }
        match row.site {
            Some("kimi.ai") => {
                assert!(has(MoonshotWeb::SEARCH_AI.as_str()));
                assert!(has(MoonshotWeb::FETCH_AI.as_str()));
                if row.kind == Kind::Account {
                    assert!(has(KimiSite::Ai.host()));
                }
            }
            Some("kimi.com") => {
                assert!(has(MoonshotWeb::SEARCH.as_str()));
                assert!(has(MoonshotWeb::FETCH.as_str()));
                if row.kind == Kind::Account {
                    assert!(has(KimiSite::Com.host()));
                }
            }
            _ => {}
        }
    }
    assert!(
        routes
            .warned("subscription:openai")
            .is_some_and(|openai| openai.origins.contains(&crucible_auth::OpenAiOAuth::ISSUER))
    );
    assert!(routes.warned("key:google").is_some_and(|google| {
        google
            .origins
            .contains(&origin(Google::VENDOR.as_str()).to_string().as_str())
    }));
    assert_eq!(
        Moonshot::CODING_AI.as_str().split('/').nth(2),
        Some("api.kimi.ai")
    );
    for unwarned in ["key:anthropic", "key:openai"] {
        assert!(routes.warned(unwarned).is_none(), "{unwarned}");
    }
}

#[test]
fn a_condition_opens_its_sentence_and_every_warning_cites_its_page() {
    for warned in WARNED {
        let warning = warned.warning;
        if let Some(condition) = warning.condition {
            assert!(warning.sentence.starts_with(condition), "{}", warned.route);
        }
        assert!(warning.link.starts_with("https://"), "{}", warned.route);
        assert!(!warning.sentence.contains('\u{2014}'), "{}", warned.route);
        assert_eq!(
            warning.cited(),
            format!("{}, {}", warning.source, warning.read)
        );
    }
}

fn serving(route: &str) -> Serving {
    Serving {
        route: Some(route.to_owned()),
        at: None,
    }
}

#[test]
fn a_warned_origin_is_held_until_its_route_has_a_yes() {
    let consent = Consent::new(Routes::production());
    let google = origin("https://generativelanguage.googleapis.com/v1beta/interactions");

    assert_eq!(consent.held(&google).as_deref(), Some("key:google"));
    assert!(consent.asks("key:google").is_some());

    consent.record("key:google");
    assert_eq!(consent.held(&google), None);
    assert!(consent.asks("key:google").is_none());

    consent.forget(["key:google"]);
    assert_eq!(consent.held(&google).as_deref(), Some("key:google"));

    for unwarned in [
        "https://api.anthropic.com/v1/messages",
        "https://api.openai.com/v1/responses",
        "https://gateway.example/v1",
    ] {
        assert_eq!(consent.held(&origin(unwarned)), None, "{unwarned}");
    }
}

#[test]
fn a_given_yes_lets_a_sign_in_through_until_it_is_withdrawn() {
    let consent = Consent::new(Routes::production());
    let issuer = origin("https://auth.kimi.ai/api/oauth/device_authorization");

    assert!(consent.held(&issuer).is_some());
    consent.give("subscription:moonshot@kimi.ai");
    assert_eq!(consent.held(&issuer), None);
    consent.withdraw("subscription:moonshot@kimi.ai");
    assert!(consent.held(&issuer).is_some());
}

/// Two routes of one site share its model address. What is let through there
/// is decided by the route the provider is served on, so a yes to the key row
/// does not send a sign-in's requests, and the other way round.
#[test]
fn a_shared_origin_follows_the_route_its_provider_is_served_on() {
    let consent = Consent::new(Routes::production());
    let model = origin("https://api.kimi.com/coding/v1/chat/completions");
    consent.record("key:moonshot");

    consent.serving(BTreeMap::from([(
        "moonshot".to_owned(),
        serving("subscription:moonshot"),
    )]));
    assert_eq!(
        consent.held(&model).as_deref(),
        Some("subscription:moonshot")
    );

    consent.serving(BTreeMap::from([(
        "moonshot".to_owned(),
        serving("key:moonshot"),
    )]));
    assert_eq!(consent.held(&model), None);
}

/// A `baseUrl` crucible does not recognise is unwarned, wherever it points:
/// its requests are not held, even at a host a warned route also uses.
#[test]
fn an_unrecognised_base_url_is_not_held() {
    let consent = Consent::new(Routes::production());
    let base = origin("https://api.kimi.com/v2/chat/completions");
    consent.serving(BTreeMap::from([(
        "moonshot".to_owned(),
        Serving {
            route: None,
            at: Some(base.clone()),
        },
    )]));
    assert_eq!(consent.held(&base), None);
    assert!(
        consent
            .held(&origin("https://auth.kimi.com/api/oauth/token"))
            .is_some()
    );
}

#[test]
fn a_route_nobody_warns_about_is_never_asked_and_an_unknown_yes_means_nothing() {
    let consent = Consent::new(Routes::production());
    consent.recorded(["key:nobody".to_owned(), "model:nobody/x".to_owned()]);
    assert!(consent.asks("key:anthropic").is_none());
    assert!(consent.asks("key:nobody").is_none());
    assert!(consent.asks("key:google").is_some());
}
