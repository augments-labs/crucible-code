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
    for key in Driver::ALL.into_iter().filter_map(Driver::profile_key) {
        assert!(others.declared(key).is_some(), "{key} is not declared");
    }
}

/// The key of a catalog agent, spelled `written`.
fn key(written: &str) -> AgentKey {
    AgentKey::new(written).expect("an agent key")
}

#[test]
fn a_catalog_instance_is_read_back_serving_the_agent_its_key_names() {
    let profile = rooted("home/me/goose");
    let read = settings(
        &serde_json::json!({"externalAgents": {
            "mine": {"driver": "catalog", "agent": "goose", "executable": "goose", "profileDir": profile},
            "work": {"driver": "claude", "executable": "claude"}
        }})
        .to_string(),
    );
    let agents = read.external_agents();
    let [mine, work] = agents.as_slice() else {
        panic!("two instances were written, got {agents:?}");
    };

    assert_eq!(mine.driver(), Driver::Catalog);
    assert_eq!(mine.agent(), Some(&key("goose")));
    assert_eq!(mine.serves(), Serves::Agent(key("goose")));
    assert_eq!(mine.profile(), Some(profile.as_str()));
    assert_eq!(work.agent(), None);
    assert_eq!(work.serves(), Serves::Provider("anthropic"));
}

/// A catalog entry that names no agent a key could be is one the document
/// check refused, so a reader that met it would be reading a document never
/// checked, and reads it as absent.
#[test]
fn a_catalog_entry_naming_no_agent_reads_as_absent() {
    for agent in [
        None,
        Some("Goose"),
        Some("claude"),
        Some("native"),
        Some("con"),
    ] {
        let mut record = serde_json::json!({"driver": "catalog", "executable": "goose"});
        if let (Some(agent), Some(fields)) = (agent, record.as_object_mut()) {
            fields.insert("agent".into(), agent.into());
        }
        let document = serde_json::json!({"externalAgents": {"mine": record}});
        let read = Settings {
            value: document,
            ..settings("{}")
        };
        assert!(read.external_agents().is_empty(), "{agent:?}");
    }
}

#[test]
fn each_driver_serves_its_provider_and_a_catalog_one_only_its_key() {
    let goose = key("goose");
    assert_eq!(
        Driver::Claude.serves(None),
        Some(Serves::Provider("anthropic"))
    );
    assert_eq!(
        Driver::Antigravity.serves(Some(&goose)),
        Some(Serves::Provider("google"))
    );
    assert_eq!(
        Driver::Catalog.serves(Some(&goose)),
        Some(Serves::Agent(goose.clone()))
    );
    assert_eq!(Driver::Catalog.serves(None), None);
    assert_eq!(Serves::Agent(goose.clone()).to_string(), "goose");
    assert_eq!(Serves::Provider("google").to_string(), "google");
}

/// A key that took a word a driver is spelled with, or the native runner's
/// name, would name a route, a session and credentials something else already
/// owns; every driver this build declares is walked, so a driver added later
/// is refused as a key the day it is added.
#[test]
fn no_driver_s_word_and_not_the_native_runner_s_name_is_an_agent_key() {
    for driver in Driver::ALL {
        assert!(reserved(driver.name()).is_some(), "{driver:?}");
        assert!(unkeyed(driver.name()).is_some(), "{driver:?}");
        assert_eq!(keyed(driver.name()), None, "{driver:?}");
    }
    assert!(reserved(NATIVE_RUNNER).is_some());
    for device in DEVICES {
        assert!(reserved(device).is_some(), "{device}");
    }
    assert_eq!(reserved("goose"), None);
    assert_eq!(keyed("goose"), Some(key("goose")));
}

/// The pattern an editor checks a catalog instance's `agent` against and the
/// key the reader takes are two spellings of one rule: the same alphabet and
/// the same length, published as the schema says it.
#[test]
fn the_published_agent_pattern_is_the_one_an_agent_key_takes() {
    let schema: Value = serde_json::from_str(&crate::schema()).expect("the schema is JSON");
    assert_eq!(
        schema.pointer("/properties/externalAgents/additionalProperties/properties/agent/pattern"),
        Some(&Value::from(AGENT))
    );
    assert_eq!(
        AGENT,
        format!("^[a-z0-9-]{{1,{}}}$", crucible_types::AGENT_KEY_BYTES)
    );
    for byte in 0_u8..=127 {
        let in_class = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        assert_eq!(
            AgentKey::new(&char::from(byte).to_string()).is_ok(),
            in_class,
            "{byte:#04x}"
        );
    }
}

#[test]
fn an_unwritten_instance_names_an_agent_exactly_when_it_is_a_catalog_one() {
    let goose = key("goose");
    let catalog = ExternalAgent::unwritten("goose", Driver::Catalog, "goose", None, Some(&goose))
        .expect("a catalog instance naming its agent");
    assert_eq!(catalog.serves(), Serves::Agent(goose.clone()));
    assert!(!catalog.enabled());
    assert!(ExternalAgent::unwritten("goose", Driver::Catalog, "goose", None, None).is_none());
    assert!(
        ExternalAgent::unwritten("claude", Driver::Claude, "claude", None, Some(&goose)).is_none()
    );
    assert_eq!(
        ExternalAgent::unwritten("claude", Driver::Claude, "claude", None, None)
            .map(|agent| agent.serves()),
        Some(Serves::Provider("anthropic"))
    );
}
