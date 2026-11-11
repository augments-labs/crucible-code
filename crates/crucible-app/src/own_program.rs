//! A program the user installed themselves, run as the catalog agent an
//! `agent:` key names: what the sandbox is told of the directory it lies in.
//!
//! An own program is no release crucible pinned or reviewed, so the directory
//! it lies in is decided once, here, for the probe that admits it and for
//! every launch alike. That directory is refused when it is the user's home or
//! the top of the file system, or is, lies within or contains a place no agent
//! is made able to write; the one exception is a home search directory such as
//! `~/.local/bin`, which may itself be read. A directory the sandbox already
//! lets every program read gets no rule of its own, and any other is given
//! read-only, never writable. Everything in that directory is readable to the
//! agent while it runs.

use std::path::{Path, PathBuf};

use crate::launching::{Guarded, Overlap, Reach, beneath, guarded, is_beneath, overlap};

/// The home search directories an own program may lie in, though each is a
/// place no agent is made able to write: reading one is what running from it
/// takes, and it is never given writable.
pub const HOME_SEARCH: &[&str] = &[
    "bin",
    ".local/bin",
    ".cargo/bin",
    "AppData/Local/Microsoft/WindowsApps",
];

/// What the sandbox is told of the directory an own program lies in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Root {
    /// It is not given, for the reason the sentence says, and the program is
    /// neither probed nor started.
    Refused(String),
    /// The sandbox already lets every program read it, so no rule is added.
    NoRule,
    /// It is given read-only.
    Grant(PathBuf),
}

/// The platform whose sandbox has or has not been checked against a
/// directory programs are commonly installed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    /// macOS, whose sandbox has not been checked against Homebrew's links.
    MacOs,
    /// Windows, whose sandbox has not been checked against the Microsoft
    /// Store's aliases.
    Windows,
    /// Any other.
    Other,
}

impl Platform {
    /// The platform this build runs on.
    pub(crate) const HERE: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Other
    };

    /// Why `root` is not given on this platform yet, where it is not.
    fn unrecorded(self, root: &Path) -> Option<String> {
        let shown = root.display();
        match self {
            Self::MacOs if is_beneath(root, Path::new("/opt/homebrew/bin")) => Some(format!(
                "{shown} is not given to an agent on macOS yet: whether the sandbox there can \
                 read the programs Homebrew links into it has not been checked."
            )),
            Self::Windows
                if root
                    .components()
                    .any(|part| part.as_os_str().eq_ignore_ascii_case("WindowsApps")) =>
            {
                Some(format!(
                    "{shown} is not given to an agent on Windows yet: whether the sandbox there \
                     can read and start the programs the Microsoft Store links into it has not \
                     been checked."
                ))
            }
            Self::MacOs | Self::Windows | Self::Other => None,
        }
    }
}

/// What the sandbox is told of the directory `program`, a canonical path,
/// lies in, for its probe and its launch alike: refused where that directory
/// is `home`, contains it, is `/`, or is, lies within or contains a place no
/// agent is made able to write, crucible's own home `crucible` and its session
/// logs `sessions` among them, but for a home search directory itself; no
/// rule where the sandbox already reads it; else given read-only.
#[must_use]
pub fn root(program: &Path, home: &Path, crucible: &Path, sessions: &Path) -> Root {
    decided(
        program,
        home,
        &guarded(home, crucible, sessions),
        Platform::HERE,
        crucible_sandbox_local::runtime_directories(),
    )
}

/// [`root`] as it is decided on `platform`, whose sandbox already reads every
/// directory of `runtime`.
pub(crate) fn decided(
    program: &Path,
    home: &Path,
    guarded: &[Guarded],
    platform: Platform,
    runtime: &[&str],
) -> Root {
    let Some(root) = program.parent().filter(|root| root.parent().is_some()) else {
        return Root::Refused(format!(
            "{} lies at the top of the file system, which holds every file on it, so its \
             directory is not given to an agent.",
            program.display()
        ));
    };
    let shown = root.display();
    if let Some(found @ (Overlap::Is | Overlap::Contains)) = overlap(root, home, Reach::Itself) {
        return Root::Refused(format!(
            "{shown} {} your home, which holds every file you keep, so it is not given to an \
             agent.",
            found.says()
        ));
    }
    if let Some(why) = platform.unrecorded(root) {
        return Root::Refused(why);
    }
    let searched = HOME_SEARCH
        .iter()
        .any(|dir| overlap(root, &beneath(home, dir), Reach::Itself) == Some(Overlap::Is));
    for place in guarded {
        for at in &place.at {
            match overlap(root, at, place.reach) {
                Some(Overlap::Is) if searched => {}
                Some(found @ (Overlap::Is | Overlap::Within | Overlap::Contains)) => {
                    return Root::Refused(format!(
                        "{shown} {} {} ({}), so it is not given to an agent.",
                        found.says(),
                        place.place,
                        place.why
                    ));
                }
                Some(Overlap::Outside) | None => {}
            }
        }
    }
    if runtime.iter().any(|dir| is_beneath(root, Path::new(dir))) {
        return Root::NoRule;
    }
    Root::Grant(root.to_path_buf())
}
