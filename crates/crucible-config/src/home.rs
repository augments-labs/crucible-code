//! Where crucible keeps its own files.
//!
//! One directory, holding the user-level configuration file and the session
//! logs. One rather than two because the split it replaces was Linux-specific:
//! a data directory under `$XDG_DATA_HOME` means nothing on Windows and is the
//! wrong place on macOS, so a single dot-directory beside the user's other tool
//! directories is the more portable answer and the easier one to explain.
//!
//! Resolved from the environment and then handed down as a path. Nothing below
//! this crate asks where anything is.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// The variable that moves the whole directory.
///
/// Read before any configuration file is opened — it is what says where the
/// user's file *is* — which makes it the one setting of crucible's own that a
/// file cannot carry. [`crate::env::too_late`] is what refuses it there, rather
/// than letting it be written somewhere it would quietly never apply.
pub const HOME: &str = "CRUCIBLE_CODE_HOME";

/// What the directory is called, under the user's home.
const DIRECTORY: &str = ".crucible";

/// Where session logs live inside it.
const SESSIONS: &str = "sessions";

/// Where crucible keeps its own files.
#[derive(Debug, Clone)]
pub struct Home {
    /// The directory itself.
    path: PathBuf,
    /// Where session logs are, which is not always inside it — see [`older`].
    sessions: PathBuf,
}

impl Home {
    /// Finds it, from the environment.
    ///
    /// `from` is a parameter rather than `std::env` read in here, so that a
    /// test can hand it an environment it is also allowed to delete.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Homeless`] when neither variable names an absolute path,
    /// which is the one case where guessing would scatter files somewhere the
    /// user would never find them.
    pub fn find(from: &dyn Fn(&str) -> Option<OsString>) -> Result<Self, ConfigError> {
        // Named outright: everything is under it and nothing outside it is even
        // looked at. This is how a container, a test or a benchmark probe says
        // "keep it all here", and a fallback that could still send writes to the
        // real home directory would break that promise where it matters most.
        // It is also the path that touches no disk at all.
        if let Some(named) = absolute(from, HOME) {
            let sessions = named.join(SESSIONS);
            return Ok(Self {
                path: named,
                sessions,
            });
        }

        let path = user(from)
            .map(|home| home.join(DIRECTORY))
            .ok_or(ConfigError::Homeless { named: HOME })?;

        let here = path.join(SESSIONS);
        let sessions = if here.is_dir() {
            here
        } else {
            older(from).filter(|tree| tree.is_dir()).unwrap_or(here)
        };

        Ok(Self { path, sessions })
    }

    /// The directory itself.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where session logs are kept.
    #[must_use]
    pub fn sessions(&self) -> &Path {
        &self.sessions
    }
}

/// Where sessions were kept before they moved into the home directory.
///
/// A tree that is already there is read where it is — never copied, moved or
/// deleted. Copying doubles the disk it takes and moving breaks `--continue`
/// for anyone who upgrades in the middle of a piece of work, and both are
/// irreversible operations on files the user is told are theirs. So the older
/// tree simply keeps being used, and only a machine that has never run crucible
/// before starts out in the new place.
///
/// This is the layout crucible shipped through `0.0.2`, kept here exactly as it
/// was so that a tree written by that version is the tree this finds.
fn older(from: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    absolute(from, "XDG_DATA_HOME")
        .map(|data| data.join("crucible").join(SESSIONS))
        .or_else(|| {
            absolute(from, "HOME").map(|home| {
                home.join(".local")
                    .join("share")
                    .join("crucible")
                    .join(SESSIONS)
            })
        })
}

/// The user's own home directory, whatever this platform calls it.
///
/// `HOME` first everywhere, because it is the one a user can set to move their
/// files and the one every Unix already has. Windows sets `USERPROFILE` instead
/// and leaves `HOME` to whichever shell was started, so it is asked second
/// rather than first.
///
/// That order is also what puts a Git Bash session and a PowerShell one in the
/// same directory rather than two. A `HOME` inherited from Git Bash can arrive
/// spelled `/c/Users/ada`, which Windows does not call an absolute path — so
/// [`absolute`] declines it and `USERPROFILE` answers instead, which is the
/// directory the shell meant either way.
fn user(from: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    absolute(from, "HOME").or_else(|| profile(from))
}

/// What Windows calls it.
#[cfg(windows)]
fn profile(from: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    absolute(from, "USERPROFILE")
}

/// Nothing: every other platform sets `HOME` and means it.
#[cfg(not(windows))]
fn profile(_from: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    None
}

/// One variable, as a path, when it names an absolute one.
///
/// A relative path is ignored rather than resolved against the working
/// directory: crucible is started from wherever you happen to be working, and
/// resolving it there would scatter a home directory across every repository on
/// the machine.
fn absolute(from: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    from(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Scratch;

    /// An absolute path, spelled the way this platform spells one.
    ///
    /// Whether a variable names a home is decided by `is_absolute`, and a
    /// leading slash is not enough for Windows — a path is absolute there only
    /// with a drive or a share in front of it. A fixture hard-coding a Unix path
    /// would be testing the wrong question on half the targets.
    fn rooted(path: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\{}", path.replace('/', r"\"))
        } else {
            format!("/{path}")
        }
    }

    /// An environment holding exactly these variables and nothing else.
    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let held: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();

        move |wanted| {
            held.iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn the_variable_crucible_owns_places_the_whole_directory() {
        let named = rooted("srv/crucible");
        let home = Home::find(&environment(&[
            (HOME, &named),
            ("HOME", &rooted("home/ada")),
        ]))
        .expect("an absolute path was given");

        // Taken as the directory itself, not as somewhere to put `.crucible`:
        // somebody who names it has named it.
        assert_eq!(home.path(), Path::new(&named));
        assert_eq!(home.sessions(), Path::new(&named).join(SESSIONS));
    }

    #[test]
    fn without_it_the_directory_sits_in_the_users_home() {
        let user = rooted("home/ada");
        let home = Home::find(&environment(&[("HOME", &user)])).expect("HOME was given");

        assert_eq!(home.path(), Path::new(&user).join(DIRECTORY));
    }

    #[test]
    fn a_relative_path_is_ignored_rather_than_resolved_where_crucible_was_started() {
        let user = rooted("home/ada");
        let home = Home::find(&environment(&[(HOME, "crucible"), ("HOME", &user)]))
            .expect("HOME is still absolute");

        // Not `./crucible` under whichever repository this was run in, which
        // would be a different home directory in every one of them.
        assert_eq!(home.path(), Path::new(&user).join(DIRECTORY));
    }

    #[test]
    fn windows_keeps_the_home_directory_under_a_name_of_its_own() {
        let profile = rooted("Users/ada");
        let found = Home::find(&environment(&[("USERPROFILE", &profile)]));

        if cfg!(windows) {
            let home = found.expect("USERPROFILE was given");
            assert_eq!(home.path(), Path::new(&profile).join(DIRECTORY));
        } else {
            // Everywhere else it is a variable crucible has never heard of, so
            // an environment naming only it names no home at all.
            assert!(found.is_err(), "USERPROFILE is not a home on this platform");
        }
    }

    #[test]
    fn a_shell_that_sets_home_and_one_that_does_not_land_in_the_same_directory() {
        let user = rooted("home/ada");
        let home = Home::find(&environment(&[
            ("HOME", &user),
            ("USERPROFILE", &rooted("Users/ada")),
        ]))
        .expect("HOME was given");

        // Whichever shell started crucible, the answer is the same directory —
        // which is the only reason to ask them in a fixed order at all.
        assert_eq!(home.path(), Path::new(&user).join(DIRECTORY));
    }

    #[test]
    fn sessions_from_before_the_move_are_read_where_they_already_are() {
        let scratch = Scratch::new("older");
        let older = scratch.make("data/crucible/sessions");

        let home = Home::find(&environment(&[
            ("HOME", &scratch.text("home")),
            ("XDG_DATA_HOME", &scratch.text("data")),
        ]))
        .expect("an absolute path was given");

        // Read in place. Copying them would double the disk they take and
        // moving them would break `--continue` for anyone who upgrades halfway
        // through a piece of work — and both are irreversible on a tree the
        // user is told is theirs to read and delete.
        assert_eq!(home.sessions(), older);

        // The home directory itself is the new one regardless: the fallback is
        // about a tree that already exists, not about where crucible lives.
        assert_eq!(home.path(), scratch.at("home/.crucible"));
    }

    #[test]
    fn naming_a_home_puts_everything_under_it_and_looks_nowhere_else() {
        let scratch = Scratch::new("named");
        scratch.make("data/crucible/sessions");
        scratch.make("home/.local/share/crucible/sessions");

        let home = Home::find(&environment(&[
            (HOME, &scratch.text("named")),
            ("HOME", &scratch.text("home")),
            ("XDG_DATA_HOME", &scratch.text("data")),
        ]))
        .expect("an absolute path was given");

        // Two older trees exist and neither is consulted. Naming a home is how
        // a probe, a container or a test says "keep everything here", and a
        // fallback that could still send writes to somebody's real home would
        // make that promise untrue exactly where it matters most.
        assert_eq!(home.sessions(), scratch.at("named/sessions"));
    }

    #[test]
    fn the_older_tree_is_looked_for_under_the_users_home_when_xdg_says_nothing() {
        let scratch = Scratch::new("unset");
        let older = scratch.make("home/.local/share/crucible/sessions");

        let home =
            Home::find(&environment(&[("HOME", &scratch.text("home"))])).expect("HOME was given");

        assert_eq!(home.sessions(), older);
    }

    #[test]
    fn once_sessions_are_in_the_new_place_the_older_tree_is_left_behind() {
        let scratch = Scratch::new("both");
        scratch.make("data/crucible/sessions");
        let moved = scratch.make("home/.crucible/sessions");

        let home = Home::find(&environment(&[
            ("HOME", &scratch.text("home")),
            ("XDG_DATA_HOME", &scratch.text("data")),
        ]))
        .expect("HOME was given");

        // The new place wins the moment it exists, so somebody who moved their
        // sessions by hand is not silently sent back to the old tree.
        assert_eq!(home.sessions(), moved);
    }

    #[test]
    fn a_machine_with_no_sessions_yet_gets_the_new_place() {
        let scratch = Scratch::new("fresh");

        let home = Home::find(&environment(&[
            ("HOME", &scratch.text("home")),
            ("XDG_DATA_HOME", &scratch.text("data")),
        ]))
        .expect("HOME was given");

        assert_eq!(home.sessions(), scratch.at("home/.crucible/sessions"));
    }

    #[test]
    fn nowhere_to_put_anything_is_said_rather_than_guessed() {
        let nowhere = Home::find(&environment(&[]));

        let Err(problem) = nowhere else {
            panic!("an empty environment names no home");
        };
        // The message has to name crucible's own variable: somebody running
        // without HOME — in a container, under a service manager — needs to be
        // told what to set, not that something is missing.
        assert!(problem.to_string().contains(HOME), "{problem}");
    }
}
