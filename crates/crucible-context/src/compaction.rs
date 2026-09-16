//! What a compaction asks the model for, and what it may keep.
//!
//! Compaction replaces the middle of a transcript with notes the model writes
//! about it. The request loop that asks for those notes belongs to the runner;
//! what the request says, which answer counts as whole, and which files the
//! notes carry forward are assembly facts and live here, beside the other words
//! a request is built from.

use std::fmt::Write as _;

use crucible_types::Compacted;

/// What asking for room came back with.
///
/// Three answers rather than an [`Option`], because the two that made no room
/// made none for opposite reasons and owe the reader different sentences: one
/// is a session with nothing behind the turns it keeps whole, and the other is
/// somebody who pressed a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// It was made, and this is what it took.
    Made(Compacted),
    /// There was nothing worth replacing — a session with no middle. Nothing
    /// was asked of the model and nothing changed.
    Nothing,
    /// Somebody stopped the recap while it was being written, so nothing
    /// changed. Half a session's memory is not one, and standing it in place of
    /// the messages it was meant to replace would lose the rest for good: the
    /// log still holds them, and nothing the model is sent ever would again.
    Stopped,
}

/// What the model is asked for, in place of the next turn.
///
/// Written as an instruction to carry on rather than as a request for prose: an
/// answer that reads as a report about a session is one the model then has to
/// re-read as its own memory. What this asks for is the memory itself.
///
/// The shape is fixed rather than free-form because a fixed one is harder to
/// drop a category from: left to its own wording, a recap quietly omits the one
/// decision that mattered. Each heading is a thing the next turn cannot do
/// without. The files are not the model's to recall — they are collected from
/// the calls being replaced and appended by code after validation, so the list
/// survives a second compaction instead of going out with the first recap.
pub const RECAP_REQUEST: &str = "\
Before anything else, create a structured context checkpoint from everything \
above so another model pass can continue the work. Use exactly every heading \
and subheading below, in this order. Keep each section concise. Write `(none)` \
where a section has nothing to say rather than omitting it.\n\n\
## Goal\n\
## Constraints & Preferences\n\
## Progress\n\
### Done\n\
### In Progress\n\
### Blocked\n\
## Decisions\n\
## Next Steps\n\
## Critical Context\n\n\
Preserve exact file paths, function and type names, commands, error messages, \
requirements, decisions and unfinished state. Write operational notes for \
yourself, not a report to the user. End after the content of \
`## Critical Context`; the exact `Files so far` list is appended by the program. \
Output nothing before `## Goal` or after that final section.";

/// The line the tracked files stand under in a recap.
///
/// Read back on replay to pull the list a previous recap carried into the next
/// one, so the record of which files a session touched survives being compacted
/// twice. The log line is the only copy — the runner keeps no state of its own
/// across compactions, and the recap it already wrote is the record.
const FILES: &str = "Files so far:";

/// Whether every required checkpoint section is present, ordered and filled.
///
/// `Progress` is a container; its three subsections carry the content. Every
/// other section must say something, including `(none)`, so a clean provider
/// stop cannot make a structurally truncated checkpoint look complete.
pub fn is_structured(said: &str) -> bool {
    const SECTIONS: &[(&str, bool)] = &[
        ("## Goal", true),
        ("## Constraints & Preferences", true),
        ("## Progress", false),
        ("### Done", true),
        ("### In Progress", true),
        ("### Blocked", true),
        ("## Decisions", true),
        ("## Next Steps", true),
        ("## Critical Context", true),
    ];

    if !said.starts_with("## Goal\n") || said.lines().any(|line| line == FILES) {
        return false;
    }

    let lines: Vec<&str> = said.lines().collect();
    let headings: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| line.starts_with("##").then_some(at))
        .collect();
    if headings.len() != SECTIONS.len()
        || headings
            .iter()
            .zip(SECTIONS)
            .any(|(at, (expected, _))| lines.get(*at) != Some(expected))
    {
        return false;
    }

    SECTIONS.iter().enumerate().all(|(index, (_, required))| {
        if !required {
            return true;
        }
        let Some(start) = headings.get(index).map(|heading| heading + 1) else {
            return false;
        };
        let end = headings.get(index + 1).copied().unwrap_or(lines.len());
        lines
            .get(start..end)
            .is_some_and(|section| section.iter().any(|line| !line.trim().is_empty()))
    })
}

/// Appends the file record derived from calls and prior checkpoints.
pub fn append_files(recap: &mut String, touched: &TrackedFiles) {
    while recap.ends_with(char::is_whitespace) {
        recap.pop();
    }
    let _ = write!(recap, "\n\n{FILES}\n");
    if touched.read.is_empty() && touched.modified.is_empty() {
        recap.push_str("(none yet)");
        return;
    }
    for path in &touched.read {
        let _ = writeln!(recap, "{path} (read)");
    }
    for path in &touched.modified {
        let _ = writeln!(recap, "{path} (modified)");
    }
    recap.pop();
}

/// The files a session has touched, read and modified kept apart.
///
/// The two lists a recap carries, accumulated as the span being replaced is
/// walked. Each file appears once, in the order it was first noted; a file that
/// was ever changed is on the modified list and nowhere else, because that is
/// the fact a later turn cannot do without.
#[derive(Debug, Default)]
pub struct TrackedFiles {
    read: Vec<String>,
    modified: Vec<String>,
}

impl TrackedFiles {
    /// The files only ever read, in the order first noted.
    #[must_use]
    pub fn read(&self) -> &[String] {
        &self.read
    }

    /// The files changed at least once, in the order first changed.
    #[must_use]
    pub fn modified(&self) -> &[String] {
        &self.modified
    }

    /// Notes one file, read or changed.
    ///
    /// A change wins over a read: the same file may be opened a dozen times and
    /// edited once, and the edit is what the next session needs to know about.
    /// A file already changed stays changed however many reads follow, and one
    /// already listed is not listed again.
    pub fn note(&mut self, path: &str, changed: bool) {
        if changed {
            self.read.retain(|kept| kept != path);
            if !self.modified.iter().any(|kept| kept == path) {
                self.modified.push(path.to_owned());
            }
        } else if !self.modified.iter().any(|kept| kept == path)
            && !self.read.iter().any(|kept| kept == path)
        {
            self.read.push(path.to_owned());
        }
    }
}

/// The files a prior recap carried, read back off the text it left.
///
/// Everything from the `Files so far:` line to the end, one `path (read)` or
/// `path (modified)` per line. Anything that does not parse as one of those is
/// left out rather than guessed at: a line from an older recap written some
/// other way is not a file this session touched. New recaps receive this list
/// from code, not from the model.
pub fn carried(recap: &str) -> Vec<(&str, bool)> {
    let Some((_, files)) = recap.split_once(FILES) else {
        return Vec::new();
    };

    files
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some(path) = line.strip_suffix("(modified)") {
                Some((path.trim(), true))
            } else {
                line.strip_suffix("(read)").map(|path| (path.trim(), false))
            }
        })
        .filter(|(path, _)| !path.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_structured_recap_has_every_heading_once_in_exact_order() {
        let complete = "## Goal\ngoal\n## Constraints & Preferences\n(none)\n## Progress\n### Done\ndone\n### In Progress\n(none)\n### Blocked\n(none)\n## Decisions\n(none)\n## Next Steps\nnext\n## Critical Context\n(none)";
        assert!(is_structured(complete));

        let extra = complete.replace("## Decisions", "## Surprise\nextra\n## Decisions");
        assert!(!is_structured(&extra));

        let duplicate = complete.replace("## Next Steps", "## Decisions\nagain\n## Next Steps");
        assert!(!is_structured(&duplicate));

        let empty = complete.replace("## Critical Context\n(none)", "## Critical Context");
        assert!(!is_structured(&empty));
    }

    #[test]
    fn a_recap_without_a_file_list_carries_none_forward() {
        // A recap written before this existed, or by a model that left the list
        // out, has nothing to carry — and that is an answer, not a failure.
        assert!(carried("## Goal\nbuild the thing").is_empty());
    }

    #[test]
    fn the_files_a_recap_kept_are_read_back_the_way_they_were_written() {
        let recap =
            "## State\nnext: ship it\n\nFiles so far:\nsrc/main.rs (modified)\nREADME.md (read)\n";

        assert_eq!(
            carried(recap),
            [("src/main.rs", true), ("README.md", false)]
        );
    }

    #[test]
    fn a_line_that_is_not_a_file_is_not_read_as_one() {
        // Older recap text may have carried this list itself. A line it wrote
        // some other way is not a file the session touched and is left out
        // rather than guessed at; new recaps receive the list from code.
        let recap = "Files so far:\nsrc/main.rs (read)\nnot a file line\n(modified)\n";

        assert_eq!(carried(recap), [("src/main.rs", false)]);
    }

    #[test]
    fn a_file_is_listed_once_and_a_change_outranks_a_read() {
        // The rules the recap's accuracy rests on: no file twice, and the edit
        // is the fact a later turn needs about a file it also only read.
        let mut files = TrackedFiles::default();
        files.note("src/main.rs", false);
        files.note("src/main.rs", false);
        assert_eq!(files.read(), ["src/main.rs".to_owned()]);

        // Read first, then changed: it moves, because the read is no longer the
        // truest thing to say about it.
        files.note("src/main.rs", true);
        assert!(
            files.read().is_empty(),
            "a changed file is still listed as read"
        );
        assert_eq!(files.modified(), ["src/main.rs".to_owned()]);

        // And changed first, then read: it stays changed, however many reads
        // follow.
        files.note("src/lib.rs", true);
        files.note("src/lib.rs", false);
        assert_eq!(
            files.modified(),
            ["src/main.rs".to_owned(), "src/lib.rs".to_owned()]
        );
        assert!(!files.read().iter().any(|kept| kept == "src/lib.rs"));
    }
}
