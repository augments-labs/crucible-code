//! A gantt read a line at a time: its title, its date and axis formats, its
//! sections and the tasks in them.
//!
//! The engine draws a task from a start and a length in days, so this reads
//! only what gives it both as Mermaid gives them: a start that is a date, the
//! day after a task named before it, or the day after the task above it in
//! its section; and a length in days or weeks. A task that would start on the
//! day the diagram is drawn (the first in its section with no start written),
//! a length in hours, a milestone, and the days a gantt leaves out
//! (`excludes`, `includes`) each refuse the diagram, since the engine would
//! draw it otherwise than Mermaid, or differently from one day to the next.
//! A status (`done`, `active`, `crit`) colours a bar and is not drawn.

use std::collections::BTreeSet;

use crate::scan::{Read, Refused, Step, uncommented};

/// The statements that act on or tune a gantt's drawing and are left out of
/// what the engine is handed.
const LEFT_OUT: &[&str] = &["click", "tickinterval", "todaymarker", "weekday"];

/// The statements, lowercased, that change which days a task takes, which
/// the engine does not read.
const REFUSED: &[&str] = &["excludes", "includes"];

/// The axis formats the engine draws as written.
const AXIS_FORMATS: &[&str] = &["%b %d", "%Y-%m-%d", "%m/%d", "%d", "%m-%d"];

/// The statuses a task may open with, which colour its bar.
const STATUSES: &[&str] = &["done", "active", "crit"];

/// The most digits of a task's length: enough for 27 years in days, and short
/// of the lengths that would carry a date past the engine's calendar.
const MOST_DIGITS: usize = 4;

/// `cleaned` read as a gantt into `read`: the statements kept, one to a line,
/// and what they name.
pub(crate) fn read(cleaned: &str, mut read: Read) -> Result<(String, Read), Refused> {
    let mut lines = cleaned
        .lines()
        .map(|line| uncommented(line).trim())
        .filter(|line| !line.is_empty());
    let header = lines.next().ok_or(Refused::Empty)?;
    if !header.eq_ignore_ascii_case("gantt") {
        return Err(Refused::Unread);
    }
    let mut kept = vec![header.to_owned()];
    let mut titled = false;
    let mut ids = BTreeSet::new();
    let mut above = false;
    for line in lines {
        let first = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if keyword(line, "title").is_some() {
            if titled {
                return Err(Refused::Unread);
            }
            titled = true;
        } else if let Some(format) = keyword(line, "dateFormat") {
            if format != "YYYY-MM-DD" {
                return Err(Refused::Unread);
            }
        } else if let Some(format) = keyword(line, "axisFormat") {
            if !AXIS_FORMATS.contains(&format) {
                return Err(Refused::Unread);
            }
        } else if LEFT_OUT.contains(&first.as_str()) {
            continue;
        } else if REFUSED.contains(&first.as_str()) {
            return Err(Refused::Unread);
        } else if let Some(name) = keyword(line, "section") {
            read.grouped()?;
            read.stepped(Step::Section(name.to_owned()));
            above = false;
        } else {
            task(line, above, &mut ids, &mut read)?;
            above = true;
        }
        kept.push(line.to_owned());
    }
    Ok((kept.join("\n"), read))
}

/// What follows `keyword`, written in any case and followed by a space, as
/// the engine reads a gantt's statements.
fn keyword<'line>(line: &'line str, keyword: &str) -> Option<&'line str> {
    let head = line.get(..keyword.len())?;
    let rest = line.get(keyword.len()..)?;
    (head.eq_ignore_ascii_case(keyword)
        && rest.starts_with(|next: char| next.is_ascii_whitespace()))
    .then(|| rest.trim())
}

/// A task: `Name : fields`, the fields its statuses, then an id, a start, or
/// both, then its length. `above` is whether a task stands above it in its
/// section; `ids` are the ids named so far.
fn task(
    line: &str,
    above: bool,
    ids: &mut BTreeSet<String>,
    read: &mut Read,
) -> Result<(), Refused> {
    let (name, fields) = line.split_once(':').ok_or(Refused::Unread)?;
    let name = name.trim();
    if name.is_empty() {
        return Err(Refused::Unread);
    }
    let fields: Vec<&str> = fields
        .split(',')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .collect();
    let statuses = fields
        .iter()
        .take_while(|field| STATUSES.contains(field))
        .count();
    let fields = fields.get(statuses..).unwrap_or_default();
    if fields
        .iter()
        .any(|field| is_status(field) || field.eq_ignore_ascii_case("milestone"))
    {
        return Err(Refused::Unread);
    }
    let Some((length, starts)) = fields.split_last() else {
        return Err(Refused::Unread);
    };
    if !is_length(length) {
        return Err(Refused::Unread);
    }
    let (id, after) = match starts {
        [] => (None, None),
        [only] if is_date(only) => (None, None),
        [only] => match only.strip_prefix("after ") {
            Some(after) => (None, Some(after.trim())),
            None => (Some(*only), None),
        },
        [id, start] if is_date(start) => (Some(*id), None),
        [id, start] => (
            Some(*id),
            Some(start.strip_prefix("after ").ok_or(Refused::Unread)?.trim()),
        ),
        _ => return Err(Refused::Unread),
    };
    let dated = starts.last().is_some_and(|start| is_date(start));
    if !dated && after.is_none() && !above {
        return Err(Refused::Unread);
    }
    if let Some(after) = after
        && !(is_id(after) && ids.contains(after))
    {
        return Err(Refused::Unread);
    }
    if let Some(id) = id
        && !(is_id(id) && ids.insert(id.to_owned()))
    {
        return Err(Refused::Unread);
    }
    read.labelled(name)?;
    read.placed(Step::Task(name.to_owned()))
}

/// Whether `field` is a status in any case, as the engine leaves one out
/// wherever it stands.
fn is_status(field: &str) -> bool {
    STATUSES
        .iter()
        .any(|status| field.eq_ignore_ascii_case(status))
}

/// Whether `field` is a length: a whole number of days or weeks above none,
/// in no more than [`MOST_DIGITS`] digits.
fn is_length(field: &str) -> bool {
    let Some(number) = field.strip_suffix('d').or_else(|| field.strip_suffix('w')) else {
        return false;
    };
    (1..=MOST_DIGITS).contains(&number.len())
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && number.bytes().any(|byte| byte != b'0')
}

/// Whether `field` is written as a date, `YYYY-MM-DD`. Whether it is a day
/// the calendar has is the engine's to say; one it does not have is refused
/// when the engine's reading is held to this one.
fn is_date(field: &str) -> bool {
    let bytes = field.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(at, byte)| match at {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// Whether `field` is a task's id: ASCII letters, digits and underscores.
fn is_id(field: &str) -> bool {
    !field.is_empty()
        && field
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
