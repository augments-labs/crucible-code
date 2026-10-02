//! Every key the document declares is a row or is left out for a reason.
//!
//! The list below is what is left out, and why, one line each. A key added to
//! the document fails the first test here until it is given a row or a line in
//! this list — which is the point: the menu is whole because nobody has to
//! remember it.

use super::super::{DOCUMENT, Shape};
use super::{ROWS, RowId, Values, rows};

/// What has no row, and why. An entry names a key, or a block whose every key
/// it leaves out.
const EXCLUDED: &[(&str, &str)] = &[
    (
        "provider",
        "chooses who is sent the prompt and the key; /model chooses it",
    ),
    (
        "providers",
        "provider definitions; /model, /effort and /fast change the parts a menu could",
    ),
    ("env.*", "free text handed to the commands crucible runs"),
    (
        "extensions",
        "extensions, each agreed to by name in the user's own file",
    ),
    (
        "mcp",
        "MCP servers: programs to start, with their arguments and environment",
    ),
    (
        "systemPrompt.custom",
        "the system prompt, which is free text",
    ),
    (
        "systemPrompt.append",
        "added to the system prompt, and free text",
    ),
    (
        "permissions.mode",
        "loosens what runs unasked; shown in Status, changed with /mode",
    ),
    ("permissions.allow", "a permission rule, which is free text"),
    ("permissions.ask", "a permission rule, which is free text"),
    ("permissions.deny", "a permission rule, which is free text"),
    (
        "permissions.extraDirectories",
        "widens where tools may reach, as a list of paths",
    ),
    (
        "sandbox.enabled",
        "turning it off weakens the sandbox; shown in Status, changed with /sandbox",
    ),
    (
        "sandbox.filesystem",
        "paths the sandbox opens or closes: free text that decides confinement",
    ),
    (
        "sandbox.network.allowedDomains",
        "domains the sandbox opens: free text that decides confinement",
    ),
    (
        "sandbox.network.deniedDomains",
        "domains the sandbox closes: free text that decides confinement",
    ),
    ("sandbox.network.allowLocalBinding", "loosens the sandbox"),
    (
        "sandbox.network.allowUnixSockets",
        "socket paths that loosen the sandbox",
    ),
    (
        "sandbox.limits",
        "a command's resource ceilings, which are sandbox policy",
    ),
    (
        "compaction.reserve",
        "a count of tokens; the one number a menu steps is the scroll speed",
    ),
    (
        "compaction.keep",
        "a count of tokens; the one number a menu steps is the scroll speed",
    ),
    (
        "compaction.recap",
        "a count of tokens; the one number a menu steps is the scroll speed",
    ),
    (
        "compaction.askOnResume",
        "a count of tokens; the one number a menu steps is the scroll speed",
    ),
    (
        "compaction.spendCeiling",
        "a count of tokens; the one number a menu steps is the scroll speed",
    ),
    (
        "promptCaching.allowedMechanisms",
        "a list, where a row holds one choice",
    ),
    (
        "promptCaching.requestedRetention.maxSeconds",
        "seconds; the one number a menu steps is the scroll speed",
    ),
    ("promptCaching.namespace", "free text"),
    (
        "contentUse.accepted",
        "a yes to a vendor, given where that vendor is first asked",
    ),
];

/// Every key a document can hold a value at, dotted, with `*` for a name the
/// user chooses.
fn leaves(shape: &'static Shape, path: &mut Vec<&'static str>, found: &mut Vec<String>) {
    match shape {
        Shape::Fields(fields) => {
            for field in *fields {
                path.push(field.name);
                leaves(&field.shape, path, found);
                path.pop();
            }
        }
        Shape::Named {
            declared, others, ..
        } => {
            for field in *declared {
                path.push(field.name);
                leaves(&field.shape, path, found);
                path.pop();
            }
            path.push("*");
            leaves(others, path, found);
            path.pop();
        }
        // A value. Spelled out rather than closed with a wildcard, so a shape
        // that later holds keys of its own is decided about here.
        Shape::Text
        | Shape::Choice(_)
        | Shape::Count
        | Shape::Limit(_)
        | Shape::TextSet { .. }
        | Shape::Flag
        | Shape::Whole(_)
        | Shape::Pattern(_)
        | Shape::List { .. }
        | Shape::Opaque => found.push(path.join(".")),
    }
}

fn every_key() -> Vec<String> {
    let mut found = Vec::new();
    leaves(&DOCUMENT, &mut Vec::new(), &mut found);
    found
}

fn excluded(key: &str) -> bool {
    EXCLUDED.iter().any(|(left, _)| {
        key == *left
            || key
                .strip_prefix(left)
                .is_some_and(|below| below.starts_with('.'))
    })
}

#[test]
fn every_declared_key_has_a_settings_row_or_a_reason_it_has_none() {
    let missing: Vec<String> = every_key()
        .into_iter()
        .filter(|key| !ROWS.iter().any(|row| row.key == key) && !excluded(key))
        .collect();
    assert!(
        missing.is_empty(),
        "these keys have no /settings row and no entry in EXCLUDED: {missing:#?}"
    );
}

#[test]
fn every_settings_exclusion_names_a_key_and_says_why() {
    let keys = every_key();
    for (left, why) in EXCLUDED {
        assert!(
            keys.iter().any(|key| {
                key == left
                    || key
                        .strip_prefix(left)
                        .is_some_and(|below| below.starts_with('.'))
            }),
            "{left} is excluded but names no key"
        );
        assert!(
            !why.trim().is_empty(),
            "{left} is excluded without a reason"
        );
        assert!(
            !ROWS.iter().any(|row| excluded(row.key)),
            "a row is excluded as well"
        );
    }
}

#[test]
fn every_settings_row_offers_what_its_declaration_accepts() {
    for row in ROWS {
        let field = row
            .field()
            .unwrap_or_else(|| panic!("{} is not a declared key", row.key));
        assert!(
            !field.widens,
            "{} loosens what crucible does unasked",
            row.key
        );
        let agrees = match (row.values, &field.shape) {
            (Values::Flag, Shape::Flag) => true,
            (Values::Choice(offered), Shape::Choice(accepted)) => offered == *accepted,
            (Values::Whole { least, most }, Shape::Whole(whole)) => {
                least == whole.least && most == whole.most
            }
            (Values::Named, Shape::Text) => row.key == "output.syntaxTheme",
            _ => false,
        };
        assert!(
            agrees,
            "{} offers what its declaration does not accept",
            row.key
        );
        assert!(row.usual().is_some(), "{} states no default", row.key);
    }
}

#[test]
fn settings_rows_are_named_once() {
    for (at, row) in ROWS.iter().enumerate() {
        let later = ROWS.iter().skip(at + 1);
        for other in later {
            assert_ne!(row.key, other.key);
            assert_ne!(row.label, other.label);
        }
    }
}

#[test]
fn a_settings_row_takes_only_the_words_its_declaration_reads() {
    let take = |key: &str, word: &str| super::row(key).is_some_and(|row| row.takes(word));
    assert!(take("output.scrollRail", "false"));
    assert!(!take("output.scrollRail", "0"));
    assert!(take("output.theme", "light"));
    assert!(!take("output.theme", "plaid"));
    let speed = "env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED";
    for word in ["3", "12", "30"] {
        assert!(take(speed, word), "{word}");
    }
    for word in ["2", "31", "06", "+6", "six", ""] {
        assert!(!take(speed, word), "{word}");
    }
    // Which names there are is the host's to say; any name is one to ask it.
    assert!(take("output.syntaxTheme", "base16"));
    assert!(!take("output.syntaxTheme", ""));
}

#[test]
fn every_row_has_its_own_identity() {
    // Written out so a new identity has to be listed here, and so fails
    // until a row carries it.
    let every = [
        RowId::Theme,
        RowId::SyntaxTheme,
        RowId::Glyphs,
        RowId::Colour,
        RowId::ToolDetail,
        RowId::ScrollRail,
        RowId::ScrollSpeed,
        RowId::Send,
        RowId::Tone,
        RowId::Compaction,
        RowId::UpdateCheck,
        RowId::CacheMode,
        RowId::CacheIsolation,
        RowId::CacheRetention,
        RowId::CachePersistent,
    ];
    for id in every {
        let carrying = rows().iter().filter(|row| row.id() == id).count();
        assert_eq!(carrying, 1, "{id:?} is carried by {carrying} rows");
    }
    assert_eq!(
        rows().len(),
        every.len(),
        "a row carries no listed identity"
    );
}
