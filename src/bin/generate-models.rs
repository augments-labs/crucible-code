//! Writes the model table, from the database a contributor pipes in.
//!
//! Not a bench probe, and the second thing under `src/bin/` that is not: it is
//! run by hand, what it writes is checked in, and no release contains it.
//!
//! It reads JSON from standard input and never fetches it. This keeps the
//! generator deterministic over the document it is handed and leaves obtaining
//! that document to the contributor.
//!
//! One file comes out of one run, and nothing in the suite reproduces it: the
//! database it was read from is served over the network and this repository
//! keeps no copy. What holds the table is the run and the diff it leaves, plus
//! the tests beside the catalogue that say every model crucible offers has a
//! row here and reads what its vendor documents.
//!
//! ```text
//! cat models.json | cargo run --quiet --bin generate-models
//! ```
//!
//! Every model crucible offers is named here, against the key the database
//! spells it with. The two are not always the same word: one vendor is asked
//! for by the names its coding console serves, and the database lists the names
//! its open platform serves. Mapping them is a fact somebody checked, written
//! down once and reviewed in a diff — not a rule that strips or guesses at a
//! name, which is the thing the lookup must never do.

// This program's diagnostics are for whoever ran it at a terminal, so the lint
// against printing is refusing the only way it has to say what went wrong. It
// ships in no release and is on no render path, which is what that lint
// protects.
#![allow(clippy::print_stderr)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;

use crucible_core::{Modalities, Modality};
use serde_json::Value;

/// Every model crucible offers, and the key the database lists it under.
///
/// The third field is `None` where the database has no entry at all: the model
/// gets no row, the lookup answers "nothing known", and the session falls back
/// to the configuration or to the provider refusing — the honest answer, and
/// the reason nothing here invents a number from a name.
///
/// The fourth is a divisor applied to the database's window, for a model the
/// vendor serves at a fraction of the size the database lists under the shared
/// key. It is a fact somebody checked, written down once beside the model —
/// never a figure read out of the model's own name, which is the guess this
/// program must never make. The output ceiling is left alone: it is a property
/// of the model, and the same model answers at the same length whatever slice
/// of its window a request may use.
const OFFERED: &[(&str, &str, Option<&str>, Option<u32>)] = &[
    (
        "anthropic",
        "claude-fable-5-1",
        Some("claude-fable-5-1"),
        None,
    ),
    ("anthropic", "claude-fable-5", Some("claude-fable-5"), None),
    ("anthropic", "claude-opus-5", Some("claude-opus-5"), None),
    (
        "anthropic",
        "claude-sonnet-5",
        Some("claude-sonnet-5"),
        None,
    ),
    (
        "anthropic",
        "claude-haiku-4-5",
        Some("claude-haiku-4-5"),
        None,
    ),
    ("google", "gemini-3.8-flash", Some("gemini-3.8-flash"), None),
    ("google", "gemini-3.7-flash", Some("gemini-3.7-flash"), None),
    ("google", "gemini-3.6-flash", Some("gemini-3.6-flash"), None),
    (
        "google",
        "gemini-3.1-pro-preview",
        Some("gemini-3.1-pro-preview"),
        None,
    ),
    ("moonshot", "k3", Some("kimi-k3"), None),
    // The same model held to a quarter of its window, which the database does
    // not list separately. The divisor is the vendor's stated figure, checked
    // once and written down — not read out of the `256k` in the name.
    ("moonshot", "k3-256k", Some("kimi-k3"), Some(4)),
    ("moonshot", "kimi-for-coding", Some("kimi-k2.7-code"), None),
    (
        "moonshot",
        "kimi-for-coding-highspeed",
        Some("kimi-k2.7-code-highspeed"),
        None,
    ),
    ("openai", "gpt-6-astra", Some("gpt-6-astra"), None),
    ("openai", "gpt-5.6-sol", Some("gpt-5.6-sol"), None),
    ("openai", "gpt-5.6-terra", Some("gpt-5.6-terra"), None),
    ("openai", "gpt-5.6-luna", Some("gpt-5.6-luna"), None),
    ("openai", "gpt-5.5", Some("gpt-5.5"), None),
];

/// The table this writes.
const TABLE: &str = "src/cli/models.rs";

/// Which provider of the database each of crucible's is.
fn listed(provider: &'static str) -> &'static str {
    match provider {
        "moonshot" => "moonshotai",
        other => other,
    }
}

fn main() {
    let mut body = String::new();
    if std::io::stdin().read_to_string(&mut body).is_err() {
        eprintln!("generate-models: expected the model database JSON on stdin");
        std::process::exit(2);
    }
    let Ok(database) = serde_json::from_str::<Value>(&body) else {
        eprintln!("generate-models: stdin was not the database's JSON");
        std::process::exit(2);
    };

    if let Err(why) = generate(&database) {
        eprintln!("generate-models: {why}");
        std::process::exit(1);
    }
}

/// The table, from one reading.
fn generate(database: &Value) -> Result<(), String> {
    let found = found(database)?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let body = written(&rows(&found)?);
    std::fs::write(root.join(TABLE), body)
        .map_err(|problem| format!("{TABLE} could not be written: {problem}"))
}

/// One offered model, found in the database.
struct Found<'a> {
    /// The provider crucible asks it of.
    provider: &'static str,
    /// The model, spelled the way crucible asks for it.
    model: &'static str,
    /// The model as the database spells it.
    key: &'static str,
    /// The fraction of the listed window this one is served at, where it is one.
    divisor: Option<u32>,
    /// What the database says about it.
    entry: &'a Value,
}

/// Every offered model the database lists.
///
/// A model in the catalogue that the database has never heard of stops the run
/// rather than being written down as unknown. What a table of limits is for is
/// the rows in it, and a row this program could not read is a window crucible
/// would go on guessing at while a file said otherwise.
fn found(database: &Value) -> Result<Vec<Found<'_>>, String> {
    let mut found = Vec::new();
    for (provider, model, key, divisor) in OFFERED {
        let Some(key) = key else { continue };
        let entry = database
            .get(listed(provider))
            .and_then(|provider| provider.get("models"))
            .and_then(|models| models.get(key));
        let Some(entry) = entry else {
            return Err(format!("the database does not list {provider}/{key}"));
        };
        found.push(Found {
            provider,
            model,
            key,
            divisor: *divisor,
            entry,
        });
    }
    Ok(found)
}

/// What each of them is worth writing down.
fn rows<'a>(found: &[Found<'a>]) -> Result<BTreeMap<(&'a str, &'a str), Row>, String> {
    let mut rows = BTreeMap::new();
    for one in found {
        let (provider, key) = (one.provider, one.key);
        let Some(limit) = one.entry.get("limit") else {
            return Err(format!("{provider}/{key} lists no limits"));
        };

        // The input limit where the vendor states one apart from the whole
        // window, because that is what a request has to fit in. The two differ
        // by exactly what an answer may produce — one vendor lists 400 000 and
        // 272 000 against a 128 000 answer, and 1 050 000 and 922 000 against
        // the same — so the whole window counts the answer as well. Taking it
        // would state a window too large by exactly the room being reserved for
        // that answer, which is the one number this must not double-count.
        let window = limit
            .get("input")
            .and_then(Value::as_u64)
            .filter(|input| *input > 0)
            .or_else(|| limit.get("context").and_then(Value::as_u64));
        let output = limit.get("output").and_then(Value::as_u64);

        let (Some(window), Some(output)) = (window, output) else {
            return Err(format!("{provider}/{key} lists no usable limits"));
        };
        let (Ok(window), Ok(output)) = (u32::try_from(window), u32::try_from(output)) else {
            return Err(format!("{provider}/{key} states a limit too large to hold"));
        };

        // A model served at a fraction of the listed window is written at that
        // fraction, the divisor stated beside it rather than read from anywhere.
        let window = match one.divisor {
            Some(by) if by > 1 => window / by,
            _ => window,
        };
        let accepts = accepts(one.entry).map_err(|why| format!("{provider}/{key} {why}"))?;
        rows.insert(
            (provider, one.model),
            Row {
                window,
                output,
                accepts,
            },
        );
    }
    Ok(rows)
}

/// One model's answers, on the way to being written out.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// The most one request may carry, in tokens.
    window: u32,
    /// The most one answer may produce, in tokens.
    output: u32,
    /// What the model reads.
    accepts: Modalities,
}

/// What the database says this model accepts.
///
/// Both ways of failing here are loud on purpose, and they are opposite. An
/// entry with no `modalities` at all must stop the run rather than write an
/// empty set, because an empty set makes a capable model look incapable and
/// nothing downstream can tell that apart from a model that really takes only
/// text. A word this build has never heard of must stop it too: that is the
/// database's vocabulary having gained a member, which is a change to what
/// crucible can be asked about and belongs in a diff somebody reads.
fn accepts(entry: &Value) -> Result<Modalities, String> {
    let input = entry
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array);
    let Some(input) = input else {
        return Err(String::from("lists no modalities.input"));
    };
    let mut accepts = Modalities::empty();
    for word in input {
        let Some(word) = word.as_str() else {
            return Err(format!(
                "lists {word} among its modalities, which is not a word"
            ));
        };
        accepts = accepts.insert(word.parse::<Modality>().map_err(|why| why.to_string())?);
    }
    Ok(accepts)
}

/// A set as the expression that rebuilds it.
///
/// The variant is written from `Debug`, which is the variant's own name, so a
/// modality renamed in core renames itself here rather than in a second list
/// that would quietly keep spelling the old one.
fn spelled(accepts: Modalities) -> String {
    let mut out = String::from("Modalities::empty()");
    for one in accepts.iter() {
        let _ = write!(out, ".insert(Modality::{one:?})");
    }
    out
}

/// A number with the separators this project's lint asks for.
///
/// Written here rather than left to whoever regenerates the file, because a
/// generated file nobody may edit cannot be fixed by hand afterwards.
fn grouped(number: u32) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('_');
        }
        out.push(digit);
    }
    out
}

/// The table, as the file that is checked in.
fn written(rows: &BTreeMap<(&str, &str), Row>) -> String {
    let mut out = String::from(
        "//! What each model crucible offers accepts and produces.\n\
         //!\n\
         //! Generated by `generate-models` from JSON supplied on standard input. A row\n\
         //! changed by hand is discarded by the next refresh. The source database is\n\
         //! not checked in, so the diff and the tests beside the catalogue hold every\n\
         //! model crucible offers to a row here.\n\
         //!\n\
         //! Keyed on the model name exactly as crucible asks for it. A name not in this\n\
         //! table has no answer here at all, which is deliberate: a window guessed from a\n\
         //! name that merely resembles one is wrong by a factor nobody would notice until\n\
         //! a session had already thrown half of itself away.\n\n\
         use crucible_core::{Modalities, Modality};\n\n\
         /// What one model accepts and produces.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub(crate) struct Facts {\n\
         \x20   /// The provider it is asked of.\n\
         \x20   pub(crate) provider: &'static str,\n\
         \x20   /// The model, spelled the way crucible asks for it.\n\
         \x20   pub(crate) model: &'static str,\n\
         \x20   /// The most one request may carry, in tokens.\n\
         \x20   pub(crate) window: u32,\n\
         \x20   /// The most one answer may produce, in tokens.\n\
         \x20   pub(crate) output: u32,\n\
         \x20   /// What the model reads. Half of what may be attached; the\n\
         \x20   /// other half is what the provider can spell.\n\
         \x20   pub(crate) accepts: Modalities,\n\
         }\n\n\
         /// Every model this build knows the limits of, sorted so a diff reads.\n\
         pub(crate) const FACTS: &[Facts] = &[\n",
    );
    for ((provider, model), row) in rows {
        let _ = write!(
            out,
            "    Facts {{\n        provider: {provider:?},\n        model: {model:?},\n        \
             window: {},\n        output: {},\n        accepts: {},\n    }},\n",
            grouped(row.window),
            grouped(row.output),
            spelled(row.accepts),
        );
    }
    out.push_str("];\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn google_models_are_read_from_the_google_database_namespace() {
        assert_eq!(listed("google"), "google");
        assert_eq!(listed("moonshot"), "moonshotai");
        assert_eq!(listed("custom"), "custom");
    }

    #[test]
    fn all_six_new_model_entries_are_regenerated_under_their_exact_names() {
        for (provider, model) in [
            ("google", "gemini-3.8-flash"),
            ("google", "gemini-3.7-flash"),
            ("google", "gemini-3.6-flash"),
            ("google", "gemini-3.1-pro-preview"),
            ("anthropic", "claude-fable-5-1"),
            ("openai", "gpt-6-astra"),
        ] {
            assert!(
                OFFERED.contains(&(provider, model, Some(model), None)),
                "missing generator entry {provider}/{model}"
            );
        }
    }

    /// The set the four Anthropic rows and the four OpenAI rows carry.
    fn documents() -> Modalities {
        Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf)
    }

    #[test]
    fn generating_models_reads_the_modalities_the_database_lists() {
        let entry = json!({
            "limit": { "context": 200_000, "output": 64_000 },
            "modalities": { "input": ["text", "image", "pdf"], "output": ["text"] },
        });
        assert_eq!(accepts(&entry), Ok(documents()));
    }

    #[test]
    fn generating_models_without_modalities_fails_rather_than_writing_an_empty_set() {
        let entry = json!({ "limit": { "context": 200_000, "output": 64_000 } });
        assert!(
            accepts(&entry).is_err(),
            "a model with no modalities is not a model that reads nothing"
        );

        let output_only = json!({ "modalities": { "output": ["text"] } });
        assert!(
            accepts(&output_only).is_err(),
            "output alone says nothing about what is read"
        );
    }

    #[test]
    fn generating_models_from_a_sixth_word_fails_rather_than_skipping_it() {
        let entry = json!({ "modalities": { "input": ["text", "hologram"] } });
        let why = accepts(&entry).expect_err("a word this build has never heard of");
        assert!(
            why.contains("hologram"),
            "the failure has to name the word: {why}"
        );
    }

    #[test]
    fn generating_models_writes_a_set_as_the_expression_that_rebuilds_it() {
        assert_eq!(spelled(Modalities::empty()), "Modalities::empty()");
        assert_eq!(
            spelled(documents()),
            "Modalities::empty().insert(Modality::Text)\
             .insert(Modality::Image).insert(Modality::Pdf)",
        );
    }
}
