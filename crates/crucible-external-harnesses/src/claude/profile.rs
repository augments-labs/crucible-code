//! The environment the agent is started in.
//!
//! A launch's environment starts from nothing and holds only what is named
//! here: the few variables a program needs to run at all, taken from the
//! shell, and the profile's own. Every variable that would make the agent
//! bill a key or a cloud account instead of the signed-in plan is left out
//! because it is not named, so the account the agent uses is the one its
//! configuration directory holds. That directory is the instance's profile
//! where it names one, and the agent's usual one under the user's home where
//! it does not; the home itself is passed as the shell has it and never
//! replaced. The agent is also told to send nothing it does not need to answer
//! and to leave its own installation alone.

use std::ffi::OsString;
use std::path::Path;

/// The variables taken from the shell, which say where programs and the
/// user's own files are and how text is written, and nothing about any
/// account, so their values may be shown where what the agent says is kept.
pub(crate) const PASSED: [&str; 15] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "USER",
    "LOGNAME",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "SYSTEMDRIVE",
    "NUMBER_OF_PROCESSORS",
];

/// The environment for a launch whose configuration directory is `profile`,
/// where there is one, from the `ambient` one. None of its variables says
/// anything about an account, so each value may be shown.
///
/// On Windows a variable is matched whatever its case, as Windows matches it;
/// one whose name or value is not text is left out, since none of those named
/// is.
pub(crate) fn environment(
    ambient: impl IntoIterator<Item = (OsString, OsString)>,
    profile: Option<&Path>,
) -> Vec<(&'static str, String)> {
    let mut environment: Vec<(&'static str, String)> = ambient
        .into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter_map(|(name, value)| {
            let passed = PASSED.iter().find(|passed| named(&name, passed))?;
            Some((*passed, value))
        })
        .collect();
    environment.extend(
        [
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ("DISABLE_AUTOUPDATER", "1"),
        ]
        .map(|(name, value)| (name, value.to_owned())),
    );
    if let Some(profile) = profile {
        environment.push(("CLAUDE_CONFIG_DIR", profile.display().to_string()));
    }
    environment
}

/// Whether `name` is the variable `wanted`, as the platform compares names.
fn named(name: &str, wanted: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(wanted)
    } else {
        name == wanted
    }
}
