//! The environment the agent is started in.
//!
//! A launch's environment starts from nothing and holds only what is named
//! here: the few variables a program needs to run at all, taken from the
//! shell, and the profile's own. The agent reads its account, project and home
//! from variables a shell may hold for another use, and none of those is
//! named, so the agent signs in, keeps its files and unpacks itself inside the
//! profile alone: its home for sign-in is the profile's directory, and its
//! token is a file there rather than an entry in the shared keyring, so a
//! sign-in another Google product keeps is never read as its own. The
//! configuration refuses an instance without a profile, since its usual home
//! is the one the Antigravity IDE signs in under; a launch given none keeps its
//! files there, its token still in a file.

use std::ffi::OsString;
use std::path::Path;

/// The variables taken from the shell, which say where programs and the
/// user's own files are and how text is written, and nothing about any
/// account, so their values may be shown where what the agent says is kept.
const PASSED: [&str; 15] = [
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

/// The variables naming a directory for temporary files, which the agent
/// unpacks itself into: the profile's, so what a stopped agent leaves behind
/// stays where its own files are.
const TEMPORARY: [&str; 3] = ["TMPDIR", "TEMP", "TMP"];

/// The command the agent is given to open a link with, which opens nothing,
/// so the link is shown to the user instead.
#[cfg(windows)]
const NO_BROWSER: &str = "cmd.exe /c rem";
#[cfg(not(windows))]
const NO_BROWSER: &str = "true";

/// The environment for a launch in `profile`, where there is one, from the
/// `ambient` one. None of its variables says anything about an account, so
/// each value may be shown.
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
            ("AGY_ACP_FORCE_FILE_STORAGE", "1"),
            ("AGY_CLI_DISABLE_AUTO_UPDATE", "1"),
            ("BROWSER", NO_BROWSER),
            ("PYTHONUNBUFFERED", "1"),
        ]
        .map(|(name, value)| (name, value.to_owned())),
    );
    if let Some(profile) = profile {
        let home = profile.display().to_string();
        environment.extend(
            ["GEMINI_HOME"]
                .into_iter()
                .chain(TEMPORARY)
                .map(|name| (name, home.clone())),
        );
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
