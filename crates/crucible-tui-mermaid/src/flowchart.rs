//! A flowchart read a statement at a time: its header, and its chains of
//! boxes and the lines between them.
//!
//! A flowchart with a group (`subgraph`) in it is refused before the engine
//! sees it: the engine draws a group's boxes in a different place from one
//! drawing to the next, so the same answer would not draw the same twice.
//!
//! Only the forms the engine is known to read the way Mermaid does are read
//! here; a line with a longer tail (`--->`), a circle or cross at both ends
//! (`o--o`), an invisible link (`~~~`), or text run onto a line without a
//! space (`--text-->`) the engine reads as something else, so it refuses the
//! diagram rather than letting a drawing say what the source did not.

use crate::scan::{Read, Refused, is_name, opens_with};

/// The statements that style, link, act on or describe a flowchart, which
/// draw nothing and are left out of what the engine is handed.
const LEFT_OUT: &[&str] = &[
    "style",
    "linkStyle",
    "classDef",
    "class",
    "click",
    "accTitle",
    "accDescr",
];

/// The directions a flowchart may run.
const DIRECTIONS: &[&str] = &["TB", "TD", "BT", "LR", "RL"];

/// A box's shape: what opens it, and what may close it.
const SHAPES: &[(&str, &[&str])] = &[
    ("(((", &[")))"]),
    ("((", &["))"]),
    ("([", &["])"]),
    ("[[", &["]]"]),
    ("[(", &[")]"]),
    ("[/", &["/]", "\\]"]),
    ("[\\", &["\\]", "/]"]),
    ("{{", &["}}"]),
    ("(", &[")"]),
    ("[", &["]"]),
    ("{", &["}"]),
    (">", &["]"]),
];

/// The characters an unquoted label stops at.
const LABEL_ENDS: &[char] = &['[', ']', '(', ')', '{', '}', '"', '|'];

/// `cleaned` read as a flowchart into `read`: the statements kept, one to a
/// line, and what they name.
pub(crate) fn read(cleaned: &str, mut read: Read) -> Result<(String, Read), Refused> {
    let mut statements = statements(cleaned);
    let header = statements.next().ok_or(Refused::Empty)?;
    header_of(header)?;
    let mut kept = vec![header.to_owned()];
    for statement in statements {
        let first = statement
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(':');
        if LEFT_OUT.contains(&first) {
            continue;
        }
        if opens_with(statement, "subgraph", &[]) {
            return Err(Refused::Grouped);
        }
        if opens_with(statement, "end", &[]) || opens_with(statement, "direction", &[]) {
            return Err(Refused::Unread);
        }
        chain(statement, &mut read)?;
        kept.push(statement.to_owned());
    }
    Ok((kept.join("\n"), read))
}

/// The statements of `cleaned`, trimmed, with comment lines and blank ones
/// left out: one to a line, and more than one where a semicolon outside
/// quotes and brackets parts them.
fn statements(cleaned: &str) -> impl Iterator<Item = &str> {
    cleaned
        .lines()
        .filter(|line| !line.trim_start().starts_with("%%"))
        .flat_map(parted)
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
}

/// `line` parted at each semicolon outside quotes and brackets.
fn parted(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut quoted = false;
    let mut depth = 0_usize;
    let mut from = 0;
    for (at, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '[' | '(' | '{' if !quoted => depth = depth.saturating_add(1),
            ']' | ')' | '}' if !quoted => depth = depth.saturating_sub(1),
            ';' if !quoted && depth == 0 => {
                parts.push(line.get(from..at).unwrap_or_default());
                from = at + 1;
            }
            _ => {}
        }
    }
    parts.push(line.get(from..).unwrap_or_default());
    parts
}

/// Whether `header` opens a flowchart the way this reads one: `graph` or
/// `flowchart`, and a direction at most.
fn header_of(header: &str) -> Result<(), Refused> {
    let mut words = header.split_whitespace();
    let opens = words.next().unwrap_or_default().to_lowercase();
    let way = words.next();
    if !matches!(opens.as_str(), "graph" | "flowchart")
        || way.is_some_and(|way| !DIRECTIONS.contains(&way))
        || words.next().is_some()
    {
        return Err(Refused::Unread);
    }
    Ok(())
}

/// A statement's text, read from the front.
struct Cursor<'a> {
    rest: &'a str,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }

    fn eat(&mut self, text: &str) -> bool {
        match self.rest.strip_prefix(text) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn spaces(&mut self) {
        self.rest = self.rest.trim_start();
    }

    /// The run of `character` at the front, taken.
    fn run_of(&mut self, character: char) -> usize {
        let taken = self.rest.len() - self.rest.trim_start_matches(character).len();
        self.rest = self.rest.get(taken..).unwrap_or_default();
        taken
    }

    /// The text up to the first of `ends`, taken.
    fn until(&mut self, ends: impl Fn(char) -> bool) -> &'a str {
        let at = self.rest.find(ends).unwrap_or(self.rest.len());
        let (taken, rest) = self.rest.split_at(at);
        self.rest = rest;
        taken
    }

    /// The name of a box at the front, taken: letters, digits and
    /// underscores, and a hyphen followed by one of them.
    fn name(&mut self) -> Option<&'a str> {
        let mut end = 0;
        let mut characters = self.rest.char_indices().peekable();
        while let Some((at, character)) = characters.next() {
            let named = character.is_alphanumeric() || character == '_';
            let joined = character == '-'
                && end > 0
                && characters
                    .peek()
                    .is_some_and(|(_, next)| next.is_alphanumeric() || *next == '_');
            if !named && !joined {
                break;
            }
            end = at + character.len_utf8();
        }
        let (name, rest) = self.rest.split_at(end);
        self.rest = rest;
        (!name.is_empty()).then_some(name)
    }
}

/// A chain of groups of boxes and the lines between them, read into `read`.
fn chain(statement: &str, read: &mut Read) -> Result<(), Refused> {
    let mut cursor = Cursor { rest: statement };
    let mut left = group(&mut cursor, read)?;
    loop {
        cursor.spaces();
        if cursor.rest.is_empty() {
            return Ok(());
        }
        let label = arrow(&mut cursor)?;
        cursor.spaces();
        let right = group(&mut cursor, read)?;
        for _ in 0..left.saturating_mul(right) {
            read.joined(label.clone())?;
        }
        left = right;
    }
}

/// Boxes joined by `&`, read into `read`; how many there were.
fn group(cursor: &mut Cursor<'_>, read: &mut Read) -> Result<usize, Refused> {
    let mut boxes = 0_usize;
    loop {
        node(cursor, read)?;
        boxes = boxes.saturating_add(1);
        let before = cursor.rest;
        cursor.spaces();
        if !cursor.eat("&") {
            cursor.rest = before;
            return Ok(boxes);
        }
        cursor.spaces();
    }
}

/// One box: its name, its shape and label where it has them, and the class it
/// is styled with.
fn node(cursor: &mut Cursor<'_>, read: &mut Read) -> Result<(), Refused> {
    let name = cursor.name().ok_or(Refused::Unread)?;
    let label = SHAPES
        .iter()
        .find(|(opens, _)| cursor.rest.starts_with(opens))
        .map(|(opens, closes)| shaped(cursor, opens, closes))
        .transpose()?;
    styled(cursor)?;
    read.named(name, label)
}

/// The label inside a shape opened by `opens`, and the shape closed.
fn shaped(cursor: &mut Cursor<'_>, opens: &str, closes: &[&str]) -> Result<String, Refused> {
    cursor.eat(opens);
    let slanted = opens.ends_with(['/', '\\']);
    let label = label(cursor, |character| {
        LABEL_ENDS.contains(&character) || (slanted && matches!(character, '/' | '\\'))
    })?;
    if !closes.iter().any(|close| cursor.eat(close)) {
        return Err(Refused::Unread);
    }
    Ok(label)
}

/// A label: in double quotes, or up to the first character `ends` takes.
/// Either way it holds something, and no double quote.
fn label(cursor: &mut Cursor<'_>, ends: impl Fn(char) -> bool) -> Result<String, Refused> {
    let text = if cursor.eat("\"") {
        let text = cursor.until(|character| character == '"');
        if !cursor.eat("\"") {
            return Err(Refused::Unread);
        }
        text
    } else {
        cursor.until(ends)
    };
    if text.trim().is_empty() {
        return Err(Refused::Unread);
    }
    Ok(text.to_owned())
}

/// The `:::name` a box is styled with, where it has one.
fn styled(cursor: &mut Cursor<'_>) -> Result<(), Refused> {
    if cursor.eat(":::") && !cursor.name().is_some_and(is_name) {
        return Err(Refused::Unread);
    }
    Ok(())
}

/// A line between boxes, in one of the forms the engine reads as Mermaid
/// does; its label, where it has one.
fn arrow(cursor: &mut Cursor<'_>) -> Result<Option<String>, Refused> {
    cursor.eat("<");
    let mut label = None;
    if let Some((opens, closes)) = [("-- ", "--"), ("== ", "=="), ("-. ", ".-")]
        .into_iter()
        .find(|(opens, _)| {
            cursor
                .rest
                .strip_prefix(opens.trim_end())
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        })
    {
        cursor.eat(opens.trim_end());
        let text = cursor.rest.find(closes).ok_or(Refused::Unread)?;
        let (said, rest) = cursor.rest.split_at(text);
        if said.trim().is_empty() || said.contains('|') {
            return Err(Refused::Unread);
        }
        label = Some(said.trim().to_owned());
        // The closing half is read as a whole line's body from here: `--`
        // is the `-` run, `==` the `=` run, `.-` the dotted tail.
        cursor.rest = rest;
        if closes == ".-" {
            cursor.eat(".");
            body_tail(cursor, '-', true)?;
        } else {
            let stroke = if closes == "==" { '=' } else { '-' };
            body_tail(cursor, stroke, false)?;
        }
    } else {
        body(cursor)?;
    }
    let before = cursor.rest;
    cursor.spaces();
    if cursor.eat("|") {
        if label.is_some() {
            return Err(Refused::Unread);
        }
        label = Some(
            self::label(cursor, |character| matches!(character, '|' | '"'))?
                .trim()
                .to_owned(),
        );
        if !cursor.eat("|") {
            return Err(Refused::Unread);
        }
    } else {
        cursor.rest = before;
    }
    Ok(label)
}

/// A line's body with no text in it: `-->`, `---`, `==>`, `===`, `-.->`,
/// `-.-`, and `--o` or `--x`.
fn body(cursor: &mut Cursor<'_>) -> Result<(), Refused> {
    if cursor.eat("-.") {
        return body_tail(cursor, '-', true);
    }
    match cursor.peek() {
        Some(stroke @ ('-' | '=')) => body_tail(cursor, stroke, false),
        _ => Err(Refused::Unread),
    }
}

/// The rest of a line's body, from a run of `stroke` to its head: two strokes
/// and a head, or three strokes and none; the dash after a dotted line's dot
/// is one, with a head or none.
fn body_tail(cursor: &mut Cursor<'_>, stroke: char, dotted: bool) -> Result<(), Refused> {
    let strokes = cursor.run_of(stroke);
    let headed = cursor.eat(">") || circled_or_crossed(cursor);
    let reads = match (dotted, headed) {
        (true, _) => strokes == 1,
        (false, true) => strokes == 2,
        (false, false) => strokes == 3,
    };
    if reads { Ok(()) } else { Err(Refused::Unread) }
}

/// A circle or cross head (`o` or `x`) where a space or a label follows it,
/// so the first letter of a box's name is not taken for one.
fn circled_or_crossed(cursor: &mut Cursor<'_>) -> bool {
    let mut characters = cursor.rest.chars();
    let head = characters.next();
    let after = characters.next();
    if matches!(head, Some('o' | 'x'))
        && after.is_none_or(|after| after.is_whitespace() || after == '|')
    {
        cursor.rest = cursor.rest.get(1..).unwrap_or_default();
        return true;
    }
    false
}
