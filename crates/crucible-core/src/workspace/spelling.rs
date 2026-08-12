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

use std::path::Path;

/// A path spelled the way one is written.
///
/// Two things separate the two on Windows, and either alone leaves a rule that
/// cannot match. The separator is one: `/` is what the pattern language has, a
/// matcher normalises a candidate to it before comparing, and a path left as it
/// arrived mints `src[\]main.rs` — the backslash escaped as the literal
/// character it is not. Somebody would answer "always" and be asked the same
/// question next turn.
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

/// A path spelled the way one is written, which is the way it already is. A
/// backslash here is a character in a filename rather than a separator, and
/// anything minted from such a file has to keep it.
#[cfg(not(windows))]
#[must_use]
pub fn written(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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

    #[cfg(not(windows))]
    #[test]
    fn a_backslash_in_a_name_is_part_of_the_name() {
        assert_eq!(written(Path::new(r"odd\name.rs")), r"odd\name.rs");
    }
}
