//! What the auth commands read a key as, which row a word settles on, and the
//! documents and sentences they make of a home this test writes.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _};

use crucible_config::Home;
use serde_json::Value;

use super::{
    Desk, Login, MAX_SECRET, Refused, SURROUNDING, Secret, State, failed, forgotten, kept,
};
use crate::sample::Sample;

/// A made-up key, spelled so that one repeated anywhere shows.
const SENTINEL: &str = "sk-auth-unit-sentinel-5e4d3c2b1a";

/// What `pointer` points at in `value`, or null where nothing is there.
fn at<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    value.pointer(pointer).unwrap_or(&Value::Null)
}

/// A moment the tests measure lapse times against.
const NOW: u64 = 2_000_000_000;

/// The desk of `sample`'s home, with `exported` the only variables set.
fn desk(sample: &Sample, exported: &[(&'static str, &'static str)]) -> Desk {
    let home = sample.home();
    fs::create_dir_all(&home).expect("a temporary home");
    let exported = exported.to_vec();
    Desk::open(
        Home::find(&|name: &str| {
            (name == crucible_config::HOME).then(|| OsString::from(home.clone()))
        }),
        Box::new(move |name| {
            exported
                .iter()
                .find(|(set, _)| *set == name)
                .map(|(_, value)| (*value).to_owned())
        }),
    )
    .expect("a desk over a home this test made")
}

/// A store holding an Anthropic key, a lapsed Kimi account login, and a key
/// under a name this build has no row for.
const HELD: &str = r#"{"version":2,"keys":{"anthropic":"fabricated-anthropic-key","custom@example.com":"fabricated-custom-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{"device_id":"01234567-89ab-4cde-8fab-0123456789ab","expires_in":"3600"},"expires_at":1000000000,"refreshed_at":1}},"identities":{"moonshot@kimi.ai":"00000000-0000-4000-8000-000000000000"}}"#;

#[test]
fn a_key_at_the_bound_is_taken_and_one_past_it_refused_not_cut() {
    let at = format!("{}\n", "k".repeat(MAX_SECRET));
    assert!(
        Secret::read(&mut at.as_bytes()).is_ok(),
        "a key at the bound"
    );

    let past = "k".repeat(MAX_SECRET.saturating_add(1));
    assert!(matches!(
        Secret::read(&mut past.as_bytes()),
        Err(Refused::Oversized)
    ));
    assert!(matches!(Secret::typed(&past), Err(Refused::Oversized)));

    // An input that never ends is refused after the bound, not read whole.
    let mut endless = io::repeat(b'k').take(u64::MAX);
    assert!(matches!(
        Secret::read(&mut endless),
        Err(Refused::Oversized)
    ));
    let mut padded = format!("{}{}", " ".repeat(SURROUNDING + 1), "k".repeat(MAX_SECRET));
    padded.push('\n');
    assert!(matches!(
        Secret::read(&mut padded.as_bytes()),
        Err(Refused::Oversized)
    ));
}

#[test]
fn what_is_read_is_never_said_back() {
    let secret = Secret::read(&mut format!("  {SENTINEL}\n").as_bytes()).expect("one key");
    assert!(!format!("{secret:?}").contains(SENTINEL), "Debug showed it");

    for (input, refused) in [
        (format!("{SENTINEL} {SENTINEL}"), "spaced"),
        (format!("{SENTINEL}\u{7}"), "a control character"),
        ("   \n".to_owned(), "empty"),
    ] {
        let said = Secret::read(&mut input.as_bytes())
            .expect_err(refused)
            .to_string();
        assert!(!said.contains(SENTINEL), "{refused}: {said}");
    }
    let binary = [0xff_u8, 0xfe, 0x00];
    assert!(matches!(
        Secret::read(&mut binary.as_slice()),
        Err(Refused::Binary)
    ));
}

#[test]
fn a_word_settles_on_its_row_or_is_refused_without_being_repeated() {
    let sample = Sample::new("auth-words");
    let desk = desk(&sample, &[]);

    let Ok(Login::Key(way)) = desk.login("anthropic", true) else {
        panic!("anthropic takes a key");
    };
    assert_eq!(way.stored(), "anthropic");
    let Ok(Login::Key(way)) = desk.login("moonshot@kimi.ai", true) else {
        panic!("a row's stored name takes a key");
    };
    assert_eq!(
        (way.provider(), way.shown()),
        ("moonshot", "MoonshotAI · kimi.ai")
    );
    let Ok(Login::Either { key, account }) = desk.login("openai", false) else {
        panic!("openai is a key or an account");
    };
    assert_eq!((key.stored(), account.stored()), ("openai", "openai"));
    assert_eq!(desk.provider("moonshot@kimi.ai").ok(), Some("moonshot"));

    let refused = desk.login(SENTINEL, true).expect_err("nobody serves it");
    assert!(matches!(refused, Refused::Unknown { .. }));
    let said = refused.to_string();
    assert!(!said.contains(SENTINEL), "{said}");
    assert!(
        said.contains("anthropic"),
        "the names served are said: {said}"
    );
    assert!(desk.provider(SENTINEL).is_err());
}

#[test]
fn status_says_each_source_and_state_in_one_bounded_document() {
    let sample = Sample::new("auth-status");
    sample.holding(HELD);
    let desk = desk(&sample, &[("OPENAI_API_KEY", SENTINEL)]);

    let status = desk.status(None, NOW);
    let bytes = status.json();
    let text = String::from_utf8(bytes).expect("UTF-8");
    assert!(!text.contains(SENTINEL), "a variable's value was said");
    assert!(!text.contains("fabricated"), "a stored value was said");
    assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
    let document: Value = serde_json::from_str(&text).expect("one JSON document");

    assert_eq!(at(&document, "/format_version"), 1);
    assert_eq!(at(&document, "/kind"), "auth-status");
    assert_eq!(at(&document, "/status"), "complete");
    assert_eq!(at(&document, "/acceptance"), "unchecked");
    assert_eq!(at(&document, "/truncated"), false);
    assert_eq!(at(&document, "/problem"), &Value::Null);
    let providers = at(&document, "/providers").as_array().expect("a list");
    let one = |name: &str| {
        providers
            .iter()
            .find(|entry| at(entry, "/provider") == name)
            .unwrap_or_else(|| panic!("{name} is listed"))
            .clone()
    };

    let anthropic = one("anthropic");
    assert_eq!(at(&anthropic, "/state"), "configured");
    assert_eq!(at(&anthropic, "/source"), "stored-key");
    assert_eq!(at(&anthropic, "/variable"), "ANTHROPIC_API_KEY");
    assert_eq!(at(&anthropic, "/variable_set"), false);
    assert_eq!(
        at(&anthropic, "/stored"),
        &serde_json::json!([{"name": "anthropic", "kind": "key", "expires_at": null}])
    );

    let moonshot = one("moonshot");
    assert_eq!(at(&moonshot, "/state"), "expired");
    assert_eq!(at(&moonshot, "/source"), "account");
    assert_eq!(
        at(&moonshot, "/stored"),
        &serde_json::json!([{"name": "moonshot@kimi.ai", "kind": "account", "expires_at": 1_000_000_000}])
    );

    let openai = one("openai");
    assert_eq!(at(&openai, "/state"), "configured");
    assert_eq!(at(&openai, "/source"), "environment");
    assert_eq!(at(&openai, "/variable_set"), true);

    let deepseek = one("deepseek");
    assert_eq!(at(&deepseek, "/state"), "absent");
    assert_eq!(at(&deepseek, "/source"), &Value::Null);
    assert_eq!(
        status.exit(),
        0,
        "nothing was named, so absent is an answer"
    );

    for entry in providers {
        for field in [
            "provider",
            "state",
            "source",
            "variable",
            "variable_set",
            "variable_configured",
            "base_url_configured",
            "stored",
            "reason",
        ] {
            assert!(entry.get(field).is_some(), "{field} is in every entry");
        }
    }

    let human = status.human(NOW);
    assert!(human.contains("lapsed"), "{human}");
    assert!(human.contains("not checked here"), "{human}");
    assert!(!human.contains(SENTINEL));
}

#[test]
fn a_named_provider_with_nothing_is_a_failure_and_one_with_something_is_not() {
    let sample = Sample::new("auth-named");
    sample.holding(HELD);
    let desk = desk(&sample, &[]);

    let absent = desk.status(Some("deepseek"), NOW);
    assert_eq!(absent.exit(), 1);
    let held = desk.status(Some("anthropic"), NOW);
    assert_eq!(held.exit(), 0);
    let document: Value = serde_json::from_slice(&held.json()).expect("a document");
    assert_eq!(
        at(&document, "/providers").as_array().map(Vec::len),
        Some(1)
    );
}

#[test]
fn a_store_that_cannot_be_read_leaves_every_provider_unverified() {
    let sample = Sample::new("auth-unreadable");
    sample.holding("{ not a store");
    let desk = desk(&sample, &[]);

    let status = desk.status(None, NOW);
    let document: Value = serde_json::from_slice(&status.json()).expect("a document");
    assert_eq!(at(&document, "/status"), "incomplete");
    assert!(at(&document, "/problem").is_string());
    let providers = at(&document, "/providers").as_array().expect("a list");
    assert!(
        providers
            .iter()
            .all(|entry| at(entry, "/state") == "unverified"),
        "{document}"
    );
    assert_eq!(status.exit(), 1);
    assert_ne!(State::Unverified.as_str(), State::Absent.as_str());
}

#[test]
fn a_failed_run_still_writes_one_document() {
    let text = String::from_utf8(failed(&Refused::Unconfigured)).expect("UTF-8");
    let document: Value = serde_json::from_str(&text).expect("one document");
    assert_eq!(at(&document, "/format_version"), 1);
    assert_eq!(at(&document, "/kind"), "auth-status");
    assert_eq!(at(&document, "/status"), "failed");
    assert_eq!(at(&document, "/providers"), &serde_json::json!([]));
    assert_eq!(at(&document, "/truncated"), false);
    assert!(at(&document, "/problem").is_string());
}

#[test]
fn a_key_is_kept_under_its_row_and_said_by_name_alone() {
    let sample = Sample::new("auth-keep");
    let desk = desk(&sample, &[]);
    let Ok(Login::Key(way)) = desk.login("anthropic", true) else {
        panic!("anthropic takes a key");
    };
    let secret = Secret::typed(SENTINEL).expect("a key");

    let kept_now = desk.keep(&way, &secret).expect("a writable home");
    let said = kept(&kept_now);
    assert!(said.contains("under anthropic"), "{said}");
    assert!(!said.contains(SENTINEL), "{said}");
    let text = fs::read_to_string(sample.home().join("auth.json")).expect("the store");
    assert!(text.contains(SENTINEL), "the key went to the store");

    let Ok(Login::Key(marked)) = desk.login("minimax@minimax.io", true) else {
        panic!("a MiniMax row takes a key");
    };
    let refused = desk
        .keep(&marked, &secret)
        .expect_err("its keys are marked otherwise");
    assert!(matches!(refused, Refused::Misfit(_)), "{refused}");
    assert!(!refused.to_string().contains(SENTINEL), "{refused}");
    let after = fs::read_to_string(sample.home().join("auth.json")).expect("the store");
    assert_eq!(after, text, "a key that does not fit is not stored");
}

#[test]
fn a_logout_takes_one_provider_out_and_says_what_a_launch_still_finds() {
    let sample = Sample::new("auth-logout");
    sample.holding(HELD);
    sample.user(r#"{"providers":{"moonshot":{"apiKeyEnv":"MY_MOONSHOT_KEY"}}}"#);
    let desk = desk(&sample, &[("MY_MOONSHOT_KEY", SENTINEL)]);

    let gone = desk.forget("moonshot").expect("a writable home");
    assert_eq!(
        gone.went,
        vec![("Kimi Code · kimi.ai", "moonshot@kimi.ai".to_owned())]
    );
    let said = forgotten(&gone);
    assert!(said.contains("MY_MOONSHOT_KEY"), "{said}");
    assert!(said.contains("unset it there"), "{said}");
    assert!(!said.contains(SENTINEL), "{said}");

    let text = fs::read_to_string(sample.home().join("auth.json")).expect("the store");
    let document: Value = serde_json::from_str(&text).expect("the store parses");
    assert_eq!(at(&document, "/keys/anthropic"), "fabricated-anthropic-key");
    assert_eq!(
        at(&document, "/keys/custom@example.com"),
        "fabricated-custom-key"
    );
    assert!(
        at(&document, "/subscriptions")
            .get("moonshot@kimi.ai")
            .is_none(),
        "{text}"
    );

    let again = desk.forget("moonshot").expect("a writable home");
    assert!(again.went.is_empty());
    assert!(forgotten(&again).contains("nothing was stored by Crucible for moonshot"));
}
