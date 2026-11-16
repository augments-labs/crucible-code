//! A git graph read a line at a time: its commits, the branches they are made
//! on, and the merges between them.
//!
//! The engine reads a git graph more loosely than Mermaid: it passes over a
//! word it does not know, makes a branch named twice its checkout, and
//! merges a branch into itself. So this holds each statement to what Mermaid
//! accepts and refuses the diagram where Mermaid would: a branch named twice,
//! made before its branch has a commit, a checkout of one not made, a merge
//! of a branch into itself, into one at the same commit, or of one with no
//! commit of its own, and an id given twice. A commit's type, a cherry-pick,
//! a branch's order and a merge's own id or tag are not drawn as Mermaid
//! draws them, and refuse it too; so does a setting written before the graph
//! (`%%{init: …}%%`), which may rename `main` or turn its labels.

use std::collections::{BTreeMap, BTreeSet};

use crate::scan::{Read, Refused, Step, uncommented};

/// The branch a git graph starts on.
const MAIN: &str = "main";

/// The commits made on one branch, and the commit it stands at.
#[derive(Debug, Clone, Copy, Default)]
struct Branch {
    /// Whether a commit has been made on the branch itself.
    committed: bool,
    /// The commit the branch stands at, by its place in the graph.
    head: Option<usize>,
}

/// `cleaned` read as a git graph into `read`: the statements kept, one to a
/// line, and what they name.
pub(crate) fn read(cleaned: &str, mut read: Read) -> Result<(String, Read), Refused> {
    if cleaned
        .lines()
        .any(|line| line.trim_start().starts_with("%%{"))
    {
        return Err(Refused::Unread);
    }
    let mut lines = cleaned
        .lines()
        .map(|line| uncommented(line).trim())
        .filter(|line| !line.is_empty());
    let header = lines.next().ok_or(Refused::Empty)?;
    if header != "gitGraph" {
        return Err(Refused::Unread);
    }
    let mut kept = vec![header.to_owned()];
    let mut branches = BTreeMap::from([(MAIN.to_owned(), Branch::default())]);
    let mut current = MAIN.to_owned();
    let mut ids = BTreeSet::new();
    let mut commits = 0_usize;
    for line in lines {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["commit", ..] => {
                let (id, tag) = attributes(line.get("commit".len()..).unwrap_or_default())?;
                if let Some(id) = &id
                    && !ids.insert(id.clone())
                {
                    return Err(Refused::Unread);
                }
                read.placed(Step::Commit {
                    branch: current.clone(),
                    id,
                    tag,
                    merged: None,
                })?;
                moved(&mut branches, &current, commits);
                commits = commits.saturating_add(1);
            }
            ["branch", name] => {
                let from = branches.get(&current).copied().unwrap_or_default();
                if !is_branch(name) || branches.contains_key(*name) || from.head.is_none() {
                    return Err(Refused::Unread);
                }
                read.joined(None)?;
                read.stepped(Step::Branch((*name).to_owned()));
                branches.insert(
                    (*name).to_owned(),
                    Branch {
                        committed: false,
                        head: from.head,
                    },
                );
                (*name).clone_into(&mut current);
            }
            ["checkout", name] if branches.contains_key(*name) => {
                (*name).clone_into(&mut current);
            }
            ["merge", name] => {
                let into = branches.get(&current).copied().unwrap_or_default();
                let from = branches.get(*name).copied().ok_or(Refused::Unread)?;
                if *name == current
                    || !from.committed
                    || into.head.is_none()
                    || into.head == from.head
                {
                    return Err(Refused::Unread);
                }
                read.placed(Step::Commit {
                    branch: current.clone(),
                    id: None,
                    tag: None,
                    merged: Some((*name).to_owned()),
                })?;
                moved(&mut branches, &current, commits);
                commits = commits.saturating_add(1);
            }
            _ => return Err(Refused::Unread),
        }
        kept.push(line.to_owned());
    }
    Ok((kept.join("\n"), read))
}

/// `branch` moved on to the commit at `commit`, made on it.
fn moved(branches: &mut BTreeMap<String, Branch>, branch: &str, commit: usize) {
    if let Some(moved) = branches.get_mut(branch) {
        moved.committed = true;
        moved.head = Some(commit);
    }
}

/// The id and the tag a commit is written with: what follows `commit`, each
/// as `id: "…"` or `tag: "…"`, at most once.
fn attributes(written: &str) -> Result<(Option<String>, Option<String>), Refused> {
    let mut rest = written.trim_start();
    let (mut id, mut tag) = (None, None);
    while !rest.is_empty() {
        let (key, after) = rest.split_once(':').ok_or(Refused::Unread)?;
        let held = match key.trim_end() {
            "id" => &mut id,
            "tag" => &mut tag,
            _ => return Err(Refused::Unread),
        };
        let quoted = after
            .trim_start()
            .strip_prefix('"')
            .ok_or(Refused::Unread)?;
        let (value, after) = quoted.split_once('"').ok_or(Refused::Unread)?;
        if held.is_some() || value.trim().is_empty() {
            return Err(Refused::Unread);
        }
        *held = Some(value.to_owned());
        rest = after.trim_start();
    }
    Ok((id, tag))
}

/// Whether `name` is a branch's name as this reads one: letters, digits, and
/// `_`, `-`, `/` and `.`.
fn is_branch(name: &str) -> bool {
    name.chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-' | '/' | '.'))
}
