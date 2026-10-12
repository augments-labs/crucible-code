//! The environment the agent is started in.
//!
//! A launch's environment starts from nothing and holds only what is named
//! here: the few variables a program needs to run at all, taken from the
//! shell, and the profile's own. The agent reads its account, project and home
//! from variables a shell may hold for another use, and none of those is
//! named, so the agent signs in, keeps its files and unpacks itself inside the
//! profile alone: its home for sign-in is the profile's directory, and its
//! token is a file there rather than an entry in the shared keyring, so a
//! sign-in another Google product keeps is never read as its own.

use std::ffi::OsString;
use std::path::Path;

/// The variables taken from the shell, which say where programs and the
/// user's own files are and how text is written, and nothing about any
/// account.
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

/// The program beside the agent's executable that runs its tools.
#[cfg(windows)]
const HARNESS: &str = "localharness_external.exe";
#[cfg(not(windows))]
const HARNESS: &str = "localharness_external";

/// The environment for a launch of `executable` in `profile`, from the
/// `ambient` one.
///
/// On Windows a variable is matched whatever its case, as Windows matches it;
/// one whose name or value is not text is left out, since none of those named
/// is.
pub(crate) fn environment(
    ambient: impl IntoIterator<Item = (OsString, OsString)>,
    profile: &Path,
    executable: Option<&Path>,
) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = ambient
        .into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter_map(|(name, value)| {
            let passed = PASSED.iter().find(|passed| named(&name, passed))?;
            Some(((*passed).to_owned(), value))
        })
        .collect();
    let home = profile.display().to_string();
    environment.extend(
        [
            ("GEMINI_HOME", home.as_str()),
            ("AGY_ACP_FORCE_FILE_STORAGE", "1"),
            ("AGY_CLI_DISABLE_AUTO_UPDATE", "1"),
            ("BROWSER", NO_BROWSER),
            ("PYTHONUNBUFFERED", "1"),
        ]
        .into_iter()
        .chain(TEMPORARY.map(|name| (name, home.as_str())))
        .map(|(name, value)| (name.to_owned(), value.to_owned())),
    );
    if let Some(harness) = executable
        .and_then(Path::parent)
        .map(|directory| directory.join(HARNESS))
    {
        environment.push((
            "ANTIGRAVITY_HARNESS_PATH".to_owned(),
            harness.display().to_string(),
        ));
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
