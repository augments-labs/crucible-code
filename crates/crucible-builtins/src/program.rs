//! Whether a program is on the `PATH`, where it is, and what to call it.
//!
//! Two callers want this for different reasons — one is choosing the shell every
//! command line is read by, the other is naming a converter to somebody who has
//! to run it — and both depend on the same answer about what a `PATH` element is
//! allowed to be. One copy, because the rule below is the kind that is only ever
//! wrong once.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The first absolute directory on the `PATH` holding `program`.
///
/// Absolute is the whole of the check. An empty element is what a `PATH` picks
/// up from a shell profile that appended to an unset variable, and a relative
/// one is rarer and worse; both mean "resolve this against wherever you happen
/// to be", and where this program happens to be is a directory the model can
/// write to. A directory that cannot be named from the root is not one crucible
/// looks in.
#[must_use]
pub fn on_path(lookup: impl Fn(&str) -> Option<OsString>, program: &str) -> Option<PathBuf> {
    let path = lookup("PATH")?;
    std::env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

/// What this platform spells `program` as.
///
/// Beside [`on_path`] because a caller with a bare name needs both answers and
/// neither is worth deciding twice: an executable is `ffmpeg` on two platforms
/// and `ffmpeg.exe` on the third, and a lookup that skips this finds nothing on
/// the one that is different.
#[must_use]
pub fn spelled(program: &str) -> String {
    if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_owned()
    }
}

/// Where a program lives when its installer does not put it on the `PATH`.
///
/// Two things need this. `LibreOffice` ships a command line on all three
/// platforms and adds it to the `PATH` on exactly one of them; `ffmpeg` has no
/// installer of its own on two of them and arrives through whichever package
/// manager the reader already uses. Without the rows below, a Mac with either
/// one installed is told nothing here can do anything with the file, which is
/// worse than saying nothing at all — it is a true-sounding answer that is
/// false.
///
/// The line those rows are held to: **a directory is added because an installer
/// is known to write there, never because a program is often found there.** A
/// package manager's shim directory is documented by the manager and qualifies.
/// The folder somebody happened to unpack an archive into is a guess, and a
/// guess that misses costs the same as having said nothing while sounding like
/// it did not.
///
/// Each entry is a directory, and a leading `$Variable` is read from the
/// environment and dropped if unset. Unix paths are absolute already.
#[cfg(windows)]
const ELSEWHERE: &[(&str, &[&str])] = &[
    (
        "soffice",
        &[
            r"$ProgramFiles\LibreOffice\program",
            r"$ProgramFiles(x86)\LibreOffice\program",
        ],
    ),
    ("ffmpeg", SHIMS),
    ("ffprobe", SHIMS),
];

/// Where the Windows package managers put a command they installed.
///
/// `ffmpeg` publishes an archive rather than an installer here, so where a
/// hand-unpacked build sits is the reader's to know and not this program's to
/// guess. What is knowable is where a package manager puts its shim: each of
/// these three documents the directory and each adds it to the `PATH`, which
/// is what makes them directories an installer writes rather than directories a
/// program turns up in.
///
/// Shared by both names because `ffprobe` is installed by the same package as
/// `ffmpeg` and lands beside it.
#[cfg(windows)]
const SHIMS: &[&str] = &[
    r"$LOCALAPPDATA\Microsoft\WinGet\Links",
    r"$ProgramFiles\WinGet\Links",
    r"$ProgramData\chocolatey\bin",
    r"$USERPROFILE\scoop\shims",
];

/// Where a program lives when its installer does not put it on the `PATH`.
#[cfg(target_os = "macos")]
const ELSEWHERE: &[(&str, &[&str])] = &[
    (
        "soffice",
        &[
            "/Applications/LibreOffice.app/Contents/MacOS",
            "/opt/homebrew/bin",
            "/usr/local/bin",
        ],
    ),
    ("ffmpeg", PREFIXES),
    ("ffprobe", PREFIXES),
];

/// The prefixes the macOS package managers install into.
///
/// Homebrew's two — one per architecture — and `MacPorts`' one, each the default
/// its own documentation names. `ffprobe` shares them for the same reason it
/// shares the Windows list: one package installs both.
#[cfg(target_os = "macos")]
const PREFIXES: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];

/// Where a program lives when its installer does not put it on the `PATH`.
///
/// Nothing: every packaged converter on this platform installs into a directory
/// that is already on the `PATH`, and guessing at the ones that do not would be
/// inventing paths rather than knowing them.
#[cfg(not(any(windows, target_os = "macos")))]
const ELSEWHERE: &[(&str, &[&str])] = &[];

/// The directory named by `entry`, with a leading `$Variable` resolved.
fn directory(lookup: &impl Fn(&str) -> Option<OsString>, entry: &str) -> Option<PathBuf> {
    let Some(rest) = entry.strip_prefix('$') else {
        return Some(PathBuf::from(entry));
    };
    let (variable, under) = rest.split_once(['\\', '/'])?;
    Some(Path::new(&lookup(variable)?).join(under))
}

/// How a command line must name `program`, or `None` where it is not installed.
///
/// A bare name where the `PATH` already finds it, because that is what a person
/// reading the suggestion would type; an absolute path where it does not,
/// because otherwise the suggestion is one a shell cannot run.
pub(crate) fn named(lookup: impl Fn(&str) -> Option<OsString>, program: &str) -> Option<String> {
    let spelling = spelled(program);
    if on_path(&lookup, &spelling).is_some() {
        return Some(program.to_owned());
    }

    let (_, directories) = ELSEWHERE.iter().find(|(named, _)| *named == program)?;
    directories
        .iter()
        .filter_map(|entry| directory(&lookup, entry))
        .map(|directory| directory.join(&spelling))
        .find(|candidate| candidate.is_file())
        .map(|found| found.display().to_string())
}

/// How a command line must name `program`, read from this process's environment.
pub(crate) fn installed(program: &str) -> Option<String> {
    named(|name| std::env::var_os(name), program)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `PATH` of exactly the directories given.
    fn path_of(directories: &[&str]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let path = OsString::from(directories.join(":"));
        move |name| (name == "PATH").then(|| path.clone())
    }

    #[test]
    fn a_program_the_path_already_finds_is_named_the_way_it_would_be_typed() {
        let held = std::env::var("PATH").expect("crucible was started with a PATH");
        let real = move |name: &str| (name == "PATH").then(|| OsString::from(held.clone()));

        // `sh` is the one program every platform this runs its tests on has.
        #[cfg(not(windows))]
        assert_eq!(named(real, "sh").as_deref(), Some("sh"));
        #[cfg(windows)]
        let _ = real;
    }

    #[test]
    fn a_program_nowhere_on_the_path_and_nowhere_known_is_not_named() {
        assert!(named(path_of(&["/nonexistent-directory"]), "pandoc").is_none());
    }

    #[test]
    fn a_relative_path_element_is_never_looked_in() {
        assert!(named(path_of(&[".", "relative/bin"]), "sh").is_none());
    }

    /// A `$Variable` with nothing after it names no directory at all, and the
    /// row holding it is skipped in silence — which is the one failure this
    /// table cannot report, because a lookup that found nothing and a lookup
    /// that never looked read the same from outside.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn every_row_names_a_directory_that_can_be_resolved() {
        let root = if cfg!(windows) {
            r"C:\fabricated"
        } else {
            "/fabricated"
        };
        let fabricated = |name: &str| (name != "PATH").then(|| OsString::from(root));

        for (program, directories) in ELSEWHERE {
            for entry in *directories {
                let resolved = directory(&fabricated, entry);
                assert!(
                    resolved.as_deref().is_some_and(Path::is_absolute),
                    "{program}: {entry} names no absolute directory"
                );
            }
        }
    }

    /// The same claim as the `LibreOffice` one below, for what a video needs.
    ///
    /// Skipped where `ffmpeg` is genuinely absent: a row cannot be asserted to
    /// be populated on a machine that never installed it, so what this proves
    /// is that the lookup reaches the directory — not that the directory has
    /// anything in it.
    #[cfg(windows)]
    #[test]
    fn ffmpeg_off_the_path_is_found_where_a_package_manager_puts_it() {
        // Everything but `PATH`, so an answer can only have come from the table.
        let without_path = |name: &str| (name != "PATH").then(|| std::env::var_os(name)).flatten();

        let shimmed = SHIMS
            .iter()
            .filter_map(|entry| directory(&without_path, entry))
            .map(|directory| directory.join("ffmpeg.exe"))
            .find(|candidate| candidate.is_file());
        let Some(shimmed) = shimmed else {
            return;
        };

        let found = named(without_path, "ffmpeg");

        assert_eq!(found.as_deref(), shimmed.to_str());
    }

    /// As above, on the platform whose package managers install into a prefix.
    #[cfg(target_os = "macos")]
    #[test]
    fn ffmpeg_off_the_path_is_found_where_a_package_manager_puts_it() {
        let installed = PREFIXES
            .iter()
            .map(|prefix| Path::new(prefix).join("ffmpeg"))
            .find(|candidate| candidate.is_file());
        let Some(installed) = installed else {
            return;
        };

        let found = named(path_of(&["/nonexistent-directory"]), "ffmpeg");

        assert_eq!(found.as_deref(), installed.to_str());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn libreoffice_off_the_path_is_found_where_its_installer_puts_it() {
        let bundle = Path::new("/Applications/LibreOffice.app/Contents/MacOS/soffice");
        if !bundle.is_file() {
            return;
        }

        let found = named(path_of(&["/nonexistent-directory"]), "soffice");

        assert_eq!(found.as_deref(), Some(bundle.to_str().unwrap_or_default()));
    }
}
