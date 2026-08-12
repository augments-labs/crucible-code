//! Finding a place in JSON text, and putting one more thing there.
//!
//! Text rather than values throughout. `serde_json` says what a document
//! *means*, and by the time it has said so the layout is gone — so the value is
//! what decides whether something is already there, and this decides where it
//! goes.
//!
//! The text is known to parse before anything here is called, which is what
//! lets the scanning stay this short: it never has to report a fault, only fail
//! to find something.

/// A value's extent in the text: its first byte, and the byte after its last.
#[derive(Debug, Clone, Copy)]
pub(super) struct Span {
    start: usize,
    end: usize,
}

/// The whole document, if it is an object.
pub(super) fn root(text: &str) -> Option<Span> {
    let start = text.find('{')?;
    Some(Span {
        start,
        end: end_of_value(text, start),
    })
}

/// The value `key` is written against inside `object`, when the text names it
/// once and names it plainly.
///
/// `None` where the object has no such member, where it has one under a
/// spelling this does not recognise, and where it writes the key twice. The
/// last two are the same fault seen from either side: JSON permits a key to
/// appear more than once and the parser keeps the last of them, so a member
/// found by reading the text is not necessarily the member the document means.
/// The caller refuses rather than guessing, because writing into the wrong one
/// adds a rule the document itself already shadows — and leaves a second copy
/// of a key behind to say so.
pub(super) fn member(text: &str, object: Span, key: &str) -> Option<Span> {
    let bytes = text.as_bytes();
    let mut at = object.start + 1;
    let mut found = None;

    while at < object.end.saturating_sub(1) {
        if bytes.get(at) != Some(&b'"') {
            at += 1;
            continue;
        }

        // At this depth a string is always a key, so what follows it is a
        // colon and then the value it names.
        let after = end_of_string(text, at);
        let start = after_colon(text, after);
        let value = Span {
            start,
            end: end_of_value(text, start),
        };

        // Spellings are compared byte for byte, so a key holding an escape is
        // one this cannot read — and therefore one it cannot rule out being a
        // second way of writing the key it is looking for.
        let written = &text[at + 1..after - 1];
        if written.contains('\\') {
            return None;
        }
        if written == key {
            if found.is_some() {
                return None;
            }
            found = Some(value);
        }
        at = value.end;
    }

    found
}

/// The text with `item` inside `container`, laid out the way its neighbours
/// are.
///
/// `item` is handed the indentation its own lines are to carry, or `None` when
/// it is going on a line that already has other things on it. Matching the
/// layout is not decoration: the next person to open the file reads what
/// crucible wrote and what they wrote as one document.
pub(super) fn insert(text: &str, container: Span, item: impl Fn(Option<&str>) -> String) -> String {
    let body = &text[container.start + 1..container.end - 1];

    // Nothing to sit beside, so nothing to match. The brackets close up around
    // the one thing now in them.
    if body.trim().is_empty() {
        return splice(text, container.start + 1..container.end - 1, &item(None));
    }

    let last = container.end - 1 - trailing_space(&text[..container.end - 1]);
    let written = if body.contains('\n') {
        let indent = indentation(text, last);
        format!(",\n{indent}{}", item(Some(&indent)))
    } else {
        format!(", {}", item(None))
    };

    splice(text, last..last, &written)
}

/// The text with `range` replaced.
fn splice(text: &str, range: std::ops::Range<usize>, written: &str) -> String {
    let mut spliced = String::with_capacity(text.len() + written.len());
    spliced.push_str(&text[..range.start]);
    spliced.push_str(written);
    spliced.push_str(&text[range.end..]);
    spliced
}

/// How much whitespace the text ends in.
fn trailing_space(text: &str) -> usize {
    text.len() - text.trim_end().len()
}

/// The whitespace the line holding the byte before `at` starts with.
fn indentation(text: &str, at: usize) -> String {
    let line = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    text[line..at]
        .chars()
        .take_while(|character| character.is_whitespace())
        .collect()
}

/// The byte after the string starting at `at`.
fn end_of_string(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = at + 1;

    while let Some(byte) = bytes.get(i) {
        match byte {
            // Whatever it stands for, it is not the end of the string.
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Where the value named by the key ending at `from` begins.
fn after_colon(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = from;

    while bytes.get(i).is_some_and(|byte| *byte != b':') {
        i += 1;
    }
    i += 1;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// The byte after the value starting at `at`.
fn end_of_value(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();

    match bytes.get(at) {
        Some(b'"') => end_of_string(text, at),
        Some(b'{' | b'[') => end_of_brackets(text, at),
        // A number, or one of the three words. It ends where the thing holding
        // it goes on.
        _ => {
            let mut i = at;
            while bytes.get(i).is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']') && !byte.is_ascii_whitespace()
            }) {
                i += 1;
            }
            i
        }
    }
}

/// The byte after the bracket matching the one at `at`.
fn end_of_brackets(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = at;

    while let Some(byte) = bytes.get(i) {
        match byte {
            // Jumped over whole, so a bracket inside a string is text.
            b'"' => {
                i = end_of_string(text, i);
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}
