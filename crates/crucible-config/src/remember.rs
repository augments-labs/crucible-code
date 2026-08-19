//! Adding one answer to a configuration file without disturbing the rest of it.
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

use crucible_core::{Effort, Minted};
use serde_json::Value;

use crate::error::ConfigError;

mod splice;

#[cfg(test)]
mod tests;

/// The file crucible writes when it has no rule to add to.
const FRESH: &str = "{\n  \"permissions\": {\n    \"allow\": [\n      RULE\n    ]\n  }\n}\n";

/// The file crucible writes when the provider to ask is all it has to say.
const ASKED: &str = "{\n  \"provider\": PROVIDER\n}\n";

/// The file crucible writes when the theme is all it has to say.
const DRAWN: &str = "{\n  \"output\": {\n    \"theme\": THEME\n  }\n}\n";

/// The file crucible writes when it has nothing to write a provider's answer
/// beside.
const CHOSEN: &str = "{\n  \"providers\": {\n    PROVIDER: {\n      KEY: ANSWER\n    }\n  }\n}\n";

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
        at: "permissions.allow".into(),
        written: written.clone().into(),
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

/// The text of a configuration file that asks `provider` from now on.
///
/// `text` is what the file holds now, and empty for a file that is not there
/// yet. The name goes to the top-level `provider` key — the one setting that
/// chooses a vendor — and one already written there is written over rather
/// than added beside: the same key twice is a document the parser reads one
/// way and its author reads the other.
///
/// # Errors
///
/// [`ConfigError::Malformed`] when the text is not JSON, and
/// [`ConfigError::Unspliceable`] when it is JSON that no answer can be written
/// into without rewriting.
pub fn asking(text: &str, file: &str, provider: &str) -> Result<String, ConfigError> {
    // As JSON reads it. A provider name is somebody else's string, and one
    // holding a quote written raw would end the document.
    let written = Value::String(provider.to_owned()).to_string();

    if text.trim().is_empty() {
        return Ok(ASKED.replace("PROVIDER", &written));
    }

    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;

    if value.get("provider").and_then(Value::as_str) == Some(provider) {
        return Ok(text.to_owned());
    }

    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        at: "provider".into(),
        written: written.clone().into(),
    };
    let root = splice::root(text)
        .filter(|_| value.is_object())
        .ok_or_else(refuse)?;

    // One level rather than the three the two below walk, which is the whole
    // difference between a key that chooses a provider and a key that says
    // something about one already chosen.
    if value.get("provider").is_none() {
        return Ok(splice::insert(text, root, |_| {
            format!("\"provider\": {written}")
        }));
    }

    let was = splice::member(text, root, "provider").ok_or_else(refuse)?;
    Ok(splice::over(text, was, &written))
}

/// The text of a configuration file that draws with `theme`.
///
/// `text` is what the file holds now, and empty for a file that is not there
/// yet. The name goes to `output.theme`, and the `output` object is created
/// along with it where the file has none. A theme already written there is
/// written over rather than added beside: the same key twice is a document the
/// parser reads one way and its author reads the other.
///
/// # Errors
///
/// [`ConfigError::Malformed`] when the text is not JSON, and
/// [`ConfigError::Unspliceable`] when it is JSON that no answer can be written
/// into without rewriting — which is the moment to tell somebody what to type
/// rather than to guess at their file.
pub fn drawing(text: &str, file: &str, theme: &str) -> Result<String, ConfigError> {
    let written = Value::String(theme.to_owned()).to_string();

    if text.trim().is_empty() {
        return Ok(DRAWN.replace("THEME", &written));
    }

    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;

    if value.get("output").and_then(|output| output.get("theme"))
        == Some(&Value::String(theme.to_owned()))
    {
        return Ok(text.to_owned());
    }

    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        at: "output.theme".into(),
        written: written.clone().into(),
    };
    let root = splice::root(text)
        .filter(|_| value.is_object())
        .ok_or_else(refuse)?;

    // No `output` block at all: the key and the object around it go in
    // together. `insert` hands back the indentation of the line it is going on,
    // and `None` where there is none to match — a line that already has other
    // things on it. Written on one line there, the way `allowing` answers the
    // same case: a block with a hard-coded indent inside a file that has none
    // is this program deciding how somebody else's file is laid out, and a
    // tab-indented file would get a mix of both.
    let Some(output) = value.get("output") else {
        return Ok(splice::insert(text, root, |indent| match indent {
            Some(indent) => {
                format!("\"output\": {{\n{indent}  \"theme\": {written}\n{indent}}}")
            }
            None => format!("\"output\": {{\"theme\": {written}}}"),
        }));
    };

    if !output.is_object() {
        return Err(refuse());
    }

    let block = splice::member(text, root, "output").ok_or_else(refuse)?;
    if output.get("theme").is_none() {
        return Ok(splice::insert(text, block, |_| {
            format!("\"theme\": {written}")
        }));
    }

    let was = splice::member(text, block, "theme").ok_or_else(refuse)?;
    Ok(splice::over(text, was, &written))
}

/// The text of a configuration file that asks `provider` for `model`.
///
/// `text` is what the file holds now, and empty for a file that is not there
/// yet. The name goes to `providers.<provider>.model`, and every object on the
/// way to it is created along with it. A model already written there is written
/// over rather than added beside: the same key twice is a document the parser
/// reads one way and its author reads the other.
///
/// # Errors
///
/// [`ConfigError::Malformed`] when the text is not JSON, and
/// [`ConfigError::Unspliceable`] when it is JSON that no answer can be written
/// into without rewriting — which is the moment to tell somebody what to type
/// rather than to guess at their file. [`ConfigError::Unremovable`] when the
/// rung chosen for the previous model cannot be lifted out for the same
/// reason.
pub fn choosing(
    text: &str,
    file: &str,
    provider: &str,
    model: &str,
) -> Result<String, ConfigError> {
    let written = beside(text, file, provider, "model", model)?;
    without_effort(&written, file, provider)
}

/// Removes a rung chosen for the previous model.
fn without_effort(text: &str, file: &str, provider: &str) -> Result<String, ConfigError> {
    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;
    let Some(chosen) = value.get("providers").and_then(|all| all.get(provider)) else {
        return Ok(text.to_owned());
    };
    if chosen.get("effort").is_none() {
        return Ok(text.to_owned());
    }
    let refuse = || ConfigError::Unremovable {
        file: file.into(),
        at: format!("providers.{provider}.effort").into(),
    };
    let root = splice::root(text).ok_or_else(refuse)?;
    let providers = splice::member(text, root, "providers").ok_or_else(refuse)?;
    let provider = splice::member(text, providers, provider).ok_or_else(refuse)?;
    splice::remove(text, provider, "effort").ok_or_else(refuse)
}

/// The text of a configuration file that asks `provider` to think this hard.
///
/// The same splice as [`choosing`], into the key beside it. A rung is written
/// as the word the ladder spells it with, because that is the word the schema
/// accepts and the word the person reading the file afterwards has to
/// recognise.
///
/// # Errors
///
/// [`ConfigError::Malformed`] and [`ConfigError::Unspliceable`], for the same
/// reasons as [`choosing`].
pub fn thinking(
    text: &str,
    file: &str,
    provider: &str,
    effort: Effort,
) -> Result<String, ConfigError> {
    beside(text, file, provider, "effort", effort.as_str())
}

/// The text of a configuration file where `providers.<provider>.<key>` says
/// `answer`.
///
/// One walk for both answers, because they differ in a key and in nothing else:
/// the object to create, the object to insert into, and the value to write over
/// are the same three cases either way, and two copies of them would be two
/// places for the day a fourth case appears.
///
/// An answer already written there is written over rather than added beside:
/// the same key twice is a document the parser reads one way and its author
/// reads the other.
fn beside(
    text: &str,
    file: &str,
    provider: &str,
    key: &str,
    answer: &str,
) -> Result<String, ConfigError> {
    // Both as JSON reads them. A provider name is somebody else's string, and
    // one holding a quote written raw would end the document.
    let named = Value::String(provider.to_owned()).to_string();
    let written = Value::String(answer.to_owned()).to_string();

    if text.trim().is_empty() {
        return Ok(CHOSEN
            .replace("PROVIDER", &named)
            .replace("KEY", &Value::String(key.to_owned()).to_string())
            .replace("ANSWER", &written));
    }

    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;

    if answered(&value, provider, key) == Some(answer) {
        return Ok(text.to_owned());
    }

    // The key as JSON reads it too, for the same reason the two values are.
    let spelled = Value::String(key.to_owned()).to_string();
    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        at: format!("providers.{provider}.{key}").into(),
        written: written.clone().into(),
    };
    let root = splice::root(text)
        .filter(|_| value.is_object())
        .ok_or_else(refuse)?;

    // Outwards in, the same walk `allowing` makes: whichever of the three is
    // already there is where this stops. A block the parsed value holds and the
    // text does not is a spelling this cannot find, and inserting beside it
    // would write a second copy of a key.
    let Some(providers) = value.get("providers") else {
        return Ok(splice::insert(text, root, |indent| match indent {
            Some(indent) => format!(
                "\"providers\": {{\n{indent}  {named}: {{\n{indent}    {spelled}: {written}\n{indent}  }}\n{indent}}}"
            ),
            None => format!("\"providers\": {{{named}: {{{spelled}: {written}}}}}"),
        }));
    };
    let block = splice::member(text, root, "providers").ok_or_else(refuse)?;

    let Some(chosen) = providers.get(provider) else {
        return Ok(splice::insert(text, block, |indent| match indent {
            Some(indent) => {
                format!("{named}: {{\n{indent}  {spelled}: {written}\n{indent}}}")
            }
            None => format!("{named}: {{{spelled}: {written}}}"),
        }));
    };
    let held = splice::member(text, block, provider).ok_or_else(refuse)?;

    if chosen.get(key).is_none() {
        return Ok(splice::insert(text, held, |_| {
            format!("{spelled}: {written}")
        }));
    }

    let was = splice::member(text, held, key).ok_or_else(refuse)?;
    Ok(splice::over(text, was, &written))
}

/// The answer a document already gives under this provider's key.
fn answered<'a>(value: &'a Value, provider: &str, key: &str) -> Option<&'a str> {
    value.get("providers")?.get(provider)?.get(key)?.as_str()
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
