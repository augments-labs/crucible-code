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
//! So the shell is the same everywhere, and Windows is only where it has to be
//! found rather than assumed.

use std::path::PathBuf;

/// What to say when there is no shell to run anything with.
///
/// Read by somebody who has to fix it, so it names the thing to install rather
/// than the call that failed.
#[cfg(windows)]
pub(super) const ABSENT: &str =
    "install Git for Windows, which carries one, or put an sh.exe on the PATH";

/// What to say when there is no shell to run anything with.
#[cfg(not(windows))]
pub(super) const ABSENT: &str = "no sh on the PATH";

/// Where a POSIX shell is.
///
/// Unix has one under a name the loader resolves, so there is nothing to look
/// for.
#[cfg(not(windows))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the answer is always yes here and sometimes no on Windows. One \
              signature is what keeps the caller from having to know which \
              platform it is on to know whether it has a shell."
)]
pub(super) fn find() -> Option<PathBuf> {
    Some(PathBuf::from("sh"))
}

/// Where a POSIX shell is.
///
/// Windows ships none, so this is the one Git for Windows carries. Its
/// installer puts `sh.exe` under `usr\bin` and leaves that directory off the
/// PATH unless the optional Unix tools were chosen — so the PATH is asked first,
/// where an MSYS, Cygwin or Git Bash session answers, and the two places the
/// installer actually writes to are tried after.
#[cfg(windows)]
pub(super) fn find() -> Option<PathBuf> {
    if let Some(found) = on_path("sh.exe") {
        return Some(found);
    }

    [
        ("ProgramFiles", "Git"),
        ("ProgramFiles(x86)", "Git"),
        ("LocalAppData", r"Programs\Git"),
    ]
    .into_iter()
    .filter_map(|(variable, under)| {
        let root = std::env::var_os(variable)?;
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

/// The first directory on the PATH holding `program`.
#[cfg(windows)]
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}
