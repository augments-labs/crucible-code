//! The agent never sees a credential the user's shell holds for another use.

use std::ffi::OsString;
use std::path::Path;

use crate::antigravity::profile::{PASSED, environment};

/// Every variable that would choose the agent's account, project or home over
/// the profile's, as the published integration strips them.
const STRIPPED: [&str; 17] = [
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GOOGLE_GENAI_USE_VERTEXAI",
    "GCLOUD_PROJECT",
    "CLOUDSDK_CORE_PROJECT",
    "AGY_ACP_CCPA_PROJECT",
    "AGY_ACP_ENABLE_OAUTH",
    "GEMINI_HOME",
    "AGY_ACP_FORCE_FILE_STORAGE",
    "ANTIGRAVITY_HARNESS_PATH",
    "BROWSER",
    "PYTHONUNBUFFERED",
    "ELECTRON_RUN_AS_NODE",
];

/// The variables the profile sets itself, each to its own value.
const SET: [&str; 4] = [
    "GEMINI_HOME",
    "AGY_ACP_FORCE_FILE_STORAGE",
    "BROWSER",
    "PYTHONUNBUFFERED",
];

/// With every one of them set in the shell, none reaches the agent, and the
/// ones the profile needs hold the profile's values.
#[test]
fn no_ambient_google_credential_reaches_the_agent() {
    let ambient = STRIPPED
        .iter()
        .map(|name| {
            (
                OsString::from(name),
                OsString::from("ambient-sentinel-value"),
            )
        })
        .chain([
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            // Lower case is the same variable on Windows, and refused here
            // everywhere.
            (
                OsString::from("gemini_api_key"),
                OsString::from("ambient-sentinel-value"),
            ),
        ]);
    let profile = Path::new("/profiles/work");

    let environment = environment(ambient, Some(profile));

    for (name, value) in &environment {
        assert!(
            !value.contains("ambient-sentinel-value"),
            "{name} carried the shell's value"
        );
        assert!(
            !STRIPPED.contains(&name.to_uppercase().as_str()) || SET.contains(name),
            "{name} reached the agent"
        );
    }
    let value = |wanted: &str| {
        environment
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(value("GEMINI_HOME"), Some("/profiles/work"));
    assert_eq!(value("AGY_ACP_FORCE_FILE_STORAGE"), Some("1"));
    assert_eq!(value("PATH"), Some("/usr/bin"));
    assert!(value("BROWSER").is_some());
}

/// Every variable a launch can carry, each value of which is shown where what
/// the agent says is kept: a variable added to the launch is a value shown, so
/// it is named here too, where a reviewer reads it as one.
#[test]
fn every_variable_a_launch_can_carry_is_one_named_here() {
    let ambient = PASSED
        .iter()
        .chain(STRIPPED.iter())
        .map(|name| (OsString::from(name), OsString::from("set")));
    let mut names: Vec<&str> = environment(ambient, Some(Path::new("/profiles/work")))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.sort_unstable();

    assert_eq!(
        names,
        [
            "AGY_ACP_FORCE_FILE_STORAGE",
            "AGY_CLI_DISABLE_AUTO_UPDATE",
            "BROWSER",
            "COMSPEC",
            "GEMINI_HOME",
            "HOME",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "LOGNAME",
            "NUMBER_OF_PROCESSORS",
            "PATH",
            "PATHEXT",
            "PYTHONUNBUFFERED",
            "SYSTEMDRIVE",
            "SYSTEMROOT",
            "TEMP",
            "TMP",
            "TMPDIR",
            "TZ",
            "USER",
            "USERPROFILE",
            "WINDIR",
        ]
    );
}
