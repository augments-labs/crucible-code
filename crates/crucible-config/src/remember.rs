//! Adding one rule to a configuration file without disturbing the rest of it.
//!
//! The file this writes to is one somebody may have opened and edited, so it is
//! spliced rather than re-serialised: every byte crucible did not put there
//! stays where it was, in the order and the spacing it was written in. A
//! round-trip through a JSON value would sort the keys, drop the layout and
//! hand back a file the author no longer recognises — for the sake of adding
//! one line.
//!
//! Nothing here opens a file. This crate says what a document may hold; the
//! wiring above it reads and writes.

use crucible_core::Minted;
use serde_json::Value;

use crate::error::ConfigError;

mod splice;

#[cfg(test)]
mod tests;

/// The file crucible writes when it has nothing to add to.
const FRESH: &str = "{\n  \"permissions\": {\n    \"allow\": [\n      RULE\n    ]\n  }\n}\n";

/// The text of a configuration file with one more `allow` rule in it.
///
/// `text` is what the file holds now, and empty for a file that is not there
/// yet. The rule goes into `permissions.allow`, which is created along with the
/// block around it if the file has neither.
///
/// # Errors
///
/// [`ConfigError::Malformed`] when the text is not JSON, and
/// [`ConfigError::Unspliceable`] when it is JSON that no rule can be added to
/// without rewriting — which is the moment to tell somebody what to type rather
/// than to guess at their file.
pub fn allowing(text: &str, file: &str, rule: &Minted) -> Result<String, ConfigError> {
    // The rule as JSON reads it. A minted rule spells a glob character with a
    // backslash class, and a backslash is something JSON reads too — written
    // raw it would come back a different rule, or no document at all.
    let written = Value::String(rule.as_str().to_owned()).to_string();

    if text.trim().is_empty() {
        return Ok(FRESH.replace("RULE", &written));
    }

    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;

    if already(&value, rule.as_str()) {
        return Ok(text.to_owned());
    }

    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        rule: rule.as_str().into(),
    };
    let root = splice::root(text)
        .filter(|_| value.is_object())
        .ok_or_else(refuse)?;

    // Outwards in: whichever of the three is already there is where this stops.
    // A block the parsed value holds and the text does not is a spelling this
    // cannot find, and inserting beside it would write a second copy of a key.
    let Some(permissions) = value.get("permissions") else {
        return Ok(splice::insert(text, root, |indent| match indent {
            Some(indent) => format!(
                "\"permissions\": {{\n{indent}  \"allow\": [\n{indent}    {written}\n{indent}  ]\n{indent}}}"
            ),
            None => format!("\"permissions\": {{\"allow\": [{written}]}}"),
        }));
    };
    let block = splice::member(text, root, "permissions").ok_or_else(refuse)?;

    if permissions.get("allow").is_none() {
        return Ok(splice::insert(text, block, |indent| match indent {
            Some(indent) => format!("\"allow\": [\n{indent}  {written}\n{indent}]"),
            None => format!("\"allow\": [{written}]"),
        }));
    }
    let allow = splice::member(text, block, "allow").ok_or_else(refuse)?;

    Ok(splice::insert(text, allow, |_| written.clone()))
}

/// Whether this rule is one the file already states.
///
/// Configuration is read once, at the start, so a rule added to the file by
/// hand mid-session is one the engine still asks about — and answering
/// `always` to it would otherwise write a second copy.
fn already(value: &Value, rule: &str) -> bool {
    value
        .get("permissions")
        .and_then(|permissions| permissions.get("allow"))
        .and_then(Value::as_array)
        .is_some_and(|allow| allow.iter().any(|written| written.as_str() == Some(rule)))
}
