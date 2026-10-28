//! The environment the agent is started in.
//!
//! A launch's environment starts from nothing and holds only what is named
//! here: the few variables a program needs to run at all, taken from the
//! shell as they are for every agent the catalog reviewed, and the profile's
//! own. The agent reads its account, project and home
//! from variables a shell may hold for another use, and none of those is
//! named, so the agent signs in, keeps its files and unpacks itself inside the
//! profile alone: its home for sign-in is the profile's directory, and its
//! token is a file there rather than an entry in the shared keyring, so a
//! sign-in another Google product keeps is never read as its own. The
//! configuration refuses an instance without a profile, since its usual home
//! is the one the Antigravity IDE signs in under; a launch given none keeps its
//! files there, its token still in a file.
//!
//! A business sign-in is billed to a Google Cloud project in a location,
//! which the agent reads from its own settings file in the profile and from
//! no variable. [`business`] writes that file before the agent is started for
//! the sign-in, holding the method, project and location and nothing else,
//! and follows no link at the settings directory or the file, so the write
//! never leaves the profile.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::OAUTH_BUSINESS;
use crate::catalog::passed;

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
pub(crate) fn environment(
    ambient: impl IntoIterator<Item = (OsString, OsString)>,
    profile: Option<&Path>,
) -> Vec<(&'static str, String)> {
    let mut environment = passed(ambient);
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

/// The directory in the profile the agent keeps its settings in.
const SETTINGS_DIRECTORY: &str = "antigravity-acp";

/// The agent's settings file in [`SETTINGS_DIRECTORY`].
const SETTINGS: &str = "settings.json";

/// Writes the settings a business sign-in in `profile` is made with: the
/// method, and the Google Cloud `project` and `location` it is billed to,
/// which the agent reads from this file alone. The file holds that and
/// nothing else, replacing whatever the agent kept there, and none of it is a
/// credential.
///
/// The write follows no link and leaves the profile at neither name: a link
/// at the settings directory or at the file refuses it before anything is
/// written, and the file is replaced through a draft beside it, so a name
/// that turned into a link meanwhile is replaced rather than written through.
///
/// # Errors
///
/// [`io::Error`] where either name is a link, the settings directory cannot
/// be made one, or the file cannot be replaced.
pub fn business(profile: &Path, project: &str, location: &str) -> io::Result<()> {
    let directory = profile.join(SETTINGS_DIRECTORY);
    let file = directory.join(SETTINGS);
    for name in [&directory, &file] {
        match fs::symlink_metadata(name) {
            Ok(found) if found.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a link stands where the agent keeps its settings, and they are written only inside its profile",
                ));
            }
            Ok(_) => {}
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => {}
            Err(problem) => return Err(problem),
        }
    }
    crucible_privacy::directory(&directory).map_err(crucible_privacy::PrivacyError::into_io)?;
    // Each value spelled by the JSON writer, and the keys in the order the
    // agent's own settings name them.
    let settings = format!(
        r#"{{"auth":{{"type":{}}},"gcp":{{"project":{},"location":{}}}}}"#,
        Value::from(OAUTH_BUSINESS),
        Value::from(project),
        Value::from(location),
    );
    replace(&directory, &file, settings.as_bytes())
}

/// Replaces `file` in `directory` with `bytes`, through a draft of its own
/// beside it that is removed unless it replaced the file.
fn replace(directory: &Path, file: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let draft = directory.join(format!(".settings.{}.{next}.writing", std::process::id()));
        let mut written = match crucible_privacy::create_write(&draft) {
            Ok(written) => written,
            Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(problem) => return Err(problem.into_io()),
        };
        let synced = written.write_all(bytes).and_then(|()| written.sync_all());
        drop(written);
        let landed = synced.and_then(|()| {
            crucible_privacy::replace(&draft, file).map_err(crucible_privacy::PrivacyError::into_io)
        });
        if landed.is_err() {
            // A draft left behind is one the agent never reads.
            let _ = fs::remove_file(&draft);
        }
        return landed;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name for the agent's settings draft",
    ))
}
