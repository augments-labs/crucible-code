//! A state diagram read a line at a time: its header, its states and the
//! composite states around them, its notes, and its transitions.
//!
//! A state described with `Name : what it does` is refused: the engine draws
//! the description in place of the name, so the drawing would lose what the
//! diagram calls the state. A note that is not beside a state is refused too,
//! since the engine leaves it out.

use std::collections::BTreeSet;

use crate::scan::{Read, Refused, is_name, opens_with, uncommented};

/// The statements that style, act on or describe a state diagram, which draw
/// nothing and are left out of what the engine is handed.
const LEFT_OUT: &[&str] = &[
    "classDef",
    "class",
    "style",
    "linkStyle",
    "click",
    "accTitle",
    "accDescr",
    "scale",
];

/// The directions a state diagram, or a composite state in one, may run.
const DIRECTIONS: &[&str] = &["TB", "TD", "BT", "LR", "RL"];

/// The kinds a pseudostate is declared as.
const PSEUDOSTATES: &[&str] = &["<<choice>>", "<<fork>>", "<<join>>"];

/// `cleaned` read as a state diagram into `read`: the statements kept, one to
/// a line, and what they name.
pub(crate) fn read(cleaned: &str, mut read: Read) -> Result<(String, Read), Refused> {
    let mut lines = cleaned
        .lines()
        .map(|line| uncommented(line).trim())
        .filter(|line| !line.is_empty());
    let header = lines.next().ok_or(Refused::Empty)?;
    if !matches!(
        header.to_lowercase().as_str(),
        "statediagram" | "statediagram-v2"
    ) {
        return Err(Refused::Unread);
    }
    let mut kept = vec![header.to_owned()];
    let mut scopes: Vec<&str> = Vec::new();
    let mut ends = BTreeSet::new();
    while let Some(line) = lines.next() {
        let first = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(':');
        if LEFT_OUT.contains(&first) || line == "hide empty description" {
            continue;
        }
        if line == "}" {
            scopes.pop().ok_or(Refused::Unread)?;
        } else if let Some(way) = line.strip_prefix("direction")
            && opens_with(line, "direction", &[])
        {
            if !DIRECTIONS.contains(&way.trim()) {
                return Err(Refused::Unread);
            }
        } else if let Some(note) = line.strip_prefix("note")
            && opens_with(line, "note", &[])
        {
            let beside = note_beside(note)?;
            read.named(beside.state, None)?;
            read.grouped()?;
            read.joined(None)?;
            let Some(said) = beside.said else {
                kept.push(line.to_owned());
                loop {
                    let said = lines.next().ok_or(Refused::Unread)?;
                    kept.push(said.to_owned());
                    if said == "end note" {
                        break;
                    }
                    read.labelled(said)?;
                }
                continue;
            };
            read.labelled(said)?;
        } else if let Some(declared) = line.strip_prefix("state")
            && opens_with(line, "state", &[])
        {
            let (name, label, composite) = declaration(declared)?;
            read.named(name, label)?;
            if composite {
                scopes.push(name);
            }
        } else {
            transition(
                line,
                scopes.last().copied().unwrap_or_default(),
                &mut ends,
                &mut read,
            )?;
        }
        kept.push(line.to_owned());
    }
    if !scopes.is_empty() {
        return Err(Refused::Unread);
    }
    Ok((kept.join("\n"), read))
}

/// A note's place: the state it is beside, and its text where it is on the
/// line (`: text`) rather than on the lines to `end note`.
struct Beside<'a> {
    state: &'a str,
    said: Option<&'a str>,
}

/// `note` read as `left of State` or `right of State`, with `: text` after it
/// or nothing.
fn note_beside(note: &str) -> Result<Beside<'_>, Refused> {
    let (place, said) = match note.split_once(':') {
        Some((place, text)) if !text.trim().is_empty() => (place, Some(text.trim())),
        Some(_) => return Err(Refused::Unread),
        None => (note, None),
    };
    let mut words = place.split_whitespace();
    let (Some("left" | "right"), Some("of"), Some(state), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Err(Refused::Unread);
    };
    if !is_name(state) {
        return Err(Refused::Unread);
    }
    Ok(Beside { state, said })
}

/// A `state` declaration after its keyword: its name, its label where it is
/// given one, and whether it opens a composite state.
fn declaration(declared: &str) -> Result<(&str, Option<String>, bool), Refused> {
    let declared = declared.trim();
    let (declared, composite) = match declared.strip_suffix('{') {
        Some(opened) => (opened.trim_end(), true),
        None => (declared, false),
    };
    if let Some(quoted) = declared.strip_prefix('"') {
        let (label, rest) = quoted.split_once('"').ok_or(Refused::Unread)?;
        let name = rest
            .trim_start()
            .strip_prefix("as")
            .filter(|name| name.starts_with(char::is_whitespace))
            .map(str::trim)
            .ok_or(Refused::Unread)?;
        if label.trim().is_empty() || !is_name(name) {
            return Err(Refused::Unread);
        }
        return Ok((name, Some(label.to_owned()), composite));
    }
    let mut words = declared.split_whitespace();
    let name = words.next().ok_or(Refused::Unread)?;
    let kind = words.next();
    if !is_name(name)
        || words.next().is_some()
        || kind.is_some_and(|kind| composite || !PSEUDOSTATES.contains(&kind))
    {
        return Err(Refused::Unread);
    }
    Ok((name, None, composite))
}

/// `Left --> Right`, with `: label` after it or nothing, read into `read`.
/// The start and end markers (`[*]`) are one state each per composite state
/// and side, as the engine draws them.
fn transition<'a>(
    line: &'a str,
    scope: &'a str,
    ends: &mut BTreeSet<(&'a str, bool)>,
    read: &mut Read,
) -> Result<(), Refused> {
    let (left, right) = line.split_once("-->").ok_or(Refused::Unread)?;
    let (left, rest) = side(left.trim())?;
    let (right, rest_of_right) = side(right.trim_start())?;
    let label = match rest_of_right.trim_start().strip_prefix(':') {
        Some(label) if !label.trim().is_empty() => Some(label.trim().to_owned()),
        None if rest_of_right.trim().is_empty() => None,
        _ => return Err(Refused::Unread),
    };
    if !rest.is_empty() {
        return Err(Refused::Unread);
    }
    for (side, end) in [(left, false), (right, true)] {
        if side == "[*]" {
            if ends.insert((scope, end)) {
                read.marked(&format!("[*] {scope} {end}"), None)?;
            }
        } else {
            read.named(side, None)?;
        }
    }
    read.joined(label)
}

/// One side of a transition at the front of `text`: a start or end marker,
/// or a state's name with the `:::class` it is styled with; and what follows.
fn side(text: &str) -> Result<(&str, &str), Refused> {
    if let Some(rest) = text.strip_prefix("[*]") {
        return Ok(("[*]", rest));
    }
    let (name, rest) = text.split_at(text.find(|c| !named(c)).unwrap_or(text.len()));
    if !is_name(name) {
        return Err(Refused::Unread);
    }
    let Some(styled) = rest.strip_prefix(":::") else {
        return Ok((name, rest));
    };
    let (class, rest) = styled.split_at(styled.find(|c| !named(c)).unwrap_or(styled.len()));
    if !is_name(class) {
        return Err(Refused::Unread);
    }
    Ok((name, rest))
}

/// Whether `character` may stand in a state's name.
fn named(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-')
}
