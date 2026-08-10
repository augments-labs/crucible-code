//! Proving that a command line changes nothing outside the workspace.
//!
//! A shell reaches whatever the user can, so this proves nothing about shells:
//! it recognises a handful of filesystem programs whose arguments are all
//! paths, and answers [`Reach::Workspace`] only when every one of those paths
//! was resolved inside. Everything else is [`Reach::Anything`] — a program not
//! listed, a flag not listed, a word that is not the path it appears to be, a
//! path that lands elsewhere. That is the whole of what the answer means, and
//! the direction it fails in is the reason it can be trusted at all.
//!
//! The flag tables hold only flags that carry no value of their own. A flag
//! that takes one — `mkdir -m 755`, `cp -t dir` — would put a word in the line
//! that is not a path, and the arithmetic of which word belongs to which flag
//! is exactly the kind of thing to be wrong about quietly. So they are absent,
//! and a line using one is asked about.

use crucible_core::{Reach, Workspace};

/// A program whose arguments this can read, with the flags it recognises.
struct Program {
    /// The word the line has to start with, spelled as it is invoked.
    name: &'static str,
    /// Single-letter flags, which may also arrive bundled as `-rf`.
    letters: &'static str,
    /// Whole words, matched entire.
    words: &'static [&'static str],
}

impl Program {
    /// Whether this flag is one of the program's, and carries no value.
    fn takes(&self, flag: &str) -> bool {
        if flag.starts_with("--") {
            // `--` itself ends the options and is in no table, so a line using
            // it is asked about rather than half-read.
            self.words.contains(&flag)
        } else {
            // `-rf` is `-r` and `-f`, and every letter has to be recognised.
            flag[1..]
                .chars()
                .all(|letter| self.letters.contains(letter))
        }
    }
}

/// The programs that change files and take nothing but paths.
///
/// `sed` is deliberately absent: `sed -i` edits in place, and the argument
/// that says so is a script rather than a path.
const PROGRAMS: [Program; 6] = [
    Program {
        name: "mkdir",
        letters: "pv",
        words: &["--parents", "--verbose"],
    },
    Program {
        name: "rmdir",
        letters: "pv",
        words: &["--parents", "--verbose"],
    },
    Program {
        name: "touch",
        letters: "acmv",
        words: &["--no-create"],
    },
    Program {
        name: "rm",
        letters: "rRfidv",
        words: &[
            "--recursive",
            "--force",
            "--interactive",
            "--dir",
            "--verbose",
        ],
    },
    Program {
        name: "cp",
        letters: "rRfinpavT",
        words: &[
            "--recursive",
            "--force",
            "--interactive",
            "--no-clobber",
            "--archive",
            "--verbose",
            "--no-target-directory",
        ],
    },
    Program {
        name: "mv",
        letters: "finvT",
        words: &[
            "--force",
            "--interactive",
            "--no-clobber",
            "--verbose",
            "--no-target-directory",
        ],
    },
];

/// Characters that mean a word is not the path it looks like.
///
/// A glob or a `~` is expanded by the shell before the program sees it, so the
/// path checked here would not be the path changed. A quote or a backslash
/// means the word was assembled by quoting rules this does not apply, and the
/// text kept them, so what looks like two words may be one.
const UNREADABLE: [char; 7] = ['*', '?', '[', '~', '\'', '"', '\\'];

/// How far this command line was proved to reach.
pub(super) fn of(parts: &[Box<str>], workspace: &Workspace) -> Reach {
    match parts {
        // One command, because a proof about a line is a proof about all of
        // it, and the second command is where the interesting one hides.
        [only] => confined(only, workspace),
        _ => Reach::Anything,
    }
}

/// Whether this one simple command changes nothing outside the workspace.
fn confined(part: &str, workspace: &Workspace) -> Reach {
    let mut words = part.split(' ');

    let Some(program) = words.next().and_then(known) else {
        return Reach::Anything;
    };

    let mut named = false;
    for word in words {
        if word.contains(UNREADABLE) {
            return Reach::Anything;
        }

        // A flag in the table carries no value, so the next word is still a
        // path. One that is not in it may take the next word with it, and then
        // the word resolved here is not the one that gets changed.
        if let Some(flag) = flag(word) {
            if program.takes(flag) {
                continue;
            }
            return Reach::Anything;
        }

        // `creatable` and not `existing`: `mkdir` names a directory that is not
        // there yet, and both answer the only question being asked — where this
        // lands once symbolic links are followed.
        if workspace.creatable(word).is_err() {
            return Reach::Anything;
        }
        named = true;
    }

    // A program with no path to work on is doing something else — printing its
    // version, reading its own defaults — and nothing here read what.
    if named {
        Reach::Workspace
    } else {
        Reach::Anything
    }
}

/// The program this word names, when it is one of the few this can read.
fn known(word: &str) -> Option<&'static Program> {
    PROGRAMS.iter().find(|program| program.name == word)
}

/// The word as a flag, when it is one. A bare `-` is a filename by convention,
/// not an option.
fn flag(word: &str) -> Option<&str> {
    (word.starts_with('-') && word.len() > 1).then_some(word)
}

#[cfg(test)]
mod tests;
