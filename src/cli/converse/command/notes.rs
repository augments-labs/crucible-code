//! `/release-notes`: every release crucible has had, read out of the changelog
//! it was built with.
//!
//! The one place that knows how `CHANGELOG.md` is written. The file is built
//! into the binary rather than read from anywhere, so what is printed is the
//! history of the very tree the program came from, and asking for it opens no
//! socket and reads no file. Nothing here is read or laid out until the
//! command runs.

/// The changelog of the tree this binary was built from.
pub(super) const CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");

/// One release, as the changelog records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Release<'a> {
    /// Its number, as the heading writes it: `0.43.3`.
    pub(super) version: &'a str,
    /// The day it was cut, as the heading writes it.
    pub(super) date: &'a str,
    /// Everything under its heading, up to the next release's.
    body: &'a str,
}

/// The groups an older release's row names first, in this order; any other is
/// named after them, by name.
const ORDER: [&str; 5] = ["added", "changed", "fixed", "removed", "security"];

/// Every release `changelog` records, oldest first.
///
/// A release is a `## [x.y.z] - date` heading and what is under it, up to the
/// next heading of that level. `Unreleased` has no number and is no release,
/// and the link lines at the foot of the file belong to none.
pub(super) fn releases(changelog: &str) -> Vec<Release<'_>> {
    let mut found = Vec::new();
    let mut open: Option<(&str, &str, usize)> = None;
    let mut at = 0;

    for line in changelog.split_inclusive('\n') {
        let starts = at;
        at += line.len();
        if !line.starts_with("## ") {
            continue;
        }
        if let Some((version, date, from)) = open.take() {
            found.push(Release::of(version, date, changelog.get(from..starts)));
        }
        open = heading(line.trim_end()).map(|(version, date)| (version, date, at));
    }
    if let Some((version, date, from)) = open {
        found.push(Release::of(version, date, changelog.get(from..)));
    }

    found.reverse();
    found
}

/// The version and date a release's heading gives: `## [0.43.3] - 2026-09-29`.
fn heading(line: &str) -> Option<(&str, &str)> {
    let (version, rest) = line.strip_prefix("## [")?.split_once(']')?;
    let date = rest.strip_prefix(" - ")?.trim();
    asked(version)
        .filter(|_| !date.is_empty())
        .map(|version| (version, date))
}

impl<'a> Release<'a> {
    /// The release under a heading, its body cut free of the link lines that
    /// close the file.
    fn of(version: &'a str, date: &'a str, body: Option<&'a str>) -> Self {
        let body = body.unwrap_or_default();
        let end = body
            .split_inclusive('\n')
            .scan(0, |at, line| {
                let starts = *at;
                *at += line.len();
                Some((starts, line))
            })
            .find(|(_, line)| is_link(line))
            .map_or(body.len(), |(starts, _)| starts);
        Self {
            version,
            date,
            body: body.get(..end).unwrap_or(body).trim(),
        }
    }
}

/// Whether `line` is one of the link definitions at the foot of the file:
/// `[0.43.3]: https://...`.
fn is_link(line: &str) -> bool {
    line.strip_prefix('[')
        .and_then(|rest| rest.split_once("]: "))
        .is_some()
}

impl Release<'_> {
    /// How many entries each group holds, in the order the row says them.
    ///
    /// An entry is a bullet at the start of a line under the group's heading;
    /// a group that holds paragraphs and no bullet counts its paragraphs.
    /// Bullets above any heading are changes. A release with no heading and
    /// no bullet counts nothing.
    pub(super) fn groups(&self) -> Vec<(String, usize)> {
        let mut groups: Vec<(String, usize, usize)> = Vec::new();
        let mut blank = true;

        for line in self.body.lines() {
            if let Some(name) = line.strip_prefix("### ") {
                groups.push((name.trim().to_lowercase(), 0, 0));
                blank = true;
                continue;
            }
            if line.trim().is_empty() {
                blank = true;
                continue;
            }
            if line.starts_with("- ") {
                if groups.is_empty() {
                    groups.push(("changed".to_owned(), 0, 0));
                }
                if let Some((_, bullets, _)) = groups.last_mut() {
                    *bullets += 1;
                }
            } else if blank && !line.starts_with(' ') {
                if let Some((_, _, paragraphs)) = groups.last_mut() {
                    *paragraphs += 1;
                }
            }
            blank = false;
        }

        let mut counted: Vec<(String, usize)> = Vec::new();
        for (name, bullets, paragraphs) in groups {
            let entries = if bullets > 0 { bullets } else { paragraphs };
            match counted.iter_mut().find(|(seen, _)| *seen == name) {
                Some((_, count)) => *count += entries,
                None => counted.push((name, entries)),
            }
        }
        counted.retain(|(_, count)| *count > 0);
        counted.sort_by(|(one, _), (other, _)| {
            let rank = |name: &str| ORDER.iter().position(|named| *named == name);
            match (rank(one), rank(other)) {
                (Some(one), Some(other)) => one.cmp(&other),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => one.cmp(other),
            }
        });
        counted
    }

    /// The groups as the row of an older release says them:
    /// `8 added · 2 changed · 18 fixed`.
    pub(super) fn counted(&self) -> String {
        self.groups()
            .iter()
            .map(|(name, count)| format!("{count} {name}"))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The release's words as they are drawn in full.
    ///
    /// The changelog is markdown, which the transcript reads as it reads an
    /// answer, with one mark of its own on top: a key written between
    /// `<kbd>` tags. The key's name is what a reader wants, so the tags go.
    pub(super) fn text(&self) -> String {
        self.body.replace("<kbd>", "").replace("</kbd>", "")
    }
}

/// The version a word asks for, where it is one: `0.41.1`, or `v0.41.1`.
///
/// Three numbers joined by dots, as every release is numbered; the leading
/// `v` is how tags and release pages write them.
pub(super) fn asked(word: &str) -> Option<&str> {
    let version = word.strip_prefix('v').unwrap_or(word);
    let mut parts = version.split('.');
    let numbered = parts
        .by_ref()
        .take(3)
        .filter(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        .count();
    (numbered == 3 && parts.next().is_none()).then_some(version)
}

#[cfg(test)]
mod tests;
