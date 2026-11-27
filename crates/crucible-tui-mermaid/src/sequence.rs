//! A sequence diagram read a line at a time: its participants and the
//! messages between them.
//!
//! The engine draws participants and messages and nothing that changes how
//! either is read, so this reads those and refuses the rest: a note, an
//! activation, numbering, a group (`box`) and a block (`loop`, `alt`, `opt`,
//! `par`, `critical`, `break`, `rect`) each refuse the diagram rather than be
//! drawn otherwise than Mermaid draws them. A message is split as the engine
//! splits one, at the first of its arrows found, so the two readings meet the
//! same ends, and a participant declared after it was named, or twice, is
//! refused rather than relabelled.

use crate::scan::{Read, Refused, Step, is_name, uncommented};

/// The statements that describe a sequence diagram to a screen reader, which
/// draw nothing and are left out of what the engine is handed.
const LEFT_OUT: &[&str] = &["accTitle", "accDescr"];

/// The words, lowercased, that open a statement this does not draw.
const REFUSED: &[&str] = &[
    "autonumber",
    "note",
    "activate",
    "deactivate",
    "box",
    "loop",
    "alt",
    "opt",
    "par",
    "critical",
    "break",
    "else",
    "and",
    "option",
    "end",
    "rect",
];

/// The arrows a message is written with, in the order the engine tries them.
const ARROWS: &[&str] = &["-->>", "-->", "->>", "->"];

/// `cleaned` read as a sequence diagram into `read`: the statements kept, one
/// to a line, and what they name.
pub(crate) fn read(cleaned: &str, mut read: Read) -> Result<(String, Read), Refused> {
    let mut lines = cleaned
        .lines()
        .map(|line| uncommented(line).trim())
        .filter(|line| !line.is_empty());
    let header = lines.next().ok_or(Refused::Empty)?;
    if !header.eq_ignore_ascii_case("sequenceDiagram") {
        return Err(Refused::Unread);
    }
    let mut kept = vec![header.to_owned()];
    for line in lines {
        let first = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(':');
        if LEFT_OUT.contains(&first) {
            continue;
        }
        let word = first.to_lowercase();
        if REFUSED.contains(&word.as_str()) || line.to_lowercase().starts_with("sequencediagram") {
            return Err(Refused::Unread);
        }
        if let Some(declared) = declaration(line) {
            participant(declared, &mut read)?;
        } else {
            message(line, &mut read)?;
        }
        kept.push(line.to_owned());
    }
    Ok((kept.join("\n"), read))
}

/// What follows `participant` or `actor`, either written in any case and
/// followed by a space, as the engine reads a declaration.
fn declaration(line: &str) -> Option<&str> {
    ["participant", "actor"].iter().find_map(|keyword| {
        let head = line.get(..keyword.len())?;
        let rest = line.get(keyword.len()..)?;
        (head.eq_ignore_ascii_case(keyword)
            && rest.starts_with(|next: char| next.is_ascii_whitespace()))
        .then(|| rest.trim())
    })
}

/// A participant declared: `Name`, or `Name as Label`.
fn participant(declared: &str, read: &mut Read) -> Result<(), Refused> {
    let (name, label) = match separated(declared) {
        Some(at) => {
            let name = declared.get(..at).unwrap_or_default().trim();
            let label = declared.get(at..).unwrap_or_default();
            let Some(label) = label.strip_prefix(" as ") else {
                return Err(Refused::Unread);
            };
            (name, broken_as_spaces(label.trim()))
        }
        None => (declared, declared.to_owned()),
    };
    if !is_name(name) || label.trim().is_empty() || read.boxes.contains_key(name) {
        return Err(Refused::Unread);
    }
    read.named(name, Some(label.clone()))?;
    read.stepped(Step::Participant {
        name: name.to_owned(),
        label,
    });
    Ok(())
}

/// Where ` as `, in any case, first stands in `declared`, as the engine finds
/// the label it declares.
fn separated(declared: &str) -> Option<usize> {
    declared.as_bytes().windows(4).position(|four| {
        matches!(four, [b' ', a, s, b' '] if a.eq_ignore_ascii_case(&b'a') && s.eq_ignore_ascii_case(&b's'))
    })
}

/// A message: `From`, an arrow, `To`, and its label after a colon or none.
fn message(line: &str, read: &mut Read) -> Result<(), Refused> {
    let (from, rest) = ARROWS
        .iter()
        .find_map(|arrow| line.split_once(arrow))
        .ok_or(Refused::Unread)?;
    let (to, label) = match rest.split_once(':') {
        Some((to, label)) => (to, broken_as_spaces(label.trim())),
        None => (rest, String::new()),
    };
    let (from, to) = (from.trim(), to.trim());
    if !is_name(from) || !is_name(to) {
        return Err(Refused::Unread);
    }
    for name in [from, to] {
        if !read.boxes.contains_key(name) {
            read.named(name, None)?;
            read.stepped(Step::Participant {
                name: name.to_owned(),
                label: name.to_owned(),
            });
        }
    }
    read.joined((!label.is_empty()).then(|| label.clone()))?;
    read.stepped(Step::Message {
        from: from.to_owned(),
        to: to.to_owned(),
        label,
    });
    Ok(())
}

/// `label` with each line break Mermaid writes as `<br>` made a space, as the
/// engine draws a participant's or a message's label on one line.
fn broken_as_spaces(label: &str) -> String {
    ["<br/>", "<br>", "<br />", "<BR/>", "<BR>", "<BR />"]
        .iter()
        .fold(label.to_owned(), |label, broken| label.replace(broken, " "))
}
