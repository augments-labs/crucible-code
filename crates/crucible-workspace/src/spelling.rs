//! How a path is spelled once it stops being a path and becomes text.
//!
//! Every path crucible shows, matches or remembers passes through here, because
//! all three have to be the same string. A rule is written about what a listing
//! printed, a prompt asks about what a rule would match, and a model hands back
//! the name a search gave it. Two spellings of one file turn each of those into
//! a near miss, and a near miss between a deny rule and the file it names reads
//! as protection while being none.
//!
//! A path written down only to be compared back to itself is the exception, and
//! the only one — a session log recording which directory it belongs to wants
//! identity rather than legibility, and normalising both sides of a comparison
//! can only make two paths that differ agree. Nothing here refuses to be used
//! there; the point is that a site not going through this door is not
//! automatically one to correct.
//!
//! A name that is not text — bytes that are not UTF-8, or on Windows a lone
//! surrogate — comes out with a replacement character where it could not be
//! written, so two such paths can share one spelling. What has to tell files
//! apart keeps the path itself beside the spelling rather than trusting this.
//!
//! A directory handed to a person to type into a shell is spelled for that
//! shell instead, by [`typed`]: it is gone to, never matched, so it keeps the
//! separators the shell reads and drops only what nobody types.

use std::path::{Path, PathBuf};

/// A path spelled the way one is written.
///
/// Two things separate the two on Windows, and either alone leaves a rule that
/// cannot match. The separator is one: `/` is what the pattern language has, a
/// matcher normalises a candidate to it before comparing, and a path left as it
/// arrived mints `src[\]main.rs` — the backslash escaped as the literal
/// character it is not. Somebody would write that rule and still be asked.
///
/// The prefix is the other. Resolving a path yields the extended-length
/// spelling, `\\?\C:\...`, which nobody writes and nobody would recognise their
/// own project in. `deny read(C:/Users/you/.ssh/**)` is an absolute pattern, it
/// would be matched against `//?/C:/...`, and a rule that reads as protection
/// and is not is worse than no rule. Taken off a plain drive path only: the
/// short spelling of `\\?\UNC\server\share` is not a prefix of it, and a deny
/// that stopped matching because this rewrote a path is the same failure from
/// the other side.
#[cfg(windows)]
#[must_use]
pub fn written(path: &Path) -> String {
    let path = path.to_string_lossy();
    let plain = path.strip_prefix(r"\\?\").filter(|rest| {
        let mut ahead = rest.chars();
        matches!(
            (ahead.next(), ahead.next(), ahead.next()),
            (Some(drive), Some(':'), Some('\\')) if drive.is_ascii_alphabetic()
        )
    });

    plain.unwrap_or(&path).replace('\\', "/")
}

/// A path spelled the way one is written, which is the way it already is
/// unless part of it is not text: those bytes come out as the replacement
/// character, as the module documentation says. A backslash here is a
/// character in a filename rather than a separator, and anything minted from
/// such a file has to keep it.
#[cfg(not(windows))]
#[must_use]
pub fn written(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A path spelled the way someone types it at a shell prompt, separators and
/// all.
///
/// What [`written`] does for a pattern, this does for a command a person
/// pastes: resolving gives `\\?\C:\...`, which cmd will not `cd` into and
/// nobody recognises their own project in. Here a share is shortened too,
/// `\\?\UNC\server\share` to `\\server\share`, because a shell is handed
/// a directory to go to, not a rule to match, and that is how one is typed.
/// Anything else, and any path that is not text, is left as it is.
#[cfg(windows)]
#[must_use]
pub fn typed(path: &Path) -> PathBuf {
    let Some(spelled) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(share) = spelled.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    let plain = spelled.strip_prefix(r"\\?\").filter(|rest| {
        let mut ahead = rest.chars();
        matches!(
            (ahead.next(), ahead.next(), ahead.next()),
            (Some(drive), Some(':'), Some('\\')) if drive.is_ascii_alphabetic()
        )
    });

    PathBuf::from(plain.unwrap_or(spelled))
}

/// A path spelled the way someone types it at a shell prompt, which is the way
/// it already is.
#[cfg(not(windows))]
#[must_use]
pub fn typed(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_spelled_with_the_separator_a_pattern_uses() {
        // The one assertion that holds on every platform: whatever comes back
        // is something the pattern language can match and somebody can type.
        let written = written(Path::new("src").join("cli").join("parse.rs").as_path());

        assert_eq!(written, "src/cli/parse.rs");
    }

    #[cfg(windows)]
    #[test]
    fn resolving_a_path_does_not_leave_a_prefix_nobody_writes() {
        assert_eq!(
            written(Path::new(r"\\?\C:\Users\you\.ssh")),
            "C:/Users/you/.ssh"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_share_keeps_the_prefix_that_is_part_of_naming_it() {
        // `\\?\UNC\server\share` does not shorten to `server\share`, so taking
        // the prefix off would name a different place — or nothing at all.
        assert_eq!(
            written(Path::new(r"\\?\UNC\server\share")),
            "//?/UNC/server/share"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_resolved_path_is_typed_without_its_prefix() {
        assert_eq!(
            typed(Path::new(r"\\?\C:\Users\ada\projects\website")),
            Path::new(r"C:\Users\ada\projects\website")
        );
        assert_eq!(
            typed(Path::new(r"\\?\UNC\server\share\x")),
            Path::new(r"\\server\share\x")
        );
        assert_eq!(typed(Path::new(r"D:\code")), Path::new(r"D:\code"));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_path_is_typed_as_it_is() {
        assert_eq!(
            typed(Path::new(r"/srv/odd\name")),
            Path::new(r"/srv/odd\name")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_backslash_in_a_name_is_part_of_the_name() {
        assert_eq!(written(Path::new(r"odd\name.rs")), r"odd\name.rs");
    }
}
