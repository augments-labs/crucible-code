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
//!
//! Resolving every word is only a proof where the words are the files. A copy
//! or a move given a directory writes `directory/name`, which is no word in the
//! line — nothing resolved it, and a symbolic link left at that name is
//! followed straight out of the tree. So those two are read for the shapes that
//! leave the filename to the program, and refused on them.
//!
//! One shape is refused with the proof in hand: a recursive `rm`. `rm -rf .`
//! from the workspace root cannot reach outside the tree, and that is true and
//! not enough. [`Reach::Workspace`] is what a mode may act on *without asking*,
//! and every other line this reads changes the things it names — one call, one
//! bounded loss, whatever was typed. A recursive delete changes everything
//! beneath them instead. That is a difference in kind rather than degree, so it
//! is where the answer stops.
//!
//! And one place inside the workspace is not somewhere a proof may lead: the
//! files this agent's permissions are read from. They sit in the tree, so every
//! check above passes over them happily, and a line that changed one would be
//! deciding every question asked after the next start. `reaches_configuration`
//! is where that is refused, and it is the permission engine's own answer
//! rather than a second list kept in step with it by hand.

use std::path::Path;

use crucible_core::{Reach, Workspace, reaches_configuration};

/// A program whose arguments this can read, with the flags it recognises.
struct Program {
    /// The word the line has to start with, spelled as it is invoked.
    name: &'static str,
    /// Single-letter flags, which may also arrive bundled as `-rf`.
    letters: &'static str,
    /// Whole words, matched entire.
    words: &'static [&'static str],
    /// Which of the above turn the call into one about everything underneath
    /// what it names.
    recursive: Recursion,
    /// Whether the last path is a destination the program may finish itself.
    ///
    /// `cp a b` writes `b`, which is a word here and gets resolved like one.
    /// `cp a dir` writes `dir/a`, which is no word at all — so nothing resolved
    /// it, and a symbolic link sitting at that name is followed out of the tree
    /// by the copy. Where this is set, the shapes that leave the program to
    /// work the name out are refused rather than half-read.
    destination: bool,
}

/// The flags that make a program descend, in both spellings one can arrive in.
///
/// A subset of the tables above rather than a table of its own, because a flag
/// this does not otherwise recognise is refused before it gets here — so the
/// only thing this has to be right about is which of the recognised ones
/// descend.
struct Recursion {
    /// Single letters, which may arrive bundled: `-rf` descends as `-r` does.
    letters: &'static str,
    /// Whole words, matched entire.
    words: &'static [&'static str],
}

/// For a program that has no such flag.
const NEVER: Recursion = Recursion {
    letters: "",
    words: &[],
};

impl Program {
    /// Whether this flag is one of the program's, and carries no value.
    fn takes(&self, flag: &str) -> bool {
        if flag.starts_with("--") {
            // `--` itself ends the options and is in no table, so a line using
            // it is asked about rather than half-read. Which also settles what
            // a path spelled `-r` after one means here: nothing, because the
            // line stopped being read at the terminator.
            self.words.contains(&flag)
        } else {
            // `-rf` is `-r` and `-f`, and every letter has to be recognised.
            flag[1..]
                .chars()
                .all(|letter| self.letters.contains(letter))
        }
    }

    /// Whether this flag makes the call one about everything under the paths it
    /// names.
    ///
    /// Asked only of a word [`flag`] already called a flag, so a file named
    /// `-r` — or, far likelier, a path with `-r` somewhere inside it — is never
    /// put to this at all. The bundle is the spelling to get right: `-rf` and
    /// `-fr` descend exactly as `-r` does, and a match written against the whole
    /// word would see neither.
    fn recurses(&self, flag: &str) -> bool {
        if flag.starts_with("--") {
            self.recursive.words.contains(&flag)
        } else {
            flag.chars()
                .skip(1)
                .any(|letter| self.recursive.letters.contains(letter))
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
        recursive: NEVER,
        destination: false,
    },
    Program {
        name: "rmdir",
        letters: "pv",
        words: &["--parents", "--verbose"],
        // Not `rm`. `-p` climbs rather than descends, and every directory it
        // removes has to be empty already — so nothing is destroyed that was
        // not already nothing.
        recursive: NEVER,
        destination: false,
    },
    Program {
        name: "touch",
        letters: "acmv",
        words: &["--no-create"],
        recursive: NEVER,
        destination: false,
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
        // Both letters, because `-R` is the same flag, and the long word,
        // because a list holding two of the three spellings is a rule about
        // spelling. `-d` is not here: it removes an empty directory, which is
        // `rmdir` under another name.
        recursive: Recursion {
            letters: "rR",
            words: &["--recursive"],
        },
        destination: false,
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
        // For a reason of its own rather than `rm`'s. A recursive copy does not
        // destroy what it descends over — it stops the words naming the files.
        // Each one stands for a tree this never walked, and whether everything
        // in that tree lands inside depends on what the copy does with the
        // links it finds on the way, which is a property of the program and not
        // of the line. `-a` is here because it implies `-R`, and both letters
        // because `-r` and `-R` are the same flag.
        recursive: Recursion {
            letters: "rRa",
            words: &["--recursive", "--archive"],
        },
        destination: true,
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
        recursive: NEVER,
        // A rename replaces a symbolic link rather than following one, so `mv`
        // is not the escape `cp` is. It is still a line whose last word is not
        // the file that changes, which is the whole of what this proves — and a
        // move between filesystems is a copy underneath, where the far end is
        // opened by name like any other.
        destination: true,
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

    let mut paths = 0usize;
    let mut last = None;

    for word in words {
        if word.contains(UNREADABLE) {
            return Reach::Anything;
        }

        // A flag in the table carries no value, so the next word is still a
        // path. One that is not in it may take the next word with it, and then
        // the word resolved here is not the one that gets changed.
        //
        // And one that makes the program descend ends the reading too, though
        // for the other reason: not that the paths cannot be proved, but that
        // proving them stopped being the question — see the note at the top.
        if let Some(flag) = flag(word) {
            if program.takes(flag) && !program.recurses(flag) {
                continue;
            }
            return Reach::Anything;
        }

        // `creatable` and not `existing`: `mkdir` names a directory that is not
        // there yet, and both answer the only question being asked — where this
        // lands once symbolic links are followed.
        let Ok(path) = workspace.creatable(word) else {
            return Reach::Anything;
        };

        // Inside the workspace and still not something to prove. The engine's
        // own configuration lives in the tree, and a write there decides every
        // question from the next start on — so it is asked about rather than
        // waved through by a mode this line could rewrite.
        if reaches_configuration(path.as_path()) {
            return Reach::Anything;
        }

        paths += 1;
        last = Some(path);
    }

    // A program with no path to work on is doing something else — printing its
    // version, reading its own defaults — and nothing here read what.
    let Some(last) = last else {
        return Reach::Anything;
    };

    if program.destination && finishes_the_name(paths, last.as_path()) {
        return Reach::Anything;
    }

    Reach::Workspace
}

/// Whether the program is left to work out the path it writes.
///
/// Two shapes say so. A destination that is a directory takes the name from the
/// source, and more than one source says the destination is a directory
/// whatever is there at the moment this reads — which is the half worth having
/// twice, because the other half is an answer about one instant and the copy
/// runs at a later one.
fn finishes_the_name(paths: usize, last: &Path) -> bool {
    paths > 2 || last.is_dir()
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
