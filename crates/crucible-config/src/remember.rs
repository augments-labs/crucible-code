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

use crucible_models::{Effort, Speed};
use crucible_tools::Minted;
use serde_json::Value;

use crate::error::ConfigError;
use crate::shape::rows::{Row, Values};

mod splice;

#[cfg(test)]
mod tests;

/// The file crucible writes when it has no rule to add to.
const FRESH: &str = "{\n  \"permissions\": {\n    \"allow\": [\n      RULE\n    ]\n  }\n}\n";

/// The file crucible writes when the provider to ask is all it has to say.
const ASKED: &str = "{\n  \"provider\": PROVIDER\n}\n";

/// The file crucible writes when the theme is all it has to say.
const DRAWN: &str = "{\n  \"output\": {\n    \"KEY\": THEME\n  }\n}\n";

/// The file crucible writes when somebody says to stop asking and there is
/// nothing to write it beside.
const UNASKED: &str = "{\n  \"compaction\": {\n    \"askOnResume\": 0\n  }\n}\n";

/// The file crucible writes when a yes is all it has to say.
const ACCEPTED: &str = "{\n  \"contentUse\": {\n    \"accepted\": [\n      ROUTE\n    ]\n  }\n}\n";

/// The file crucible writes when it has nothing to write a provider's answer
/// beside.
const CHOSEN: &str = "{\n  \"providers\": {\n    PROVIDER: {\n      KEY: ANSWER\n    }\n  }\n}\n";

/// The text of a configuration file that says yes to sending on `route`.
///
/// `text` is what the file holds now, and empty for a file that is not there
/// yet. The route goes into `contentUse.accepted`, created with the block
/// around it where the file has neither; a route already there leaves the
/// file as it was.
///
/// # Errors
///
/// [`ConfigError::Malformed`] when the text is not JSON, and
/// [`ConfigError::Unspliceable`] when it is JSON no answer can be written into
/// without rewriting.
pub fn accepting(text: &str, file: &str, route: &str) -> Result<String, ConfigError> {
    let written = Value::String(route.to_owned()).to_string();

    if text.trim().is_empty() {
        return Ok(ACCEPTED.replace("ROUTE", &written));
    }

    let value = parsed(text, file)?;
    let said = |block: &Value| {
        block
            .get("accepted")
            .and_then(Value::as_array)
            .is_some_and(|routes| routes.iter().any(|one| one.as_str() == Some(route)))
    };
    if value.get("contentUse").is_some_and(said) {
        return Ok(text.to_owned());
    }

    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        at: "contentUse.accepted".into(),
        written: written.clone().into(),
    };
    let root = splice::root(text)
        .filter(|_| value.is_object())
        .ok_or_else(refuse)?;

    // Outwards in, as a rule is added: whichever of the two is already there
    // is where this stops.
    let Some(block) = value.get("contentUse") else {
        return Ok(splice::insert(text, root, |indent| match indent {
            Some(indent) => format!(
                "\"contentUse\": {{\n{indent}  \"accepted\": [\n{indent}    {written}\n{indent}  ]\n{indent}}}"
            ),
            None => format!("\"contentUse\": {{\"accepted\": [{written}]}}"),
        }));
    };
    if !block.is_object() {
        return Err(refuse());
    }
    let at = splice::member(text, root, "contentUse").ok_or_else(refuse)?;
    match block.get("accepted") {
        None => {
            return Ok(splice::insert(text, at, |indent| match indent {
                Some(indent) => format!("\"accepted\": [\n{indent}  {written}\n{indent}]"),
                None => format!("\"accepted\": [{written}]"),
            }));
        }
        Some(list) if !list.is_array() => return Err(refuse()),
        Some(_) => {}
    }
    let accepted = splice::member(text, at, "accepted").ok_or_else(refuse)?;
    Ok(splice::insert(text, accepted, |_| written.clone()))
}

/// The text of a configuration file with no yes to any route `gone` picks.
///
/// Only the `accepted` list is written over, with the routes left in it in
/// the order they stood; every other byte stays where it was. A file that
/// says yes to none of them is handed back as it is.
///
/// # Errors
///
/// [`ConfigError::Malformed`] when the text is not JSON, and
/// [`ConfigError::Unremovable`] when it is JSON the list cannot be found in
/// without rewriting.
pub fn forgetting(
    text: &str,
    file: &str,
    gone: impl Fn(&str) -> bool,
) -> Result<String, ConfigError> {
    if text.trim().is_empty() {
        return Ok(text.to_owned());
    }
    let value = parsed(text, file)?;
    let Some(held) = value
        .get("contentUse")
        .and_then(|block| block.get("accepted"))
        .and_then(Value::as_array)
    else {
        return Ok(text.to_owned());
    };
    let kept: Vec<&Value> = held
        .iter()
        .filter(|one| !one.as_str().is_some_and(&gone))
        .collect();
    if kept.len() == held.len() {
        return Ok(text.to_owned());
    }

    let refuse = || ConfigError::Unremovable {
        file: file.into(),
        at: "contentUse.accepted".into(),
    };
    let root = splice::root(text).ok_or_else(refuse)?;
    let block = splice::member(text, root, "contentUse").ok_or_else(refuse)?;
    let accepted = splice::member(text, block, "accepted").ok_or_else(refuse)?;
    let written: Vec<String> = kept.iter().map(ToString::to_string).collect();
    Ok(splice::over(
        text,
        accepted,
        &format!("[{}]", written.join(", ")),
    ))
}

/// `text` as JSON, or the error naming where it stopped being JSON.
fn parsed(text: &str, file: &str) -> Result<Value, ConfigError> {
    serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })
}

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
    output(text, file, "theme", theme)
}

/// The text of a configuration file that stops asking about a large session.
///
/// Written as a zero rather than by taking the key out, because silence and
/// "never" are different answers here: one takes the wiring's own figure, and
/// this is somebody saying they have decided.
///
/// # Errors
///
/// [`ConfigError::Malformed`] where the file is not JSON.
pub fn unasked(text: &str, file: &str) -> Result<String, ConfigError> {
    const KEY: &str = "askOnResume";
    const WRITTEN: &str = "0";

    if text.trim().is_empty() {
        return Ok(UNASKED.to_owned());
    }

    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;

    if value
        .get("compaction")
        .and_then(|block| block.get(KEY))
        .and_then(Value::as_u64)
        == Some(0)
    {
        return Ok(text.to_owned());
    }

    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        at: format!("compaction.{KEY}").into(),
        written: WRITTEN.into(),
    };
    let root = splice::root(text)
        .filter(|_| value.is_object())
        .ok_or_else(refuse)?;

    // The same walk `output` makes, one block over: whichever of the two is
    // already there is where this stops, and a block written on one line stays
    // on one line rather than being given an indent this program chose.
    let Some(block) = value.get("compaction") else {
        return Ok(splice::insert(text, root, |indent| match indent {
            Some(indent) => {
                format!("\"compaction\": {{\n{indent}  \"{KEY}\": {WRITTEN}\n{indent}}}")
            }
            None => format!("\"compaction\": {{\"{KEY}\": {WRITTEN}}}"),
        }));
    };

    if !block.is_object() {
        return Err(refuse());
    }

    let at = splice::member(text, root, "compaction").ok_or_else(refuse)?;
    if block.get(KEY).is_none() {
        return Ok(splice::insert(text, at, |_| {
            format!("\"{KEY}\": {WRITTEN}")
        }));
    }

    let was = splice::member(text, at, KEY).ok_or_else(refuse)?;
    Ok(splice::over(text, was, WRITTEN))
}

/// Writes the boolean sandbox choice without changing any other setting.
///
/// # Errors
///
/// Refuses malformed or invalid current configuration and non-object containers.
pub fn sandboxing(text: &str, file: &str, enabled: bool) -> Result<String, ConfigError> {
    let text = if text.trim().is_empty() { "{}" } else { text };
    let document = crate::document::Document::parse(text, file, crate::document::Origin::User)?;
    let value = document.value();
    let written = if enabled { "true" } else { "false" };
    if value
        .get("sandbox")
        .and_then(|block| block.get("enabled"))
        .and_then(Value::as_bool)
        == Some(enabled)
    {
        return Ok(text.to_owned());
    }
    let refused = || ConfigError::Unspliceable {
        file: file.into(),
        at: "sandbox.enabled".into(),
        written: written.into(),
    };
    let root = splice::root(text).ok_or_else(refused)?;
    let Some(block) = value.get("sandbox") else {
        return Ok(splice::insert(text, root, |indent| match indent {
            Some(indent) => {
                format!("\"sandbox\": {{\n{indent}  \"enabled\": {written}\n{indent}}}")
            }
            None => format!("\"sandbox\": {{\"enabled\": {written}}}"),
        }));
    };
    let at = splice::member(text, root, "sandbox").ok_or_else(refused)?;
    if block.get("enabled").is_none() {
        return Ok(splice::insert(text, at, |_| {
            format!("\"enabled\": {written}")
        }));
    }
    let was = splice::member(text, at, "enabled").ok_or_else(refused)?;
    Ok(splice::over(text, was, written))
}

/// The text of a configuration file that reads fenced code in `theme`.
///
/// The same splice as [`drawing`], one key over. See it for what is preserved.
///
/// # Errors
///
/// As [`drawing`].
pub fn reading(text: &str, file: &str, theme: &str) -> Result<String, ConfigError> {
    output(text, file, "syntaxTheme", theme)
}

/// The text of a configuration file where `row` says `word`.
///
/// `word` is spelled as the menu holds a value — `true`, `dark`, `12` — and
/// written as the key's declaration wants it: a flag as a boolean, the scroll
/// speed as a number, everything else as a string. Every object on the way to
/// the key is created along with it, and every byte crucible did not put there
/// stays where it was.
///
/// The result is read back the way a start reads it before it is handed back,
/// so an answer the key does not take is refused here and the file is never
/// written with it: a theme crucible does not draw, a speed past its bounds,
/// a retention that needs a ceiling the file does not give.
///
/// # Errors
///
/// [`ConfigError::Malformed`] when the text is not JSON,
/// [`ConfigError::Unspliceable`] when it is JSON no answer can be written into
/// without rewriting, and the error a start would meet when the answer is not
/// one the key takes.
pub fn setting(text: &str, file: &str, row: &Row, word: &str) -> Result<String, ConfigError> {
    let answer = match row.values() {
        Values::Flag => word
            .parse::<bool>()
            .map_or_else(|_| Value::from(word), Value::Bool),
        Values::Whole { .. } => word
            .parse::<u64>()
            .map_or_else(|_| Value::from(word), Value::from),
        Values::Choice(_) | Values::Named => Value::from(word),
    };
    let written = spliced(text, file, row, &answer)?;
    let document = crate::document::Document::parse(&written, file, crate::document::Origin::User)?;
    crate::settings::Settings::resolve_checked(vec![document])?;
    Ok(written)
}

/// `text` with `answer` at `row`'s key, every object on the way created.
fn spliced(text: &str, file: &str, row: &Row, answer: &Value) -> Result<String, ConfigError> {
    let path: Vec<&str> = row.path().collect();
    let written = answer.to_string();

    if text.trim().is_empty() {
        return Ok(format!(
            "{{\n  {}\n}}\n",
            nested(&path, &written, Some("  "))
        ));
    }

    let value = parsed(text, file)?;
    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        at: row.key().into(),
        written: written.clone().into(),
    };
    let mut span = splice::root(text)
        .filter(|_| value.is_object())
        .ok_or_else(refuse)?;
    let mut held = &value;

    // Outwards in, as every other answer here is written: whichever object is
    // already there is where this stops, and the rest goes in with the key.
    for (depth, name) in path.iter().enumerate() {
        let Some(next) = held.get(name) else {
            let below = path.get(depth..).unwrap_or_default();
            return Ok(splice::insert(text, span, |indent| {
                nested(below, &written, indent)
            }));
        };
        if depth + 1 == path.len() {
            if next == answer {
                return Ok(text.to_owned());
            }
            let was = splice::member(text, span, name).ok_or_else(refuse)?;
            return Ok(splice::over(text, was, &written));
        }
        if !next.is_object() {
            return Err(refuse());
        }
        span = splice::member(text, span, name).ok_or_else(refuse)?;
        held = next;
    }
    Err(refuse())
}

/// `"a": {"b": written}` for the path `a.b`, laid out at `indent` where the
/// line it goes on has one, and on one line where it does not.
fn nested(path: &[&str], written: &str, indent: Option<&str>) -> String {
    let Some((first, rest)) = path.split_first() else {
        return written.to_owned();
    };
    let key = Value::from(*first).to_string();
    if rest.is_empty() {
        return format!("{key}: {written}");
    }
    match indent {
        Some(indent) => {
            let inner = format!("{indent}  ");
            let below = nested(rest, written, Some(&inner));
            format!("{key}: {{\n{inner}{below}\n{indent}}}")
        }
        None => format!("{key}: {{{}}}", nested(rest, written, None)),
    }
}

/// One key of the `output` block, spliced in beside whatever else is there.
fn output(text: &str, file: &str, key: &str, value_of: &str) -> Result<String, ConfigError> {
    let written = Value::String(value_of.to_owned()).to_string();

    if text.trim().is_empty() {
        return Ok(DRAWN.replace("KEY", key).replace("THEME", &written));
    }

    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;

    if value.get("output").and_then(|output| output.get(key))
        == Some(&Value::String(value_of.to_owned()))
    {
        return Ok(text.to_owned());
    }

    let refuse = || ConfigError::Unspliceable {
        file: file.into(),
        at: format!("output.{key}").into(),
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
                format!("\"output\": {{\n{indent}  \"{key}\": {written}\n{indent}}}")
            }
            None => format!("\"output\": {{\"{key}\": {written}}}"),
        }));
    };

    if !output.is_object() {
        return Err(refuse());
    }

    let block = splice::member(text, root, "output").ok_or_else(refuse)?;
    if output.get(key).is_none() {
        return Ok(splice::insert(text, block, |_| {
            format!("\"{key}\": {written}")
        }));
    }

    let was = splice::member(text, block, key).ok_or_else(refuse)?;
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
/// rung, or the speed kept for another model, cannot be lifted out for the
/// same reason.
pub fn choosing(
    text: &str,
    file: &str,
    provider: &str,
    model: &str,
) -> Result<String, ConfigError> {
    let was = named(text, provider);
    let written = beside(text, file, provider, "model", &Value::from(model))?;
    let written = without(&written, file, provider, "effort")?;
    // The speed was chosen at that model's price, so it stays only with it.
    if was.as_deref() == Some(model) {
        Ok(written)
    } else {
        without(&written, file, provider, "fast")
    }
}

/// The model `text` names for `provider`, where it is a file that names one.
fn named(text: &str, provider: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    answered(&value, provider, "model")
        .and_then(Value::as_str)
        .map(|named| named.trim().to_owned())
}

/// Takes `providers.<provider>.<key>` out, where the file has it.
fn without(text: &str, file: &str, provider: &str, key: &str) -> Result<String, ConfigError> {
    let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
        file: file.into(),
        line: source.line(),
        column: source.column(),
        problem: crate::document::without_position(&source.to_string()).into(),
    })?;
    let Some(chosen) = value.get("providers").and_then(|all| all.get(provider)) else {
        return Ok(text.to_owned());
    };
    if chosen.get(key).is_none() {
        return Ok(text.to_owned());
    }
    let refuse = || ConfigError::Unremovable {
        file: file.into(),
        at: format!("providers.{provider}.{key}").into(),
    };
    let root = splice::root(text).ok_or_else(refuse)?;
    let providers = splice::member(text, root, "providers").ok_or_else(refuse)?;
    let provider = splice::member(text, providers, provider).ok_or_else(refuse)?;
    splice::remove(text, provider, key).ok_or_else(refuse)
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
    beside(
        text,
        file,
        provider,
        "effort",
        &Value::from(effort.as_str()),
    )
}

/// The text of a configuration file that asks `provider` for `model`, fast.
///
/// The speed is a choice about one model at its price, so the model is
/// written with it, over whichever the file named, and the rung stays.
///
/// # Errors
///
/// [`ConfigError::Malformed`] and [`ConfigError::Unspliceable`], for the same
/// reasons as [`choosing`].
pub fn hastening(
    text: &str,
    file: &str,
    provider: &str,
    model: &str,
) -> Result<String, ConfigError> {
    let written = beside(text, file, provider, "model", &Value::from(model))?;
    beside(&written, file, provider, "fast", &Value::Bool(true))
}

/// The text of a configuration file that asks `provider` for standard: the
/// speed taken out, since a file that says nothing asks for standard.
///
/// # Errors
///
/// [`ConfigError::Malformed`], and [`ConfigError::Unremovable`] where a speed
/// written cannot be lifted out without rewriting.
pub fn slowing(text: &str, file: &str, provider: &str) -> Result<String, ConfigError> {
    if text.trim().is_empty() {
        return Ok(text.to_owned());
    }
    without(text, file, provider, "fast")
}

/// The speed `text`, the user's own file, asks `provider` for `model`: what
/// [`hastening`] wrote, read back the way a start reads it.
///
/// Fast only for the model the file names for the provider: a speed was
/// chosen for a model, at that model's price, and a file that names none
/// names no price.
///
/// # Errors
///
/// [`ConfigError`] where `text` is not a configuration file crucible reads.
pub fn hastened(text: &str, file: &str, provider: &str, model: &str) -> Result<Speed, ConfigError> {
    if text.trim().is_empty() {
        return Ok(Speed::Standard);
    }
    let document = crate::document::Document::parse(text, file, crate::document::Origin::User)?;
    let settings = crate::settings::Settings::resolve_checked(vec![document])?;
    // Trimmed as a run trims the name it asks for, and blank is no name.
    let beside = settings
        .model(provider)
        .map(str::trim)
        .filter(|named| !named.is_empty())
        == Some(model);
    Ok(if beside {
        settings.speed(provider)
    } else {
        Speed::Standard
    })
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
    answer: &Value,
) -> Result<String, ConfigError> {
    // Both as JSON reads them. A provider name is somebody else's string, and
    // one holding a quote written raw would end the document.
    let named = Value::String(provider.to_owned()).to_string();
    let written = answer.to_string();

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
fn answered<'a>(value: &'a Value, provider: &str, key: &str) -> Option<&'a Value> {
    value.get("providers")?.get(provider)?.get(key)
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
