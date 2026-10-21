use crate::document::{Document, Origin};
use crate::sample::rooted;
use crate::shape;

use super::*;

fn settings(text: &str) -> Settings {
    Settings::resolve(vec![Document::sample(text, Origin::User)])
}

#[test]
fn each_access_type_is_read_back_as_its_variant() {
    let read = settings(
        r#"{"providers": {
            "openai": {"access": {"type": "subscription"}},
            "google": {"access": {"type": "apiKey"}},
            "anthropic": {"access": {"type": "externalAgent", "instance": "personal"}}
        }}"#,
    );
    assert_eq!(read.access("openai"), Some(Access::Subscription));
    assert_eq!(read.access("google"), Some(Access::ApiKey));
    assert_eq!(
        read.access("anthropic"),
        Some(Access::ExternalAgent {
            instance: "personal".into()
        })
    );
}

#[test]
fn routes_stand_in_for_both_blocks_the_launch_read_and_keep_the_rest() {
    let read = settings(
        r#"{"providers": {
            "anthropic": {"access": {"type": "externalAgent", "instance": "work"}, "model": "claude-opus-5-5"},
            "openai": {"access": {"type": "subscription"}}
        },
        "externalAgents": {"work": {"driver": "claude", "executable": "claude"}}}"#,
    );
    let routes = Routes::of(
        r#"{"providers": {
            "anthropic": {"access": {"type": "apiKey"}},
            "google": {"access": {"type": "externalAgent", "instance": "home"}}
        },
        "externalAgents": {"home": {"driver": "claude", "executable": "claude", "enabled": false}}}"#,
        "config.json",
    )
    .expect("a user file");

    let written = read.routed(&routes);

    assert_eq!(written.access("anthropic"), Some(Access::ApiKey));
    assert_eq!(written.access("openai"), None);
    assert_eq!(
        written.access("google"),
        Some(Access::ExternalAgent {
            instance: "home".into()
        })
    );
    assert_eq!(written.model("anthropic"), Some("claude-opus-5-5"));
    let agents = written.external_agents();
    assert_eq!(
        agents.iter().map(ExternalAgent::name).collect::<Vec<_>>(),
        ["home"]
    );
    assert!(!agents.iter().any(ExternalAgent::enabled));
    let bare = read.routed(&Routes::of("", "config.json").expect("an empty user file"));
    assert_eq!(bare.access("anthropic"), None);
    assert!(bare.external_agents().is_empty());
    assert_eq!(bare.model("anthropic"), Some("claude-opus-5-5"));
    assert_eq!(read.access("openai"), Some(Access::Subscription));
    assert!(Routes::of("{", "config.json").is_err());
}

#[test]
fn a_provider_whose_block_says_nothing_has_no_access() {
    let read = settings(r#"{"providers": {"openai": {"model": "gpt-5"}}}"#);
    assert_eq!(read.access("openai"), None);
    assert_eq!(read.access("anthropic"), None);
    assert_eq!(settings("{}").access("openai"), None);
}

#[test]
fn an_instance_is_read_back_with_its_drivers_profile() {
    let profile = rooted("home/me/.claude-personal");
    let other = rooted("home/me/agy");
    let read = settings(
        &serde_json::json!({"externalAgents": {
            "personal": {"driver": "claude", "executable": "claude", "configDir": profile},
            "work": {"enabled": false, "driver": "antigravity", "executable": "agy", "profileDir": other}
        }})
        .to_string(),
    );
    let agents = read.external_agents();
    let [personal, work] = agents.as_slice() else {
        panic!("two instances were written, got {agents:?}");
    };

    assert_eq!(personal.name(), "personal");
    assert!(
        personal.enabled(),
        "enabled is true where it is not written"
    );
    assert_eq!(personal.driver(), Driver::Claude);
    assert_eq!(personal.executable(), "claude");
    assert_eq!(personal.profile(), Some(profile.as_str()));
    assert_eq!(personal.model(), None, "the agent's own default");

    assert_eq!(work.name(), "work");
    assert!(!work.enabled());
    assert_eq!(work.driver(), Driver::Antigravity);
    assert_eq!(work.profile(), Some(other.as_str()));
}

#[test]
fn an_instance_is_read_back_with_the_model_it_is_started_with() {
    let read = settings(
        &serde_json::json!({"externalAgents": {
            "personal": {"driver": "claude", "executable": "claude", "model": "opus"}
        }})
        .to_string(),
    );
    let agents = read.external_agents();
    let [personal] = agents.as_slice() else {
        panic!("one instance was written, got {agents:?}");
    };

    assert_eq!(personal.model(), Some("opus"));
}

#[test]
fn a_machine_with_no_instances_reads_none() {
    assert!(settings("{}").external_agents().is_empty());
}

#[test]
fn the_spellings_the_shape_accepts_are_the_ones_the_types_read() {
    // The shape's lists are built from these types' names, so this pins the
    // count: a variant added to a type and not to its list is a value the
    // schema refuses and the reader would have read.
    assert_eq!(KINDS.len(), Kind::ALL.len());
    for kind in Kind::ALL {
        assert!(KINDS.contains(&kind.name()), "{}", kind.name());
    }
    assert_eq!(DRIVERS.len(), Driver::ALL.len());
    for driver in Driver::ALL {
        assert!(DRIVERS.contains(&driver.name()), "{}", driver.name());
    }
}

#[test]
fn every_drivers_profile_key_is_declared_for_an_instance() {
    let shape::Shape::Fields(document) = &shape::DOCUMENT else {
        panic!("the document is a block of fields");
    };
    let agents = document
        .iter()
        .find(|field| field.name == "externalAgents")
        .expect("externalAgents is declared");
    let shape::Shape::Named { others, .. } = &agents.shape else {
        panic!("externalAgents is keyed by instance");
    };
    for driver in Driver::ALL {
        assert!(
            others.declared(driver.profile_key()).is_some(),
            "{} is not declared",
            driver.profile_key()
        );
    }
}
