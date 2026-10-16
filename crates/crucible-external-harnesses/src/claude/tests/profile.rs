//! The agent never sees a credential the user's shell holds for another use.

use std::ffi::OsString;
use std::path::Path;

use crate::claude::profile::environment;

/// Every variable that would choose the agent's account, endpoint or
/// configuration over the profile's.
const STRIPPED: [&str; 18] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_MANTLE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CLAUDE_CONFIG_DIR",
    "HTTPS_PROXY",
    "NODE_OPTIONS",
];

/// The shell's environment, with each stripped variable set to a sentinel.
fn ambient() -> Vec<(OsString, OsString)> {
    STRIPPED
        .iter()
        .map(|name| (OsString::from(name), OsString::from("ambient-sentinel")))
        .chain([
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("HOME"), OsString::from("/home/user")),
        ])
        .collect()
}

/// The value `name` holds in `environment`, where it is set once.
fn value<'a>(environment: &'a [(&str, String)], name: &str) -> Option<&'a str> {
    let mut held = environment.iter().filter(|(set, _)| *set == name);
    let first = held.next().map(|(_, value)| value.as_str());
    assert!(held.next().is_none(), "{name} is set twice");
    first
}

#[test]
fn no_ambient_account_or_endpoint_reaches_the_agent() {
    let environment = environment(ambient(), Some(Path::new("/profiles/work")));

    for name in STRIPPED {
        let held = value(&environment, name);
        assert!(held != Some("ambient-sentinel"), "{name} reached the agent");
    }
    assert!(
        environment
            .iter()
            .all(|(_, value)| value != "ambient-sentinel"),
        "{environment:?}"
    );
}

/// The profile is the agent's configuration directory, and the home is the
/// shell's, never replaced.
#[test]
fn the_profile_is_the_configuration_directory_and_the_home_is_untouched() {
    let environment = environment(ambient(), Some(Path::new("/profiles/work")));

    assert_eq!(
        value(&environment, "CLAUDE_CONFIG_DIR"),
        Some("/profiles/work")
    );
    assert_eq!(value(&environment, "HOME"), Some("/home/user"));
    assert_eq!(value(&environment, "PATH"), Some("/usr/bin"));
    assert_eq!(
        value(&environment, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        Some("1")
    );
    assert_eq!(value(&environment, "DISABLE_AUTOUPDATER"), Some("1"));
}

/// With no profile the agent uses its usual configuration, and still none of
/// the shell's.
#[test]
fn without_a_profile_the_configuration_directory_is_the_agents_own() {
    let environment = environment(ambient(), None);

    assert_eq!(value(&environment, "CLAUDE_CONFIG_DIR"), None);
    assert_eq!(value(&environment, "HOME"), Some("/home/user"));
}
