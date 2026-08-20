//! Writes the model table, from the database a contributor pipes in.
//!
//! Not a bench probe, and the second thing under `src/bin/` that is not: it is
//! run by hand, its output is checked in, and no release contains it.
//!
//! It reads rather than fetches. What reaches the network is a `curl` in
//! `scripts/models.sh`, written where somebody can see it, and this turns what
//! that returned into Rust. A generator that fetched would be a second thing in
//! this repository that decides on its own to talk to a server.
//!
//! ```text
//! scripts/models.sh
//! ```
//!
//! Every model crucible offers is named here, against the key the database
//! spells it with. The two are not always the same word: one vendor is asked
//! for by the names its coding console serves, and the database lists the names
//! its open platform serves. Mapping them is a fact somebody checked, written
//! down once and reviewed in a diff — not a rule that strips or guesses at a
//! name, which is the thing the lookup must never do.

// This program's whole output is a file on stdout and its diagnostics are for
// whoever ran it, so the lint against printing is refusing the one thing it is
// for. It ships in no release and is on no render path, which is what that lint
// protects.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;

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
    ("anthropic", "claude-fable-5", Some("claude-fable-5"), None),
    ("anthropic", "claude-opus-5", Some("claude-opus-5"), None),
    ("anthropic", "claude-sonnet-5", Some("claude-sonnet-5"), None),
    ("anthropic", "claude-haiku-4-5", Some("claude-haiku-4-5"), None),
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
    ("openai", "gpt-5.6-sol", Some("gpt-5.6-sol"), None),
    ("openai", "gpt-5.6-terra", Some("gpt-5.6-terra"), None),
    ("openai", "gpt-5.6-luna", Some("gpt-5.6-luna"), None),
    ("openai", "gpt-5.5", Some("gpt-5.5"), None),
];

/// Which provider of the database each of crucible's is.
fn listed(provider: &str) -> &'static str {
    match provider {
        "moonshot" => "moonshotai",
        "anthropic" => "anthropic",
        _ => "openai",
    }
}

fn main() {
    let mut body = String::new();
    if std::io::stdin().read_to_string(&mut body).is_err() {
        eprintln!("generate-models: nothing on stdin — run scripts/models.sh");
        std::process::exit(2);
    }
    let Ok(database) = serde_json::from_str::<Value>(&body) else {
        eprintln!("generate-models: stdin was not the database's JSON");
        std::process::exit(2);
    };

    let mut rows: BTreeMap<(&str, &str), (u32, u32)> = BTreeMap::new();
    for (provider, offered, key, divisor) in OFFERED {
        let Some(key) = key else { continue };
        let limit = database
            .get(listed(provider))
            .and_then(|entry| entry.get("models"))
            .and_then(|models| models.get(key))
            .and_then(|model| model.get("limit"));
        let Some(limit) = limit else {
            eprintln!("generate-models: the database does not list {provider}/{key}");
            std::process::exit(1);
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
            eprintln!("generate-models: {provider}/{key} lists no usable limits");
            std::process::exit(1);
        };
        let (Ok(window), Ok(output)) = (u32::try_from(window), u32::try_from(output)) else {
            eprintln!("generate-models: {provider}/{key} states a limit too large to hold");
            std::process::exit(1);
        };

        // A model served at a fraction of the listed window is written at that
        // fraction, the divisor stated beside it rather than read from anywhere.
        let window = match divisor {
            Some(by) if *by > 1 => window / by,
            _ => window,
        };
        rows.insert((provider, offered), (window, output));
    }

    print!("{}", written(&rows));
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
fn written(rows: &BTreeMap<(&str, &str), (u32, u32)>) -> String {
    let mut out = String::from(
        "//! What each model crucible offers accepts and produces.\n\
         //!\n\
         //! Generated. Do not edit: `scripts/models.sh` writes this file, and a test\n\
         //! refuses a tree where the two disagree. What it is generated *from* is a\n\
         //! public database of model limits, read over the network by a `curl` in that\n\
         //! script rather than by anything here.\n\
         //!\n\
         //! Keyed on the model name exactly as crucible asks for it. A name not in this\n\
         //! table has no answer here at all, which is deliberate: a window guessed from a\n\
         //! name that merely resembles one is wrong by a factor nobody would notice until\n\
         //! a session had already thrown half of itself away.\n\n\
         /// What one model accepts and produces, in tokens.\n\
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
         }\n\n\
         /// Every model this build knows the limits of, sorted so a diff reads.\n\
         pub(crate) const FACTS: &[Facts] = &[\n",
    );
    for ((provider, model), (window, output)) in rows {
        let _ = write!(
            out,
            "    Facts {{\n        provider: {provider:?},\n        model: {model:?},\n        \
             window: {},\n        output: {},\n    }},\n",
            grouped(*window),
            grouped(*output),
        );
    }
    out.push_str("];\n");
    out
}
