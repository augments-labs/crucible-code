//! Splicing a durable rule into a user-owned configuration document.

use crucible_models::Speed;
use crucible_tools::{Ask, Command, Mode, Remember, Sensitivity, Settled, Verdict, narrowest};
use crucible_types::{AgentKey, ToolArgs, ToolCall, ToolId};

use crate::document::{Document, Origin};
use crate::settings::access::Serves;
use crate::settings::{Driver, ExternalAgent, Settings};

use super::*;

const FILE: &str = "~/.crucible/config.json";

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
            sent: command.into(),
            parts: Box::from([Box::from(command)]),
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

/// Drives a future to its answer, the way a test drives a fake's future by
/// hand: this suite's [`Ask`] fake never really waits, so asking it once
/// always has one, and a fake that did not would fail here rather than hang.
fn answered_now<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("the fake did not answer on the first poll"),
    }
}

/// What the engine this file describes does with one command.
///
/// Nobody answers, so a call that still reaches the user comes back refused —
/// which is how a rule that landed is told from one that did not.
fn settles(text: &str, command: &str) -> Settled {
    struct Nobody;

    impl Ask for Nobody {
        fn ask<'a>(
            &'a mut self,
            _call: &'a ToolCall,
            _sensitivity: &'a Sensitivity,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (Verdict, Remember)> + Send + 'a>>
        {
            Box::pin(async { (Verdict::Deny, Remember::Never) })
        }
    }

    let document = Document::parse(text, FILE, Origin::User)
        .expect("what was written is a document crucible reads");

    answered_now(
        Settings::resolve(vec![document])
            .permission(Mode::Ask)
            .decide(&call(), &running(command), &mut Nobody),
    )
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

/// The settings the file this text describes resolves to.
///
/// The whole property both provider answers owe: what was spliced in is read
/// back by the parser that reads it at startup. A key spelled one way here and
/// another way there is a file crucible writes and then ignores.
fn resolving(text: &str) -> Settings {
    let document = Document::parse(text, "config.json", Origin::User)
        .expect("what was written is a document crucible reads");

    Settings::resolve(vec![document])
}

#[test]
fn a_file_that_is_not_there_yet_becomes_one_holding_the_rung() {
    let written = thinking("", "config.json", "anthropic", Effort::Max)
        .expect("an empty file is one to write whole");

    assert_eq!(resolving(&written).effort("anthropic"), Some(Effort::Max));
}

#[test]
fn a_rung_and_a_model_are_two_answers_under_one_provider() {
    // The two keys are spliced by one walk, so this is the case that says the
    // walk is putting each under its own name: a second answer that landed on
    // the first would be a model silently replaced by the word `max`.
    let chosen = choosing("", "config.json", "openai", "gpt-5.6-terra")
        .expect("an empty file is one to write whole");
    let written = thinking(&chosen, "config.json", "openai", Effort::Low)
        .expect("a provider already written is one to write beside");

    let settings = resolving(&written);
    assert_eq!(settings.model("openai"), Some("gpt-5.6-terra"));
    assert_eq!(settings.effort("openai"), Some(Effort::Low));
}

#[test]
fn choosing_another_model_removes_the_previous_models_rung() {
    let first =
        choosing("", "config.json", "moonshot", "k3").expect("an empty file is one to write whole");
    let with_effort = thinking(&first, "config.json", "moonshot", Effort::Max)
        .expect("a provider already written is one to write beside");
    let written = choosing(&with_effort, "config.json", "moonshot", "k3-256k")
        .expect("a model already written is one to write over");

    let settings = resolving(&written);
    assert_eq!(settings.model("moonshot"), Some("k3-256k"));
    assert_eq!(settings.effort("moonshot"), None);
    assert!(!written.contains("\"effort\""), "{written}");
}

#[test]
fn a_first_member_rung_is_removed_without_rewriting_its_neighbour() {
    let text = concat!(
        "{\n  \"providers\": {\n    \"openai\": {\n",
        "      \"effort\": \"high\",\n      \"kept\" : [ 1, 2 ],\n",
        "      \"model\": \"old\"\n    }\n  }\n}\n"
    );
    let written = choosing(text, "config.json", "openai", "new")
        .expect("a model already written is one to write over");

    assert!(written.contains("\"kept\" : [ 1, 2 ]"), "{written}");
    assert!(!written.contains("\"effort\""), "{written}");
    let value: serde_json::Value =
        serde_json::from_str(&written).expect("what was written is still JSON");
    assert_eq!(
        value
            .pointer("/providers/openai/model")
            .and_then(serde_json::Value::as_str),
        Some("new")
    );
}

#[test]
fn a_rung_chosen_again_is_written_over_rather_than_written_twice() {
    let first = thinking("", "config.json", "moonshot", Effort::High)
        .expect("an empty file is one to write whole");
    let written = thinking(&first, "config.json", "moonshot", Effort::Xhigh)
        .expect("a rung already there is one to write over");

    assert_eq!(resolving(&written).effort("moonshot"), Some(Effort::Xhigh));
    assert_eq!(written.matches("effort").count(), 1, "{written}");
}

#[test]
fn fast_is_written_with_the_model_it_was_chosen_for_and_standard_takes_it_out() {
    let chosen = choosing("", "config.json", "openai", "gpt-5.6-sol")
        .expect("an empty file is one to write whole");

    let fast = hastening(&chosen, "config.json", "openai", "gpt-5.6-sol")
        .expect("a provider already written is one to write beside");
    assert_eq!(resolving(&fast).speed("openai"), Speed::Fast);
    assert_eq!(resolving(&fast).model("openai"), Some("gpt-5.6-sol"));
    assert!(fast.contains("\"fast\": true"), "{fast}");

    let again = hastening(&fast, "config.json", "openai", "gpt-5.6-sol")
        .expect("a speed already there is one to leave");
    assert_eq!(again, fast);

    let standard =
        slowing(&fast, "config.json", "openai").expect("a speed written is one to take out");
    assert_eq!(resolving(&standard).speed("openai"), Speed::Standard);
    assert!(!standard.contains("\"fast\""), "{standard}");
    assert_eq!(standard, chosen);
}

#[test]
fn fast_chosen_for_a_model_the_file_does_not_name_names_it() {
    // The speed is a choice about one model at one price, so the model goes
    // into the file with it, whether the file named another or none.
    for text in [
        "",
        "{\n  \"providers\": {\"openai\": {\"model\": \"gpt-6-astra\"}}\n}\n",
    ] {
        let fast = hastening(text, "config.json", "openai", "gpt-5.5")
            .expect("a file a speed can be written into");
        assert_eq!(resolving(&fast).model("openai"), Some("gpt-5.5"), "{fast}");
        assert_eq!(resolving(&fast).speed("openai"), Speed::Fast, "{fast}");
    }
}

#[test]
fn standard_asked_of_a_file_with_no_speed_changes_nothing() {
    for text in ["", "{\n  \"theme\": \"dark\"\n}\n"] {
        let written = slowing(text, "config.json", "openai")
            .expect("nothing to take out is nothing to refuse");
        assert_eq!(written, text);
    }
}

#[test]
fn choosing_another_model_takes_the_speed_out_and_the_same_one_keeps_it() {
    let chosen = choosing("", "config.json", "openai", "gpt-5.6-sol")
        .expect("an empty file is one to write whole");
    let fast = hastening(&chosen, "config.json", "openai", "gpt-5.6-sol")
        .expect("a provider already written is one to write beside");

    let same =
        choosing(&fast, "config.json", "openai", "gpt-5.6-sol").expect("the model already written");
    assert_eq!(resolving(&same).speed("openai"), Speed::Fast);

    let other = choosing(&fast, "config.json", "openai", "gpt-5.6-luna")
        .expect("a model already written is one to write over");
    assert_eq!(resolving(&other).speed("openai"), Speed::Standard);
    assert!(!other.contains("\"fast\""), "{other}");
}

#[test]
fn a_speed_is_read_for_the_model_the_file_names_and_for_no_other() {
    // A run on another model than the file names was never shown that
    // model's price, and a file that names none names no price at all.
    let fast = hastening("", "config.json", "openai", "gpt-5.6-sol")
        .expect("an empty file is one to write whole");
    assert_eq!(
        hastened(&fast, "config.json", "openai", "gpt-5.6-sol").unwrap(),
        Speed::Fast
    );
    assert_eq!(
        hastened(&fast, "config.json", "openai", "gpt-5.5").unwrap(),
        Speed::Standard
    );
    assert_eq!(
        hastened(&fast, "config.json", "anthropic", "claude-opus-5").unwrap(),
        Speed::Standard
    );
    let unnamed = r#"{"providers": {"openai": {"fast": true}}}"#;
    assert_eq!(
        hastened(unnamed, "config.json", "openai", "gpt-5.5").unwrap(),
        Speed::Standard
    );
    assert_eq!(
        hastened("", "config.json", "openai", "gpt-5.6-sol").unwrap(),
        Speed::Standard
    );
    assert!(hastened("{", "config.json", "openai", "gpt-5.6-sol").is_err());
}

#[test]
fn a_model_named_with_spaces_around_it_is_the_model_a_run_asks() {
    // A run trims the name it reads, so the speed kept beside it is read for
    // the same trimmed name, and choosing it again keeps the speed.
    let padded = r#"{"providers": {"openai": {"model": " gpt-5.6-sol ", "fast": true}}}"#;
    assert_eq!(
        hastened(padded, "config.json", "openai", "gpt-5.6-sol").unwrap(),
        Speed::Fast
    );
    let again = choosing(padded, "config.json", "openai", "gpt-5.6-sol")
        .expect("the model already written");
    assert_eq!(resolving(&again).speed("openai"), Speed::Fast);
}

#[test]
fn a_blank_model_in_the_file_names_no_model_to_keep_a_speed_for() {
    let blank = r#"{"providers": {"openai": {"model": "   ", "fast": true}}}"#;
    assert_eq!(
        hastened(blank, "config.json", "openai", "").unwrap(),
        Speed::Standard
    );
}

#[test]
fn a_speed_the_file_spells_as_no_is_standard() {
    let settings = resolving(r#"{"providers": {"openai": {"fast": false}}}"#);
    assert_eq!(settings.speed("openai"), Speed::Standard);
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

#[test]
fn the_provider_to_ask_is_written_where_a_file_holding_nothing_else_says_it() {
    let written = asking("", "config.json", "openai").expect("an empty file is one to write whole");

    assert_eq!(resolving(&written).provider(), Some("openai"));
}

#[test]
fn the_provider_to_ask_is_written_beside_what_the_file_already_held() {
    // The case `/login` meets: a file that has been set up for one vendor and
    // is now being told about another. Everything else stays where it was.
    let written = asking(
        r#"{"providers": {"openai": {"model": "gpt-5.6-terra"}}}"#,
        "config.json",
        "anthropic",
    )
    .expect("an object is one to write into");

    let settings = resolving(&written);
    assert_eq!(settings.provider(), Some("anthropic"));
    assert_eq!(settings.model("openai"), Some("gpt-5.6-terra"));
}

#[test]
fn a_provider_chosen_again_is_written_over_rather_than_written_twice() {
    // Two of the same key is a document the parser reads one way and whoever
    // opens it reads the other — and this one decides which vendor sees the
    // prompt, so the two readings disagree about that.
    let first = asking("", "config.json", "moonshot").expect("an empty file is one to write whole");
    let written = asking(&first, "config.json", "anthropic").expect("a name already there");

    assert_eq!(resolving(&written).provider(), Some("anthropic"));
    assert_eq!(written.matches("provider").count(), 1, "{written}");
}

/// The file `drawing` produces, or the error it refused with.
fn drawn(text: &str, theme: &str) -> Result<String, ConfigError> {
    super::drawing(text, "config.json", theme)
}

#[test]
fn a_file_that_is_not_there_becomes_one_holding_the_theme() {
    assert_eq!(
        drawn("", "light").expect("a file to be written"),
        "{\n  \"output\": {\n    \"theme\": \"light\"\n  }\n}\n"
    );
}

#[test]
fn a_theme_goes_into_an_output_block_that_is_already_there() {
    // Every byte the author put there stays where it was, which is the whole
    // reason this splices rather than re-serialising.
    let was = "{\n  \"output\": {\n    \"glyphs\": \"ascii\"\n  }\n}\n";

    assert_eq!(
        drawn(was, "dark").expect("a theme to be written"),
        "{\n  \"output\": {\n    \"glyphs\": \"ascii\",\n    \"theme\": \"dark\"\n  }\n}\n"
    );
}

#[test]
fn a_theme_already_written_is_written_over_rather_than_beside() {
    // The same key twice is a document the parser reads one way and its author
    // reads the other.
    let was = "{\n  \"output\": {\n    \"theme\": \"dark\"\n  }\n}\n";
    let now = drawn(was, "light").expect("a theme to be written");

    assert_eq!(now.matches("\"theme\"").count(), 1, "{now}");
    assert!(now.contains("\"light\""), "{now}");
}

#[test]
fn taking_the_theme_that_is_already_written_changes_nothing_at_all() {
    let was = "{\n  \"output\": {\n    \"theme\": \"dark\"\n  }\n}\n";

    assert_eq!(drawn(was, "dark").expect("the same file"), was);
}

#[test]
fn a_syntax_theme_makes_the_output_block_under_its_own_key() {
    // The defect this catches: the branch that creates the `output` block
    // spelled the key `theme` whatever key it was asked to write, so a fenced
    // code theme remembered into a file with no `output` block landed under
    // the drawing theme's key — and `output.theme` being a closed choice, the
    // next start refused a file crucible had written itself.
    let was = "{\n  \"provider\": \"anthropic\"\n}\n";
    let now =
        super::reading(was, "config.json", "Solarized (dark)").expect("a block to be written");

    let read: serde_json::Value = serde_json::from_str(&now).expect("valid json");
    assert_eq!(
        read.get("output").and_then(|at| at.get("syntaxTheme")),
        Some(&"Solarized (dark)".into()),
        "{now}"
    );
    assert_eq!(read.get("output").and_then(|at| at.get("theme")), None);
}

#[test]
fn an_output_block_is_created_beside_whatever_else_the_file_holds() {
    let was = "{\n  \"provider\": \"anthropic\"\n}\n";
    let now = drawn(was, "ansi").expect("a block to be written");

    assert!(now.contains("\"provider\": \"anthropic\""), "{now}");
    assert!(now.contains("\"theme\": \"ansi\""), "{now}");

    // And it reads back as the document it looks like.
    let read: serde_json::Value = serde_json::from_str(&now).expect("valid json");
    assert_eq!(
        read.get("output").and_then(|at| at.get("theme")),
        Some(&"ansi".into())
    );
    assert_eq!(read.get("provider"), Some(&"anthropic".into()));
}

#[test]
fn a_file_written_on_one_line_gets_the_block_on_one_line() {
    // `insert` hands back `None` for the indentation where the line it is going
    // on already has other things on it. A block with a hard-coded indent there
    // would be this program deciding how somebody else's file is laid out.
    let now = drawn("{\"provider\": \"anthropic\"}", "light").expect("a block");

    assert!(
        !now.contains('\n'),
        "a line was broken that nobody broke: {now}"
    );
    let read: serde_json::Value = serde_json::from_str(&now).expect("valid json");
    assert_eq!(
        read.get("output").and_then(|at| at.get("theme")),
        Some(&"light".into())
    );
}

#[test]
fn a_document_that_is_not_json_is_refused_by_name() {
    let refused = drawn("{not json", "dark").expect_err("a refusal");

    assert!(
        matches!(refused, ConfigError::Malformed { .. }),
        "{refused:?}"
    );
}

#[test]
fn a_document_no_theme_can_be_written_into_says_what_to_type() {
    // A root that is not an object has nowhere to put a key, and guessing at
    // somebody's file is the one thing this must not do.
    let refused = drawn("[1, 2, 3]", "dark").expect_err("a refusal");

    assert!(
        matches!(refused, ConfigError::Unspliceable { .. }),
        "{refused:?}"
    );
}

#[test]
fn sandbox_choice_is_a_boolean_and_preserves_every_other_setting() {
    let text = "{\n  \"sandbox\": {\"enabled\": false, \"limits\": {\"outputBytes\": 512}},\n  \"provider\": \"openai\"\n}\n";
    assert_eq!(
        super::sandboxing(text, "test", true).unwrap(),
        text.replace("false", "true")
    );
    for text in [
        "",
        "{}",
        r#"{"sandbox":{}}"#,
        r#"{"sandbox":{"filesystem":{"readOnly":["vendor"]}}}"#,
    ] {
        let written = super::sandboxing(text, "test", true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            value.pointer("/sandbox/enabled"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(super::sandboxing(&written, "test", true).unwrap(), written);
    }
    assert!(super::sandboxing(r#"{"sandbox":{"mode":"required"}}"#, "test", true).is_err());
}

/// What the file says yes to, as the settings read it.
fn accepted(text: &str) -> Vec<String> {
    let settings = Settings::resolve(vec![Document::sample(text, Origin::User)]);
    settings
        .content_accepted()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_yes_is_written_into_a_file_that_is_not_there_yet() {
    let written = accepting("", FILE, "key:google").unwrap();
    assert_eq!(accepted(&written), ["key:google"]);
    assert_eq!(
        written,
        "{\n  \"contentUse\": {\n    \"accepted\": [\n      \"key:google\"\n    ]\n  }\n}\n"
    );
}

#[test]
fn a_yes_is_added_beside_what_the_file_already_says_and_only_once() {
    let text =
        "{\n  \"provider\":   \"google\",\n  \"contentUse\": {\"accepted\": [\"key:google\"]}\n}\n";
    let written = accepting(text, FILE, "api.moonshot.ai").unwrap();
    assert_eq!(accepted(&written), ["key:google", "api.moonshot.ai"]);
    assert!(written.starts_with("{\n  \"provider\":   \"google\",\n"));
    assert_eq!(accepting(&written, FILE, "key:google").unwrap(), written);

    let bare = "{\"provider\": \"google\"}";
    let written = accepting(bare, FILE, "key:google").unwrap();
    assert_eq!(accepted(&written), ["key:google"]);
    let block = "{\"contentUse\": {}}";
    assert_eq!(
        accepted(&accepting(block, FILE, "key:google").unwrap()),
        ["key:google"]
    );
}

/// A block or a list that is not the shape a yes goes into is refused rather
/// than written into: a list spliced into a string would leave a file that no
/// longer says what the user wrote.
#[test]
fn a_yes_is_not_written_into_a_block_or_list_of_another_shape() {
    for text in [
        "{\"contentUse\": []}",
        "{\"contentUse\": {\"accepted\": \"key:google\"}}",
        "{\"contentUse\": {\"accepted\": null}}",
    ] {
        let refused = accepting(text, FILE, "key:google").expect_err(text);
        assert!(
            matches!(refused, ConfigError::Unspliceable { .. }),
            "{text}: {refused:?}"
        );
    }
}

#[test]
fn forgetting_takes_out_the_named_routes_and_nothing_else() {
    let text = "{\n  \"provider\": \"moonshot\",\n  \"contentUse\": {\n    \"accepted\": [\"key:google\", \"key:moonshot\", \"api.moonshot.ai\"]\n  }\n}\n";
    let written = forgetting(text, FILE, |route| {
        ["key:moonshot", "api.moonshot.ai", "key:nobody"].contains(&route)
    })
    .unwrap();
    assert_eq!(accepted(&written), ["key:google"]);
    assert!(
        written.starts_with(
            "{\n  \"provider\": \"moonshot\",\n  \"contentUse\": {\n    \"accepted\": "
        )
    );
    assert_eq!(
        forgetting(&written, FILE, |route| route == "key:moonshot").unwrap(),
        written
    );
    for untouched in ["", "{}", "{\"provider\": \"google\"}"] {
        assert_eq!(
            forgetting(untouched, FILE, |route| route == "key:google").unwrap(),
            untouched
        );
    }
}

fn row_of(key: &str) -> &'static crate::Row {
    crate::row(key).expect("the test names a row")
}

#[test]
fn a_settings_row_is_written_into_an_empty_file_with_the_blocks_around_it() {
    let written = setting(
        "",
        FILE,
        row_of("promptCaching.persistentResources.mode"),
        "reuse",
    )
    .expect("an empty file takes any row");
    assert_eq!(
        written,
        "{\n  \"promptCaching\": {\n    \"persistentResources\": {\n      \"mode\": \"reuse\"\n    }\n  }\n}\n"
    );
}

#[test]
fn a_settings_row_is_spliced_beside_what_the_file_already_says() {
    let text = "{\n  \"output\": {\n    \"theme\": \"dark\"\n  },\n  \"provider\": \"openai\"\n}\n";

    let flipped = setting(text, FILE, row_of("output.scrollRail"), "false").expect("spliced");
    assert_eq!(
        flipped,
        "{\n  \"output\": {\n    \"theme\": \"dark\",\n    \"scrollRail\": false\n  },\n  \"provider\": \"openai\"\n}\n"
    );

    let over = setting(&flipped, FILE, row_of("output.theme"), "light").expect("spliced");
    assert!(over.contains("\"theme\": \"light\""), "{over}");
    assert!(!over.contains("dark"), "{over}");

    let speed = setting(
        &over,
        FILE,
        row_of("env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED"),
        "12",
    )
    .expect("spliced");
    let document = Document::parse(&speed, FILE, Origin::User).expect("still a document");
    let settings = Settings::resolve(vec![document]);
    assert_eq!(settings.scroll_speed(&|_| None).expect("read").rows(), 12);
    assert!(!settings.scroll_rail());
    assert_eq!(settings.provider(), Some("openai"));
}

#[test]
fn a_settings_row_refuses_what_its_key_does_not_take_and_leaves_no_text() {
    let text = "{\n  \"output\": {}\n}\n";
    for (key, word) in [
        ("output.theme", "plaid"),
        ("output.scrollRail", "maybe"),
        ("env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED", "31"),
        ("env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED", "2"),
        // Retention that needs a ceiling the file does not give it.
        ("promptCaching.requestedRetention.class", "extended"),
    ] {
        assert!(
            setting(text, FILE, row_of(key), word).is_err(),
            "{key} took {word}"
        );
    }
}

#[test]
fn a_settings_row_already_saying_the_answer_leaves_the_file_as_it_was() {
    let text = "{\"output\": {\"glyphs\": \"ascii\"}}";
    assert_eq!(
        setting(text, FILE, row_of("output.glyphs"), "ascii").expect("read"),
        text
    );
}

#[test]
fn disabling_an_agent_writes_its_enabled_false_and_nothing_else() {
    let text = "{\n  \"provider\": \"anthropic\",\n  \"externalAgents\": {\n    \"work\": {\n      \"driver\": \"claude\",\n      \"executable\": \"claude\"\n    },\n    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\"}\n  }\n}\n";
    let written = disabling(text, FILE, "work").unwrap();
    assert_eq!(
        written,
        text.replace(
            "\"executable\": \"claude\"\n    },",
            "\"executable\": \"claude\",\n      \"enabled\": false\n    },"
        )
    );
    let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
    let mut enabled: Vec<(String, bool)> = settings
        .external_agents()
        .iter()
        .map(|agent| (agent.name().to_owned(), agent.enabled()))
        .collect();
    enabled.sort();
    assert_eq!(
        enabled,
        [("home".to_owned(), true), ("work".to_owned(), false)]
    );
    // Written again, it is already so.
    assert_eq!(disabling(&written, FILE, "work").unwrap(), written);
}

#[test]
fn disabling_an_agent_takes_away_every_access_reached_through_it_and_no_other() {
    let text = "{\n  \"provider\": \"openai\",\n  \"providers\": {\n    \"openai\": {\n      \"model\": \"m\",\n      \"access\": {\"instance\": \"work\", \"type\": \"externalAgent\"}\n    },\n    \"google\": {\"access\": {\"type\": \"externalAgent\", \"instance\": \"work\"}},\n    \"moonshot\": {\"access\": {\"type\": \"externalAgent\", \"instance\": \"home\"}},\n    \"anthropic\": {\"access\": {\"type\": \"apiKey\"}}\n  },\n  \"externalAgents\": {\n    \"work\": {\"driver\": \"claude\", \"executable\": \"claude\"},\n    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\"}\n  }\n}\n";
    let written = disabling(text, FILE, "work").unwrap();
    assert_eq!(
        written,
        text.replace(
            "\"model\": \"m\",\n      \"access\": {\"instance\": \"work\", \"type\": \"externalAgent\"}\n",
            "\"model\": \"m\"\n"
        )
        .replace(
            "\"google\": {\"access\": {\"type\": \"externalAgent\", \"instance\": \"work\"}}",
            "\"google\": {}"
        )
        .replace(
            "\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}",
            "\"work\": {\"driver\": \"claude\", \"executable\": \"claude\", \"enabled\": false}"
        )
    );
    let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
    assert_eq!(settings.referencing("work"), None);
    assert_eq!(settings.referencing("home").as_deref(), Some("moonshot"));
    // Written again, it is already so.
    assert_eq!(disabling(&written, FILE, "work").unwrap(), written);
    // An agent already disabled still lets go of what reaches it.
    let disabled = text.replace(
        "\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}",
        "\"work\": {\"driver\": \"claude\", \"executable\": \"claude\", \"enabled\": false}",
    );
    assert_eq!(disabling(&disabled, FILE, "work").unwrap(), written);
}

#[test]
fn disabling_an_agent_takes_an_access_written_first_with_the_comma_after_it() {
    let text = r#"{"providers": {"openai": {"access": {"type": "externalAgent", "instance": "work"}, "model": "m"}}, "externalAgents": {"work": {"enabled": false, "driver": "claude", "executable": "claude"}}}"#;
    assert_eq!(
        disabling(text, FILE, "work").unwrap(),
        text.replace(
            r#"{"access": {"type": "externalAgent", "instance": "work"}, "model": "m"}"#,
            // The space after the comma is a neighbour's, and stays.
            r#"{ "model": "m"}"#
        )
    );
}

#[test]
fn disabling_an_enabled_agent_turns_its_word_over_in_place() {
    let text = r#"{"externalAgents": {"work": {"enabled": true, "driver": "claude", "executable": "claude"}}}"#;
    assert_eq!(
        disabling(text, FILE, "work").unwrap(),
        text.replace("\"enabled\": true", "\"enabled\": false")
    );
}

#[test]
fn disabling_an_agent_the_file_does_not_name_is_refused_and_writes_nothing() {
    for text in [
        "",
        "{}",
        r#"{"externalAgents": {}}"#,
        r#"{"externalAgents": {"home": {"driver": "claude", "executable": "claude"}}}"#,
    ] {
        assert!(
            matches!(
                disabling(text, FILE, "work"),
                Err(ConfigError::Unspliceable { .. })
            ),
            "{text}"
        );
    }
    assert!(matches!(
        disabling("{", FILE, "work"),
        Err(ConfigError::Malformed { .. })
    ));
}

#[test]
fn connecting_an_agent_writes_the_provider_s_access_asks_it_then_enables_it_and_nothing_else() {
    let text = "{\n  \"provider\": \"openai\",\n  \"externalAgents\": {\n    \"work\": {\n      \"driver\": \"claude\",\n      \"executable\": \"claude\",\n      \"enabled\": false\n    },\n    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\", \"enabled\": false}\n  }\n}\n";
    let written = connecting(text, FILE, "anthropic", "work").unwrap();
    assert_eq!(
        written,
        "{\n  \"provider\": \"anthropic\",\n  \"externalAgents\": {\n    \"work\": {\n      \"driver\": \"claude\",\n      \"executable\": \"claude\",\n      \"enabled\": true\n    },\n    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\", \"enabled\": false}\n  },\n  \"providers\": {\n    \"anthropic\": {\n      \"access\": {\"instance\":\"work\",\"type\":\"externalAgent\"}\n    }\n  }\n}\n"
    );
    let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
    assert_eq!(
        settings.access("anthropic"),
        Some(crate::settings::Access::ExternalAgent {
            instance: "work".into()
        })
    );
    let mut enabled: Vec<(String, bool)> = settings
        .external_agents()
        .iter()
        .map(|agent| (agent.name().to_owned(), agent.enabled()))
        .collect();
    enabled.sort();
    assert_eq!(
        enabled,
        [("home".to_owned(), false), ("work".to_owned(), true)]
    );
    assert_eq!(settings.provider(), Some("anthropic"));
    // Written again, it is already so.
    assert_eq!(
        connecting(&written, FILE, "anthropic", "work").unwrap(),
        written
    );
}

#[test]
fn connecting_an_agent_that_says_nothing_of_enabled_leaves_it_saying_nothing() {
    let text = r#"{"externalAgents": {"work": {"driver": "claude", "executable": "claude"}}, "providers": {"anthropic": {"access": {"type": "subscription"}}}}"#;
    assert_eq!(
        connecting(text, FILE, "anthropic", "work").unwrap(),
        text.replace(
            r#"{"type": "subscription"}}}}"#,
            r#"{"instance":"work","type":"externalAgent"}}}, "provider": "anthropic"}"#
        )
    );
}

#[test]
fn reaching_a_provider_directly_writes_its_access_and_asks_it_and_nothing_else() {
    use crate::settings::Access;

    let text = "{\n  \"provider\": \"openai\",\n  \"externalAgents\": {\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}},\n  \"providers\": {\n    \"anthropic\": {\n      \"access\": {\"type\": \"externalAgent\", \"instance\": \"work\"}\n    }\n  }\n}\n";
    for (access, spelled) in [
        (Access::Subscription, r#"{"type":"subscription"}"#),
        (Access::ApiKey, r#"{"type":"apiKey"}"#),
    ] {
        let written = reaching(text, FILE, "anthropic", &access).unwrap();
        assert_eq!(
            written,
            text.replace("\"openai\"", "\"anthropic\"")
                .replace(r#"{"type": "externalAgent", "instance": "work"}"#, spelled)
        );
        let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
        assert_eq!(settings.access("anthropic"), Some(access.clone()));
        assert_eq!(settings.provider(), Some("anthropic"));
        // Written again, it is already so.
        assert_eq!(
            reaching(&written, FILE, "anthropic", &access).unwrap(),
            written
        );
    }
}

#[test]
fn reaching_a_provider_into_a_file_it_cannot_be_written_into_writes_nothing() {
    use crate::settings::Access;

    assert!(matches!(
        reaching("{", FILE, "anthropic", &Access::Subscription),
        Err(ConfigError::Malformed { .. })
    ));
    assert!(matches!(
        reaching("[]", FILE, "anthropic", &Access::Subscription),
        Err(ConfigError::Unspliceable { .. })
    ));
}

#[test]
fn connecting_an_agent_the_file_does_not_name_is_refused_and_writes_nothing() {
    for text in [
        "",
        "{}",
        r#"{"externalAgents": {}}"#,
        r#"{"externalAgents": {"home": {"driver": "claude", "executable": "claude"}}}"#,
    ] {
        assert!(
            matches!(
                connecting(text, FILE, "anthropic", "work"),
                Err(ConfigError::Unspliceable { .. })
            ),
            "{text}"
        );
    }
    assert!(matches!(
        connecting("{", FILE, "anthropic", "work"),
        Err(ConfigError::Malformed { .. })
    ));
}

#[test]
fn installing_an_agent_writes_its_block_the_provider_s_access_and_enables_it_last() {
    let profile = if cfg!(windows) {
        r"C:\Users\me\.crucible\agents\antigravity"
    } else {
        "/home/me/.crucible/agents/antigravity"
    };
    for (text, driver, executable, profile) in [
        ("", Driver::Claude, "claude", None),
        (
            "{\n  \"provider\": \"anthropic\"\n}\n",
            Driver::Claude,
            "claude",
            None,
        ),
        (
            r#"{"externalAgents": {"home": {"driver": "claude", "executable": "claude"}}}"#,
            Driver::Antigravity,
            "agy_acp_server.par",
            Some(profile),
        ),
    ] {
        let name = driver.name();
        let written = installing(
            text,
            FILE,
            "anthropic",
            &ExternalAgent::unwritten(name, driver, executable, profile, None)
                .expect("an instance of a built-in driver"),
        )
        .unwrap_or_else(|problem| panic!("{text}: {problem}"));
        let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
        assert_eq!(
            settings.access("anthropic"),
            Some(crate::settings::Access::ExternalAgent {
                instance: name.into()
            }),
            "{written}"
        );
        let installed = settings
            .external_agents()
            .into_iter()
            .find(|agent| agent.name() == name)
            .map(|agent| {
                (
                    agent.enabled(),
                    agent.driver(),
                    agent.executable().to_owned(),
                    agent.profile().map(str::to_owned),
                )
            });
        assert_eq!(
            installed,
            Some((
                true,
                driver,
                executable.to_owned(),
                profile.map(str::to_owned)
            )),
            "{written}"
        );
        // Everything the file held before is still held.
        if let Ok(before) = serde_json::from_str::<Value>(text) {
            let after: Value = serde_json::from_str(&written).unwrap_or_default();
            for (key, value) in before.as_object().into_iter().flatten() {
                if key != "externalAgents" {
                    assert_eq!(after.get(key), Some(value), "{written}");
                }
            }
        }
    }
}

#[test]
fn installing_an_agent_the_file_already_names_is_refused() {
    let text =
        r#"{"externalAgents": {"claude": {"driver": "claude", "executable": "claude-work"}}}"#;
    assert!(matches!(
        installing(
            text,
            FILE,
            "anthropic",
            &ExternalAgent::unwritten("claude", Driver::Claude, "claude", None, None)
                .expect("an instance of a built-in driver"),
        ),
        Err(ConfigError::Unspliceable { .. })
    ));
}

/// A catalog instance is written with the agent it runs beside its driver,
/// and its profile directory under the key its driver reads, so the file
/// read back names the same agent.
#[test]
fn installing_a_catalog_agent_writes_the_agent_it_runs() {
    let profile = crate::sample::rooted("home/me/.crucible/agents/goose");
    let executable = crate::sample::rooted("opt/goose");
    let goose = AgentKey::new("goose").expect("an agent key");
    for profile in [None, Some(profile.as_str())] {
        let unwritten =
            ExternalAgent::unwritten("goose", Driver::Catalog, &executable, profile, Some(&goose))
                .expect("a catalog instance naming its agent");

        let written = installing("", FILE, "anthropic", &unwritten)
            .unwrap_or_else(|problem| panic!("{profile:?}: {problem}"));

        let block: Value = serde_json::from_str(&written).unwrap_or_default();
        assert_eq!(
            block.pointer("/externalAgents/goose/agent"),
            Some(&Value::from("goose")),
            "{written}"
        );
        let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
        let read = settings
            .external_agents()
            .into_iter()
            .find(|agent| agent.name() == "goose")
            .map(|agent| {
                (
                    agent.driver(),
                    agent.serves(),
                    agent.profile().map(str::to_owned),
                )
            });
        assert_eq!(
            read,
            Some((
                Driver::Catalog,
                Serves::Agent(goose.clone()),
                profile.map(str::to_owned)
            )),
            "{written}"
        );
    }
}

#[test]
fn removing_an_agent_cuts_out_its_block_and_nothing_else() {
    let text = "{\n  \"provider\": \"anthropic\",\n  \"externalAgents\": {\n    \"work\": {\n      \"driver\": \"claude\",\n      \"executable\": \"claude\"\n    },\n    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\"}\n  }\n}\n";
    let written = removing(text, FILE, "work").unwrap();
    assert_eq!(
        written,
        "{\n  \"provider\": \"anthropic\",\n  \"externalAgents\": {\n    \n    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\"}\n  }\n}\n"
    );
    let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
    let names: Vec<String> = settings
        .external_agents()
        .iter()
        .map(|agent| agent.name().to_owned())
        .collect();
    assert_eq!(names, ["home"]);

    // The only one leaves the object empty.
    let only = r#"{"externalAgents": {"work": {"driver": "claude", "executable": "claude"}}}"#;
    let emptied = removing(only, FILE, "work").unwrap();
    assert_eq!(emptied, r#"{"externalAgents": {}}"#);

    // The last one goes with the comma before it.
    let last = removing(text, FILE, "home").unwrap();
    assert!(!last.contains("home"), "{last}");
    assert!(last.contains("\"work\""), "{last}");
    Settings::resolve(vec![Document::sample(&last, Origin::User)]);
}

#[test]
fn removing_an_agent_a_provider_is_reached_through_is_refused_and_writes_nothing() {
    let text = r#"{"providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "work"}}},
        "externalAgents": {"work": {"driver": "claude", "executable": "claude"}}}"#;
    match removing(text, FILE, "work") {
        Err(ConfigError::StillReferenced {
            instance, provider, ..
        }) => assert_eq!((&*instance, &*provider), ("work", "anthropic")),
        other => panic!("{other:?}"),
    }
    // Reached through another instance, it does not hold this one.
    let elsewhere = text.replace(r#""instance": "work""#, r#""instance": "home""#);
    assert!(removing(&elsewhere, FILE, "work").is_ok());
}

#[test]
fn removing_an_agent_the_file_does_not_name_is_refused() {
    for text in [
        "",
        "{}",
        r#"{"externalAgents": {}}"#,
        r#"{"externalAgents": {"home": {"driver": "claude", "executable": "claude"}}}"#,
    ] {
        assert!(
            matches!(
                removing(text, FILE, "work"),
                Err(ConfigError::Unremovable { .. })
            ),
            "{text}"
        );
    }
}

#[test]
fn choosing_an_agents_model_writes_its_model_and_nothing_else() {
    let text = "{\n  \"externalAgents\": {\n    \"work\": {\n      \"driver\": \"claude\",\n      \"executable\": \"claude\"\n    },\n    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\", \"model\": \"haiku\"}\n  }\n}\n";
    let written = choosing_agent_model(text, FILE, "work", Some("opus")).unwrap();
    assert_eq!(
        written,
        text.replace(
            "\"executable\": \"claude\"\n    },",
            "\"executable\": \"claude\",\n      \"model\": \"opus\"\n    },"
        )
    );
    let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
    let mut models: Vec<(String, Option<String>)> = settings
        .external_agents()
        .iter()
        .map(|agent| (agent.name().to_owned(), agent.model().map(Into::into)))
        .collect();
    models.sort();
    assert_eq!(
        models,
        [
            ("home".to_owned(), Some("haiku".to_owned())),
            ("work".to_owned(), Some("opus".to_owned()))
        ]
    );
    // Written again, it is already so.
    assert_eq!(
        choosing_agent_model(&written, FILE, "work", Some("opus")).unwrap(),
        written
    );
}

#[test]
fn choosing_an_agents_model_writes_over_the_one_named_or_takes_it_out() {
    let text = r#"{"externalAgents": {"work": {"driver": "claude", "model": "haiku", "executable": "claude"}}}"#;
    assert_eq!(
        choosing_agent_model(text, FILE, "work", Some("claude-opus-5-5[1m]")).unwrap(),
        text.replace("\"haiku\"", "\"claude-opus-5-5[1m]\"")
    );
    // Left to the agent, the entry names no model at all. The space after the
    // comma is a neighbour's, and stays.
    assert_eq!(
        choosing_agent_model(text, FILE, "work", None).unwrap(),
        r#"{"externalAgents": {"work": {"driver": "claude",  "executable": "claude"}}}"#
    );
    let unnamed = r#"{"externalAgents": {"work": {"driver": "claude", "executable": "claude"}}}"#;
    assert_eq!(
        choosing_agent_model(unnamed, FILE, "work", None).unwrap(),
        unnamed
    );
}

#[test]
fn an_agents_model_its_program_could_not_take_is_not_written() {
    let text = r#"{"externalAgents": {"work": {"driver": "claude", "executable": "claude"}}}"#;
    // Read as an option by the program it is handed to.
    assert!(
        choosing_agent_model(text, FILE, "work", Some("--dangerously-skip-permissions")).is_err()
    );
    assert!(choosing_agent_model(text, FILE, "work", Some("a \"quoted\" name")).is_err());
    // An entry the file does not hold is not made up.
    assert!(matches!(
        choosing_agent_model(text, FILE, "home", Some("opus")),
        Err(ConfigError::Unspliceable { .. })
    ));
}

#[test]
fn an_agents_effort_is_written_beside_its_model_or_taken_out() {
    let text = r#"{"externalAgents": {"work": {"driver": "claude", "executable": "claude", "model": "claude-opus-5-5"}}}"#;
    let written = thinking_agent(text, FILE, "work", Some(Effort::High)).unwrap();
    let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
    let efforts: Vec<_> = settings
        .external_agents()
        .iter()
        .map(|agent| (agent.model().map(str::to_owned), agent.effort()))
        .collect();
    assert_eq!(
        efforts,
        [(Some("claude-opus-5-5".to_owned()), Some(Effort::High))]
    );
    assert_eq!(thinking_agent(&written, FILE, "work", None).unwrap(), text);
    // An agent that chooses its own model takes no effort either.
    let chooses = r#"{"externalAgents": {"work": {"driver": "antigravity", "executable": "agy", "profileDir": "/home/me/agy"}}}"#;
    assert!(thinking_agent(chooses, FILE, "work", Some(Effort::High)).is_err());
}

#[test]
fn pointing_instances_at_a_new_program_writes_each_executable_and_nothing_else() {
    let text = "{\n  \"externalAgents\": {\n    \"work\": {\n      \"driver\": \"catalog\",\n      \"agent\": \"goose\",\n      \"executable\": \"/h/programs/goose/1.0.0/goose\"\n    },\n    \"home\": {\"driver\": \"catalog\", \"agent\": \"goose\", \"executable\": \"/h/programs/goose/1.0.0/goose\"},\n    \"mine\": {\"driver\": \"catalog\", \"agent\": \"goose\", \"executable\": \"/usr/bin/goose\"}\n  }\n}\n";
    let was = "/h/programs/goose/1.0.0/goose";
    let to = "/h/programs/goose/2.0.0/goose";

    let (written, left) = pointing(text, FILE, &[("work", was, to), ("home", was, to)]).unwrap();

    assert!(left.is_empty(), "{left:?}");

    assert_eq!(
        written,
        text.replacen("/h/programs/goose/1.0.0/goose", to, 2)
    );
    let settings = Settings::resolve(vec![Document::sample(&written, Origin::User)]);
    let mut executables: Vec<(String, String)> = settings
        .external_agents()
        .iter()
        .map(|agent| (agent.name().to_owned(), agent.executable().to_owned()))
        .collect();
    executables.sort();
    assert_eq!(
        executables,
        [
            ("home".to_owned(), to.to_owned()),
            ("mine".to_owned(), "/usr/bin/goose".to_owned()),
            ("work".to_owned(), to.to_owned()),
        ]
    );
    // Written again, it is already so.
    assert_eq!(
        pointing(&written, FILE, &[("work", was, to)]).unwrap(),
        (written.clone(), Vec::new())
    );
    // A path is written as JSON spells it.
    let (windows, _) = pointing(text, FILE, &[("work", was, "C:\\h\\goose \"2\".exe")]).unwrap();
    assert!(
        windows.contains(r#""executable": "C:\\h\\goose \"2\".exe""#),
        "{windows}"
    );
}

#[test]
fn pointing_an_instance_the_file_does_not_name_is_refused_and_writes_nothing() {
    let text =
        "{\"externalAgents\": {\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}}}";

    let refused = pointing(
        text,
        FILE,
        &[("work", "claude", "/a"), ("gone", "/b", "/c")],
    );

    assert!(
        matches!(refused, Err(ConfigError::Unspliceable { .. })),
        "{refused:?}"
    );
}

#[test]
fn pointing_leaves_an_instance_whose_program_changed_since_it_was_shown_and_names_it() {
    let text = "{\"externalAgents\": {\"work\": {\"driver\": \"catalog\", \"agent\": \"goose\", \"executable\": \"/usr/local/bin/goose\"}, \"home\": {\"driver\": \"catalog\", \"agent\": \"goose\", \"executable\": \"/h/1/goose\"}}}";

    let (written, left) = pointing(
        text,
        FILE,
        &[
            ("work", "/h/1/goose", "/h/2/goose"),
            ("home", "/h/1/goose", "/h/2/goose"),
        ],
    )
    .unwrap();

    assert_eq!(left, ["work".to_owned()]);
    assert_eq!(written, text.replace("\"/h/1/goose\"", "\"/h/2/goose\""));
    assert!(written.contains("/usr/local/bin/goose"));
}
