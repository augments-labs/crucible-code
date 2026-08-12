//! What answering `always` leaves behind in a file somebody else writes too.

use crucible_core::{
    Ask, Command, Mode, Reach, Remember, Sensitivity, Settled, ToolArgs, ToolCall, ToolId, Verdict,
    narrowest,
};

use crate::document::{Document, Origin};
use crate::settings::Settings;

use super::*;

const FILE: &str = ".crucible/config.local.json";

fn call() -> ToolCall {
    ToolCall {
        id: ToolId::new("one"),
        name: "bash".into(),
        args: ToolArgs::new("{}"),
    }
}

fn running(command: &str) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            parts: Box::from([Box::from(command)]),
            reach: Reach::Anything,
        },
    }
}

/// The rule an `always` about this command would write down.
fn rule(command: &str) -> Minted {
    narrowest(&call(), &running(command)).expect("one command can be written down")
}

/// The text with that command allowed in it.
fn allowing_command(text: &str, command: &str) -> String {
    allowing(text, FILE, &rule(command)).expect("the test wrote a spliceable file")
}

/// What the engine this file describes does with one command.
///
/// Nobody answers, so a call that still reaches the user comes back refused —
/// which is how a rule that landed is told from one that did not.
fn settles(text: &str, command: &str) -> Settled {
    struct Nobody;

    impl Ask for Nobody {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            (Verdict::Deny, Remember::Never)
        }
    }

    let document = Document::parse(text, FILE, Origin::ProjectLocal)
        .expect("what was written is a document crucible reads");

    Settings::resolve(vec![document])
        .permission(Mode::Ask)
        .decide(&call(), &running(command), &mut Nobody)
}

/// The property every case owes: the call that was said `always` to runs
/// without asking next time, and nothing else does.
fn allows_only(text: &str, command: &str) {
    assert!(
        matches!(settles(text, command), Settled::Approved(_)),
        "{command} is not allowed by\n{text}"
    );
    assert!(
        matches!(settles(text, "echo hello"), Settled::Refused),
        "a second command is allowed by\n{text}"
    );
}

#[test]
fn a_file_that_is_not_there_yet_becomes_one_holding_the_rule() {
    let written = allowing_command("", "cargo test");

    allows_only(&written, "cargo test");
}

#[test]
fn a_file_with_no_permissions_block_gains_one() {
    let written = allowing_command(r#"{"output": {"color": "never"}}"#, "cargo test");

    allows_only(&written, "cargo test");
    assert!(
        written.contains(r#""output": {"color": "never"}"#),
        "{written}"
    );
}

#[test]
fn a_permissions_block_with_no_allow_list_gains_one() {
    let written = allowing_command(r#"{"permissions": {"mode": "ask"}}"#, "cargo test");

    allows_only(&written, "cargo test");
    assert!(written.contains(r#""mode": "ask""#), "{written}");
}

#[test]
fn an_allow_list_with_nothing_in_it_takes_the_first_rule() {
    let written = allowing_command(r#"{"permissions": {"allow": []}}"#, "cargo test");

    allows_only(&written, "cargo test");
}

#[test]
fn a_rule_joins_the_entries_already_written() {
    let written = allowing_command(
        r#"{"permissions": {"allow": ["bash(git status)"]}}"#,
        "cargo test",
    );

    allows_only(&written, "cargo test");
    assert!(
        matches!(settles(&written, "git status"), Settled::Approved(_)),
        "the rule that was already there stopped working: {written}"
    );
}

#[test]
fn a_rule_already_written_down_is_not_written_twice() {
    let already = r#"{"permissions": {"allow": ["bash(cargo test)"]}}"#;

    assert_eq!(allowing_command(already, "cargo test"), already);
}

#[test]
fn everything_the_file_already_said_is_left_exactly_as_it_was() {
    // The reason this splices rather than re-serialising. A round-trip would
    // sort the keys and drop the spacing, and hand somebody back a file they
    // have to read again to recognise.
    let hand_written = "{\n\t\"permissions\": {\n\t\t\"deny\":  [\"bash(curl *)\"],\n\t\t\"allow\": [\n\t\t\t\"bash(git status)\"\n\t\t]\n\t}\n}\n";

    let written = allowing_command(hand_written, "cargo test");

    for kept in [
        "\n\t\"permissions\": {",
        "\n\t\t\"deny\":  [\"bash(curl *)\"],",
        "\n\t\t\t\"bash(git status)\"",
        "\n\t}\n}\n",
    ] {
        assert!(written.contains(kept), "{kept:?} is gone from\n{written}");
    }
    allows_only(&written, "cargo test");
}

#[test]
fn a_rule_lands_beside_the_entries_it_joins() {
    // Not a demand that the layout be pretty — a demand that it be the layout
    // already in the file, since the next person to open it reads both.
    let written = allowing_command(
        "{\n  \"permissions\": {\n    \"allow\": [\n      \"bash(git status)\"\n    ]\n  }\n}\n",
        "cargo test",
    );

    assert!(
        written.contains("\n      \"bash(git status)\",\n      \"bash(cargo test)\"\n"),
        "{written}"
    );
}

#[test]
fn a_list_written_on_one_line_stays_on_one_line() {
    let written = allowing_command(r#"{"permissions": {"allow": ["bash(git status)"]}}"#, "ls");

    assert!(
        written.contains(r#"["bash(git status)", "bash(ls)"]"#),
        "{written}"
    );
}

#[test]
fn a_rule_holding_a_character_json_reads_survives_the_trip() {
    // The minted rule escapes glob characters with a backslash class, and a
    // backslash is something JSON reads too. Written raw it would come back as
    // a different rule, or as no document at all.
    let written = allowing_command("", "rm *.tmp");

    allows_only(&written, "rm *.tmp");
    assert!(
        matches!(settles(&written, "rm anything.tmp"), Settled::Refused),
        "the rule widened on the way through the file: {written}"
    );
}

#[test]
fn a_file_that_is_not_json_says_so_instead_of_being_overwritten() {
    let err = allowing("{ oh dear", FILE, &rule("cargo test"))
        .expect_err("a file crucible cannot read is not one it may replace");

    assert!(matches!(err, ConfigError::Malformed { .. }), "{err:?}");
}

#[test]
fn a_block_written_twice_is_left_for_a_person_to_fix() {
    // JSON says nothing about a key written twice and the parser keeps the
    // last of them, so the block the file *means* is not the block a scan of
    // the text finds first. Splicing into the one it found would put the rule
    // where the file's own meaning already shadows it, and leave a second
    // `allow` behind in a block that had one.
    let err = allowing(
        "{\n  \"permissions\": {\"allow\": [\"bash(git status)\"]},\n  \"permissions\": {\"mode\": \"ask\"}\n}\n",
        FILE,
        &rule("cargo test"),
    )
    .expect_err("a file whose keys disagree is not one to write into");

    assert!(matches!(err, ConfigError::Unspliceable { .. }), "{err:?}");
}

#[test]
fn a_list_written_twice_is_left_for_a_person_to_fix() {
    let err = allowing(
        r#"{"permissions": {"allow": ["bash(git status)"], "allow": ["bash(ls)"]}}"#,
        FILE,
        &rule("cargo test"),
    )
    .expect_err("a file whose keys disagree is not one to write into");

    assert!(matches!(err, ConfigError::Unspliceable { .. }), "{err:?}");
}

#[test]
fn a_key_spelled_with_an_escape_is_left_for_a_person_to_fix() {
    // `\u0070ermissions` is `permissions` to the parser and is not the text
    // this searches for, so the file holds a block that a scan cannot find and
    // a scan finds a block that may not be the one the file means.
    let err = allowing(
        "{\"permissions\": {\"allow\": []}, \"\\u0070ermissions\": {\"mode\": \"ask\"}}",
        FILE,
        &rule("cargo test"),
    )
    .expect_err("a file whose keys disagree is not one to write into");

    assert!(matches!(err, ConfigError::Unspliceable { .. }), "{err:?}");
}

#[test]
fn a_file_that_is_not_an_object_is_left_for_a_person_to_fix() {
    let err = allowing("[1, 2]", FILE, &rule("cargo test"))
        .expect_err("a rule has nowhere to go in a list");

    let said = err.to_string();
    assert!(matches!(err, ConfigError::Unspliceable { .. }), "{err:?}");
    assert!(said.contains("bash(cargo test)"), "{said}");
    assert!(said.contains(FILE), "{said}");
}
