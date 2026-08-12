//! What a walk still refuses, file by file.
//!
//! A verdict is reached about the directory a search walks, so every file
//! below it is one nobody was asked about, and a rule written about such a
//! file has no other moment to be honoured in. The target a walk builds is
//! spelled only the ways the denials in force will read, which is what these
//! pin: a spelling that stopped being built would read back as a path no
//! pattern names, and a denial would quietly stop covering a file.
//!
//! Each kind of pattern is put in force on its own as well as together. Two of
//! them together ask for both spellings between them, so a target would still
//! hold whichever one either pattern went on to read — the case that can tell
//! a pattern from its opposite is the one where only that pattern is written.

use std::fs;
use std::path::Path;

use super::*;
use crate::workspace::{Workspace, WorkspacePath, written};

/// How a denial was written, which is what decides the spelling it is read
/// against.
#[derive(Clone, Copy)]
enum Written {
    /// As an absolute path. Names `secret.env`.
    Absolutely,
    /// Relative to the workspace root. Names `sub/keys.txt`.
    BelowRoot,
}

/// A workspace holding a file for each kind of denial to name, and a `grep`
/// approved to walk it under exactly the denials asked for.
struct Walk {
    workspace: Workspace,
    from: WorkspacePath,
    approved: Approved,
}

impl Walk {
    fn under(name: &str, denials: &[Written]) -> Self {
        let base =
            std::env::temp_dir().join(format!("crucible-walk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("secret.env"), "k").unwrap();
        fs::write(root.join("sub/keys.txt"), "k").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let workspace = Workspace::open(&root).unwrap();
        let from = workspace.existing(".").unwrap();

        // None of them names the directory the walk starts at, so the call
        // itself is allowed and what is left for them to stop is a file it
        // reaches on its own.
        let texts: Vec<String> = denials
            .iter()
            .map(|denial| match denial {
                Written::Absolutely => {
                    format!("grep({}/secret.env)", written(workspace.root()))
                }
                Written::BelowRoot => "grep(sub/keys.txt)".to_owned(),
            })
            .collect();

        let mut permission = with(
            Mode::FullAccess,
            &texts
                .iter()
                .map(|text| (Disposition::Deny, text.as_str()))
                .collect::<Vec<_>>(),
        );

        let mut answer = Answer::once(Verdict::Deny);
        let settled = permission.decide(
            &call("grep"),
            &Sensitivity::ReadOnly {
                target: Target::resolved(&workspace, &from),
            },
            &mut answer,
        );

        let Settled::Approved(approved) = settled else {
            panic!("no denial names the directory the walk starts at, so it is allowed")
        };

        Self {
            workspace,
            from,
            approved,
        }
    }

    /// Whether the walk refuses a file below the root it started at.
    fn refuses(&self, below: &str) -> bool {
        self.reaching(&self.workspace.root().join(below))
    }

    /// Whether the walk refuses a path, wherever it came from.
    fn reaching(&self, path: &Path) -> bool {
        self.approved.denies(&self.workspace, &self.from, path)
    }
}

impl Drop for Walk {
    fn drop(&mut self) {
        if let Some(base) = self.workspace.root().parent() {
            let _ = fs::remove_dir_all(base);
        }
    }
}

#[test]
fn a_denial_written_as_an_absolute_path_reaches_a_walked_file_on_its_own() {
    let walk = Walk::under("absolutely", &[Written::Absolutely]);

    assert!(walk.refuses("secret.env"), "the file it names");
    assert!(
        !walk.refuses("sub/keys.txt"),
        "a file nothing in force names"
    );
}

#[test]
fn a_denial_written_below_the_root_reaches_a_walked_file_on_its_own() {
    let walk = Walk::under("below-root", &[Written::BelowRoot]);

    assert!(walk.refuses("sub/keys.txt"), "the file it names");
    assert!(!walk.refuses("secret.env"), "a file nothing in force names");
}

#[test]
fn both_kinds_of_denial_hold_together() {
    let walk = Walk::under("together", &[Written::Absolutely, Written::BelowRoot]);

    assert!(walk.refuses("secret.env"));
    assert!(walk.refuses("sub/keys.txt"));
    assert!(
        !walk.refuses("src/main.rs"),
        "a file no denial names must stay in the answer"
    );
}

#[test]
fn a_path_the_walk_could_not_have_descended_to_is_refused() {
    let walk = Walk::under("not-descended", &[Written::BelowRoot]);

    // Nothing written here names it. It is refused anyway: this call cannot
    // say where such a path came from, and the answer that costs nothing is
    // the one that keeps a file out of an answer.
    assert!(walk.reaching(&walk.workspace.root().with_file_name("beside.txt")));
}

#[test]
fn the_target_a_call_is_decided_about_holds_both_spellings() {
    // The one built per file is spelled sparingly; the one built per call is
    // not, and must not become so. A prompt shows the short spelling and the
    // rule a "don't ask again" mints is written from it, so both have to be
    // there however few of them the denials in force happen to read.
    let walk = Walk::under("both-spellings", &[Written::BelowRoot]);
    let file = walk.workspace.existing("sub/keys.txt").unwrap();
    let target = Target::resolved(&walk.workspace, &file);

    assert_eq!(target.below_root(), Some("sub/keys.txt"));
    assert_eq!(
        target.absolute(),
        Some(&*written(&walk.workspace.root().join("sub/keys.txt")))
    );
}
