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
//! cat models.json | cargo run --quiet --bin generate-models -- --drift
//! ```
//!
//! The second writes nothing. It reports, as Markdown on standard output, what
//! the database says against what this build offers: a newer model in a family
//! crucible offers, an offered model gone or deprecated, and rungs listed
//! apart from the ones offered. It exits 0 with nothing to report, 1 with
//! something, and 2 when the input could not be read. A scheduled workflow runs
//! it, and what it reports is settled by hand from the vendors' own pages.
//!
//! Every model crucible offers is named here, against the key the database
//! spells it with. The two are not always the same word: one vendor is asked
//! for by the names its coding console serves, and the database lists the names
//! its open platform serves. Mapping them is a fact somebody checked, written
//! down once and reviewed in a diff — not a rule that strips or guesses at a
//! name, which is the thing the lookup must never do.

// This program's diagnostics are for whoever ran it at a terminal, and its
// drift report is for the workflow that reads it, so the lints against
// printing are refusing the only ways it has to answer. It
// ships in no release and is on no render path, which is what that lint
// protects.
#![allow(clippy::print_stderr)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write as _};

use crucible_models::Effort;
use crucible_types::{Modalities, Modality};
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
/// program must never make. The output ceiling is otherwise left alone: it is a
/// property of the model, and the same model answers at the same length
/// whatever slice of its window a request may use, short of the window itself.
const OFFERED: &[Offer] = &[
    (
        "anthropic",
        "claude-fable-5-1",
        Some("claude-fable-5-1"),
        None,
    ),
    ("anthropic", "claude-fable-5", Some("claude-fable-5"), None),
    (
        "anthropic",
        "claude-opus-5-5",
        Some("claude-opus-5-5"),
        None,
    ),
    ("anthropic", "claude-opus-5", Some("claude-opus-5"), None),
    (
        "anthropic",
        "claude-sonnet-5-5",
        Some("claude-sonnet-5-5"),
        None,
    ),
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
    ("deepseek", "deepseek-flash", Some("deepseek-flash"), None),
    ("deepseek", "deepseek-v4-pro", Some("deepseek-v4-pro"), None),
    ("meta", "muse-spark-1.3", Some("muse-spark-1.3"), None),
    (
        "meta",
        "muse-spark-1.3-contributor",
        Some("muse-spark-1.3-contributor"),
        None,
    ),
    ("meta", "muse-spark-1.2", Some("muse-spark-1.2"), None),
    (
        "meta",
        "muse-spark-1.2-contributor",
        Some("muse-spark-1.2-contributor"),
        None,
    ),
    ("mimo", "mimo-v2.6-pro", Some("mimo-v2.6-pro"), None),
    ("mimo", "mimo-v2.6-flash", Some("mimo-v2.6-flash"), None),
    ("minimax", "MiniMax-M3", Some("MiniMax-M3"), None),
    ("minimax", "MiniMax-M2.7", Some("MiniMax-M2.7"), None),
    ("openai", "gpt-6-astra", Some("gpt-6-astra"), None),
    ("openai", "gpt-6.1-sol", Some("gpt-6.1-sol"), None),
    ("openai", "gpt-6-sol", Some("gpt-6-sol"), None),
    ("openai", "gpt-6-luna", Some("gpt-6-luna"), None),
    ("openai", "gpt-5.6-sol", Some("gpt-5.6-sol"), None),
    ("openai", "gpt-5.6-terra", Some("gpt-5.6-terra"), None),
    ("openai", "gpt-5.6-luna", Some("gpt-5.6-luna"), None),
    ("openai", "gpt-5.5", Some("gpt-5.5"), None),
    ("qwen", "qwen3.8-max", Some("qwen3.8-max"), None),
    ("qwen", "qwen3.8-flash", Some("qwen3.8-flash"), None),
    ("qwen", "qwen3.7-plus", Some("qwen3.7-plus"), None),
    ("qwen", "qwen3.6-plus", Some("qwen3.6-plus"), None),
    ("xai", "grok-4.7", Some("grok-4.7"), None),
    ("xai", "grok-4.6", Some("grok-4.6"), None),
    ("zai", "glm-5.3", Some("glm-5.3"), None),
    ("zai", "glm-5.3-flash", Some("glm-5.3-flash"), None),
    ("zai", "glm-5.2", Some("glm-5.2"), None),
];

/// One model crucible offers: its provider, its name, the key the database
/// lists it under and the divisor applied to the listed window.
type Offer = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<u32>,
);

/// Models the drift check has reported and that crucible chose not to offer,
/// and why, so that a decision once made is not put again every fortnight.
const PASSED: &[(&str, &str, &str)] = &[
    (
        "openai",
        "gpt-daybreak-red-latest",
        "an alias the vendor's models page does not list, naming no model a session could be told it asked",
    ),
    (
        "meta",
        "muse-wonder-1.0",
        "a preview whose terms on what Meta does with what is sent have not been read",
    ),
    (
        "zai",
        "glm-5.3-flashx",
        "the fast form of glm-5.3-flash, held back until it is known how Z.ai refuses it",
    ),
    (
        "qwen",
        "qwen3.8-omni-flash",
        "the vendor's audio and video model; its coding models are the 3.8 max and flash already offered",
    ),
];

/// Offered models whose rungs crucible keeps apart from the database's, and
/// why: the vendor's own pages are the authority, and the database is not.
const KEPT: &[(&str, &str, &str)] = &[(
    "meta",
    "muse-spark-1.3",
    "Meta's parameter lists leave out the max some of its pages name, so it is not offered until a source settles it",
)];

/// The table this writes.
const TABLE: &str = "crates/crucible-app/src/models.rs";

/// Which provider of the database each of crucible's is.
fn listed(provider: &'static str) -> &'static str {
    match provider {
        "moonshot" => "moonshotai",
        // The pay-as-you-go listing; a plan serves the same models under the
        // same names, and some of them with a smaller ceiling, which the
        // window crucible starts at is far below either way.
        "qwen" => "alibaba",
        "mimo" => "xiaomi",
        other => other,
    }
}

/// What a run was asked to do.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// Rewrite the model table.
    Generate,
    /// Report the shipped lists against the database, and write nothing.
    Drift,
}

/// The mode the first argument names, or `None` for one this does not take.
fn mode(first: Option<&str>) -> Option<Mode> {
    match first {
        None => Some(Mode::Generate),
        Some("--drift") => Some(Mode::Drift),
        Some(_) => None,
    }
}

fn main() {
    let Some(mode) = mode(std::env::args().nth(1).as_deref()) else {
        eprintln!("generate-models: takes no argument, or --drift");
        std::process::exit(2);
    };
    let mut body = String::new();
    if std::io::stdin().read_to_string(&mut body).is_err() {
        eprintln!("generate-models: expected the model database JSON on stdin");
        std::process::exit(2);
    }
    let Ok(database) = serde_json::from_str::<Value>(&body) else {
        eprintln!("generate-models: stdin was not the database's JSON");
        std::process::exit(2);
    };

    if mode == Mode::Drift {
        std::process::exit(drifted(&database));
    }
    if let Err(why) = generate(&database) {
        eprintln!("generate-models: {why}");
        std::process::exit(1);
    }
}

/// The drift check: the report on standard output, and whether it found
/// anything as the exit status.
fn drifted(database: &Value) -> i32 {
    let Ok(providers) = crucible_app::providers::providers().map(|registry| registry.snapshot())
    else {
        eprintln!("generate-models: this build's providers could not be registered");
        return 2;
    };
    let rungs =
        |provider: &str, model: &str| crucible_app::providers::rungs(&providers, provider, model);
    let found = drift(database, OFFERED, PASSED, KEPT, &rungs);
    if found.is_empty() {
        return 0;
    }
    if std::io::stdout()
        .write_all(reported(&found).as_bytes())
        .is_err()
    {
        eprintln!("generate-models: the report could not be written");
        return 2;
    }
    1
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

        // The whole window an answer is written into, which counts the answer
        // where the input limit does not.
        let whole = match limit
            .get("context")
            .and_then(Value::as_u64)
            .filter(|whole| *whole > 0)
        {
            Some(whole) => u32::try_from(whole)
                .map_err(|_| format!("{provider}/{key} states a limit too large to hold"))?,
            None => window,
        };

        // A model served at a fraction of the listed window is written at that
        // fraction, the divisor stated beside it rather than read from anywhere.
        let (window, whole) = match one.divisor {
            Some(by) if by > 1 => (window / by, whole / by),
            _ => (window, whole),
        };
        // And no answer runs longer than the whole window it is written into,
        // whatever the database lists beside a window it serves whole.
        let output = output.min(whole);
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

/// What the drift check found the database to say against this build.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Finding {
    /// A model released after every offered one of its family.
    Newer {
        provider: &'static str,
        family: String,
        key: String,
        released: String,
    },
    /// An offered model the database no longer lists.
    Gone {
        provider: &'static str,
        model: &'static str,
    },
    /// An offered model the database lists as deprecated.
    Retired {
        provider: &'static str,
        model: &'static str,
    },
    /// An offered model whose listed rungs are not the ones offered.
    Rungs {
        provider: &'static str,
        model: &'static str,
        listed: Vec<Effort>,
        offered: Vec<Effort>,
    },
}

/// What the database says against what this build offers.
///
/// Read through [`OFFERED`] and [`listed`], so the providers it looks at and the
/// keys it reads are the generator's own, and it never proposes a provider or a
/// family crucible does not already offer: those are chosen by hand. A newer
/// model is one released after every offered model of its family, that calls
/// tools and answers in text, and that nobody has declined in `passed`. Rungs
/// are compared only where the database lists effort at all, since one that
/// lists none says nothing either way, only in the words crucible's ladder
/// has, and not for a model whose difference is `kept`.
fn drift(
    database: &Value,
    offered: &[Offer],
    passed: &[(&str, &str, &str)],
    kept: &[(&str, &str, &str)],
    rungs: &dyn Fn(&str, &str) -> Vec<Effort>,
) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut newest: BTreeMap<(&'static str, &str), &str> = BTreeMap::new();
    for (provider, model, key, _) in offered {
        let Some(key) = key else { continue };
        let models = database
            .get(listed(provider))
            .and_then(|provider| provider.get("models"));
        let Some(entry) = models.and_then(|models| models.get(key)) else {
            found.push(Finding::Gone { provider, model });
            continue;
        };
        if entry.get("status").and_then(Value::as_str) == Some("deprecated") {
            found.push(Finding::Retired { provider, model });
        }
        if let (Some(family), Some(released)) = (word(entry, "family"), word(entry, "release_date"))
        {
            let latest = newest.entry((provider, family)).or_insert(released);
            *latest = (*latest).max(released);
        }
        let listed = efforts(entry);
        let offered = rungs(provider, model);
        let settled = kept
            .iter()
            .any(|(of, model, _)| of == provider && model == key);
        if !listed.is_empty() && listed != offered && !settled {
            found.push(Finding::Rungs {
                provider,
                model,
                listed,
                offered,
            });
        }
    }

    for ((provider, family), latest) in &newest {
        let Some(models) = database
            .get(listed(provider))
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (key, entry) in models {
            let released = word(entry, "release_date").unwrap_or_default();
            let known = offered
                .iter()
                .any(|(of, _, offered, _)| of == provider && *offered == Some(key.as_str()))
                || passed
                    .iter()
                    .any(|(of, declined, _)| of == provider && declined == key);
            if word(entry, "family") == Some(*family)
                && released > *latest
                && !known
                && coding(entry)
            {
                found.push(Finding::Newer {
                    provider,
                    family: (*family).to_owned(),
                    key: key.clone(),
                    released: released.to_owned(),
                });
            }
        }
    }
    found
}

/// One field of an entry, where it is a string.
fn word<'a>(entry: &'a Value, field: &str) -> Option<&'a str> {
    entry.get(field).and_then(Value::as_str)
}

/// Whether a model is one a coding session could be asked of: one that calls
/// tools, answers in text alone, and is not retired.
fn coding(entry: &Value) -> bool {
    let text = entry
        .get("modalities")
        .and_then(|modalities| modalities.get("output"))
        .and_then(Value::as_array)
        .is_some_and(|output| output.iter().all(|one| one == "text") && !output.is_empty());
    entry.get("tool_call") == Some(&Value::Bool(true))
        && text
        && word(entry, "status") != Some("deprecated")
}

/// The rungs of crucible's ladder the database lists for a model, in ladder
/// order; a word the ladder does not have is left out.
fn efforts(entry: &Value) -> Vec<Effort> {
    let listed: Vec<&str> = entry
        .get("reasoning_options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|option| word(option, "type") == Some("effort"))
        .filter_map(|option| option.get("values").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    Effort::LADDER
        .into_iter()
        .filter(|rung| listed.contains(&rung.as_str()))
        .collect()
}

/// The findings, as the Markdown an issue is opened with.
/// A name read from the database as it may be shown in a code span: on one
/// line, with no backtick to end the span and no control character, since the
/// database is a third party's and the report is posted and logged as it is.
fn shown(name: &str) -> String {
    name.chars()
        .filter(|one| *one != '`' && !one.is_control())
        .collect()
}

fn reported(found: &[Finding]) -> String {
    let spelled = |rungs: &[Effort]| {
        if rungs.is_empty() {
            return String::from("none");
        }
        rungs
            .iter()
            .map(|rung| rung.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = String::from(
        "The model database at models.dev lists these against what crucible offers.\n\n",
    );
    for one in found {
        let _ = match one {
            Finding::Newer {
                provider,
                family,
                key,
                released,
            } => writeln!(
                out,
                "- {provider}: `{}` is newer in the `{}` family ({}) and is not offered.",
                shown(key),
                shown(family),
                shown(released)
            ),
            Finding::Gone { provider, model } => {
                writeln!(out, "- {provider}: `{model}` is no longer listed.")
            }
            Finding::Retired { provider, model } => {
                writeln!(out, "- {provider}: `{model}` is listed as deprecated.")
            }
            Finding::Rungs {
                provider,
                model,
                listed,
                offered,
            } => writeln!(
                out,
                "- {provider}: `{model}` lists {}; crucible offers {}.",
                spelled(listed),
                spelled(offered)
            ),
        };
    }
    out.push_str(
        "\nThe database is a lead, not the authority: check each against the vendor's \
         own pages and settle it as the repository's `refresh-the-models` skill says.\n",
    );
    out
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
/// renamed modality renames itself here rather than in a second list that would
/// quietly keep spelling the old one.
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
         use crucible_types::{Modalities, Modality};\n\n\
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
    fn what_is_written_around_the_rows_is_what_the_checked_in_table_has_around_them() {
        // The rows come from a database that is not checked in, so a refresh is
        // the only thing that would otherwise show the two had drifted: a field
        // the table spells one way and this writes another is a refresh that
        // changes a file nobody meant to change. No rows in, so what comes out
        // is everything that is not a row.
        let table = include_str!("../../crates/crucible-app/src/models.rs");
        let around = written(&BTreeMap::new());
        let opening = around
            .strip_suffix("];\n")
            .expect("a table with no rows still closes");

        assert!(opening.ends_with("&[\n"), "{opening}");
        assert_eq!(table.get(..opening.len()), Some(opening));
        assert!(
            table.ends_with("    },\n];\n"),
            "the table closes after a row"
        );
    }

    #[test]
    fn an_answer_is_held_within_the_whole_window_and_not_the_input_alone() {
        let entry = |limit: Value| json!({ "limit": limit, "modalities": { "input": ["text"] } });
        let row = |entry: &Value, divisor: Option<u32>| {
            let found = [Found {
                provider: "fabricated",
                model: "m",
                key: "m",
                divisor,
                entry,
            }];
            rows(&found).map(|rows| {
                rows.get(&("fabricated", "m"))
                    .map(|row| (row.window, row.output))
            })
        };

        // An input limit apart from the window leaves the answer its own room.
        let split = entry(json!({ "context": 200_000, "input": 72_000, "output": 128_000 }));
        assert_eq!(row(&split, None), Ok(Some((72_000, 128_000))));
        // A window served at a fraction holds no answer longer than itself.
        let whole = entry(json!({ "context": 1_048_576, "output": 1_048_576 }));
        assert_eq!(row(&whole, Some(4)), Ok(Some((262_144, 262_144))));
        // A whole window of nothing is no window stated, as an input limit of
        // nothing is, and one too large to hold stops the run as the window
        // would, rather than falling back to the input limit unsaid.
        let nothing = entry(json!({ "context": 0, "input": 72_000, "output": 128_000 }));
        assert_eq!(row(&nothing, None), Ok(Some((72_000, 72_000))));
        let huge =
            entry(json!({ "context": 5_000_000_000_u64, "input": 72_000, "output": 128_000 }));
        assert!(row(&huge, None).is_err());
    }

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

    /// One offered model, asked for by the key the database lists it under.
    const SHIPPED: &[Offer] = &[(
        "anthropic",
        "claude-opus-5-5",
        Some("claude-opus-5-5"),
        None,
    )];

    /// A coding model of the database, as the drift check reads one.
    fn listing(family: &str, released: &str, efforts: &[&str]) -> Value {
        json!({
            "family": family,
            "release_date": released,
            "tool_call": true,
            "modalities": { "input": ["text"], "output": ["text"] },
            "reasoning_options": [{ "type": "effort", "values": efforts }],
        })
    }

    /// The rungs offered for every model, as a shipped build would answer.
    fn every(_: &str, _: &str) -> Vec<Effort> {
        Effort::LADDER.to_vec()
    }

    fn database(models: &Value) -> Value {
        json!({ "anthropic": { "models": models } })
    }

    /// An entry with one field of it set, a field of the top level or the
    /// modalities.
    fn with(mut entry: Value, field: &str, value: Value) -> Value {
        if let Some(slot) = entry.pointer_mut(field) {
            *slot = value;
        } else if let Some(fields) = entry.as_object_mut() {
            fields.insert(field.trim_start_matches('/').to_owned(), value);
        }
        entry
    }

    const RUNGS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

    #[test]
    fn drift_reports_a_newer_coding_model_in_a_family_crucible_offers() {
        let database = database(&json!({
            "claude-opus-5-5": listing("claude-opus", "2026-09-01", RUNGS),
            "claude-opus-5-6": listing("claude-opus", "2026-10-01", RUNGS),
            "claude-opus-5-4": listing("claude-opus", "2026-08-01", RUNGS),
        }));

        assert_eq!(
            drift(&database, SHIPPED, &[], &[], &every),
            vec![Finding::Newer {
                provider: "anthropic",
                family: String::from("claude-opus"),
                key: String::from("claude-opus-5-6"),
                released: String::from("2026-10-01"),
            }],
            "an older model of the family is no news, and a newer one is"
        );
    }

    #[test]
    fn drift_leaves_out_what_no_coding_session_could_be_asked() {
        let image = with(
            listing("claude-opus", "2026-10-01", RUNGS),
            "/modalities/output",
            json!(["image"]),
        );
        let toolless = with(
            listing("claude-opus", "2026-10-02", RUNGS),
            "/tool_call",
            json!(false),
        );
        let retired = with(
            listing("claude-opus", "2026-10-03", RUNGS),
            "/status",
            json!("deprecated"),
        );
        let database = database(&json!({
            "claude-opus-5-5": listing("claude-opus", "2026-09-01", RUNGS),
            "claude-opus-image": image,
            "claude-opus-toolless": toolless,
            "claude-opus-retired": retired,
            "claude-opus-declined": listing("claude-opus", "2026-10-04", RUNGS),
            "claude-other-5": listing("claude-other", "2026-10-05", RUNGS),
        }));
        let passed = [("anthropic", "claude-opus-declined", "fabricated")];

        assert_eq!(
            drift(&database, SHIPPED, &passed, &[], &every),
            Vec::new(),
            "an image model, a model that calls no tools, a retired one, one \
             already declined and a family crucible never offered are all quiet"
        );
    }

    #[test]
    fn drift_reports_a_shipped_model_gone_or_retired() {
        let retired = with(
            listing("claude-opus", "2026-09-01", RUNGS),
            "/status",
            json!("deprecated"),
        );

        assert_eq!(
            drift(&database(&json!({})), SHIPPED, &[], &[], &every),
            vec![Finding::Gone {
                provider: "anthropic",
                model: "claude-opus-5-5",
            }]
        );
        assert_eq!(
            drift(
                &database(&json!({ "claude-opus-5-5": retired })),
                SHIPPED,
                &[],
                &[],
                &every
            ),
            vec![Finding::Retired {
                provider: "anthropic",
                model: "claude-opus-5-5",
            }]
        );
    }

    #[test]
    fn drift_reports_rungs_the_database_lists_apart_from_the_ones_offered() {
        let fewer = |_: &str, _: &str| vec![Effort::Low, Effort::Medium, Effort::High];
        let database = database(&json!({
            "claude-opus-5-5": listing("claude-opus", "2026-09-01", &["none", "minimal", "low", "medium", "high", "max"]),
        }));

        assert_eq!(
            drift(&database, SHIPPED, &[], &[], &fewer),
            vec![Finding::Rungs {
                provider: "anthropic",
                model: "claude-opus-5-5",
                listed: vec![Effort::Low, Effort::Medium, Effort::High, Effort::Max],
                offered: vec![Effort::Low, Effort::Medium, Effort::High],
            }],
            "words that are no rung here are not a difference, and a missing one is"
        );
    }

    #[test]
    fn drift_does_not_report_again_rungs_kept_apart_from_the_database_on_purpose() {
        let fewer = |_: &str, _: &str| vec![Effort::Low, Effort::Medium, Effort::High];
        let database = database(&json!({
            "claude-opus-5-5": listing("claude-opus", "2026-09-01", &["low", "medium", "high", "max"]),
        }));
        let kept = [("anthropic", "claude-opus-5-5", "fabricated")];

        assert_eq!(drift(&database, SHIPPED, &[], &kept, &fewer), vec![]);
    }

    #[test]
    fn an_argument_other_than_drift_is_refused_rather_than_rewriting_the_table() {
        assert_eq!(mode(None), Some(Mode::Generate));
        assert_eq!(mode(Some("--drift")), Some(Mode::Drift));
        assert_eq!(mode(Some("--drfit")), None);
        assert_eq!(mode(Some("--help")), None);
    }

    #[test]
    fn drift_writes_a_name_from_the_database_as_one_line_inside_its_code_span() {
        let report = reported(&[Finding::Newer {
            provider: "anthropic",
            family: String::from("claude`\n::stop-commands::x"),
            key: String::from("claude-opus-5-6` @org/team\r"),
            released: String::from("2026-10-01\n- forged"),
        }]);

        assert_eq!(
            report.lines().filter(|line| line.starts_with("- ")).count(),
            1
        );
        assert!(report.contains("`claude-opus-5-6 @org/team`"), "{report}");
        assert!(report.contains("`claude::stop-commands::x`"), "{report}");
        assert!(!report.contains('\r'), "{report}");
    }

    #[test]
    fn drift_takes_a_model_listing_no_rungs_as_saying_nothing_about_them() {
        let mut unlisted = listing("claude-opus", "2026-09-01", RUNGS);
        unlisted
            .as_object_mut()
            .map(|entry| entry.remove("reasoning_options"));

        assert_eq!(
            drift(
                &database(&json!({ "claude-opus-5-5": unlisted })),
                SHIPPED,
                &[],
                &[],
                &every
            ),
            Vec::new()
        );
    }

    #[test]
    fn drift_writes_one_line_per_finding_and_names_the_skill_that_settles_them() {
        let found = [
            Finding::Gone {
                provider: "anthropic",
                model: "claude-opus-5-5",
            },
            Finding::Rungs {
                provider: "anthropic",
                model: "claude-opus-5-5",
                listed: vec![Effort::Low, Effort::Max],
                offered: vec![Effort::Low],
            },
        ];
        let report = reported(&found);

        assert!(
            report.contains("- anthropic: `claude-opus-5-5` is no longer listed"),
            "{report}"
        );
        assert!(
            report.contains("lists low, max; crucible offers low"),
            "{report}"
        );
        assert!(report.contains("`refresh-the-models` skill"), "{report}");
    }

    #[test]
    fn every_provider_says_on_which_day_its_vendor_was_last_read() {
        let providers = crucible_app::providers::providers()
            .expect("the built-in providers register")
            .snapshot();
        for one in crucible_app::providers::offered(&providers) {
            let day = one.reviewed.parse::<jiff::civil::Date>();
            assert!(
                day.is_ok() && one.reviewed.len() == "YYYY-MM-DD".len(),
                "{}: reviewed {:?} is not a day written as YYYY-MM-DD",
                one.name,
                one.reviewed
            );
        }
    }

    #[test]
    fn every_rung_difference_kept_is_on_a_model_offered_and_says_why() {
        for (provider, key, why) in KEPT {
            assert!(
                OFFERED
                    .iter()
                    .any(|(of, _, offered, _)| of == provider && *offered == Some(*key)),
                "{provider}/{key} keeps its rungs but is not offered"
            );
            assert!(
                !why.is_empty(),
                "{provider}/{key} keeps its rungs for no reason"
            );
        }
    }

    #[test]
    fn every_declined_model_is_an_offered_provider_s_left_unoffered_for_a_reason() {
        for (provider, key, why) in PASSED {
            assert!(
                OFFERED.iter().any(|(offered, ..)| offered == provider),
                "{provider} is no provider crucible offers"
            );
            assert!(
                !OFFERED
                    .iter()
                    .any(|(_, _, offered, _)| *offered == Some(*key)),
                "{provider}/{key} is offered and declined at once"
            );
            assert!(
                !why.is_empty(),
                "{provider}/{key} is declined for no reason"
            );
        }
    }
}
