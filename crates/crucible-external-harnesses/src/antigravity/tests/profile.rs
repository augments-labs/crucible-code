//! The agent never sees a credential the user's shell holds for another use,
//! and the settings a business sign-in is written stay in its profile.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::antigravity::business;
use crate::antigravity::profile::environment;
use crate::catalog::PASSED;
use crate::tests::Owned;

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

/// The file in `profile` the agent reads a business sign-in's project and
/// location from.
fn settings_in(profile: &Path) -> PathBuf {
    profile.join("antigravity-acp").join("settings.json")
}

/// The names in `directory`, sorted.
fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_unstable();
    names
}

/// A business sign-in's settings name its method, project and location and
/// nothing else, whatever the profile held there before, and no draft is
/// left beside them.
#[test]
fn a_business_sign_in_writes_exactly_its_project_and_location_in_the_profile() {
    let profile = Owned::new("agy-business");

    business(&profile.0, "my-project-id", "us-central1").expect("the settings written");
    assert_eq!(
        fs::read_to_string(settings_in(&profile.0)).expect("the settings read"),
        r#"{"auth":{"type":"oauth-business"},"gcp":{"project":"my-project-id","location":"us-central1"}}"#
    );

    fs::write(
        settings_in(&profile.0),
        r#"{"auth":{"type":"oauth-personal"},"ui":{"theme":"dark"}}"#,
    )
    .expect("the agent's own settings");
    business(&profile.0, "another-project", "eu").expect("the settings written again");
    assert_eq!(
        fs::read_to_string(settings_in(&profile.0)).expect("the settings read"),
        r#"{"auth":{"type":"oauth-business"},"gcp":{"project":"another-project","location":"eu"}}"#
    );
    assert_eq!(names(&profile.0.join("antigravity-acp")), ["settings.json"]);
}

/// Something other than a directory where the agent keeps its settings is
/// refused rather than replaced.
#[test]
fn a_file_where_the_settings_directory_belongs_refuses_the_write() {
    let profile = Owned::new("agy-business-file");
    fs::write(profile.0.join("antigravity-acp"), "kept").expect("a file");

    assert!(business(&profile.0, "my-project-id", "us-central1").is_err());
    assert_eq!(
        fs::read_to_string(profile.0.join("antigravity-acp")).expect("the file read"),
        "kept"
    );
}

/// A link at either name refuses the write, and nothing is written where it
/// leads or beside it.
#[cfg(unix)]
#[test]
fn a_link_at_either_name_refuses_the_write_and_writes_nothing_where_it_leads() {
    use std::os::unix::fs::symlink;

    let outside = Owned::new("agy-business-outside");
    let linked = Owned::new("agy-business-linked-directory");
    symlink(&outside.0, linked.0.join("antigravity-acp")).expect("a link to a directory");
    assert!(business(&linked.0, "my-project-id", "us-central1").is_err());
    assert!(names(&outside.0).is_empty(), "something was written through the link");

    let kept = outside.0.join("settings.json");
    fs::write(&kept, "kept").expect("a file outside");
    let linked = Owned::new("agy-business-linked-file");
    fs::create_dir(linked.0.join("antigravity-acp")).expect("the settings directory");
    symlink(&kept, settings_in(&linked.0)).expect("a link to a file");
    assert!(business(&linked.0, "my-project-id", "us-central1").is_err());
    assert_eq!(fs::read_to_string(&kept).expect("the file read"), "kept");
    assert!(
        fs::symlink_metadata(settings_in(&linked.0))
            .expect("the link")
            .file_type()
            .is_symlink(),
        "the link was replaced"
    );
    assert_eq!(names(&linked.0.join("antigravity-acp")), ["settings.json"]);
}
