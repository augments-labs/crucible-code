//! What a turn can reach while the application waits for it.
//!
//! A conversation waits for a turn by polling it on the calling thread,
//! entered into the application's runtime. Two things must then never happen
//! inside the turn, and neither is something the runtime refuses by itself:
//!
//! - **A nested `block_on`.** Entering a runtime is not what Tokio's guard
//!   against a nested `block_on` watches, so one reached from inside a waited
//!   turn would block the turn's thread on a second wait rather than panic.
//! - **A waiting crossing.** Every ledger entry that waits refuses on a
//!   thread that has entered a runtime, so one reached from inside a turn
//!   would end the turn on a refusal rather than on its own ending.
//!
//! Both are kept out by what the shipped sources say, and these read them:
//! every Rust file shipped under `crates/` and under the command line's own
//! `src/` — which a terminal turn reaches too, through the permission prompt
//! and the event relay — the way the bridge ledger's own check reads them. A
//! file under a directory named `tests`, or named `tests.rs` or ending in
//! `_tests.rs`, is not shipped, and neither is a file listed in [`TEST_ONLY`].
//! Every line of a shipped file is read, its own `#[cfg(test)]` module
//! included: a `block_on` such a module holds is allowed by its exact line
//! and how many times the file may hold it, in [`BLOCK_ON_ALLOWED`], rather
//! than told apart from shipped code by a reading of the source, which can
//! only err by hiding one. The runner cannot name the application, which the
//! crate graph forbids, so the one waiting crossing the application makes is
//! outside every turn it waits for, and it is the only waiting crossing there
//! is.
//!
//! What is looked for is the name itself, which no import can hide: a
//! `block_on` is a method or a function named that, and a waiting entry is
//! the variant's own name, however `Bridge` was reached. A glob import from
//! Tokio, from `futures` or from the bridges is refused outright as well,
//! since it is the one spelling that brings such a name in without writing
//! it where it is imported.

#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// The files under a shipped `src/` that only a test build compiles: the
/// command line's test doubles and sample data, each declared `#[cfg(test)]`
/// where it is named.
const TEST_ONLY: &[&str] = &["src/cli/fake.rs", "src/cli/sample.rs"];

/// The workspace this crate is a member of.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Every shipped Rust file under `crates/` and under the root `src/`, as a
/// path relative to the workspace with `/` between its parts, beside its
/// text.
fn shipped() -> Vec<(String, String)> {
    let root = workspace();
    let mut found = Vec::new();
    let mut pending = vec![root.join("crates"), root.join("src")];
    while let Some(directory) = pending.pop() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&directory)
            .unwrap_or_else(|problem| panic!("{} unreadable: {problem}", directory.display()))
            .map(|entry| entry.expect("a directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            if path.is_dir() {
                if name != "tests" && name != "target" {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && name != "tests.rs"
                && !name.ends_with("_tests.rs")
                && path.strip_prefix(&root).is_ok_and(|relative| {
                    relative.components().any(|part| part.as_os_str() == "src")
                })
            {
                let relative = path
                    .strip_prefix(&root)
                    .expect("a path under the workspace")
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|problem| panic!("{relative} unreadable: {problem}"));
                if !TEST_ONLY.contains(&relative.as_str()) {
                    found.push((relative, text));
                }
            }
        }
    }
    assert!(
        found.len() > 100,
        "only {} shipped files were found, so the walk missed the tree",
        found.len()
    );
    for reached in ["src/cli/seen.rs", "crates/crucible-runner/src/runner.rs"] {
        assert!(
            found.iter().any(|(path, _)| path == reached),
            "{reached} was not walked, so the walk missed a file a turn reaches"
        );
    }
    for skipped in TEST_ONLY {
        assert!(
            root.join(skipped).is_file(),
            "{skipped} is listed as test-only and is gone, so the list is stale"
        );
        assert!(
            declared_for_tests(&root, skipped),
            "{skipped} is listed as test-only, and the `mod` that brings it in is not under \
             `#[cfg(test)]`, so it ships and has to be walked"
        );
    }
    found
}

/// Whether the file at `path`, `src/…/name.rs`, is brought in by a `mod name;`
/// in its parent module's file whose line above is `#[cfg(test)]`, so only a
/// test build compiles it.
fn declared_for_tests(root: &Path, path: &str) -> bool {
    let file = Path::new(path);
    let (Some(name), Some(directory)) = (
        file.file_stem().and_then(|stem| stem.to_str()),
        file.parent(),
    ) else {
        return false;
    };
    let parents = [
        directory.with_extension("rs"),
        directory.join("mod.rs"),
        directory.join("main.rs"),
        directory.join("lib.rs"),
    ];
    let declaration = ["mod ", name, ";"].concat();
    parents.iter().any(|parent| {
        let Ok(text) = fs::read_to_string(root.join(parent)) else {
            return false;
        };
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        lines.iter().enumerate().any(|(at, line)| {
            line.strip_prefix("pub(crate) ").unwrap_or(line) == declaration
                && at
                    .checked_sub(1)
                    .and_then(|above| lines.get(above))
                    .is_some_and(|above| *above == "#[cfg(test)]")
        })
    })
}

/// The lines of `text` that are code, a line whose first characters are `//`
/// left out, as the bridge ledger's own check leaves them out.
fn code(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
}

/// Whether `line` names `word` as a whole identifier.
fn names(line: &str, word: &str) -> bool {
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    line.match_indices(word).any(|(at, _)| {
        !line[..at].chars().next_back().is_some_and(identifier)
            && !line[at + word.len()..]
                .chars()
                .next()
                .is_some_and(identifier)
    })
}

#[test]
fn nothing_shipped_glob_imports_what_a_turn_could_wait_through() {
    let mut found: Vec<(String, String)> = Vec::new();
    for (path, text) in shipped() {
        // A `use` read whole, however many lines it spans, up to its `;`.
        let mut statement = String::new();
        for line in code(&text) {
            if statement.is_empty() && !line.split_whitespace().take(2).any(|word| word == "use") {
                continue;
            }
            statement.push_str(line);
            if !line.ends_with(';') {
                continue;
            }
            let from = ["tokio", "futures", "crucible_runtime", "Bridge"]
                .iter()
                .any(|root| names(&statement, root));
            if from && statement.contains('*') {
                found.push((path.clone(), std::mem::take(&mut statement)));
            }
            statement.clear();
        }
    }
    assert!(
        found.is_empty(),
        "a shipped file glob-imports from Tokio, `futures` or the bridges, which could bring \
         a `block_on` or a waiting crossing in unnamed: {found:#?}"
    );
}

/// Every line mentioning `block_on` a shipped file may hold, trimmed, with
/// how many times that file may hold it, and none of them is reached by a
/// turn: the runtime owner's documentation of why it is built multi-thread,
/// the runner's test helper that drives a turn to its end on a runtime of
/// the test's own, the two performance probes waiting, on their own main
/// thread, for each call they time on a runtime of the probe's own, and the
/// lines inside the `#[cfg(test)] mod tests` of the
/// bridge ledger, of the sandbox's redaction and of the worker-task check,
/// which only a test build compiles. The count makes the same line written
/// once more in that file, wherever, one too many.
const BLOCK_ON_ALLOWED: &[(&str, &str, usize)] = &[
    (
        "crates/crucible-app/src/runtime.rs",
        "//! the shape of `Handle::block_on`. Tokio's own documentation of that method,",
        1,
    ),
    (
        "crates/crucible-app/src/runtime.rs",
        "//! a `current_thread` runtime only `Runtime::block_on` can drive the IO and",
        1,
    ),
    (
        "crates/crucible-app/src/runtime.rs",
        "//! timer drivers and `Handle::block_on` cannot, so anything relying on IO or",
        1,
    ),
    (
        "crates/crucible-app/src/runtime.rs",
        "//! timers does not work unless another thread is inside `Runtime::block_on` on",
        1,
    ),
    ("crates/crucible-runner/src/fake.rs", ".block_on(self)", 1),
    // The probes' `Driver::answer`, which their `main` reaches and no turn
    // does: a probe is a program of its own, not a part of crucible.
    ("src/bin/bench-grep.rs", "self.runtime.block_on(future)", 1),
    ("src/bin/bench-tools.rs", "self.runtime.block_on(future)", 1),
    // Inside `crates/crucible-runtime/src/bridge.rs`'s `#[cfg(test)] mod tests`:
    // the documentation of the test that makes both crossings on a runtime
    // worker, and the test that drives one to its refusal on a runtime of the
    // test's own.
    (
        "crates/crucible-runtime/src/bridge.rs",
        "/// A worker thread is where a `block_on` would panic or deadlock, so both",
        1,
    ),
    (
        "crates/crucible-runtime/src/bridge.rs",
        ".block_on(async move {",
        1,
    ),
    // Inside `crates/crucible-sandbox-local/src/redaction.rs`'s
    // `#[cfg(test)] mod tests`: the test helper that reads a protected output
    // to its end on a runtime of its own.
    (
        "crates/crucible-sandbox-local/src/redaction.rs",
        "runtime().block_on(async {",
        1,
    ),
    // Inside `crates/crucible-runtime/src/worker.rs`'s `#[cfg(test)] mod
    // tests`: the test that drives the check on the thread that merely
    // entered a runtime without being spawned onto it, and the two tests
    // (current-thread and multi-thread) that drive it on a spawned task,
    // whose bodies are the same line.
    (
        "crates/crucible-runtime/src/worker.rs",
        "let seen = runtime.block_on(async { not_worker() });",
        1,
    ),
    (
        "crates/crucible-runtime/src/worker.rs",
        "let seen = runtime.block_on(async { tokio::spawn(async { not_worker() }).await.unwrap() });",
        2,
    ),
];

#[test]
fn nothing_a_turn_reaches_calls_block_on() {
    let mut found: Vec<(String, String)> = Vec::new();
    for (path, text) in shipped() {
        for line in text.lines() {
            if line.contains("block_on") {
                found.push((path.clone(), line.trim().to_owned()));
            }
        }
    }

    let unexpected: Vec<&(String, String)> = found
        .iter()
        .filter(|(path, line)| {
            !BLOCK_ON_ALLOWED
                .iter()
                .any(|(allowed, said, _)| allowed == path && said == line)
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "a shipped file names `block_on` where nothing may: {unexpected:#?}"
    );
    let counted = |allowed: &str, said: &str| {
        found
            .iter()
            .filter(|(path, line)| path == allowed && line == said)
            .count()
    };
    let stale: Vec<&(&str, &str, usize)> = BLOCK_ON_ALLOWED
        .iter()
        .filter(|(allowed, said, _)| counted(allowed, said) == 0)
        .collect();
    assert!(
        stale.is_empty(),
        "an allowed mention of `block_on` is no longer there, so the list is stale: {stale:#?}"
    );
    let multiplied: Vec<(&str, &str, usize, usize)> = BLOCK_ON_ALLOWED
        .iter()
        .filter_map(|(allowed, said, times)| {
            let now = counted(allowed, said);
            (now != *times).then_some((*allowed, *said, *times, now))
        })
        .collect();
    assert!(
        multiplied.is_empty(),
        "an allowed mention of `block_on` is not there the number of times the list allows, so \
         a shipped line may be hiding behind a test's (path, line, allowed, found): {multiplied:#?}"
    );
}

/// The ledger entries that wait, read off the ledger: each variant of
/// `Bridge` whose documentation says `- Crossing: waits.`.
fn waiting_entries() -> Vec<String> {
    let ledger = fs::read_to_string(workspace().join("crates/crucible-runtime/src/bridge.rs"))
        .expect("the bridge ledger");
    let start = ledger.find("pub enum Bridge {").expect("the ledger's enum");
    let body = &ledger[start..];
    let end = body.find("\n}\n").expect("the end of the ledger's enum");
    let mut waiting = Vec::new();
    let mut waits = false;
    for line in body[..end].lines().skip(1) {
        let line = line.trim();
        if let Some(doc) = line.strip_prefix("///") {
            if doc.trim() == "- Crossing: waits." {
                waits = true;
            }
        } else if let Some(variant) = line.strip_suffix(',') {
            if waits {
                waiting.push(variant.to_owned());
            }
            waits = false;
        }
    }
    waiting
}

#[test]
fn the_one_waiting_crossing_is_the_application_s_around_a_turn() {
    let waiting = waiting_entries();
    assert_eq!(waiting, ["AppTurn"], "the ledger's waiting entries changed");

    let mut crossed: Vec<(String, String)> = Vec::new();
    for (path, text) in shipped() {
        if path == "crates/crucible-runtime/src/bridge.rs" {
            continue;
        }
        for line in code(&text) {
            for entry in waiting.iter().filter(|entry| names(line, entry)) {
                crossed.push((path.clone(), entry.clone()));
            }
        }
    }

    // Each line naming an entry is one pair, so an entry named twice in a
    // file is seen twice. The entry is recorded rather than the line, since
    // a line written here that names a bridge by its path would read, to the
    // bridge ledger's own check, as this crate crossing it.
    crossed.sort();
    assert_eq!(
        crossed,
        [(
            "crates/crucible-app/src/conversation.rs".to_owned(),
            "AppTurn".to_owned()
        ),],
        "a waiting crossing is named somewhere other than the conversation's one wait around a \
         whole turn, where a turn could reach it"
    );
}
