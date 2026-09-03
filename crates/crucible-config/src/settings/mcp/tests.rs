use crate::document::{Document, Origin};
use crate::error::ConfigError;
use crate::shape;

use super::*;

/// An absolute program path this platform reads back.
///
/// What counts as absolute is a drive on Windows and a leading slash everywhere
/// else, and the parser applies the platform's own answer — so a test written
/// in one spelling would be a test of one platform. Forward slashes on both,
/// because Windows accepts them and a backslash inside JSON is an escape.
#[cfg(windows)]
const PROGRAM: &str = "C:/Program Files/docs-mcp/docs-mcp.exe";
#[cfg(not(windows))]
const PROGRAM: &str = "/usr/local/bin/docs-mcp";

/// An absolute directory this platform reads back.
#[cfg(windows)]
const ELSEWHERE: &str = "C:/srv/docs";
#[cfg(not(windows))]
const ELSEWHERE: &str = "/srv/docs";

/// A program and a directory that are absolute and are not there.
#[cfg(windows)]
const NOWHERE: &str = "C:/nowhere";
#[cfg(not(windows))]
const NOWHERE: &str = "/nowhere";

/// The record every test that wants a whole one starts from, in this
/// platform's spelling.
fn whole() -> String {
    WHOLE
        .replace("PROGRAM", PROGRAM)
        .replace("ELSEWHERE", ELSEWHERE)
}

/// The record every test that wants a whole one starts from.
const WHOLE: &str = r#"{
    "mcp": {
        "servers": {
            "docs": {
                "command": "PROGRAM",
                "args": ["--catalogue", "public"],
                "directory": "ELSEWHERE",
                "env": {"DOCS_LOCALE": "en"},
                "envFrom": {"DOCS_TOKEN": "EXAMPLE_DOCS_TOKEN"},
                "handshakeSeconds": 3,
                "requestSeconds": 12,
                "shutdownSeconds": 2,
                "restarts": 4,
                "required": true
            }
        }
    }
}"#;

fn read(text: &str) -> Vec<McpServer> {
    Settings::resolve(vec![Document::sample(text, Origin::User)]).mcp_servers()
}

#[test]
fn every_key_a_record_may_hold_is_read_back_from_it() {
    let found = read(&whole());
    let [server] = found.as_slice() else {
        panic!("one server was written down");
    };

    assert_eq!(server.name(), "docs");
    assert_eq!(server.command(), PROGRAM);
    assert_eq!(server.args().collect::<Vec<_>>(), ["--catalogue", "public"]);
    assert_eq!(server.directory(), Some(ELSEWHERE));
    assert_eq!(server.env().collect::<Vec<_>>(), [("DOCS_LOCALE", "en")]);
    assert_eq!(
        server.env_from().collect::<Vec<_>>(),
        [("DOCS_TOKEN", "EXAMPLE_DOCS_TOKEN")]
    );
    assert_eq!(server.handshake(), Duration::from_secs(3));
    assert_eq!(server.request(), Duration::from_secs(12));
    assert_eq!(server.shutdown(), Duration::from_secs(2));
    assert_eq!(server.restarts(), 4);
    assert!(server.required());
}

#[test]
fn a_machine_that_wrote_no_servers_down_has_none() {
    assert!(Settings::resolve(Vec::new()).mcp_servers().is_empty());
    assert!(read(r#"{"mcp": {"servers": {}}}"#).is_empty());
}

#[test]
fn a_record_that_says_only_what_to_run_takes_the_answers_the_schema_publishes() {
    // The schema states what each key falls back to and this module states what
    // the program uses. Two answers to one question, so they are read against
    // each other rather than both trusted.
    let found = read(r#"{"mcp": {"servers": {"docs": {"command": "docs-mcp"}}}}"#);
    let [server] = found.as_slice() else {
        panic!("one server was written down");
    };

    for (key, held) in [
        ("handshakeSeconds", server.handshake()),
        ("requestSeconds", server.request()),
        ("shutdownSeconds", server.shutdown()),
    ] {
        let published: u64 = shape::usual(&["mcp", "servers", "docs", key])
            .parse()
            .expect("the schema publishes a whole number of seconds");
        assert_eq!(held, Duration::from_secs(published), "{key}");
    }

    let restarts: u32 = shape::usual(&["mcp", "servers", "docs", "restarts"])
        .parse()
        .expect("the schema publishes a whole number of restarts");
    assert_eq!(server.restarts(), restarts);
    assert_eq!(
        server.required().to_string(),
        shape::usual(&["mcp", "servers", "docs", "required"])
    );

    assert_eq!(server.args().count(), 0);
    assert_eq!(server.directory(), None);
    assert_eq!(server.env().count(), 0);
    assert_eq!(server.env_from().count(), 0);
}

#[test]
fn a_file_that_arrived_with_the_checkout_cannot_write_a_server() {
    let refused = Document::parse(&whole(), "config.json", Origin::Project)
        .expect_err("a committed file may not choose whose program runs");

    assert!(
        matches!(&refused, ConfigError::Widening { path, .. } if &**path == "mcp.servers"),
        "refused at the block: {refused}"
    );
}

#[test]
fn a_name_that_would_qualify_a_tool_ambiguously_is_refused() {
    for name in ["docs:public", "docs/public"] {
        let text = format!(r#"{{"mcp": {{"servers": {{"{name}": {{"command": "docs-mcp"}}}}}}}}"#);
        let refused = Document::parse(&text, "config.json", Origin::User)
            .expect_err("a name its own tool names cannot be read back from");

        assert!(
            matches!(&refused, ConfigError::ServerName { name: written, .. } if &**written == name),
            "{name}: {refused}"
        );
    }
}

#[test]
fn a_program_named_relative_to_nowhere_is_refused() {
    let refused = Document::parse(
        r#"{"mcp": {"servers": {"docs": {"command": "./docs-mcp"}}}}"#,
        "config.json",
        Origin::User,
    )
    .expect_err("a path resolved against a directory nothing wrote down");

    assert!(
        matches!(&refused, ConfigError::Unrooted { found, .. } if &**found == "./docs-mcp"),
        "{refused}"
    );
}

#[test]
fn a_bare_program_name_is_left_for_path_to_answer() {
    let found = read(r#"{"mcp": {"servers": {"docs": {"command": "npx"}}}}"#);
    let [server] = found.as_slice() else {
        panic!("one server was written down");
    };

    assert_eq!(server.command(), "npx");
}

#[test]
fn a_directory_that_is_not_absolute_is_refused() {
    let refused = Document::parse(
        r#"{"mcp": {"servers": {"docs": {"command": "npx", "directory": "srv/docs"}}}}"#,
        "config.json",
        Origin::User,
    )
    .expect_err("a directory a configuration file cannot know what it is relative to");

    assert!(
        matches!(&refused, ConfigError::Relative { path, .. } if &**path == "mcp.servers.docs.directory"),
        "{refused}"
    );
}

#[test]
fn a_record_that_does_not_say_what_to_run_is_refused_rather_than_skipped() {
    let refused = Document::parse(
        r#"{"mcp": {"servers": {"docs": {"required": true}}}}"#,
        "config.json",
        Origin::User,
    )
    .expect_err("a server that would quietly not exist");

    assert!(
        matches!(
            &refused,
            ConfigError::Needed { path, name, .. }
                if &**path == "mcp.servers.docs" && &**name == "command"
        ),
        "{refused}"
    );
}

#[test]
fn a_key_the_block_does_not_have_is_refused() {
    // A record is a closed set of keys, so a misspelling is met where it was
    // written rather than by a server started without the setting it names.
    let refused = Document::parse(
        r#"{"mcp": {"servers": {"docs": {"command": "npx", "timeout": 5}}}}"#,
        "config.json",
        Origin::User,
    )
    .expect_err("a key no record has");

    assert!(
        matches!(refused, ConfigError::UnknownKey { .. }),
        "{refused}"
    );
}

#[test]
fn what_is_written_down_starts_nothing_and_resolves_nothing() {
    // The whole guarantee of this module, stated as a test: a record naming a
    // program that does not exist, in a directory that does not exist, taking a
    // variable that is not set, reads back without any of that being touched.
    let missing = format!("{NOWHERE}/docs-mcp");
    let found = read(&format!(
        r#"{{"mcp": {{"servers": {{"docs": {{
            "command": "{missing}",
            "directory": "{NOWHERE}",
            "envFrom": {{"DOCS_TOKEN": "CRUCIBLE_TEST_UNSET_VARIABLE"}}
        }}}}}}}}"#
    ));
    let [server] = found.as_slice() else {
        panic!("one server was written down");
    };

    assert_eq!(server.command(), missing);
    assert_eq!(server.directory(), Some(NOWHERE));
    assert_eq!(
        server.env_from().collect::<Vec<_>>(),
        [("DOCS_TOKEN", "CRUCIBLE_TEST_UNSET_VARIABLE")],
        "the name, and nothing read by it"
    );
    assert!(
        std::env::var_os("CRUCIBLE_TEST_UNSET_VARIABLE").is_none(),
        "the variable this record names is not set, and reading it back did not set it"
    );
}

#[test]
fn a_document_cannot_make_startup_walk_further_than_the_record_bounds() {
    let args = (0..ARGS + 10)
        .map(|held| format!(r#""{held}""#))
        .collect::<Vec<_>>()
        .join(", ");
    let servers = (0..SERVERS + 10)
        .map(|held| format!(r#""server{held}": {{"command": "npx", "args": [{args}]}}"#))
        .collect::<Vec<_>>()
        .join(", ");
    let found = read(&format!(r#"{{"mcp": {{"servers": {{{servers}}}}}}}"#));

    assert_eq!(found.len(), SERVERS);
    for server in &found {
        assert_eq!(server.args().count(), ARGS, "{}", server.name());
    }
}
