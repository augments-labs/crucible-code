//! What a command is started with.
//!
//! Not what crucible was started with. crucible's own environment holds the
//! provider credential, and `env` is an ordinary thing for a model to run — so
//! a child that inherited the whole of it would hand the key back as tool
//! output, which goes on the screen, into the next request and into the session
//! log. Nothing about that needs a mistake to happen first.
//!
//! So a child gets the names on the list below and nothing else, with the `env`
//! block laid over them.
//!
//! An allowlist of names is the only shape that works here, rather than a
//! denylist of the names keys are usually called. `apiKeyEnv` takes a *name*,
//! so the variable holding a key is called whatever the person who set it
//! called it: `ANTHROPIC_API_KEY` on one machine, `WORK_KEY` on the next, and
//! something nobody has thought of yet on the third. A list of the names that
//! were guessed would cover exactly the guesses.
//!
//! The list is therefore short on purpose, and it answers "what does a program
//! need to run at all" rather than "what is probably harmless". A command that
//! needs anything else is told about it in the `env` block, which is the user
//! saying so.

use std::ffi::OsString;

/// The names a command inherits from crucible's own environment.
#[cfg(not(windows))]
pub(super) const INHERITED: &[&str] = &[
    // Where a program is looked for. Without it a command line is a list of
    // names nothing resolves — `sh` itself included.
    "PATH",
    // Where a tool keeps its own configuration and its cache: `~/.gitconfig`,
    // `~/.cargo`, `~/.npmrc`. Missing, git and cargo either refuse to run or
    // write their state somewhere nobody meant them to.
    "HOME",
    // Which encoding a program writes in. Left to the C locale, every non-ASCII
    // byte in a diagnostic comes back as `?` or an escape — and the model reads
    // that diagnostic. `LC_ALL` overrides every category, `LANG` is the
    // fallback for every category, and `LC_CTYPE` is the one that decides the
    // encoding on its own.
    "LC_ALL", "LC_CTYPE", "LANG",
    // What kind of terminal a program may talk to. Anything built on terminfo —
    // `tput`, `less`, a test runner that colours its output — stops with an
    // error when this is unset rather than falling back to plain text.
    "TERM",
    // Where a temporary file goes. Not every machine has a writable `/tmp`, and
    // where it does not, this is the only place a build tool can write.
    "TMPDIR",
];

/// The names a command inherits from crucible's own environment.
///
/// Windows needs a different set, the way [`super::shell`] needs a different
/// answer: the home directory is spelled three ways here, and a program that
/// opens a socket fails without `SystemRoot`.
#[cfg(windows)]
pub(super) const INHERITED: &[&str] = &[
    // Where a program is looked for, and what counts as one. A Windows search
    // is `PATHEXT` as much as `PATH`: without it `cargo` is a name with no
    // `.exe` on the end and nothing finds it.
    "PATH",
    "PATHEXT",
    // Where `cmd.exe` is. A build script that shells out through the C runtime
    // reads this rather than looking for it.
    "COMSPEC",
    // Where Windows itself is. Sockets are the sharp one: winsock reads
    // `SystemRoot` to find its catalogue, so a program that opens a connection
    // fails before it starts and says nothing that names the cause.
    "SystemRoot",
    "SystemDrive",
    "windir",
    // Where a temporary file goes. Asked for and not found, Windows answers
    // with its own directory, which an ordinary account cannot write to.
    "TEMP",
    "TMP",
    // The home directory, under each name something reads it by. Git for
    // Windows builds `HOME` out of `HOMEDRIVE` and `HOMEPATH` when it is not
    // already set, and the POSIX tools beside its shell read the result.
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    // Where a tool keeps its configuration and its cache — the Windows halves
    // of what `HOME` answers elsewhere.
    "APPDATA",
    "LOCALAPPDATA",
    // Where software is installed. A program that locates what it wraps reads
    // these instead of searching the disk, which is what [`super::shell`] does
    // to find `sh.exe`.
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramData",
    // What kind of terminal a program may talk to, for the POSIX tools that
    // ship beside the shell and read it the way they do everywhere else.
    "TERM",
];

/// The listed variables `lookup` answers for, in the order [`INHERITED`] names
/// them.
///
/// The lookup is a parameter so the list can be tested against an environment
/// nobody had to write into this process — writing to one is `unsafe` in
/// edition 2024 and denied in this workspace, and it would leave one test
/// deciding what another one sees.
///
/// A name it does not answer for is left unset rather than invented, `PATH`
/// included: a shell started without one falls back to the default path POSIX
/// makes it carry, and inventing a different one here would be this file
/// deciding where programs come from.
pub(super) fn inherited(lookup: impl Fn(&str) -> Option<OsString>) -> Vec<(Box<str>, OsString)> {
    INHERITED
        .iter()
        .filter_map(|name| lookup(name).map(|value| ((*name).into(), value)))
        .collect()
}
