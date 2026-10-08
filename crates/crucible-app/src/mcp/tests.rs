//! What the MCP lists say about a server, and what they leave out.

use super::{SHOWN, described, listing};
use crate::AppError;
use crate::sample::Sample;

/// A secret written in every place a record can hold one.
const SECRET: &str = "swordfish-sentinel";

/// One server given a secret in its variables, its arguments and its URL, and
/// a name for one more it takes from crucible's own environment.
fn secretive() -> String {
    format!(
        r#"{{"mcp": {{"servers": {{
            "docs": {{
                "command": "docs-mcp",
                "args": ["--token", "{SECRET}", "--url=https://someone:{SECRET}@mcp.example.test/sse?key={SECRET}", "--catalogue", "public"],
                "env": {{"DOCS_TOKEN": "{SECRET}", "DOCS_LOCALE": "{SECRET}"}},
                "envFrom": {{"DOCS_KEY": "EXAMPLE_DOCS_KEY"}},
                "required": true
            }},
            "notes": {{"command": "notes-mcp"}}
        }}}}}}"#
    )
}

#[test]
fn a_list_names_every_server_and_shows_nothing_it_is_given_to_keep_secret() {
    let sample = Sample::new("mcp-list");
    let settings = sample.user(&secretive());

    let listed = listing(&settings, &sample.user_file());

    assert!(
        listed.starts_with("2 MCP servers written down in "),
        "{listed}"
    );
    assert!(listed.contains("none was started"), "{listed}");
    assert!(
        listed.contains(
            "  docs  docs-mcp, 5 arguments, 2 variables set, 1 variable taken from your \
             environment, required\n"
        ),
        "{listed}"
    );
    assert!(
        listed.contains("  notes  notes-mcp, 0 arguments\n"),
        "{listed}"
    );
    assert!(!listed.contains(SECRET), "{listed}");
}

#[test]
fn one_server_is_described_whole_with_every_secret_left_out() {
    let sample = Sample::new("mcp-get");
    let settings = sample.user(&secretive());

    let said = described(&settings, &sample.user_file(), "docs").expect("a server written down");

    assert!(said.starts_with("docs, written down in "), "{said}");
    for line in [
        "  command    docs-mcp\n",
        "  arguments  <redacted>\n",
        "             <redacted>\n",
        "             <redacted>\n",
        "             <redacted>\n",
        "             public\n",
        "  env        DOCS_LOCALE=<redacted>\n",
        "             DOCS_TOKEN=<redacted>\n",
        "  envFrom    DOCS_KEY from EXAMPLE_DOCS_KEY\n",
        "  required   yes",
    ] {
        assert!(said.contains(line), "no {line:?} in\n{said}");
    }
    assert!(!said.contains(SECRET), "{said}");
}

#[test]
fn a_server_nobody_wrote_down_is_refused_by_name_with_the_names_there_are() {
    let sample = Sample::new("mcp-unknown");
    let settings = sample.user(&secretive());

    let refused =
        described(&settings, &sample.user_file(), "dosc").expect_err("no server is called that");

    assert!(matches!(refused, AppError::NoServer { .. }), "{refused:?}");
    assert_eq!(
        refused.to_string(),
        "no mcp server called dosc; this configuration has docs, notes"
    );

    // Word for word what `--with-mcp docs` says to the same configuration.
    let empty = sample.user("{}");
    let refused = described(&empty, &sample.user_file(), "docs").expect_err("there are none");
    assert_eq!(
        refused.to_string(),
        "no mcp server called docs; this configuration has none under mcp.servers"
    );
}

#[test]
fn what_a_terminal_would_act_on_in_a_record_is_shown_as_its_escape() {
    let sample = Sample::new("mcp-escaped");
    let settings = sample.user(
        r#"{"mcp": {"servers": {"do\u001b]0;owned\u0007cs": {
            "command": "docs-mcp",
            "args": ["--title", "line\nbreak\u001b[2J", "\u202eright"]
        }}}}"#,
    );

    let listed = listing(&settings, &sample.user_file());
    let said = described(&settings, &sample.user_file(), "do\u{1b}]0;owned\u{7}cs")
        .expect("a server written down");

    for text in [&listed, &said] {
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(!text.contains('\u{7}'), "{text:?}");
        assert!(!text.contains('\u{202e}'), "{text:?}");
        assert!(!text.contains("line\nbreak"), "{text:?}");
    }
    assert!(listed.contains(r"do\u{1b}]0;owned\u{7}cs"), "{listed}");
    assert!(said.contains(r"line\nbreak\u{1b}[2J"), "{said}");
    assert!(said.contains(r"\u{202e}right"), "{said}");
}

#[test]
fn an_argument_longer_than_a_list_shows_is_cut_and_says_so() {
    let sample = Sample::new("mcp-cut");
    let long = "a".repeat(SHOWN * 3);
    let settings = sample.user(&format!(
        r#"{{"mcp": {{"servers": {{"docs": {{"command": "docs-mcp", "args": ["{long}"]}}}}}}}}"#
    ));

    let said = described(&settings, &sample.user_file(), "docs").expect("a server written down");

    let shown = format!("{}… (cut)\n", "a".repeat(SHOWN));
    assert!(said.contains(&shown), "{said}");
    assert!(!said.contains(&"a".repeat(SHOWN + 1)), "{said}");
}
