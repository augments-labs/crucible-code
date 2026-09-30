//! `/release-notes`: every release crucible has had, read out of the changelog
//! it was built with.
//!
//! The one place that knows how `CHANGELOG.md` is written. The file is built
//! into the binary rather than read from anywhere, so what is printed is the
//! history of the very tree the program came from, and asking for it opens no
//! socket and reads no file. Nothing here is read or laid out until the
//! command runs.
//!
//! A client with no terminal is answered from here too, in the contract's
//! bounded values rather than rows: the application lends it the host's
//! [`answered`], since the changelog is the host's and its format is this
//! module's.

use crucible_client_api::bounds::ITEMS;
use crucible_client_api::{Group, Name, NotesOutcome, Refusal, Text};
use crucible_tui::{Brief, Forge, Glyphs, RECORDED, Renderer, Row, Terminal, Timeline, Told};

use crate::cli::Fatal;

/// The changelog of the tree this binary was built from.
pub(super) const CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");

/// The version this binary is.
const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// How many of the newest releases are told in full.
const FULL: usize = 10;

/// What the closing row ends on: how to have one release on its own.
const ONE: &str = "/release-notes <version> prints one";

/// Where a bare number in the changelog points: crucible's own repository,
/// never whatever the reader is standing in.
fn forge() -> Forge {
    Forge::new(
        "https://github.com",
        "augments-labs/crucible-code",
        "/issues/",
    )
}

/// What `/release-notes` printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Printed {
    /// Releases, set apart from the line that asked: a timeline has a rail of
    /// its own down the left, and rows hung under a mark would be a second one
    /// beside it.
    Releases,
    /// A refusal, which hangs under the line that asked as any command's
    /// answer does.
    Refusal,
}

/// Runs `/release-notes`: every release with nothing after it, and the one
/// named with a version after it.
///
/// Printed into the transcript, where it is read by scrolling and copied like
/// anything else there, rather than stood over the box.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn run<T: Terminal>(
    rest: &str,
    renderer: &mut Renderer<T>,
    glyphs: Glyphs,
) -> Result<Printed, Fatal> {
    let releases = releases(CHANGELOG);
    let columns = renderer.columns();
    let newest = releases.last().map_or(RUNNING, |release| release.version);
    let dot = glyphs.dot();

    let words: Vec<&str> = rest.split_whitespace().collect();
    let found = match words.as_slice() {
        [] => Ok(whole(&releases, columns, glyphs)),
        [word] => match asked(word) {
            Some(version) => releases
                .iter()
                .find(|release| release.version == version)
                .map(|release| alone(release, columns, glyphs))
                .ok_or_else(|| format!("! no release {version} {dot} newest is {newest}")),
            None => Err(format!(
                "! not a version: {word} {dot} write it as {newest}"
            )),
        },
        _ => Err(format!(
            "! not a version: {} {dot} write it as {newest}",
            rest.trim()
        )),
    };

    match found {
        Ok(rows) => {
            renderer.apart()?;
            renderer.present(&rows)?;
            Ok(Printed::Releases)
        }
        Err(refusal) => {
            renderer.commit(&refusal)?;
            Ok(Printed::Refusal)
        }
    }
}

/// The release notes a client asked for: every release where it named none,
/// and otherwise the release of the version it named.
///
/// # Errors
///
/// What the contract refuses a value for, where the changelog holds one it
/// cannot carry.
pub(crate) fn answered(version: Option<&str>) -> Result<NotesOutcome, Refusal> {
    answer(CHANGELOG, version)
}

/// [`answered`], from `changelog`.
///
/// Every release is the newest [`ITEMS`] of them, which is all a list may
/// hold, with the oldest left out past that and the answer saying so. The
/// newest ten carry their words, as the terminal tells them in full; words
/// past the contract's ceiling are cut, and say so.
fn answer(changelog: &str, version: Option<&str>) -> Result<NotesOutcome, Refusal> {
    let releases = releases(changelog);
    let newest = Name::new(releases.last().map_or(RUNNING, |release| release.version))?;

    Ok(match version {
        None => {
            let left = releases.len().saturating_sub(ITEMS);
            let worded = releases.len().saturating_sub(FULL);
            NotesOutcome::Listed {
                releases: releases
                    .iter()
                    .enumerate()
                    .skip(left)
                    .map(|(at, release)| release.answered(at >= worded))
                    .collect::<Result<_, _>>()?,
                running: Name::new(RUNNING)?,
                truncated: left > 0,
            }
        }
        Some(word) => match asked(word) {
            Some(version) => match releases.iter().find(|release| release.version == version) {
                Some(release) => NotesOutcome::One(release.answered(true)?),
                None => NotesOutcome::Unknown { newest },
            },
            None => NotesOutcome::NotAVersion { newest },
        },
    })
}

/// Every release on the rail: the oldest a row each, the newest ten in full.
///
/// No more rows than the transcript keeps, less the blank row the command
/// ends on, so the first row printed is still there after the last.
pub(super) fn whole(releases: &[Release<'_>], columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let split = releases.len().saturating_sub(FULL);
    let groups: Vec<Vec<(String, usize)>> = releases.iter().map(Release::groups).collect();
    let counted: Vec<Vec<(&str, usize)>> = groups
        .iter()
        .map(|groups| {
            groups
                .iter()
                .map(|(kind, count)| (kind.as_str(), *count))
                .collect()
        })
        .collect();
    let briefs: Vec<Brief<'_>> = releases
        .iter()
        .zip(&counted)
        .map(|(release, counted)| Brief {
            version: release.version,
            date: release.date,
            counted,
        })
        .collect();
    let texts: Vec<String> = releases.iter().skip(split).map(Release::text).collect();
    let told: Vec<Told<'_>> = briefs
        .iter()
        .skip(split)
        .zip(&texts)
        .map(|(brief, text)| Told {
            brief: *brief,
            text,
        })
        .collect();
    let forge = forge();

    Timeline {
        older: briefs.get(..split).unwrap_or_default(),
        told: &told,
        running: Some(RUNNING),
        forge: Some(&forge),
        closing: Some(ONE),
        most: RECORDED.saturating_sub(1),
    }
    .rows(columns, glyphs)
}

/// One release in full, with no rail and nothing after it.
pub(super) fn alone(release: &Release<'_>, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let counted: Vec<(String, usize)> = release.groups();
    let counted: Vec<(&str, usize)> = counted
        .iter()
        .map(|(kind, count)| (kind.as_str(), *count))
        .collect();
    let text = release.text();
    let told = [Told {
        brief: Brief {
            version: release.version,
            date: release.date,
            counted: &counted,
        },
        text: &text,
    }];
    let forge = forge();

    Timeline {
        older: &[],
        told: &told,
        running: Some(RUNNING),
        forge: Some(&forge),
        closing: None,
        most: RECORDED.saturating_sub(1),
    }
    .rows(columns, glyphs)
}

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
    /// The release as a client is told it, with its words where `worded`.
    fn answered(&self, worded: bool) -> Result<crucible_client_api::Release, Refusal> {
        Ok(crucible_client_api::Release {
            version: Name::new(self.version)?,
            date: Name::new(self.date)?,
            groups: self
                .groups()
                .into_iter()
                .map(|(kind, count)| {
                    Ok(Group {
                        kind: Name::new(&kind)?,
                        count: u64::try_from(count).unwrap_or(u64::MAX),
                    })
                })
                .collect::<Result<_, Refusal>>()?,
            text: worded.then(|| Text::cut(&self.text())),
        })
    }

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
            } else if blank
                && !line.starts_with(' ')
                && let Some((_, _, paragraphs)) = groups.last_mut()
            {
                *paragraphs += 1;
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

    /// The release's words as they are drawn in full.
    ///
    /// The changelog is markdown, which the transcript reads as it reads an
    /// answer, with two habits of its own. Its prose is wrapped at eighty
    /// columns in the file, which is where the file is read and not where the
    /// words are drawn, so a line that only carries a paragraph or an entry on
    /// is joined to the one before it; a blank line, a heading, a new entry, a
    /// table and a block of code each keep their breaks. And a key is written
    /// between `<kbd>` tags, where what a reader wants is the key's name.
    pub(super) fn text(&self) -> String {
        let mut text = String::with_capacity(self.body.len());
        let mut fenced = false;
        let mut open = false;

        for line in self.body.lines() {
            let trimmed = line.trim_start();
            let fence = trimmed.starts_with("```");
            let carries = open && !fenced && !fence && !starts_a_block(trimmed);

            if carries {
                text.push(' ');
                text.push_str(trimmed);
            } else {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(line);
            }

            if fence {
                fenced = !fenced;
            }
            open = !fenced && !fence && !trimmed.is_empty() && !is_rowed(trimmed);
        }

        text.replace("<kbd>", "").replace("</kbd>", "")
    }
}

/// Whether a line of the changelog opens something of its own rather than
/// carrying on the line above: a blank, a heading, an entry, a table row.
fn starts_a_block(line: &str) -> bool {
    line.is_empty()
        || line.starts_with('#')
        || line.starts_with("- ")
        || line.starts_with("* ")
        || is_rowed(line)
        || numbered(line)
}

/// Whether a line is a row of a table, which keeps its own break.
fn is_rowed(line: &str) -> bool {
    line.starts_with('|')
}

/// Whether a line opens a numbered entry: `1. `.
fn numbered(line: &str) -> bool {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && line
            .get(digits..)
            .is_some_and(|rest| rest.starts_with(". "))
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
