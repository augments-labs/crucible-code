//! An entity-relationship diagram read a line at a time: the entities, the
//! attributes in their bodies, and the relationships between them.
//!
//! The engine reads one strictly, so this reads strictly too: a relationship
//! names its two ends and its line and nothing more, with its label after a
//! colon, in one pair of double quotes or none.

use crate::scan::{Read, Refused, is_name, uncommented};

/// `cleaned` read as an entity-relationship diagram into `read`: the
/// statements kept, one to a line, and what they name.
pub(crate) fn read(cleaned: &str, mut read: Read) -> Result<(String, Read), Refused> {
    let mut lines = cleaned
        .lines()
        .map(|line| uncommented(line).trim())
        .filter(|line| !line.is_empty());
    let header = lines.next().ok_or(Refused::Empty)?;
    if !header.eq_ignore_ascii_case("erDiagram") {
        return Err(Refused::Unread);
    }
    let mut kept = vec![header.to_owned()];
    let mut in_body = false;
    for line in lines {
        if in_body {
            in_body = line != "}";
        } else if line.contains("--") || line.contains("..") {
            relationship(line, &mut read)?;
        } else if let Some((name, body)) = line.split_once('{') {
            let name = name.trim();
            if !is_name(name) {
                return Err(Refused::Unread);
            }
            read.named(name, None)?;
            let body = body.trim();
            in_body = body.is_empty();
            if !in_body && !body.ends_with('}') {
                return Err(Refused::Unread);
            }
        } else if is_name(line) {
            read.named(line, None)?;
        } else {
            return Err(Refused::Unread);
        }
        kept.push(line.to_owned());
    }
    if in_body {
        return Err(Refused::Unread);
    }
    Ok((kept.join("\n"), read))
}

/// A relationship between two entities: `From line To`, three words, with a
/// label after a colon, or nothing.
fn relationship(line: &str, read: &mut Read) -> Result<(), Refused> {
    let (ends, label) = match line.split_once(':') {
        Some((ends, label)) if quoted_whole_or_not_at_all(label.trim()) => {
            (ends, Some(label.trim().to_owned()))
        }
        Some(_) => return Err(Refused::Unread),
        None => (line, None),
    };
    let mut words = ends.split_whitespace();
    let (Some(from), Some(joined), Some(to), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Err(Refused::Unread);
    };
    if !is_name(from) || !is_name(to) || !(joined.contains("--") || joined.contains("..")) {
        return Err(Refused::Unread);
    }
    read.named(from, None)?;
    read.named(to, None)?;
    read.joined(label)
}

/// Whether `label` is something: written in one pair of double quotes, or with
/// none in it. A quote left open is a label cut short, which Mermaid does not
/// read.
fn quoted_whole_or_not_at_all(label: &str) -> bool {
    match label
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(inside) => !inside.contains('"'),
        None => !label.is_empty() && !label.contains('"'),
    }
}
