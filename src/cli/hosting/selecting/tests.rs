use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use crucible_core::SandboxFilesystemAccess;

use super::*;
use crate::cli::sample::Sample;

/// The variable an `envFrom` record names in these tests.
const NAMED: &str = "EXAMPLE_DOCS_TOKEN";

/// What that variable holds where a test sets it.
const HELD: &str = "a-token-nothing-should-print";

/// A record naming a bare command, so the `PATH` decides where it is.
///
/// Written into the home file in every test here, never the workspace's own:
/// `mcp.servers` widens what crucible does without asking, so a file that can
/// arrive with a checkout is refused before any of this is reached.
const BARE: &str = r#"{
    "mcp": {
        "servers": {
            "docs": {
                "command": "docs-mcp",
                "args": ["--catalogue", "public"],
                "env": {"DOCS_LOCALE": "en"},
                "envFrom": {"DOCS_TOKEN": "EXAMPLE_DOCS_TOKEN"},
                "handshakeSeconds": 3,
                "requestSeconds": 12,
                "shutdownSeconds": 2,
                "restarts": 2,
                "required": true
            },
            "notes": {"command": "notes-mcp"}
        }
    }
}"#;

/// An environment of exactly these names.
fn holding(entries: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let held: HashMap<String, OsString> = entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), OsString::from(*value)))
        .collect();

    move |name: &str| held.get(name).cloned()
}

/// A directory holding a file called `command`, and a lookup whose `PATH` is
/// that directory and nothing else.
///
/// A real file because the resolver's question is whether one is there, and a
/// `PATH` of one directory because a test that inherited the machine's own
/// would be answering with whatever that machine happens to have installed.
fn installed(
    sample: &Sample,
    command: &str,
    also: &[(&str, &str)],
) -> (PathBuf, impl Fn(&str) -> Option<OsString> + use<>) {
    let directory = sample.root().join("bin");
    fs::create_dir_all(&directory).expect("a temporary directory");
    let at = directory.join(crucible_tools::program::spelled(command));
    fs::write(&at, "").expect("a temporary directory");

    let mut held: HashMap<String, OsString> = also
        .iter()
        .map(|(name, value)| ((*name).to_owned(), OsString::from(*value)))
        .collect();
    held.insert("PATH".to_owned(), directory.clone().into_os_string());

    (at, move |name: &str| held.get(name).cloned())
}

#[test]
fn a_run_that_named_no_server_selects_nothing_however_many_are_written_down() {
    let sample = Sample::new("selecting-none");
    let settings = sample.user(BARE);

    let found = selected(&[], &settings, &sample.workspace(), holding(&[]))
        .expect("naming nothing cannot fail");

    assert!(found.is_empty());
    assert_eq!(settings.mcp_servers().len(), 2, "the document did hold two");
}

#[test]
fn a_named_server_is_resolved_into_what_it_takes_to_start_it() {
    let sample = Sample::new("selecting-whole");
    let settings = sample.user(BARE);
    let (at, lookup) = installed(&sample, "docs-mcp", &[(NAMED, HELD)]);

    let found = selected(&["docs".to_owned()], &settings, &sample.workspace(), lookup)
        .expect("a record this test wrote");

    let [chosen] = found.as_slice() else {
        panic!("one server was named");
    };
    assert_eq!(&*chosen.name, "docs");
    assert_eq!(chosen.program, at);
    assert_eq!(chosen.arguments, ["--catalogue", "public"]);
    assert_eq!(chosen.handshake, Duration::from_secs(3));
    assert_eq!(chosen.request, Duration::from_secs(12));
    assert_eq!(chosen.grace, Duration::from_secs(2));
    // The number itself rather than merely some ceiling: a selection that
    // carried a larger one would restart a server the document said to give up
    // on, and one that carried a smaller one would give up on a server the
    // document was still willing to try.
    assert_eq!(chosen.restarts, 2);
    assert!(chosen.required);
}

#[test]
fn a_record_that_named_no_restarts_gets_one_start_and_no_more() {
    let sample = Sample::new("selecting-restarts-default");
    let settings = sample.user(BARE);
    let (_, lookup) = installed(&sample, "notes-mcp", &[]);

    let found = selected(
        &["notes".to_owned()],
        &settings,
        &sample.workspace(),
        lookup,
    )
    .expect("a record this test wrote");

    let [chosen] = found.as_slice() else {
        panic!("one server was named");
    };
    assert_eq!(
        chosen.restarts, 0,
        "starting somebody else's program again is something a document asks \
         for, not something crucible does by default"
    );
}

#[test]
fn a_name_nobody_wrote_down_refuses_and_says_which_names_the_document_held() {
    let sample = Sample::new("selecting-unknown");
    let settings = sample.user(BARE);

    let Err(refused) = selected(
        &["diagrams".to_owned()],
        &settings,
        &sample.workspace(),
        holding(&[]),
    ) else {
        panic!("no such server was written down");
    };

    let said = refused.to_string();
    assert!(said.contains("diagrams"), "{said}");
    assert!(said.contains("docs"), "{said}");
    assert!(said.contains("notes"), "{said}");
}

#[test]
fn a_selection_against_a_document_holding_no_servers_says_that_rather_than_a_list() {
    let sample = Sample::new("selecting-empty");
    let settings = sample.user("{}");

    let Err(refused) = selected(
        &["docs".to_owned()],
        &settings,
        &sample.workspace(),
        holding(&[]),
    ) else {
        panic!("nothing was written down");
    };

    assert!(
        refused.to_string().contains("no servers are written down"),
        "{refused}"
    );
}

#[test]
fn a_command_written_out_in_full_is_taken_as_it_stands_rather_than_looked_for() {
    let sample = Sample::new("selecting-absolute");
    let elsewhere = sample.root().join("elsewhere").join("docs-mcp");
    let document = format!(
        r#"{{"mcp": {{"servers": {{"docs": {{"command": "{}"}}}}}}}}"#,
        elsewhere.display().to_string().replace('\\', "/")
    );
    let settings = sample.user(&document);

    // An empty PATH: a run that found this is one that did not consult it.
    let found = selected(
        &["docs".to_owned()],
        &settings,
        &sample.workspace(),
        holding(&[("PATH", "")]),
    )
    .expect("an absolute command needs no PATH");

    let [chosen] = found.as_slice() else {
        panic!("one server was named");
    };
    assert_eq!(chosen.program, elsewhere);
}

#[test]
fn a_command_no_path_element_holds_refuses_and_names_the_server_and_the_command() {
    let sample = Sample::new("selecting-nowhere");
    let settings = sample.user(BARE);
    let directory = sample.root().join("bin");
    fs::create_dir_all(&directory).expect("a temporary directory");

    let Err(refused) = selected(
        &["docs".to_owned()],
        &settings,
        &sample.workspace(),
        holding(&[("PATH", &directory.display().to_string())]),
    ) else {
        panic!("nothing of that name is in that directory");
    };

    let said = refused.to_string();
    assert!(said.contains("docs"), "{said}");
    assert!(said.contains("docs-mcp"), "{said}");
    assert!(said.contains("PATH"), "{said}");
}

#[test]
fn a_server_is_started_with_what_the_record_named_and_nothing_else() {
    let sample = Sample::new("selecting-environment");
    let settings = sample.user(BARE);
    let (_, lookup) = installed(
        &sample,
        "docs-mcp",
        &[(NAMED, HELD), ("SECRET", "not-this")],
    );

    let found = selected(&["docs".to_owned()], &settings, &sample.workspace(), lookup)
        .expect("a record this test wrote");

    let [chosen] = found.as_slice() else {
        panic!("one server was named");
    };
    let mut held: Vec<(String, String)> = chosen
        .environment
        .iter()
        .map(|(name, value)| (name.to_owned(), value.to_string_lossy().into_owned()))
        .collect();
    held.sort();

    assert_eq!(
        held,
        [
            ("DOCS_LOCALE".to_owned(), "en".to_owned()),
            ("DOCS_TOKEN".to_owned(), HELD.to_owned()),
        ],
        "the whole environment, rather than crucible's own with these added"
    );
}

#[test]
fn an_envfrom_naming_a_variable_that_is_not_set_refuses_without_printing_one_that_is() {
    let sample = Sample::new("selecting-unset");
    let settings = sample.user(BARE);
    let (_, lookup) = installed(&sample, "docs-mcp", &[("SOMETHING_ELSE", HELD)]);

    let Err(refused) = selected(&["docs".to_owned()], &settings, &sample.workspace(), lookup)
    else {
        panic!("the variable it names is not set");
    };

    let said = refused.to_string();
    assert!(said.contains(NAMED), "{said}");
    assert!(!said.contains(HELD), "a refusal must not print a value");
}

#[test]
fn a_written_down_directory_is_a_writable_root_and_the_place_the_server_starts_in() {
    let sample = Sample::new("selecting-directory");
    let elsewhere = sample.root().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("a temporary directory");
    let document = format!(
        r#"{{"mcp": {{"servers": {{"docs": {{"command": "docs-mcp", "directory": "{}"}}}}}}}}"#,
        elsewhere.display().to_string().replace('\\', "/")
    );
    let settings = sample.user(&document);
    let (_, lookup) = installed(&sample, "docs-mcp", &[]);

    let found = selected(&["docs".to_owned()], &settings, &sample.workspace(), lookup)
        .expect("a record this test wrote");

    let [chosen] = found.as_slice() else {
        panic!("one server was named");
    };
    assert_eq!(chosen.policy.working_directory(), elsewhere);
    assert!(
        chosen.policy.filesystem().iter().any(|rule| {
            rule.path() == elsewhere && rule.access() == SandboxFilesystemAccess::ReadWrite
        }),
        "the directory it starts in has to be one it may reach"
    );
}

#[test]
fn a_record_that_names_no_directory_runs_where_a_confined_command_would() {
    let sample = Sample::new("selecting-workspace");
    let settings = sample.user(BARE);
    let workspace = sample.workspace();
    let (_, lookup) = installed(&sample, "docs-mcp", &[(NAMED, HELD)]);

    let found = selected(&["docs".to_owned()], &settings, &workspace, lookup)
        .expect("a record this test wrote");

    let [chosen] = found.as_slice() else {
        panic!("one server was named");
    };
    assert_eq!(chosen.policy.working_directory(), workspace.root());
}

#[test]
fn selected_servers_share_the_runs_opt_in_confinement() {
    let sample = Sample::new("selecting-mode");
    for (sandbox, expected) in [
        ("", SandboxMode::Off),
        (r#""sandbox":{"enabled":true},"#, SandboxMode::Required),
        (r#""sandbox":{"enabled":false},"#, SandboxMode::Off),
        (r#""sandbox":{"mode":"degraded"},"#, SandboxMode::Degraded),
    ] {
        let settings = sample.user(&format!(
            r#"{{{sandbox}"mcp":{{"servers":{{"docs":{{"command":"docs-mcp"}}}}}}}}"#
        ));
        let (_, lookup) = installed(&sample, "docs-mcp", &[]);
        let found = selected(&["docs".to_owned()], &settings, &sample.workspace(), lookup)
            .expect("a record this test wrote");
        let [chosen] = found.as_slice() else {
            panic!("one server was named");
        };
        assert_eq!(chosen.policy.mode(), expected);
        assert_eq!(
            chosen.policy.limits().cpu_seconds,
            (expected == SandboxMode::Required).then_some(3600)
        );
        assert_eq!(
            chosen.policy.limits().open_files,
            (expected == SandboxMode::Required).then_some(4096)
        );
    }
}
