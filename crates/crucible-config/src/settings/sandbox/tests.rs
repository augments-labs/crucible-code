//! The opt-in switch and the authority of each configuration file.

use crate::document::{Document, Origin};
use crate::{ConfigError, Settings};

#[test]
fn sandbox_mode_is_an_unknown_key_in_every_configuration_layer() {
    for origin in [Origin::User, Origin::Project, Origin::ProjectLocal] {
        for mode in [r#""required""#, r#""degraded""#, r#""off""#, "true", "null"] {
            for enabled in ["", r#""enabled":true,"#, r#""enabled":false,"#] {
                let text = format!(r#"{{"sandbox":{{{enabled}"mode":{mode}}}}}"#);
                let error = Document::parse(&text, "config.json", origin)
                    .expect_err("sandbox.enabled is the only confinement setting");
                assert!(
                    matches!(&error, ConfigError::UnknownKey { path, .. } if &**path == "sandbox.mode"),
                    "{origin:?}: {text}: {error}"
                );
                assert!(error.to_string().contains("enabled"), "{error}");
            }
        }
    }
}

#[test]
fn no_sandbox_choice_leaves_os_confinement_off() {
    assert!(!Settings::default().sandbox_enabled());
    assert!(!Settings::resolve(Vec::new()).sandbox_enabled());
    for text in [
        r"{}",
        r#"{"sandbox":{}}"#,
        r#"{"sandbox":{"$comment":"inert"}}"#,
    ] {
        for origin in [Origin::User, Origin::Project, Origin::ProjectLocal] {
            assert!(!Settings::resolve(vec![Document::sample(text, origin)]).sandbox_enabled());
        }
    }
}

#[test]
fn a_user_can_enable_required_confinement_or_explicitly_disable_it() {
    for (text, expected) in [
        (r#"{"sandbox":{"enabled":true}}"#, true),
        (r#"{"sandbox":{"enabled":false}}"#, false),
        (
            r#"{"sandbox":{"enabled":true,"$comment":"explicit","$schema":"https://example.test/schema"}}"#,
            true,
        ),
    ] {
        assert_eq!(
            Settings::resolve(vec![Document::sample(text, Origin::User)]).sandbox_enabled(),
            expected
        );
    }
}

#[test]
fn enabling_does_not_accept_a_string_number_or_null() {
    for value in [r#""true""#, "1", "null", "[]", "{}"] {
        let text = format!(r#"{{"sandbox":{{"enabled":{value}}}}}"#);
        assert!(
            matches!(Document::parse(&text, "config.json", Origin::User), Err(ConfigError::WrongType { path, .. }) if &*path == "sandbox.enabled")
        );
    }
}

#[test]
fn a_workspace_cannot_disable_confinement() {
    for origin in [Origin::Project, Origin::ProjectLocal] {
        assert!(matches!(
            Document::parse(r#"{"sandbox":{"enabled":false}}"#, "config.json", origin),
            Err(ConfigError::Widening { path, .. }) if &*path == "sandbox.enabled"
        ));
    }
}

#[test]
fn workspace_requirement_wins_regardless_of_document_order() {
    for origin in [Origin::Project, Origin::ProjectLocal] {
        let user = Document::sample(r#"{"sandbox":{"enabled":false}}"#, Origin::User);
        let project = Document::sample(r#"{"sandbox":{"enabled":true}}"#, origin);
        for documents in [
            vec![user.clone(), project.clone()],
            vec![project.clone(), user.clone()],
        ] {
            assert!(Settings::resolve(documents).sandbox_enabled());
        }
    }
}

#[test]
fn filesystem_and_command_limits_are_real_configuration_settings() {
    let text = r#"{
        "sandbox": {
            "enabled": true,
            "filesystem": {
                "writable": ["build"],
                "readOnly": ["vendor"],
                "unreadable": ["secrets"],
                "protected": ["policy.json"]
            },
            "limits": {
                "commandSeconds": 1200,
                "outputBytes": 10485760,
                "concurrentCommands": 4
            }
        }
    }"#;
    let document = Document::parse(text, "config.json", Origin::User)
        .expect("filesystem grants and command ceilings must be accepted");
    assert!(Settings::resolve(vec![document]).sandbox_enabled());
}

#[test]
fn command_limits_have_defaults_and_projects_may_only_lower_them() {
    let workspace = crucible_core::Workspace::open(env!("CARGO_MANIFEST_DIR")).unwrap();
    let defaults = Settings::default().sandbox().policy(&workspace).unwrap();
    assert_eq!(
        defaults.limits().command_time,
        Some(std::time::Duration::from_mins(20))
    );
    assert_eq!(defaults.limits().output_bytes, Some(10_485_760));
    assert_eq!(defaults.limits().concurrent_commands, Some(4));
    for (name, lower, higher) in [
        ("commandSeconds", 10, 11),
        ("outputBytes", 64, 65),
        ("concurrentCommands", 1, 2),
    ] {
        let user = Document::sample(
            &format!(r#"{{"sandbox":{{"limits":{{"{name}":{lower}}}}}}}"#),
            Origin::User,
        );
        for origin in [Origin::Project, Origin::ProjectLocal] {
            let project = Document::sample(
                &format!(r#"{{"sandbox":{{"limits":{{"{name}":{higher}}}}}}}"#),
                origin,
            );
            assert!(matches!(
                Settings::resolve_checked(vec![project, user.clone()]),
                Err(ConfigError::Sandbox { .. })
            ));
        }
    }
    let project = Document::sample(
        r#"{"sandbox":{"limits":{"commandSeconds":30,"outputBytes":100,"concurrentCommands":1}}}"#,
        Origin::Project,
    );
    let settings = Settings::resolve(vec![project]);
    let policy = settings.sandbox().policy(&workspace).unwrap();
    assert!(!policy.enabled());
    assert!(!settings.sandbox().required_by_project());
    assert_eq!(policy.limits().output_bytes, Some(100));
}

#[test]
fn path_lists_and_limit_values_are_bounded_and_not_coerced() {
    for (name, maximum) in [
        ("commandSeconds", 86400_u64),
        ("outputBytes", 67_108_864),
        ("concurrentCommands", 16),
    ] {
        for value in [
            "0".to_string(),
            (maximum + 1).to_string(),
            "-1".into(),
            "1.5".into(),
            "null".into(),
            "\"1\"".into(),
        ] {
            let text = format!(r#"{{"sandbox":{{"limits":{{"{name}":{value}}}}}}}"#);
            assert!(
                Document::parse(&text, "test", Origin::User).is_err(),
                "{text}"
            );
        }
    }
    for paths in [
        serde_json::json!([""]),
        serde_json::json!(["../outside"]),
        serde_json::json!(["private/../../outside"]),
        serde_json::json!(["nul\0path"]),
        serde_json::json!([true]),
        serde_json::json!(vec!["path"; 129]),
    ] {
        let text = serde_json::json!({"sandbox":{"filesystem":{"writable":paths}}}).to_string();
        assert!(Document::parse(&text, "test", Origin::User).is_err());
    }
}

#[test]
fn project_files_cannot_grant_writable_paths_even_when_confinement_is_disabled() {
    for origin in [Origin::Project, Origin::ProjectLocal] {
        assert!(
            Document::parse(
                r#"{"sandbox":{"filesystem":{"writable":["build"]}}}"#,
                "test",
                origin
            )
            .is_err()
        );
    }
}

#[test]
fn filesystem_policy_preserves_restrictions_and_their_sources() {
    use crucible_core::{
        SandboxFilesystemAccess as Access, SandboxFilesystemProvenance as Provenance,
    };
    let workspace = crucible_core::Workspace::open(env!("CARGO_MANIFEST_DIR")).unwrap();
    let user = Document::sample(
        r#"{"sandbox":{"enabled":true,"filesystem":{"readOnly":["vendor"],"writable":["vendor/build"],"protected":["policy.json"],"unreadable":["private"]}}}"#,
        Origin::User,
    );
    let project = Document::sample(
        r#"{"sandbox":{"filesystem":{"readOnly":["vendor"]}}}"#,
        Origin::Project,
    );
    let settings = Settings::resolve(vec![project, user]);
    let policy = settings.sandbox().policy(&workspace).unwrap();
    for (path, access, source) in [
        ("vendor", Access::ReadOnly, Provenance::ProjectConfiguration),
        (
            "vendor/build",
            Access::ReadOnly,
            Provenance::ProjectConfiguration,
        ),
        (
            "policy.json",
            Access::Protected,
            Provenance::UserConfiguration,
        ),
        ("private", Access::Unreadable, Provenance::UserConfiguration),
    ] {
        let rule = policy
            .filesystem()
            .iter()
            .find(|rule| rule.path() == workspace.root().join(path))
            .unwrap();
        assert_eq!((rule.access(), rule.provenance()), (access, source));
    }
    assert!(!format!("{:?}", settings.sandbox()).contains("policy.json"));
}

#[test]
fn a_project_read_rule_cannot_create_access_to_another_directory() {
    let workspace = crucible_core::Workspace::open(env!("CARGO_MANIFEST_DIR")).unwrap();
    let outside = workspace.root().parent().unwrap();
    for access in ["readOnly", "protected"] {
        let text = serde_json::json!({"sandbox":{"filesystem":{access:[outside]}}}).to_string();
        let document = Document::sample(&text, Origin::Project);
        assert!(
            Settings::resolve(vec![document])
                .sandbox()
                .policy(&workspace)
                .is_err()
        );
    }
}

#[test]
fn domain_configuration_narrows_user_grants_and_retains_denies() {
    let user = Document::parse(r#"{"sandbox":{"network":{"allowedDomains":["*.example.com"],"deniedDomains":["blocked.example.com"],"allowLocalBinding":true,"allowUnixSockets":["service.sock"]}}}"#, "user.json", Origin::User).unwrap();
    let project = Document::parse(r#"{"sandbox":{"network":{"allowedDomains":["build.example.com"],"deniedDomains":["extra.example.com"],"allowLocalBinding":false,"allowUnixSockets":[]}}}"#, "project.json", Origin::Project).unwrap();
    let settings = Settings::resolve_checked(vec![project, user.clone()]).unwrap();
    let workspace = crucible_core::Workspace::open(env!("CARGO_MANIFEST_DIR")).unwrap();
    let effective = settings.sandbox().enforcing_policy(&workspace).unwrap();
    let crucible_core::SandboxNetworkPolicy::Domains(network) = effective.network() else {
        panic!("domain policy expected")
    };
    assert!(network.permits_host("build.example.com"));
    assert!(!network.permits_host("other.example.com"));
    assert_eq!(network.denied().len(), 2);
    assert!(!network.allow_local_binding());
    assert!(network.unix_sockets().is_empty());
    for block in [
        r#""allowedDomains":["*"]"#,
        r#""allowUnixSockets":["other.sock"]"#,
    ] {
        let project = Document::sample(
            &format!(r#"{{"sandbox":{{"network":{{{block}}}}}}}"#),
            Origin::Project,
        );
        assert!(Settings::resolve_checked(vec![user.clone(), project]).is_err());
    }
    let project = Document::sample(
        r#"{"sandbox":{"network":{"allowLocalBinding":true}}}"#,
        Origin::Project,
    );
    assert!(Settings::resolve_checked(vec![project]).is_err());
}

#[test]
fn network_lists_reject_malformed_unbounded_and_duplicate_values() {
    for value in [
        serde_json::json!(["http://example.com"]),
        serde_json::json!(["a.*.test"]),
        serde_json::json!(["a.test", "a.test"]),
        serde_json::json!(vec!["a.test"; 65]),
    ] {
        let text = serde_json::json!({"sandbox":{"network":{"allowedDomains":value}}}).to_string();
        assert!(Document::parse(&text, "user.json", Origin::User).is_err());
    }
    let text = r#"{"sandbox":{"network":{"allowUnixSockets":["../socket"]}}}"#;
    assert!(Document::parse(text, "user.json", Origin::User).is_err());
}
