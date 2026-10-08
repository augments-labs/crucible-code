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

/// A document holding one server with `count` arguments.
fn arguments(count: usize) -> String {
    let args = (0..count)
        .map(|held| format!(r#""{held}""#))
        .collect::<Vec<_>>()
        .join(", ");
    format!(r#"{{"mcp": {{"servers": {{"docs": {{"command": "npx", "args": [{args}]}}}}}}}}"#)
}

/// A document holding one server with `count` entries under `env`.
fn variables(count: usize) -> String {
    let env = (0..count)
        .map(|held| format!(r#""VAR{held}": "held""#))
        .collect::<Vec<_>>()
        .join(", ");
    format!(r#"{{"mcp": {{"servers": {{"docs": {{"command": "npx", "env": {{{env}}}}}}}}}}}"#)
}

/// A document holding `count` servers.
fn servers(count: usize) -> String {
    let servers = (0..count)
        .map(|held| format!(r#""server{held}": {{"command": "npx"}}"#))
        .collect::<Vec<_>>()
        .join(", ");
    format!(r#"{{"mcp": {{"servers": {{{servers}}}}}}}"#)
}

/// What a document was refused for, or the panic that it was not.
fn refusal(text: &str) -> ConfigError {
    Document::parse(text, "config.json", Origin::User)
        .expect_err("a document over one of the record bounds")
}

#[test]
fn a_record_written_up_to_each_bound_is_read_back_whole() {
    // The other half of every refusal below: the boundary is the last accepted
    // document rather than the first refused one, so a machine configured right
    // up to it keeps working.
    let full = read(&arguments(ARGS));
    let [server] = full.as_slice() else {
        panic!("one server was written down");
    };
    assert_eq!(server.args().count(), ARGS);

    let full = read(&variables(VARIABLES));
    let [server] = full.as_slice() else {
        panic!("one server was written down");
    };
    assert_eq!(server.env().count(), VARIABLES);

    assert_eq!(read(&servers(SERVERS)).len(), SERVERS);
}

#[test]
fn more_arguments_than_the_bound_are_refused_rather_than_dropped_from_the_launch() {
    // The failure this refuses: a record with three hundred arguments used to
    // parse, and the server used to start with the first two hundred and
    // fifty-six of them. An argv that is not the one written down, and nothing
    // said about which arguments went missing.
    let refused = refusal(&arguments(ARGS + 1));

    assert!(
        matches!(
            &refused,
            ConfigError::TooMany { path, most, found, .. }
                if &**path == "mcp.servers.docs.args" && *most == ARGS && *found == ARGS + 1
        ),
        "{refused}"
    );
}

#[test]
fn more_variables_than_the_bound_are_refused_rather_than_dropped_from_the_launch() {
    let refused = refusal(&variables(VARIABLES + 1));

    assert!(
        matches!(
            &refused,
            ConfigError::TooMany { path, most, found, .. }
                if &**path == "mcp.servers.docs.env"
                    && *most == VARIABLES
                    && *found == VARIABLES + 1
        ),
        "{refused}"
    );
}

#[test]
fn more_servers_than_the_bound_are_refused_rather_than_hidden_from_the_reader() {
    let refused = refusal(&servers(SERVERS + 1));

    assert!(
        matches!(
            &refused,
            ConfigError::TooMany { path, most, found, .. }
                if &**path == "mcp.servers" && *most == SERVERS && *found == SERVERS + 1
        ),
        "{refused}"
    );
}

#[test]
fn a_refusal_over_a_bound_says_where_the_block_is_without_showing_what_it_held() {
    // The diagnostic a person acts on: the file, the setting and the line. What
    // it must not carry is a value out of the block, which under `env` is a
    // credential.
    let said = refusal(&variables(VARIABLES + 1)).to_string();

    assert!(said.contains("config.json"), "{said}");
    assert!(said.contains("mcp.servers.docs.env"), "{said}");
    assert!(said.contains(&VARIABLES.to_string()), "{said}");
    assert!(!said.contains("held"), "{said}");
}

#[test]
fn printing_a_server_record_names_a_variable_and_shows_nothing_of_its_value() {
    // A record carries an environment of its own, and that block is where a
    // key for the server crucible is about to start is written. The derive
    // that used to print this type printed those values, so a `{record:?}` in
    // a diagnostic somebody adds later was a leak nobody reviewed.
    let found = read(
        r#"{"mcp": {"servers": {"docs": {"command": "docs-mcp",
           "env": {"DOCS_TOKEN": "hunter2"}}}}}"#,
    );
    let [server] = found.as_slice() else {
        panic!("one server was written down");
    };

    let printed = format!("{server:?}");
    assert!(printed.contains("DOCS_TOKEN"), "got {printed}");
    assert!(!printed.contains("hunter2"), "got {printed}");

    // And the value is still there for the process that needs it.
    assert_eq!(
        server.env().collect::<Vec<_>>(),
        [("DOCS_TOKEN", "hunter2")]
    );
}

#[test]
fn printing_the_settings_shows_nothing_of_a_variable_written_under_a_server() {
    // The document-level redaction reaches the block a user writes at the top
    // of a file. A server's own block is nested inside `mcp.servers`, and a
    // secret written there is the same secret.
    let settings = Settings::resolve(vec![Document::sample(
        r#"{"env": {"TOKEN": "hunter2"},
            "mcp": {"servers": {"docs": {"command": "docs-mcp",
              "env": {"DOCS_TOKEN": "hunter3"}}}}}"#,
        Origin::User,
    )]);

    let printed = format!("{settings:?}");
    assert!(printed.contains("TOKEN"), "got {printed}");
    assert!(printed.contains("DOCS_TOKEN"), "got {printed}");
    assert!(!printed.contains("hunter2"), "got {printed}");
    assert!(!printed.contains("hunter3"), "got {printed}");
}

/// A secret no rule could take for anything else: no digits, so only the
/// place it is written in can say it is one.
const WORD: &str = "swordfish-sentinel";

/// A secret with nothing around it to say so, which only its own shape gives
/// away.
const TOKEN: &str = "Zq7Sentinel0451Secret9Kx";

/// A secret shaped like a token with no name a secret is given under inside
/// it, as `TOKEN` has one, so its shape is all that can hide it.
const SHAPED: &str = "Zq7Sentinel0451Hidden9Kx";

/// The arguments one server is given, shown, from a record written as JSON.
fn shown(args: &[&str]) -> Vec<String> {
    let args = serde_json::to_string(args).expect("a list of strings writes");
    let found = read(&format!(
        r#"{{"mcp": {{"servers": {{"docs": {{"command": "docs-mcp", "args": {args}}}}}}}}}"#
    ));
    let [server] = found.as_slice() else {
        panic!("one server, got {found:?}");
    };
    server.shown_args()
}

/// One server's arguments, and how a reader is shown them.
type Case = (Vec<String>, Vec<String>);

fn case(args: &[&str], expected: &[&str]) -> Case {
    let owned = |held: &[&str]| held.iter().map(|one| (*one).to_owned()).collect();
    (owned(args), owned(expected))
}

/// Every argument a secret is written into, in each place one can go.
fn secrets() -> Vec<Case> {
    [
        written_where_a_key_goes(),
        in_a_password_holding_what_ends_a_url(),
        before_an_at_sign_that_is_not_where_a_user_ends(),
        where_a_cut_could_show_part_of_one(),
        after_a_scheme_word_in_the_same_argument(),
        after_a_name_joined_to_what_comes_before_it(),
        after_an_argument_that_names_one_however_it_ends(),
    ]
    .concat()
}

#[test]
fn an_argument_is_shown_as_written_or_hidden_whole_and_never_in_part() {
    // Every argument that is shown wrong, not only the first, so one run says
    // which of them a rule misses.
    let wrong: Vec<String> = secrets()
        .iter()
        .filter_map(|(args, _)| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let got = shown(&args);
            let in_part = got.len() != args.len()
                || got
                    .iter()
                    .zip(&args)
                    .any(|(one, written)| one != written && one != HIDDEN);
            let leaked = got.iter().any(|one| {
                [WORD, TOKEN, SHAPED]
                    .iter()
                    .any(|secret| one.contains(secret))
            });
            (in_part || leaked).then(|| format!("{args:?} showed {got:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_place_an_argument_can_carry_a_secret_is_shown_without_it() {
    let wrong: Vec<String> = secrets()
        .iter()
        .filter_map(|(args, expected)| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let got = shown(&args);
            (&got != expected).then(|| format!("{args:?} showed {got:?}, not {expected:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A secret after a flag, a header or a name it is given under, in each way
/// a command line, a header, JSON, a URL or a connection string writes one.
fn written_where_a_key_goes() -> Vec<Case> {
    let word = WORD;
    let token = TOKEN;
    vec![
        // A flag that names a secret is hidden, and so is the argument after it.
        case(&["--token", word], &[HIDDEN, HIDDEN]),
        case(
            &["--api-key", word, "--port", "8080"],
            &[HIDDEN, HIDDEN, "--port", "8080"],
        ),
        case(&[&format!("--api-key={word}")], &[HIDDEN]),
        case(&[&format!("API_KEY={word}")], &[HIDDEN]),
        case(&[&format!("Authorization: Bearer {word}")], &[HIDDEN]),
        case(&["-H", &format!("X-Api-Key: {word}")], &["-H", HIDDEN]),
        // A scheme word names the argument after it as well.
        case(
            &["--header", "Authorization:", "Bearer", word],
            &["--header", HIDDEN, HIDDEN, HIDDEN],
        ),
        case(
            &[&format!("https://someone:{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://mcp.example.test/sse?key={word}&page=1")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://mcp.example.test/hook#{word}")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "--url=https://mcp.example.test/sse?access_token={word}"
            )],
            &[HIDDEN],
        ),
        case(&[token], &[HIDDEN]),
        case(
            &[&format!("https://hooks.example.test/services/{token}")],
            &[HIDDEN],
        ),
        case(
            &[
                "-c",
                &format!("curl -H 'Authorization: Bearer {word}' https://mcp.example.test"),
            ],
            &["-c", HIDDEN],
        ),
        // A value written as JSON holds its keys as names of its own.
        case(
            &[&format!(r#"--config={{"region":"us","apiKey":"{word}"}}"#)],
            &[HIDDEN],
        ),
        case(
            &[&format!(r#"--headers={{"Authorization":"Bearer {word}"}}"#)],
            &[HIDDEN],
        ),
        case(
            &[&format!(r#"--config={{"region":"us","id":"{token}"}}"#)],
            &[HIDDEN],
        ),
        // A URL is one wherever in an argument it starts.
        case(
            &[&format!(
                r#"{{"url":"https://someone:{word}@mcp.example.test/sse"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("url:https://someone:{word}@mcp.example.test")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"url":"https://mcp.example.test/sse","apiKey":"{word}"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:{word}/more@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:{word}@db.example.test:5432")],
            &[HIDDEN],
        ),
        // A name inside a name's value, as docker's `--env=NAME=VALUE`.
        case(&[&format!("--env=DB_PASSWORD={word}")], &[HIDDEN]),
        // Pairs run together, as a connection string or a query writes them.
        case(
            &[&format!("--connection-string=Server=h;Password={word}")],
            &[HIDDEN],
        ),
        case(
            &[
                "--connection-string",
                &format!("Host=h;Username=u;Password={word}"),
            ],
            &["--connection-string", HIDDEN],
        ),
        case(
            &[&format!("Server=db;User Id=sa;Password={word}")],
            &[HIDDEN],
        ),
        case(&[&format!("--query=page=1&token={word}")], &[HIDDEN]),
        case(&[&format!("--pairs=region=us,secret={word}")], &[HIDDEN]),
        case(&[&format!("Authorization=Bearer {word}")], &[HIDDEN]),
        case(&["Authorization=Bearer", word], &[HIDDEN, HIDDEN]),
        case(&["--pat", word], &[HIDDEN, HIDDEN]),
        case(&[&format!("--jwt={word}")], &[HIDDEN]),
    ]
}

/// A URL whose password holds a character that ends a URL or starts the next
/// pair.
fn in_a_password_holding_what_ends_a_url() -> Vec<Case> {
    let word = WORD;
    vec![
        case(
            &[&format!("https://someone:pa;{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa,{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa'{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:12,{word}@mcp.example.test")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "--url=https://someone:pa;{word}@mcp.example.test/sse"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "--url=https://someone:pa,{word}@mcp.example.test/sse"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "--url=https://someone:pa'{word}@mcp.example.test/sse"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("--url=https://someone:12,{word}@mcp.example.test")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"url":"https://someone:pa;{word}@mcp.example.test/sse"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"url":"https://someone:pa,{word}@mcp.example.test/sse"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"url":"https://someone:pa'{word}@mcp.example.test/sse"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"url":"https://someone:12,{word}@mcp.example.test"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"url":"https://someone:pa\"{word}@mcp.example.test/sse"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa\"{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa{{{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa|{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa<{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa\\{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:pa^{word}@mcp.example.test/sse")],
            &[HIDDEN],
        ),
        // A quote a shell closes only to write a quote.
        case(
            &[
                "-c",
                &format!(r#"curl 'https://someone:pa'"'"'{word}@mcp.example.test/sse'"#),
            ],
            &["-c", HIDDEN],
        ),
        case(
            &[
                "-c",
                &format!(r"curl 'https://someone:pa'\''{word}@mcp.example.test/sse'"),
            ],
            &["-c", HIDDEN],
        ),
        case(
            &[&format!(
                "postgres://someone:{word}@db1.example.test,db2.example.test/app"
            )],
            &[HIDDEN],
        ),
    ]
}

/// A pair named for a secret after a URL, its value holding an `@`, and a
/// `user:password@host` written with no scheme.
fn before_an_at_sign_that_is_not_where_a_user_ends() -> Vec<Case> {
    let word = WORD;
    vec![
        case(
            &[&format!(
                "jdbc:sqlserver://db.example.test:1433;databaseName=app;user=sa;password=Pa@{word}"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "Server=https://h.example.test;Password=abc@{word}"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("x=https://h.example.test,token=a@{word}")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"dsn":"Server=https://h.example.test;Password=abc@{word}"}}"#
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone@h.example.test;password=a@{word}")],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:pa;{word}@db.example.test:5432")],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:pa,{word}@db.example.test:5432")],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:pa&{word}@db.example.test:5432")],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:pa={word}@db.example.test:5432")],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:{word}:pa@db.example.test:5432")],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:pa:{word}@db.example.test:5432")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "--dsn=someone:pa;{word}@tcp(db.example.test:3306)/app"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                r#"{{"dsn":"someone:pa;{word}@db.example.test:5432"}}"#
            )],
            &[HIDDEN],
        ),
        case(&[&format!("someone:sa;password=a@{word}")], &[HIDDEN]),
        case(&[&format!("someone:pa@h;token=a@{word}")], &[HIDDEN]),
        case(
            &[&format!(
                "https://someone:pa,{word}@h.example.test;token=a@b"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("someone:pa,{word}@h.example.test;token=a@b")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "https://someone:pa,b@h.example.test&token={word},c@d"
            )],
            &[HIDDEN],
        ),
    ]
}

/// The shapes where hiding only the part of an argument that is a password
/// showed some of it: each is hidden whole.
fn where_a_cut_could_show_part_of_one() -> Vec<Case> {
    let word = WORD;
    vec![
        // A password holding a `:` and then an `@` before a `/`.
        case(
            &[&format!("someone:p:a@b/{word}@h.example.test")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "--dsn=root:p:a@b/{word}@tcp(h.example.test:3306)/app"
            )],
            &[HIDDEN],
        ),
        case(&[&format!("'someone::.@'{word}@[::1]:5432;x'")], &[HIDDEN]),
        // A URL password holding a pair named for a secret.
        case(
            &[&format!(
                "https://someone:pa;{word};password=x@h.example.test"
            )],
            &[HIDDEN],
        ),
        case(
            &[&format!("https://someone:12;{word};token=x@h.example.test")],
            &[HIDDEN],
        ),
        // A key written with room around it, inside a list, or escaped.
        case(&[&format!(r#"{{"apiKey" : "{word}"}}"#)], &[HIDDEN]),
        case(&[&format!("password = {word}")], &[HIDDEN]),
        case(&[&format!(r#"["--api-key","{word}"]"#)], &[HIDDEN]),
        case(&[&format!("token%3D{word}")], &[HIDDEN]),
        // A URL password holding a `/` and then a `;`.
        case(
            &[&format!("https://someone:pa/b;{word}@h.example.test/p")],
            &[HIDDEN],
        ),
        // A pair joined to a host by `&`, a quote closed inside a password,
        // and a query after several hosts.
        case(
            &[&format!("https://h.example.test@x&token={word}")],
            &[HIDDEN],
        ),
        case(
            &[&format!("sqlserver://h.example.test&token={word}@T")],
            &[HIDDEN],
        ),
        case(
            &[&format!("'https://someone:pa'{word}@h.example.test'")],
            &[HIDDEN],
        ),
        case(
            &[&format!(
                "mongodb://someone:pa@h1.example.test,h2.example.test/?replicaSet={word}"
            )],
            &[HIDDEN],
        ),
        // With no `@`, what follows a URL host's `:` and is not a port.
        case(&[&format!("https://someone:{word}/more")], &[HIDDEN]),
        // A secret that only its shape gives away.
        case(&[SHAPED], &[HIDDEN]),
        case(
            &[&format!("https://hooks.example.test/services/{SHAPED}")],
            &[HIDDEN],
        ),
    ]
}

/// A credential after an authorization scheme written in the same argument,
/// in any case and with any room between them.
fn after_a_scheme_word_in_the_same_argument() -> Vec<Case> {
    let word = WORD;
    let mut cases: Vec<Case> = ["Basic", "basic", "BASIC", "Bearer", "Token"]
        .iter()
        .flat_map(|scheme| {
            [" ", "\t", "  "].map(|room| case(&[&format!("{scheme}{room}{word}")], &[HIDDEN]))
        })
        .collect();
    cases.extend([
        case(&["Basic dXNlcjpwYXNz"], &[HIDDEN]),
        case(&[&format!("--x=Basic {word}")], &[HIDDEN]),
        case(&["-H", &format!("Basic {word}")], &["-H", HIDDEN]),
        case(
            &["--header-value", &format!("Basic {word}")],
            &["--header-value", HIDDEN],
        ),
        // Written as two arguments, the scheme names the one after it.
        case(&["Basic", word], &[HIDDEN, HIDDEN]),
    ]);
    cases
}

/// A flag or a scheme word, alone or joined to what comes before it by a
/// quote, with or without a closing mark or other punctuation after it, which
/// names the argument after it.
fn after_a_name_joined_to_what_comes_before_it() -> Vec<Case> {
    let word = WORD;
    let names = [
        "https://h.example.test'--api-key",
        "https://h.example.test\"--password",
        "https://h.example.test'-token",
        "https://h.example.test'Bearer",
        "https://h.example.test\"Basic",
        "https://h.example.test'Token",
        "--api-key",
    ];
    let after = [
        "", "'", "\"", ")", "]", "}", ".", ">", "|", "!", "?", "*", "#", "\\", "`", "\u{200b}",
        ".'", "'.", "..",
    ];
    names
        .iter()
        .flat_map(|name| {
            after.map(|close| case(&[&format!("{name}{close}"), word], &[HIDDEN, HIDDEN]))
        })
        .collect()
}

/// An argument that names a secret or a scheme anywhere in it, and so is
/// hidden, ending in whatever way: a mark, punctuation, room, a quote no shell
/// takes off, or a name with no `-` and no mark at all. The argument after it
/// is hidden too, however the name before it is read.
fn after_an_argument_that_names_one_however_it_ends() -> Vec<Case> {
    let word = WORD;
    let mut cases: Vec<Case> = [
        "--api.key,",
        "https://h.example.test'--api.key",
        "--client.secret;",
        "password=.",
        "password=(",
        "password==",
        "Authorization:>",
        "Authorization::",
        "Authorization:\u{201d}",
        "{\"password\":\u{201d}",
        "password: '",
        "--api-key .",
        "Authorization: Bearer )",
        "Bearer.",
        "X-Api-Key",
    ]
    .iter()
    .map(|name| case(&[name, word], &[HIDDEN, HIDDEN]))
    .collect();
    cases.extend([
        case(
            &["--header", "Authorization:\u{200b}", word],
            &["--header", HIDDEN, HIDDEN],
        ),
        case(
            &["--header", "X-Api-Key", word],
            &["--header", HIDDEN, HIDDEN],
        ),
        // A hidden name that names the next one hides it in turn.
        case(&["--api-key", "Bearer", word], &[HIDDEN, HIDDEN, HIDDEN]),
    ]);
    cases
}

#[test]
fn an_argument_with_nothing_secret_about_it_is_shown_as_written() {
    let plain = [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "server-filesystem-2024",
        "--catalogue",
        "public",
        "--port=8080",
        "--path=/srv/docs",
        "https://mcp.example.test/sse",
        "https://mcp.example.test:8443/sse",
        "http://[::1]:8080/sse",
        "postgres://db1.example.test,db2.example.test/app",
        "git@github.example.test:org/repo.git",
        "@scope/pkg@1.2.3",
        "/srv/cache/@scope/pkg",
        "--log-level:debug",
        "region:us",
        "--map=a:b;c",
        "/srv/docs",
        // A flag whose name says nothing about what follows it, alone or
        // joined to a URL by a quote.
        "-p",
        "8080",
        "https://h.example.test'-p",
        "8080",
        "--log-level",
        "debug",
    ];

    assert_eq!(shown(&plain), plain);
}

#[test]
fn an_argument_shaped_like_one_that_holds_a_secret_is_hidden_though_it_holds_none() {
    // Telling these from the arguments that do hold one is the cut that
    // showed part of a password, so the shape is enough to hide it whole.
    let shaped = [
        "https://registry.example.test/@scope/pkg",
        "git+ssh://git@github.example.test/org/repo.git",
        r#"["someone:pa","a@b.example.test"]"#,
        "--label=team:core,owner=a@b.example.test",
        "--tokenizer",
        "--mode=basic",
    ];

    assert_eq!(shown(&shaped), [HIDDEN; 6]);
}

#[test]
fn the_argument_after_one_that_names_a_secret_is_hidden_though_it_holds_none() {
    // Telling a name's own value from the flag's is the reading that showed a
    // secret wherever it was wrong, so a path after such an argument goes
    // too, even where the argument already holds its own value.
    assert_eq!(
        shown(&["--tokenizer=fast", "/srv/models", "--port", "8080"]),
        [HIDDEN, HIDDEN, "--port", "8080"]
    );
    assert_eq!(shown(&["keys", "/srv/docs", "-y"]), [HIDDEN, HIDDEN, "-y"]);
}

#[test]
fn an_argument_a_megabyte_long_is_shown_whole_or_hidden_whole() {
    // Each built from one unit over and over, each a shape a rule above reads:
    // marks, schemes, at signs, quotes, hosts and runs a token is made of.
    let units = [
        ":",
        "://",
        "a:@",
        "@:",
        "x",
        "-",
        "Zq7",
        "%3D",
        "a=b;",
        "\"'",
        "[::1]:",
        "h:80/",
        "https://h/",
        "--token ",
        "u:p@h ",
        "Basic ",
        "h'--key)",
    ];
    for unit in units {
        let arg = unit.repeat((1 << 20) / unit.len());
        let server = McpServer::read(
            "docs",
            &serde_json::json!({"command": "docs-mcp", "args": [arg]}),
        )
        .expect("a record with a command");
        let got = server.shown_args();
        assert!(
            got == [arg.as_str()] || got == [HIDDEN],
            "{unit:?} repeated showed {} bytes",
            got.iter().map(String::len).sum::<usize>()
        );
    }
}

#[test]
fn the_arguments_a_server_is_started_with_are_still_whole() {
    let args = ["--token", WORD];
    let found = read(&format!(
        r#"{{"mcp": {{"servers": {{"docs": {{"command": "docs-mcp", "args": ["--token", "{WORD}"]}}}}}}}}"#
    ));
    let [server] = found.as_slice() else {
        panic!("one server, got {found:?}");
    };

    assert_eq!(server.args().collect::<Vec<_>>(), args);
}
