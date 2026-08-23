//! Which shell reads a command line.
//!
//! POSIX on every platform, Windows included. [`super::command`] and
//! [`super::reach`] read the line the way a POSIX shell would, and that reading
//! is what a permission rule is written against and what proves a `mkdir` lands
//! inside the workspace. Handing the same line to `cmd.exe` — whose quoting,
//! globbing and redirection are a different language — would decide the question
//! in one language and answer it in another, which is a permission engine that
//! is wrong rather than one that is strict.
//!
//! So the shell is the same everywhere, and what differs is where it is found.
//!
//! Found is the word: on both platforms this ends at an absolute path, worked
//! out once when the tool is built. A bare `sh` handed to a spawn is resolved
//! wherever that spawn happens to be — and a command here runs with the
//! workspace as its working directory, which the model writes files into. An
//! empty element on the `PATH` means the current directory to everything that
//! resolves a name, so `PATH=/usr/bin:` and a file called `sh` in the tree are
//! together enough to make the model's own file the shell that reads every
//! command line after it, including the ones a user was asked about and
//! allowed. Resolving once, against absolute directories only, is what closes
//! that.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::program::on_path;

/// What to say when there is no shell to run anything with.
///
/// Read by somebody who has to fix it, so it names the thing to install rather
/// than the call that failed.
#[cfg(windows)]
pub(super) const ABSENT: &str =
    "install Git for Windows, which carries one, or put an sh.exe on the PATH";

/// What to say when there is no shell to run anything with.
#[cfg(not(windows))]
pub(super) const ABSENT: &str = "no sh on the PATH or in the places one lives";

/// Where a POSIX shell is, read through `lookup`.
///
/// Unix has one under a name every system puts in the same two places, so the
/// `PATH` is asked first and those are the fallback — which is also the answer
/// when `lookup` has no `PATH` to give.
#[cfg(not(windows))]
pub(super) fn find(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    on_path(&lookup, "sh").or_else(|| {
        ["/bin/sh", "/usr/bin/sh"]
            .into_iter()
            .map(PathBuf::from)
            .find(|candidate| candidate.is_file())
    })
}

/// Where a POSIX shell is, read through `lookup`.
///
/// Windows ships none, so this is the one Git for Windows carries. Its
/// installer puts `sh.exe` under `usr\bin` and leaves that directory off the
/// PATH unless the optional Unix tools were chosen — so the PATH is asked first,
/// where an MSYS, Cygwin or Git Bash session answers, and the two places the
/// installer actually writes to are tried after.
#[cfg(windows)]
pub(super) fn find(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(found) = on_path(&lookup, "sh.exe") {
        return Some(found);
    }

    [
        ("ProgramFiles", "Git"),
        ("ProgramFiles(x86)", "Git"),
        ("LocalAppData", r"Programs\Git"),
    ]
    .into_iter()
    .filter_map(|(variable, under)| {
        let root = lookup(variable)?;
        Some(
            std::path::Path::new(&root)
                .join(under)
                .join("usr")
                .join("bin")
                .join("sh.exe"),
        )
    })
    .find(|candidate| candidate.is_file())
}

// Unix only: what they pin is the arm above them, and a Windows machine
// carrying no Git installation has no shell for them to find. Spelled as two
// attributes because `cfg(test)` is also how the lints that forbid a panic know
// they are looking at a test, and they read it literally.
#[cfg(test)]
#[cfg(not(windows))]
mod tests {
    use super::*;

    /// A `PATH` with `text` at the front of crucible's own.
    fn ahead_of_the_real_one(text: &str) -> impl Fn(&str) -> Option<OsString> {
        let inherited = std::env::var("PATH").expect("crucible was started with a PATH");
        let path = OsString::from(format!("{text}{inherited}"));

        move |name: &str| match name {
            "PATH" => Some(path.clone()),
            other => std::env::var_os(other),
        }
    }

    #[test]
    fn the_shell_is_named_from_the_root() {
        let found = find(ahead_of_the_real_one("")).expect("a machine with a shell on it");

        assert!(found.is_absolute(), "{}", found.display());
    }

    #[test]
    fn an_empty_element_is_not_a_directory_to_take_a_shell_from() {
        // It means the current directory to every call that resolves a name,
        // and the current directory of a command this tool runs is the
        // workspace.
        let found = find(ahead_of_the_real_one(":")).expect("a machine with a shell on it");

        assert!(found.is_absolute(), "{}", found.display());
    }

    #[test]
    fn a_relative_element_is_not_one_either() {
        let found = find(ahead_of_the_real_one("./bin:")).expect("a machine with a shell on it");

        assert!(found.is_absolute(), "{}", found.display());
    }

    #[test]
    fn a_machine_with_no_path_still_has_the_places_a_shell_lives() {
        // A `PATH` this tool cannot read is not a machine without a shell.
        assert!(find(|_| None).is_some(), "a machine with a shell on it");
    }
}
