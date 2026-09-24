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
//! Inside a shipped file, an item marked `#[cfg(test)]` — a file's own test
//! module above all — is compiled for tests alone and is left out, from its
//! attribute to the `;` or the closing brace that ends it, braces counted
//! outside strings, characters and comments; everything after it is read. The runner cannot name the
//! application, which the crate graph forbids, so the one waiting crossing
//! the application makes is outside every turn it waits for.
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
                    found.push((relative, without_tests(&text)));
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

/// `text` with every item marked `#[cfg(test)]` blanked out, lines kept.
///
/// An item starts at a line reading exactly `#[cfg(test)]` and runs through
/// any further attributes to the first `;` before any brace, or to the brace
/// that closes the first one it opens. Braces and semicolons inside string,
/// raw-string and character literals and inside comments are not counted; a
/// lifetime is not a character literal. What the item held is replaced by
/// its newlines, so the rest of the file keeps its line numbers.
fn without_tests(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let at = |index: usize| chars.get(index).copied();
    let mut kept = String::with_capacity(text.len());
    let mut index = 0;
    let mut line_start = true;
    while index < chars.len() {
        if line_start {
            let mut first = index;
            while at(first).is_some_and(|c| c == ' ' || c == '\t') {
                first += 1;
            }
            let attribute: String = chars
                .get(first..)
                .unwrap_or_default()
                .iter()
                .take_while(|&&c| c != '\n')
                .collect();
            if attribute.trim_end() == "#[cfg(test)]" {
                let end = item_end(&chars, first + attribute.len());
                for skipped in chars.get(index..end).unwrap_or_default() {
                    if *skipped == '\n' {
                        kept.push('\n');
                    }
                }
                index = end;
                line_start = at(index.wrapping_sub(1)) == Some('\n') || index == 0;
                continue;
            }
        }
        let Some(c) = at(index) else { break };
        kept.push(c);
        line_start = c == '\n';
        index += 1;
    }
    kept
}

/// Where the item that begins at `from` ends: just past the first `;` met
/// before any brace, or just past the brace that closes the first one.
fn item_end(chars: &[char], from: usize) -> usize {
    let at = |index: usize| chars.get(index).copied();
    let mut depth = 0_usize;
    let mut opened = false;
    let mut index = from;
    while let Some(c) = at(index) {
        match c {
            '/' if at(index + 1) == Some('/') => {
                while at(index).is_some_and(|c| c != '\n') {
                    index += 1;
                }
                continue;
            }
            '/' if at(index + 1) == Some('*') => {
                index += 2;
                while at(index).is_some() && !(at(index) == Some('*') && at(index + 1) == Some('/'))
                {
                    index += 1;
                }
                index += 2;
                continue;
            }
            'r' if matches!(at(index + 1), Some('"' | '#'))
                && !at(index.wrapping_sub(1)).is_some_and(|c| c.is_alphanumeric() || c == '_') =>
            {
                let mut hashes = 0;
                let mut open = index + 1;
                while at(open) == Some('#') {
                    hashes += 1;
                    open += 1;
                }
                if at(open) == Some('"') {
                    index = open + 1;
                    loop {
                        match at(index) {
                            None => return chars.len(),
                            Some('"') if (1..=hashes).all(|n| at(index + n) == Some('#')) => {
                                index += 1 + hashes;
                                break;
                            }
                            Some(_) => index += 1,
                        }
                    }
                    continue;
                }
            }
            '"' => {
                index += 1;
                loop {
                    match at(index) {
                        None => return chars.len(),
                        Some('\\') => index += 2,
                        Some('"') => {
                            index += 1;
                            break;
                        }
                        Some(_) => index += 1,
                    }
                }
                continue;
            }
            '\'' => {
                if at(index + 1) == Some('\\') {
                    index += 3;
                    while at(index).is_some_and(|c| c != '\'') {
                        index += 1;
                    }
                    index += 1;
                    continue;
                }
                if at(index + 2) == Some('\'') {
                    index += 3;
                    continue;
                }
            }
            '{' => {
                depth += 1;
                opened = true;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if opened && depth == 0 {
                    return index + 1;
                }
            }
            ';' if !opened => return index + 1,
            _ => {}
        }
        index += 1;
    }
    chars.len()
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

/// Every line mentioning `block_on` a shipped file may hold outside its test
/// items, and none of them is reached by a turn: the runtime owner's
/// documentation of why it is built multi-thread, and the runner's test
/// helper that drives a turn to its end on a runtime of the test's own.
const BLOCK_ON_ALLOWED: &[(&str, &str)] = &[
    (
        "crates/crucible-app/src/runtime.rs",
        "//! the shape of `Handle::block_on`. Tokio's own documentation of that method,",
    ),
    (
        "crates/crucible-app/src/runtime.rs",
        "//! a `current_thread` runtime only `Runtime::block_on` can drive the IO and",
    ),
    (
        "crates/crucible-app/src/runtime.rs",
        "//! timer drivers and `Handle::block_on` cannot, so anything relying on IO or",
    ),
    (
        "crates/crucible-app/src/runtime.rs",
        "//! timers does not work unless another thread is inside `Runtime::block_on` on",
    ),
    ("crates/crucible-runner/src/fake.rs", ".block_on(self)"),
];

#[test]
fn a_test_item_is_left_out_up_to_the_brace_that_closes_it_and_no_further() {
    let text = [
        "fn shipped() { first(); }",
        "#[cfg(test)]",
        "#[allow(dead_code)]",
        "mod tests {",
        "    const OPEN: char = '{';",
        "    const LIFETIME: &'static str = \"}}} not a brace\";",
        "    const RAW: &str = r#\"{ \"quoted\" }\"#;",
        "    // a comment with a } in it",
        "    /* and a { in this one */",
        "    fn inner() { if true { hidden_block_on(); } }",
        "}",
        "fn after() { still_read(); }",
        "#[cfg(test)]",
        "const FIXTURE: &str = \"gone\";",
        "fn last() {}",
    ]
    .join("\n");

    let kept = without_tests(&text);

    assert_eq!(kept.lines().count(), text.lines().count(), "{kept}");
    for read in ["first()", "still_read()", "fn last()"] {
        assert!(kept.contains(read), "{read} was left out:\n{kept}");
    }
    for gone in ["hidden_block_on", "OPEN", "LIFETIME", "RAW", "FIXTURE"] {
        assert!(!kept.contains(gone), "{gone} was read:\n{kept}");
    }
}

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
                .any(|(allowed, said)| allowed == path && said == line)
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "a shipped file names `block_on` where nothing may: {unexpected:#?}"
    );
    let stale: Vec<&(&str, &str)> = BLOCK_ON_ALLOWED
        .iter()
        .filter(|(allowed, said)| {
            !found
                .iter()
                .any(|(path, line)| path == allowed && line == said)
        })
        .collect();
    assert!(
        stale.is_empty(),
        "an allowed mention of `block_on` is no longer there, so the list is stale: {stale:#?}"
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
fn the_only_waiting_crossing_is_the_application_s_around_a_turn() {
    let waiting = waiting_entries();
    assert_eq!(waiting, ["AppTurn"], "the ledger's waiting entries changed");

    let mut crossed: Vec<(String, String)> = Vec::new();
    for (path, text) in shipped() {
        if path == "crates/crucible-runtime/src/bridge.rs" {
            continue;
        }
        for line in code(&text) {
            if waiting.iter().any(|entry| names(line, entry)) {
                crossed.push((path.clone(), line.to_owned()));
            }
        }
    }

    assert_eq!(
        crossed,
        [(
            "crates/crucible-app/src/conversation.rs".to_owned(),
            "Bridge::AppTurn".to_owned()
        )],
        "a waiting crossing is named somewhere other than the conversation's one wait around a \
         whole turn, where a turn could reach it"
    );
}
