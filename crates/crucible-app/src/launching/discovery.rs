//! How an agent's program is found and where it comes from: the commands its
//! row is known by, looked for in order on `PATH` or taken as written, what a
//! refusal over a program not found says, and whether a program found is a
//! release in crucible's install store or the user's own.

use std::ffi::OsString;
use std::path::Path;

use crucible_external_harnesses::Discovery;
use crucible_external_harnesses::catalog::Trust;

use crate::backends::AgentRow;
use crate::installing;

/// Where an agent's program comes from, as the launch holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// It lies in crucible's install store, as a release the store keeps or
    /// somewhere there no release of its own is, which is refused.
    Stored,
    /// It lies outside the store: the user's own program, found on `PATH` or
    /// named by its path.
    Own,
}

impl Origin {
    /// Where the program at `program` comes from, judged as `stored_under`
    /// places it beneath the install store `programs`: own only where it lies
    /// nowhere in the store, so one in another agent's directory or in no
    /// version is still the store's.
    #[must_use]
    pub fn of(programs: &Path, program: &Path) -> Self {
        stored_under(programs, program).map_or(Self::Own, |_| Self::Stored)
    }
}

/// The directory `program` lies in directly beneath `programs`, as either is
/// written or once both are resolved, where it lies beneath it in either
/// form; resolved first, so a link or another spelling of a directory is
/// judged by where it leads.
pub(super) fn stored_under(programs: &Path, program: &Path) -> Option<String> {
    let resolved = |path: &Path| std::fs::canonicalize(path).ok();
    [
        (resolved(programs), resolved(program)),
        (Some(programs.to_path_buf()), Some(program.to_path_buf())),
    ]
    .into_iter()
    .find_map(|(root, path)| {
        let within = path?.strip_prefix(root?).ok()?.to_path_buf();
        within
            .components()
            .next()
            .map(|first| first.as_os_str().to_string_lossy().into_owned())
    })
}

/// What a refusal over `discovery` says of `row`'s agent called `shown`, as the
/// sentence and what to do next: for an agent built in whose release `trust`
/// offers, that what to do is to run the command that installs it. A program
/// not found is named by every command it was looked for as, and the sentence
/// says how to name one installed under another. Nothing for a program that
/// was found.
pub(crate) fn unfound(
    row: &AgentRow,
    discovery: &Discovery,
    shown: &str,
    trust: &Trust,
) -> Option<[String; 2]> {
    let looked_for = looked_for(row);
    let refused = match row {
        AgentRow::Built(built) => installing::refusal(built, discovery, shown, &looked_for, trust),
        AgentRow::Catalog(_) => discovery.refusal(shown, &looked_for),
    };
    refused.map(|refused| renamed(discovery, refused))
}

/// Every command `row`'s program is looked for as, as a refusal names them.
pub(crate) fn looked_for(row: &AgentRow) -> String {
    row.commands().join(" or ")
}

/// What a refusal over `discovery` says, `refused`, with how to name a program
/// installed under a name its row does not know, where none was found.
pub(crate) fn renamed(discovery: &Discovery, refused: [String; 2]) -> [String; 2] {
    let [said, next] = refused;
    match discovery {
        Discovery::Missing => [
            format!(
                "{said} If it is installed under another name, set executable to its path or name."
            ),
            next,
        ],
        Discovery::Found(_) | Discovery::NotExecutable(_) => [said, next],
    }
}

/// What the program configured as `executable` is: the path as written when
/// it is absolute, else what an absolute lookup on `PATH` finds. An
/// `executable` that is one of `names`, the commands its row is known by, is
/// looked for as each of them in order: the first that can be run is taken,
/// else the first that is there but cannot be run. Any other is looked for as
/// written.
pub fn discover(
    executable: &str,
    names: &[String],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Discovery {
    let written = Path::new(executable);
    if written.is_absolute() {
        return Discovery::of(Some(written));
    }
    let known = names.iter().any(|name| name == executable);
    let tried: Vec<&str> = if known {
        names.iter().map(String::as_str).collect()
    } else {
        vec![executable]
    };
    // A stale name that cannot be run must not hide a later one that can.
    let mut unrunnable = None;
    for name in tried {
        let found = crucible_builtins::program::on_path(
            &lookup,
            &crucible_builtins::program::spelled(name),
        );
        match Discovery::of(found.as_deref()) {
            runnable @ Discovery::Found(_) => return runnable,
            there @ Discovery::NotExecutable(_) => {
                unrunnable.get_or_insert(there);
            }
            Discovery::Missing => {}
        }
    }
    unrunnable.unwrap_or(Discovery::Missing)
}
