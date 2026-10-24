use std::collections::BTreeMap;

use crucible_auth::{KimiSite, Kind};
use crucible_external_harnesses::catalog::Entry;
use crucible_http::{Hold, Origin};
use crucible_provider::{Google, Moonshot, MoonshotWeb, OpenAi};

use super::*;
use crate::sample::Sample;

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
        // A plan key row's own address answers for that row.
        (
            "https://coding.dashscope.aliyuncs.com/v1",
            "subscription:qwen@coding-plan.aliyun.com",
        ),
        (
            "https://Token-Plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
            "subscription:qwen@token-plan.aliyun.com",
        ),
        // Two rows whose credential is a key share this address: the
        // `API key` list's answers.
        ("https://api.minimax.io/v1", "key:minimax@minimax.io"),
        (
            "https://api.minimax.cn/v1/chat/completions",
            "key:minimax@minimaxi.com",
        ),
        (
            "https://open.bigmodel.cn/api/paas/v4",
            "key:zai@bigmodel.cn",
        ),
        ("https://api.z.ai/api/paas/v4", "key:zai@z.ai"),
        (
            "https://api.deepseek.com/chat/completions",
            "key:deepseek@deepseek.com",
        ),
        ("https://api.meta.ai/v1/responses", "key:meta@meta.ai"),
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
        "https://dashscope.aliyuncs.com/v1",
        "https://api.z.ai/api/paas/v40",
        // The ChatGPT sign-in's own address: no key row stands there.
        "https://chatgpt.com/backend-api/codex/responses",
        "http://localhost:8080/v1",
        "not an address",
    ] {
        assert_eq!(recognised(missed), None, "{missed}");
    }
}

/// Every address a row whose credential is a key sends to lies under the
/// address recognised for it, so a `baseUrl` set to where such a row already
/// goes answers for that row, or for the `API key` list's row where the two
/// share it; and every recognised address answers for a key row or for a Kimi
/// open platform route.
#[test]
fn every_key_row_is_recognised_at_the_address_it_sends_to() {
    let rows = Rows::production();
    let vendor = |provider: &str| match provider {
        "google" => Google::VENDOR.as_str().to_owned(),
        "openai" => OpenAi::VENDOR.as_str().to_owned(),
        "anthropic" => crucible_provider::Anthropic::VENDOR.as_str().to_owned(),
        "deepseek" => crucible_provider::DeepSeek::VENDOR.as_str().to_owned(),
        "meta" => crucible_provider::Meta::VENDOR.as_str().to_owned(),
        "mimo" => crucible_provider::Mimo::VENDOR.as_str().to_owned(),
        "xai" => crucible_provider::Xai::VENDOR.as_str().to_owned(),
        _ => panic!("no default address for {provider}"),
    };
    let address = |row: &Row| {
        row.address
            .as_ref()
            .map_or_else(|| vendor(row.provider), |at| at.as_str().to_owned())
    };
    for row in rows.all().iter().filter(|row| row.kind == Kind::Key) {
        let at = address(row);
        let answers = rows
            .listed(List::Key)
            .find(|key| key.provider == row.provider && address(key) == at)
            .unwrap_or(row);
        assert_eq!(
            recognised(&at).map(str::to_owned),
            Some(row_route(answers)),
            "{at}"
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
/// sends only to its route's origins, an agent's route claims none, a Kimi row's sign-in, model and web tool
/// hosts are its site's, and route spellings are distinct. A warned model is
/// one its provider offers, and claims no origin: the host it is sent to
/// serves that provider's other models too, so it is asked about at the send.
#[test]
fn every_warned_route_names_its_row_and_every_origin_it_is_sent_to() {
    let rows = Rows::production();
    let routes = Routes::production();
    let catalogue = crate::providers::providers()
        .expect("the built-in providers")
        .snapshot();
    let mut spelled = BTreeSet::new();
    for warned in routes.all() {
        assert!(spelled.insert(&*warned.route), "{} twice", warned.route);
        if warned.route.starts_with("api.moonshot.") {
            assert_eq!(warned.origins, [format!("https://{}", warned.route)]);
            continue;
        }
        if launches(&warned.route) {
            assert!(warned.origins.is_empty(), "{}", warned.route);
            continue;
        }
        if let Some(named) = warned.route.strip_prefix("model:") {
            let (provider, model) = named.split_once('/').expect("provider/model");
            assert!(
                crate::providers::offered(&catalogue).any(|served| served.name == provider
                    && served.models.iter().any(|offered| offered.name == model)),
                "{} names no model offered",
                warned.route
            );
            assert!(warned.origins.is_empty(), "{}", warned.route);
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

/// The routes warned are exactly those whose vendor's own terms say what is
/// sent there may be used to train or improve its models, and every installed
/// agent's; every other row of the same vendors, and the standard Meta models,
/// are not asked about.
#[test]
fn the_routes_warned_are_those_whose_vendor_says_so() {
    let warned: BTreeSet<&str> = WARNED.iter().map(|warned| &*warned.route).collect();
    let expected = BTreeSet::from([
        "subscription:openai",
        "subscription:moonshot@kimi.ai",
        "subscription:moonshot",
        "subscription:minimax@token-plan.minimax.io",
        "subscription:minimax@token-plan.minimaxi.com",
        "subscription:qwen@coding-plan.aliyun.com",
        "subscription:qwen@token-plan.aliyun.com",
        "key:google",
        "key:minimax@minimax.io",
        "key:minimax@minimaxi.com",
        "key:moonshot@kimi.ai",
        "key:moonshot",
        "key:zai@bigmodel.cn",
        "model:meta/muse-spark-1.3-contributor",
        "model:meta/muse-spark-1.2-contributor",
        "api.moonshot.ai",
        "agent:claude",
        "agent:antigravity",
        "api.moonshot.cn",
    ]);
    assert_eq!(warned, expected);

    let rows = Rows::production();
    for row in rows.all() {
        let route = row_route(row);
        let asked = WARNED.iter().any(|warned| warned.route == route);
        assert_eq!(asked, expected.contains(route.as_str()), "{route}");
    }

    // A catalog agent adds exactly its own route to what a build warns about,
    // and no catalog statement moves a built-in route.
    let listed = Routes::production().cataloging([hold("goose"), hold("claude")]);
    let all: BTreeSet<&str> = listed.all().iter().map(|warned| &*warned.route).collect();
    let mut with_goose = expected;
    with_goose.insert("agent:goose");
    assert_eq!(all, with_goose);
}

/// A contributor model is asked about before its first send, and its
/// standard twin on the same key is not: neither is held at the host they
/// share, which serves the standard models too.
#[test]
fn a_contributor_model_is_asked_about_and_its_standard_twin_is_not() {
    let consent = Consent::new(Routes::production());
    consent.served("meta", Some(serving("key:meta@meta.ai")));
    let host = origin("https://api.meta.ai/v1/responses");

    for model in ["muse-spark-1.3-contributor", "muse-spark-1.2-contributor"] {
        let route = model_route("meta", model);
        assert_eq!(
            consent
                .unanswered("meta", model)
                .as_ref()
                .map(|one| &*one.route),
            Some(route.as_str())
        );
    }
    for model in ["muse-spark-1.3", "muse-spark-1.2"] {
        assert_eq!(consent.unanswered("meta", model), None, "{model}");
    }
    assert_eq!(consent.held(&host), None);

    consent.record("model:meta/muse-spark-1.3-contributor");
    assert_eq!(
        consent.unanswered("meta", "muse-spark-1.3-contributor"),
        None
    );
    assert!(
        consent
            .unanswered("meta", "muse-spark-1.2-contributor")
            .is_some()
    );
}

/// A Qwen Token Plan key of aliyun.com says what holds it before anything is
/// sent: the Personal edition's terms, which the Team edition's do not share
/// and which crucible cannot tell apart by the key.
#[test]
fn the_aliyun_token_plan_states_its_edition_first() {
    let routes = Routes::production();
    let token = routes
        .warned("subscription:qwen@token-plan.aliyun.com")
        .expect("warned");
    assert_eq!(
        token.warning.condition.as_deref(),
        Some("On the Personal edition")
    );
    assert_eq!(
        token.origins,
        ["https://token-plan.cn-beijing.maas.aliyuncs.com"]
    );
}

#[test]
fn a_condition_opens_its_sentence_and_every_warning_cites_its_page() {
    let catalog = [hold("goose")];
    for warned in WARNED.iter().chain(&catalog) {
        let warning = &warned.warning;
        if let Some(condition) = &warning.condition {
            assert!(
                warning.sentence.starts_with(condition.as_ref()),
                "{}",
                warned.route
            );
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

    consent.forget(|route| route == "key:google");
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

/// A credential taken out takes its row's yes with it, every model route of
/// its provider, and the route its provider's `baseUrl` answers for: out of
/// the user's file before the store is written, and out of what this run
/// holds. Another row's yes, and another provider's, stay.
#[test]
fn a_credential_taken_out_takes_the_yes_that_went_with_it() {
    let sample = Sample::new("letting-go-yes");
    let settings =
        sample.user(r#"{"providers": {"moonshot": {"baseUrl": "https://api.moonshot.ai/v1"}}}"#);
    let file = sample.user_file();
    let said = r#"{"contentUse": {"accepted": ["key:moonshot", "subscription:moonshot@kimi.ai", "model:moonshot/k3", "api.moonshot.ai", "key:google", "model:google/gemini"]}}"#;
    std::fs::write(&file, said).unwrap();
    let consent = Consent::new(Routes::production());
    let settings_now = sample.user(said);
    consent.recorded(
        settings_now
            .content_accepted()
            .into_iter()
            .map(str::to_owned),
    );

    let rows = Rows::production();
    let store = sample
        .store()
        .letting_go(letting_go(&consent, file.clone(), rows, &settings));
    store.keep("moonshot", "fabricated-kimi-com-key").unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        said,
        "nothing went"
    );

    // The key row replaced by another row's key.
    store
        .keep("moonshot@kimi.ai", "fabricated-kimi-ai-key")
        .unwrap();

    let left = sample.user(&std::fs::read_to_string(&file).unwrap());
    assert_eq!(
        left.content_accepted(),
        [
            "subscription:moonshot@kimi.ai",
            "key:google",
            "model:google/gemini"
        ]
    );
    for gone in ["key:moonshot", "api.moonshot.ai"] {
        assert!(consent.asks(gone).is_some(), "{gone}");
    }
    assert!(consent.asks("key:google").is_none());

    // Forgotten: the yes to the row it was on, and to the provider's models,
    // go with it.
    let before = std::fs::read_to_string(&file).unwrap().replace(
        "\"subscription:moonshot@kimi.ai\"",
        "\"key:moonshot@kimi.ai\", \"model:moonshot/k3\", \"subscription:moonshot@kimi.ai\"",
    );
    std::fs::write(&file, &before).unwrap();
    assert!(
        sample
            .user(&before)
            .content_accepted()
            .contains(&"key:moonshot@kimi.ai")
    );
    store.forget("moonshot").unwrap();
    let left = sample.user(&std::fs::read_to_string(&file).unwrap());
    assert_eq!(
        left.content_accepted(),
        [
            "subscription:moonshot@kimi.ai",
            "key:google",
            "model:google/gemini"
        ]
    );
}

/// A yes that could not be taken out stops the write: the credential stays,
/// and the file is as it was.
#[test]
fn a_yes_that_cannot_be_taken_out_leaves_the_credential() {
    let sample = Sample::new("letting-go-stuck");
    sample.user("{}");
    let file = sample.user_file();
    let consent = Consent::new(Routes::production());
    let plain = sample.store();
    plain.keep("moonshot", "fabricated-kimi-com-key").unwrap();
    let store = sample.store().letting_go(letting_go(
        &consent,
        file.clone(),
        Rows::production(),
        &Settings::default(),
    ));

    // A file that is not configuration cannot have a yes taken out of it.
    std::fs::write(&file, "{ not configuration").unwrap();
    let forgotten = store.forget("moonshot");

    assert!(forgotten.is_err(), "{forgotten:?}");
    assert!(plain.read().held("moonshot").is_some());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{ not configuration"
    );
}

/// What the file said at the start is read once: a yes the run took out since
/// does not come back from settings read before it was.
#[test]
fn the_yes_read_at_the_start_is_read_once() {
    let consent = Consent::new(Routes::production());
    consent.recorded(["key:google".to_owned()]);
    consent.forget(|route| route == "key:google");
    consent.recorded(["key:google".to_owned()]);
    assert!(consent.asks("key:google").is_some());
}

/// What a send by a provider would go on is asked about where it has no yes:
/// a model that is itself warned first, then the route the provider is served
/// on; a provider served on nothing, or on an unwarned route, asks nothing.
#[test]
fn a_send_asks_about_the_route_its_provider_is_served_on() {
    let model = Warned {
        route: "model:moonshot/k3".into(),
        shown: "K3".into(),
        warning: KIMI_AI,
        origins: &[],
    };
    let mut warned = WARNED.to_vec();
    warned.push(model);
    let consent = Consent::new(Routes::new(warned));
    assert_eq!(
        consent.unanswered("moonshot", "k3"),
        None,
        "served on nothing"
    );

    consent.served("moonshot", Some(serving("key:moonshot")));
    consent.served("anthropic", Some(serving("key:anthropic")));
    assert_eq!(
        consent
            .unanswered("moonshot", "k3")
            .as_ref()
            .map(|one| &*one.route),
        Some("model:moonshot/k3")
    );
    consent.record("model:moonshot/k3");
    assert_eq!(
        consent
            .unanswered("moonshot", "k3")
            .as_ref()
            .map(|one| &*one.route),
        Some("key:moonshot")
    );
    consent.record("key:moonshot");
    assert_eq!(consent.unanswered("moonshot", "k3"), None);
    assert_eq!(consent.unanswered("anthropic", "claude-fable-5-1"), None);
}

/// A caution is short enough to stand whole after `signed in` on a row at
/// forty columns, whatever of the row's own words is cut after it.
#[test]
fn every_caution_fits_after_signed_in_at_forty_columns() {
    for warned in WARNED {
        let row = format!("signed in · {}…", warned.warning.caution);
        assert!(row.chars().count() <= 38, "{}: {row}", warned.route);
    }
}

/// The page the routes are documented on.
const DOCUMENTED: &str = include_str!("../../../../docs/providers/content-use.md");

/// The cells of every table row of `DOCUMENTED` with `columns` cells, headers
/// and rules left out.
fn rows(columns: usize) -> Vec<Vec<String>> {
    DOCUMENTED
        .lines()
        .filter(|line| line.starts_with("| ") && !line.starts_with("| ---"))
        .map(|line| {
            line.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect::<Vec<_>>()
        })
        .filter(|cells| {
            cells.len() == columns
                && cells
                    .first()
                    .is_some_and(|first| first != "Route" && first != "Address")
        })
        .collect()
}

/// Every warning the panel draws, and every caution a row draws, is its row
/// of the docs table word for word, with the page and the day; the table
/// warns nothing this build does not; and every address a `baseUrl` is
/// recognised at is listed with the route it answers for.
#[test]
fn every_warning_is_its_row_of_the_docs_table() {
    let table = rows(9);
    assert_eq!(table.len(), WARNED.len(), "{table:?}");
    for warned in WARNED {
        let spelled = format!("`{}`", warned.route);
        let row = table
            .iter()
            .find(|row| row.get(1) == Some(&spelled))
            .unwrap_or_else(|| panic!("{} has no row", warned.route));
        let warning = &warned.warning;
        assert_eq!(
            row.get(2).map(String::as_str),
            Some(warning.condition.as_deref().unwrap_or("")),
            "{}",
            warned.route
        );
        assert_eq!(
            row.get(3).map(String::as_str),
            Some(&*warning.sentence),
            "{}",
            warned.route
        );
        assert_eq!(
            row.get(4).map(String::as_str),
            Some(&*warning.caution),
            "{}",
            warned.route
        );
        assert_eq!(
            row.get(7).map(String::as_str),
            Some(format!("[{}]({})", warning.source, warning.link).as_str()),
            "{}",
            warned.route
        );
        assert_eq!(
            row.get(8).map(String::as_str),
            Some(&*warning.read),
            "{}",
            warned.route
        );
    }

    let addresses = rows(2);
    for one in RECOGNISED {
        let address = format!("`{}`", one.address);
        let row = addresses
            .iter()
            .find(|row| row.first() == Some(&address))
            .unwrap_or_else(|| panic!("{} is not listed", one.address));
        assert!(
            row.get(1)
                .is_some_and(|route| route.starts_with(&format!("`{}`", one.route))),
            "{row:?}"
        );
    }
    assert_eq!(addresses.len(), RECOGNISED.len(), "{addresses:?}");
}

/// Stopped between the two writes: the yes is out of the user's file, the
/// store could not be written, and the credential is still there with no yes,
/// so the question stands again rather than a yes outliving what it was for.
#[test]
fn a_stop_between_the_two_writes_leaves_the_credential_and_no_yes() {
    let sample = Sample::new("letting-go-between");
    let said = r#"{"contentUse": {"accepted": ["key:moonshot"]}}"#;
    sample.user(said);
    let file = sample.user_file();
    let consent = Consent::new(Routes::production());
    consent.recorded(["key:moonshot".to_owned()]);
    sample
        .store()
        .keep("moonshot", "fabricated-kimi-com-key")
        .unwrap();
    let store = sample.store().letting_go(letting_go(
        &consent,
        file.clone(),
        Rows::production(),
        &Settings::default(),
    ));

    // The store's own write cannot land: where it writes before it replaces
    // is a directory.
    std::fs::create_dir_all(sample.home().join("auth.json.new")).unwrap();
    let forgotten = store.forget("moonshot");

    assert!(forgotten.is_err(), "{forgotten:?}");
    assert!(sample.store().read().held("moonshot").is_some());
    assert!(
        !std::fs::read_to_string(&file)
            .unwrap()
            .contains("key:moonshot")
    );
    assert!(consent.asks("key:moonshot").is_some());
}

/// A `baseUrl` changed and changed back moves no yes: the route it answers
/// for is asked about by what is sent on it, and nothing was taken out.
#[test]
fn a_base_url_changed_and_changed_back_moves_no_yes() {
    let consent = Consent::new(Routes::production());
    consent.recorded(["api.moonshot.ai".to_owned()]);
    let at = |url: &str| Serving {
        route: recognised(url).map(str::to_owned),
        at: Origin::of(url),
    };

    consent.served("moonshot", Some(at("https://api.moonshot.ai/v1")));
    assert_eq!(consent.unanswered("moonshot", "kimi-k2"), None);
    consent.served("moonshot", Some(at("https://gateway.example/v1")));
    assert_eq!(consent.unanswered("moonshot", "kimi-k2"), None);
    consent.served("moonshot", Some(at("https://api.moonshot.ai/v1")));
    assert_eq!(consent.unanswered("moonshot", "kimi-k2"), None);
    assert!(consent.asks("api.moonshot.ai").is_none());

    // And the address with no yes is asked about as soon as it is served.
    consent.served("moonshot", Some(at("https://api.moonshot.cn/v1")));
    assert_eq!(
        consent
            .unanswered("moonshot", "kimi-k2")
            .as_ref()
            .map(|one| &*one.route),
        Some("api.moonshot.cn")
    );
}

/// What a provider was served on is read again once its credential goes,
/// from what is left when it is next asked about: a key from the environment
/// may still serve the same route, whose yes went with the stored one, and
/// another provider's route stays as it was.
#[test]
fn a_credential_taken_out_has_its_provider_read_again_when_next_asked() {
    let sample = Sample::new("letting-go-served");
    sample.user(r#"{"contentUse": {"accepted": ["key:google"]}}"#);
    let consent = Consent::new(Routes::production());
    consent.recorded(["key:google".to_owned()]);
    consent.served("openai", Some(serving("subscription:openai")));
    consent.served("google", Some(serving("key:google")));
    let left: BTreeMap<&str, Serving> = BTreeMap::from([
        ("openai", serving("key:openai")),
        ("google", serving("key:google")),
    ]);
    consent.resolves(Box::new(move |name| {
        Reading::Served(left.get(name).cloned())
    }));
    let store = sample.store().letting_go(letting_go(
        &consent,
        sample.user_file(),
        Rows::production(),
        &Settings::default(),
    ));
    sample
        .store()
        .keep("openai", "fabricated-openai-key")
        .unwrap();
    sample
        .store()
        .keep("google", "fabricated-google-key")
        .unwrap();

    store.forget("openai").unwrap();
    store.forget("google").unwrap();

    // Not the sign-in that went: the key the environment still serves it on.
    assert_eq!(consent.unanswered("openai", "gpt-6-astra"), None);
    // The same route as the key that went, whose yes went with it.
    assert_eq!(
        consent
            .unanswered("google", "gemini-3.8-flash")
            .as_ref()
            .map(|one| &*one.route),
        Some("key:google")
    );
}

/// A provider whose credential went keeps claiming its origin on the route
/// it was served on until it is read again, and that route's yes went with
/// the credential: another provider's claim there, on an address crucible
/// does not know, does not let the origin through in between.
#[test]
fn a_provider_gone_stale_still_holds_its_origin_until_read_again() {
    let sample = Sample::new("letting-go-kept");
    sample.user(r#"{"contentUse": {"accepted": ["key:google"]}}"#);
    let consent = Consent::new(Routes::production());
    consent.recorded(["key:google".to_owned()]);
    consent.served("google", Some(serving("key:google")));
    let base = "https://generativelanguage.googleapis.com/elsewhere";
    consent.served(
        "another",
        Some(Serving {
            route: None,
            at: Origin::of(base),
        }),
    );
    let store = sample.store().letting_go(letting_go(
        &consent,
        sample.user_file(),
        Rows::production(),
        &Settings::default(),
    ));
    sample
        .store()
        .keep("google", "fabricated-google-key")
        .unwrap();

    store.forget("google").unwrap();

    assert_eq!(consent.held(&origin(base)).as_deref(), Some("key:google"));
}

/// A provider nobody sends through, whose sign-in was taken out, is read
/// again before a send by another provider at the same origin is asked
/// about: a claim for a sign-in that is gone holds nothing once the store says
/// so, and the send goes on its own route's yes.
#[test]
fn a_sign_in_taken_out_holds_no_other_providers_send_once_read_again() {
    let sample = Sample::new("letting-go-other");
    sample.user(r#"{"contentUse": {"accepted": ["key:moonshot", "subscription:moonshot"]}}"#);
    sample.holding(
        r#"{"version":2,"keys":{},"subscriptions":{"moonshot":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#,
    );
    let consent = Consent::new(Routes::production());
    consent.recorded([
        "key:moonshot".to_owned(),
        "subscription:moonshot".to_owned(),
    ]);
    let base = "https://api.kimi.com/coding/v1";
    consent.served(
        "anthropic",
        Some(Serving {
            route: recognised(base).map(str::to_owned),
            at: Origin::of(base),
        }),
    );
    consent.served("moonshot", Some(serving("subscription:moonshot")));
    consent.resolves(Box::new(|name| match name {
        "anthropic" => Reading::Served(Some(Serving {
            route: recognised("https://api.kimi.com/coding/v1").map(str::to_owned),
            at: Origin::of("https://api.kimi.com/coding/v1"),
        })),
        _ => Reading::Served(None),
    }));
    let store = sample.store().letting_go(letting_go(
        &consent,
        sample.user_file(),
        Rows::production(),
        &Settings::default(),
    ));

    store.forget("moonshot").unwrap();

    assert_eq!(consent.unanswered("anthropic", "claude-sonnet-5"), None);
    assert_eq!(consent.held(&origin(base)), None);
}

/// A store that cannot be read when a stale provider is asked about leaves
/// it stale, still claiming what it was last served on, and the next question
/// reads it again.
#[test]
fn a_provider_the_store_cannot_be_read_for_stays_stale_until_it_can() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let sample = Sample::new("letting-go-unread");
    sample.user("{}");
    let consent = Consent::new(Routes::production());
    consent.served("google", Some(serving("key:google")));
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reads);
    consent.resolves(Box::new(move |_| {
        if counted.fetch_add(1, Ordering::SeqCst) == 0 {
            Reading::Unread
        } else {
            Reading::Served(None)
        }
    }));
    let store = sample.store().letting_go(letting_go(
        &consent,
        sample.user_file(),
        Rows::production(),
        &Settings::default(),
    ));
    sample
        .store()
        .keep("google", "fabricated-google-key")
        .unwrap();
    store.forget("google").unwrap();

    let unread = consent.unanswered("google", "gemini-3.8-flash");
    let read = consent.unanswered("google", "gemini-3.8-flash");
    let settled = consent.unanswered("google", "gemini-3.8-flash");

    assert_eq!(unread.as_ref().map(|one| &*one.route), Some("key:google"));
    assert_eq!(read, None);
    assert_eq!(settled, None);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
}

/// A yes given at `/login` waits for the credential it was given for to be
/// written, and that write keeps it even where the row it drops shares the
/// route, as a `baseUrl` crucible recognises can make it.
#[test]
fn a_yes_given_at_login_outlasts_the_write_it_waits_for() {
    let sample = Sample::new("letting-go-given");
    let settings = sample
        .user(r#"{"providers": {"moonshot": {"baseUrl": "https://api.kimi.com/coding/v1"}}}"#);
    let consent = Consent::new(Routes::production());
    let store = sample.store().letting_go(letting_go(
        &consent,
        sample.user_file(),
        Rows::production(),
        &settings,
    ));
    sample
        .store()
        .keep("moonshot@kimi.ai", "fabricated-kimi-ai-key")
        .unwrap();
    consent.give("key:moonshot");

    store.keep("moonshot", "fabricated-kimi-com-key").unwrap();

    assert!(consent.given("key:moonshot"));
    assert!(consent.asks("key:moonshot").is_none());
}

/// Another provider's route at the origin this one is served at holds its
/// requests too, so that route is what the send asks about: a request the
/// hold keeps back is one a question can let go.
#[test]
fn a_send_asks_about_whatever_holds_its_origin() {
    let consent = Consent::new(Routes::production());
    consent.record("subscription:moonshot@kimi.ai");
    consent.served("moonshot", Some(serving("subscription:moonshot@kimi.ai")));
    let base = "https://api.kimi.ai/coding/v1/chat/completions";
    consent.served(
        "openai",
        Some(Serving {
            route: recognised(base).map(str::to_owned),
            at: Origin::of(base),
        }),
    );

    let origin = origin(base);
    let held = consent.held(&origin);
    assert_eq!(held.as_deref(), Some("key:moonshot@kimi.ai"));
    assert_eq!(
        consent
            .unanswered("moonshot", "k3")
            .as_ref()
            .map(|one| &*one.route),
        held.as_deref()
    );

    consent.record("key:moonshot@kimi.ai");
    assert_eq!(consent.held(&origin), None);
    assert_eq!(consent.unanswered("moonshot", "k3"), None);
}

/// A path spelled with dot segments or percent-encoding is the path it names,
/// so another spelling of a documented address is that address.
#[test]
fn a_documented_path_spelled_another_way_is_recognised() {
    for (spelled, route) in [
        (
            "https://generativelanguage.googleapis.com/./v1beta/interactions",
            "key:google",
        ),
        (
            "https://generativelanguage.googleapis.com/%76%31beta/interactions",
            "key:google",
        ),
        (
            "https://api.kimi.com/x/../coding/v1/chat/completions",
            "key:moonshot",
        ),
        ("https://api.kimi.com/%63oding/v1", "key:moonshot"),
        (
            "https://api.moonshot.ai/v1/./chat/completions",
            "api.moonshot.ai",
        ),
        // An empty segment is a segment a `..` can take out, as a server
        // resolving the path takes it out.
        (
            "https://api.moonshot.ai/v1//../chat/completions",
            "api.moonshot.ai",
        ),
        (
            "https://generativelanguage.googleapis.com/v1beta//../models/x",
            "key:google",
        ),
        // A slash spelled as an escape is a slash to a server that decodes
        // before it routes.
        ("https://api.kimi.com/coding%2Fv1", "key:moonshot"),
        (
            "https://api.moonshot.ai/v1%2Fchat%2Fcompletions",
            "api.moonshot.ai",
        ),
        // A server that merges adjacent slashes before it resolves reads
        // `/x//../v1` as `/v1`.
        (
            "https://api.moonshot.ai/x//../v1/chat/completions",
            "api.moonshot.ai",
        ),
        ("https://api.kimi.com/x//../coding/v1", "key:moonshot"),
        (
            "https://generativelanguage.googleapis.com/x//../v1beta/models/y",
            "key:google",
        ),
        ("https://api.moonshot.ai/x//%2E%2E/v1", "api.moonshot.ai"),
        // A server that decodes `%2E` but keeps `%2F` inside its segment.
        (
            "https://api.moonshot.ai/a%2Fb/%2E%2E/v1/chat/completions",
            "api.moonshot.ai",
        ),
        (
            "https://api.kimi.com/x%2Fy/%2E%2E/coding/v1",
            "key:moonshot",
        ),
        (
            "https://generativelanguage.googleapis.com/a%2Fb/%2e%2e/v1beta",
            "key:google",
        ),
        ("https://api.kimi.com/x%2F..%2Fcoding/v1", "key:moonshot"),
        ("https://api.kimi.com//coding/v1", "key:moonshot"),
    ] {
        assert_eq!(recognised(spelled), Some(route), "{spelled}");
    }
    assert_eq!(super::decoded("%+f%2B"), "%+f+");
    for missed in [
        "https://api.moonshot.ai/v1/../v2",
        "https://api.moonshot.ai/%2E%2E/v1x",
        "https://generativelanguage.googleapis.com/v1alpha",
        "https://api.moonshot.ai/v1/..",
        "https://api.kimi.com/coding/v1/../../x",
        "https://api.kimi.com/coding%2Fv1/%2E%2E/x",
        "https://api.kimi.com/Coding/V1",
    ] {
        assert_eq!(recognised(missed), None, "{missed}");
    }
}

/// A launch is held until its agent's route has a yes, and only an agent
/// route this build warns about is ever held at all.
#[test]
fn a_launch_is_held_until_its_agent_route_has_a_yes() {
    let consent = Consent::new(Routes::production());
    assert!(matches!(
        held(&consent, &agent_route("claude")),
        Err(Unheld::Asks(asked)) if asked.route == "agent:claude"
    ));
    consent.give("agent:claude");
    let granted = held(&consent, "agent:claude").expect("a yes holds the launch");
    assert_eq!(granted.route(), "agent:claude");
    assert!(matches!(
        held(&consent, "agent:antigravity"),
        Err(Unheld::Asks(_))
    ));
    for unknown in ["agent:codex", "key:google", "subscription:openai", ""] {
        consent.give(unknown);
        assert_eq!(
            held(&consent, unknown).map(|_| ()),
            Err(Unheld::Unknown),
            "{unknown}"
        );
    }
}

/// What a launch does besides what it is asked is on the page, in the words
/// the panel shows.
#[test]
fn the_launch_sentence_is_documented() {
    assert!(DOCUMENTED.contains(LAUNCHING), "{LAUNCHING}");
}

/// An installed agent's route is put with what starting it runs after the
/// vendor's sentence, and every other route with the vendor's alone; the
/// whole of either fits what a client is put.
#[test]
fn only_an_agent_s_sentence_has_the_launch_sentence_after_the_vendor_s() {
    for warned in &WARNED {
        let said = full_sentence(warned);
        if launches(&warned.route) {
            assert_eq!(said, format!("{} {LAUNCHING}", warned.warning.sentence));
        } else {
            assert_eq!(said, warned.warning.sentence, "{}", warned.route);
        }
        assert!(
            !crucible_client_api::Text::cut(&said).truncated(),
            "{}",
            warned.route
        );
    }
}

/// The entry a signed catalog gives the agent keyed `key`, carrying
/// `statement` as its statement of content use where one is given.
fn entry_of(key: &str, statement: Option<serde_json::Value>) -> Entry {
    use crucible_external_harnesses::catalog::{Catalog, Held, fixture};

    let file = serde_json::json!({"path": key, "size": 1, "sha256": "b".repeat(64), "exec": true});
    let mut entry = serde_json::json!({
        "key": key,
        "registry_id": key,
        "version": "1.4.1",
        "kind": "binary",
        "license": "Apache-2.0",
        "review": "https://example.com/review/1",
        "admits": {"name": key, "version": "1.4.1"},
        "targets": [{
            "platform": "any",
            "cmd": key,
            "artifacts": [{
                "url": "https://example.com/agent.zip",
                "sha256": "a".repeat(64),
                "size": 1,
                "format": "zip",
                "unpacked": 1,
                "decompressed": 1,
                "files": [file],
            }],
        }],
    });
    if let Some(statement) = statement {
        entry
            .as_object_mut()
            .expect("an entry is an object")
            .insert("content_use".into(), statement);
    }
    let catalog = serde_json::json!({
        "schema": 1,
        "serial": 1,
        "revoked": [],
        "entries": [entry],
    });
    let signed = fixture::signed(catalog.to_string().as_bytes());
    let catalog = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        std::time::SystemTime::now(),
    )
    .expect("the catalog is believed");
    catalog.entries().first().expect("an entry").clone()
}

/// A statement of content use of `text`, covering `origins`.
fn statement(text: &str, origins: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "text": text,
        "source": "https://vendor.example/terms/data?v=2",
        "date": "24 Oct 2026",
        "origins": origins,
    })
}

/// The hold the agent keyed `key` has, from a statement of its own.
fn hold(key: &str) -> Warned {
    let entry = entry_of(
        key,
        Some(statement(
            "The vendor may use what you send to improve its products.",
            &["https://api.vendor.example"],
        )),
    );
    stated(&entry).expect("a hold")
}

/// An entry's statement of content use is the hold its agent's route is asked
/// under: the vendor's words, the page and the day, owned, with the origins it
/// covers said in the words and never held at; and the route is asked about
/// until it has a yes.
#[test]
fn a_statement_is_the_hold_of_its_agents_route() {
    let warned = hold("goose");

    assert_eq!(warned.route, "agent:goose");
    assert_eq!(warned.shown, "goose");
    assert!(
        warned
            .warning
            .sentence
            .starts_with("The vendor may use what you send to improve its products.")
    );
    assert!(
        warned
            .warning
            .sentence
            .contains("https://api.vendor.example")
    );
    assert_eq!(warned.warning.condition, None);
    assert_eq!(warned.warning.link, "https://vendor.example/terms/data?v=2");
    assert_eq!(warned.warning.source, "vendor.example");
    assert_eq!(warned.warning.read, "24 Oct 2026");
    assert!(
        warned.origins.is_empty(),
        "an agent's route names no origin"
    );
    assert!(launches(&warned.route));

    let consent = Consent::new(Routes::production().cataloging([warned.clone()]));
    assert_eq!(consent.routes().warned("agent:goose"), Some(&warned));
    assert!(consent.routes().warned("agent:claude").is_some());
    assert!(matches!(
        held(&consent, "agent:goose"),
        Err(Unheld::Asks(asked)) if *asked == warned
    ));
    assert_eq!(
        consent.held(&origin("https://api.vendor.example/v1")),
        None,
        "crucible's own requests are not held on the agent's route"
    );
    consent.give("agent:goose");
    assert_eq!(
        held(&consent, "agent:goose").map(|granted| granted.route().to_owned()),
        Ok("agent:goose".to_owned())
    );
    // Another catalog agent's yes is not this one's.
    assert!(matches!(
        held(&consent, "agent:other"),
        Err(Unheld::Unknown)
    ));
}

/// An entry with no statement, or one that cannot be shown whole, gives no
/// hold, and says why; one a byte short of the ceiling gives one.
#[test]
fn an_entry_without_a_usable_statement_gives_no_hold() {
    assert_eq!(
        stated(&entry_of("goose", None)).map(|_| ()),
        Err(Unstated::Missing)
    );
    let at = |text: String, origins: &[&str]| {
        stated(&entry_of("goose", Some(statement(&text, origins))))
    };
    assert!(at("w".repeat(STATEMENT_BYTES), &[]).is_ok());
    assert_eq!(
        at("w".repeat(STATEMENT_BYTES + 1), &[]).map(|_| ()),
        Err(Unstated::TooLong {
            bytes: STATEMENT_BYTES + 1
        })
    );
    // The origins it covers are said with it, so they count.
    assert!(matches!(
        at("w".repeat(STATEMENT_BYTES), &["https://api.vendor.example"]),
        Err(Unstated::TooLong { .. })
    ));
    let mut hostless = statement("The vendor may use what you send.", &[]);
    hostless
        .as_object_mut()
        .expect("a statement is an object")
        .insert("source".into(), "https:///terms".into());
    assert_eq!(
        stated(&entry_of("goose", Some(hostless))).map(|_| ()),
        Err(Unstated::Source)
    );
    for refused in [
        Unstated::Missing,
        Unstated::Source,
        Unstated::TooLong { bytes: 5000 },
    ] {
        assert!(!refused.to_string().is_empty());
    }
}

/// A build's catalog routes are bounded, may not shadow or restate a built-in
/// route, and name only agents; a statement that would is not asked about, so
/// its agent is not launched.
#[test]
fn the_catalog_adds_only_its_own_bounded_agent_routes() {
    let mut imposter = hold("claude");
    imposter.warning.sentence = "Nothing is used.".to_owned().into();
    let mut stray = hold("goose");
    stray.route = "key:google".to_owned().into();
    let mut sending = hold("goose");
    sending.origins = &["https://api.vendor.example"];
    let routes = Routes::production().cataloging([imposter, stray, sending]);
    assert_eq!(routes.all().len(), WARNED.len());
    assert!(
        routes
            .warned("agent:claude")
            .is_some_and(|claude| claude.warning.sentence != "Nothing is used.")
    );

    let many = (0..CATALOGED + 5).map(|at| {
        let mut warned = hold("goose");
        warned.route = format!("agent:goose-{at}").into();
        warned
    });
    let routes = Routes::production().cataloging(many);
    assert_eq!(routes.all().len(), WARNED.len() + CATALOGED);
    assert!(
        routes
            .warned(&format!("agent:goose-{}", CATALOGED - 1))
            .is_some()
    );
    assert!(routes.warned(&format!("agent:goose-{CATALOGED}")).is_none());

    // The same route given twice is asked about once.
    let twice = Routes::production().cataloging([hold("goose"), hold("goose")]);
    assert_eq!(twice.all().len(), WARNED.len() + 1);
}

/// Whatever a statement says, the whole of the hold fits what a client is
/// put and what a `/login` row has room for.
#[test]
fn the_longest_statement_still_fits_what_a_client_is_put() {
    let text = "é".repeat(STATEMENT_BYTES / 2);
    let entry = entry_of("goose", Some(statement(&text, &[])));
    let warned = stated(&entry).expect("a statement at the ceiling");
    let said = full_sentence(&warned);
    assert!(said.ends_with(LAUNCHING));
    assert!(!crucible_client_api::Text::cut(&said).truncated());
    let row = format!("signed in · {}…", warned.warning.caution);
    assert!(row.chars().count() <= 38, "{row}");
}

/// The page says what a catalog agent is held on and how much of a statement
/// it takes.
#[test]
fn the_catalog_hold_is_documented() {
    assert!(DOCUMENTED.contains("## An agent from the catalog"));
    assert!(DOCUMENTED.contains(&format!("{STATEMENT_BYTES} bytes")));
    assert!(DOCUMENTED.contains(&format!("{CATALOGED} catalog agents")));
}
