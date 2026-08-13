//! What a refusal has to say, proved one document at a time.
//!
//! Every test here reads a whole document rather than calling into the walk,
//! because what is being checked is the sentence somebody with the file open
//! gets back — which is a property of the message, not of the traversal.

use crate::document::Document;

use super::*;

/// Reads a document as the layer that travels with a clone.
fn shared(text: &str) -> Result<Document, ConfigError> {
    Document::parse(text, ".crucible/config.json", Origin::Project)
}

/// Reads a document as the layer git ignores.
fn local(text: &str) -> Result<Document, ConfigError> {
    Document::parse(text, ".crucible/config.local.json", Origin::ProjectLocal)
}

/// Reads a document as the file in the user's home directory.
fn mine(text: &str) -> Result<Document, ConfigError> {
    Document::parse(text, "~/.crucible/config.json", Origin::User)
}

#[test]
fn a_variable_read_before_any_file_is_opened_is_refused_in_every_layer() {
    // It is in crucible's own namespace, so nothing above refuses it — and
    // it is read to find this very file, so a value written here would be
    // accepted, merged, and then never applied. Silently doing nothing is
    // the one outcome a hand-edited file must not have.
    for read in [
        mine as fn(&str) -> Result<Document, ConfigError>,
        local,
        shared,
    ] {
        let err = read(&format!(
            r#"{{"env": {{"{}": "/srv/crucible"}}}}"#,
            crate::HOME
        ))
        .unwrap_err();

        let said = err.to_string();
        assert!(matches!(err, ConfigError::TooLate { .. }), "got {err:?}");
        assert!(said.contains(crate::HOME), "got {said}");
    }
}

#[test]
fn one_of_crucibles_own_settings_is_refused_where_it_was_written() {
    // Its shape is right — every entry in `env` is a string — so the walk has
    // nothing to say about it, and by the time the layers have merged the file
    // it came from is gone. Here is the only place a refusal can still name
    // both, which is what makes it worth refusing rather than defaulting.
    for read in [
        mine as fn(&str) -> Result<Document, ConfigError>,
        local,
        shared,
    ] {
        let err = read(r#"{"env": {"CRUCIBLE_CODE_CLEAR_SCREEN": "sometimes"}}"#).unwrap_err();

        let said = err.to_string();
        assert!(matches!(err, ConfigError::Answer { .. }), "got {err:?}");
        assert!(said.contains("CRUCIBLE_CODE_CLEAR_SCREEN"), "got {said}");
        assert!(said.contains("line 1"), "got {said}");
        assert!(said.contains("true"), "got {said}");

        // The name and where it is, never what was set beside it. This block is
        // the environment, so the next value to go wrong could be a token.
        assert!(!said.contains("sometimes"), "got {said}");
    }
}

#[test]
fn an_answer_crucible_takes_passes_in_every_layer() {
    for read in [
        mine as fn(&str) -> Result<Document, ConfigError>,
        local,
        shared,
    ] {
        read(r#"{"env": {"CRUCIBLE_CODE_CLEAR_SCREEN": "true"}}"#).unwrap();
    }
}

#[test]
fn a_setting_that_wants_a_string_refuses_a_number() {
    let err = shared(r#"{"output": {"color": 1}}"#).unwrap_err();
    let said = err.to_string();
    assert!(matches!(err, ConfigError::WrongType { .. }), "got {err:?}");
    assert!(said.contains("output.color"), "got {said}");
}

#[test]
fn a_choice_names_what_it_accepts_rather_than_only_refusing() {
    let err = shared(r#"{"output": {"color": "beige"}}"#).unwrap_err();

    // Someone who wrote "beige" does not know the set. Listing it is both
    // the shortest thing to compute and more use than one guess at what
    // they meant.
    let said = err.to_string();
    assert!(matches!(err, ConfigError::NotAChoice { .. }), "got {err:?}");
    assert!(said.contains("auto"), "got {said}");
    assert!(said.contains("always"), "got {said}");
    assert!(said.contains("never"), "got {said}");
}

#[test]
fn a_refusal_introduces_the_list_it_carries_exactly_once() {
    // The list renders its own lead-in, because an unknown key and a wrong
    // answer both end with it. A message that adds a second one reads as
    // though the sentence was assembled rather than written.
    let said = shared(r#"{"output": {"color": "beige"}}"#)
        .unwrap_err()
        .to_string();

    assert_eq!(said.matches("accepted").count(), 1, "got {said}");
}

#[test]
fn a_file_that_is_not_json_gives_the_position_once() {
    // The parser puts the position in its own sentence, and crucible has
    // already said it in words of its own. Two of them, disagreeing about
    // punctuation, is the reader's first clue that nobody read this message.
    let said = shared("{\n  \"output\": {,\n}").unwrap_err().to_string();

    assert_eq!(said.matches("line").count(), 1, "got {said}");
}

#[test]
fn a_key_the_user_chose_is_not_checked_against_a_list() {
    // `providers` and `env` are keyed by names crucible cannot know. Only
    // the values inside them have a shape.
    local(r#"{"providers": {"anthropic": {"model": "claude-sonnet-5"}}}"#).unwrap();
    mine(r#"{"env": {"RUST_LOG": "warn"}}"#).unwrap();
}

#[test]
fn a_wrong_value_inside_a_user_named_key_still_names_its_full_path() {
    let err = shared(r#"{"providers": {"openai": {"model": []}}}"#).unwrap_err();
    let said = err.to_string();
    assert!(said.contains("providers.openai.model"), "got {said}");
}

#[test]
fn the_schema_keys_json_reserves_are_carried_rather_than_refused() {
    // `$schema` is what makes an editor complete this file at all, and
    // `$comment` is the standard's answer to JSON having no comments. A
    // document that could not hold them would lose the reason the format
    // was chosen.
    local(
        r#"{
             "$schema": "https://example.invalid/crucible-code-schema.json",
             "$comment": "0.0.x is unstable",
             "output": {"$comment": "dim the prompt", "color": "never"}
           }"#,
    )
    .unwrap();
}

#[test]
fn someone_elses_variable_is_refused_in_every_file_under_the_working_directory() {
    // Both project files, because a repository chooses what it commits and
    // crucible reads whatever is at either name. The one git is meant to
    // ignore is ignored by a rule written in the repository being cloned, so
    // trusting it is trusting the thing being defended against.
    for read in [shared as fn(&str) -> Result<Document, ConfigError>, local] {
        let err = read(r#"{"env": {"TOKEN": "hunter2"}}"#).unwrap_err();

        // The refusal has to say where to put it instead, or the next move is
        // to delete the setting rather than to move it. The two places left,
        // and neither of them is the other project file — that one refuses it
        // too, and being sent there would be being sent in a circle.
        let said = err.to_string();
        assert!(matches!(err, ConfigError::ProjectEnv { .. }), "got {err:?}");
        assert!(said.contains("home directory"), "got {said}");
        assert!(said.contains("shell"), "got {said}");

        // And it must not quote what it refused. The whole point of refusing is
        // that the value might be a secret, and an error string is one of the
        // places this workspace never writes one.
        assert!(!said.contains("hunter2"), "got {said}");
    }
}

#[test]
fn a_committed_file_cannot_hand_every_command_a_program_of_its_own() {
    // The finding this refusal was widened for. `.crucible/config.local.json`
    // is git-ignored by convention and a repository can simply commit one;
    // a `PATH` in it is every command crucible runs, silently, with no rule
    // fired and nothing on screen. `deny` cannot help, because the command the
    // user asked for is still spelled the way they asked for it.
    for name in ["PATH", "LD_PRELOAD", "BASH_ENV"] {
        let said = local(&format!(r#"{{"env": {{"{name}": "/tmp/theirs"}}}}"#))
            .unwrap_err()
            .to_string();

        assert!(said.contains(name), "got {said}");
        assert!(said.contains("line 1"), "got {said}");
    }
}

#[test]
fn crucibles_own_setting_is_allowed_even_in_the_file_that_travels() {
    // The namespace is what makes this safe to check in. A name crucible
    // owns is a knob crucible declares — it is read by this program and
    // means what this program says it means, so a project can set one for
    // everybody who clones it without that being a way to ship a secret.
    // An arbitrary name is where a key would hide, and only those are
    // refused above.
    shared(r#"{"env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "12"}}"#).unwrap();
}

#[test]
fn env_takes_anybodys_variable_in_the_file_that_came_with_the_person() {
    // The home directory is the one layer that did not arrive with a checkout,
    // so it is the one place an arbitrary variable can still be written. That
    // is what the refusal above costs somebody who wanted one project only.
    mine(r#"{"env": {"RUST_LOG": "warn", "PAGER": "cat"}}"#).unwrap();
}

#[test]
fn a_checked_in_file_cannot_hand_itself_authority_over_everyone_who_clones() {
    // The finding this refusal exists for. Nothing is put to the user: the
    // mode is read before the first turn, and a repository that committed
    // these three lines would start every clone with every call approved.
    for (written, path) in [
        (r#"{"permissions": {"mode": "fullAccess"}}"#, "mode"),
        (r#"{"permissions": {"allow": ["bash(*)"]}}"#, "allow"),
        (
            r#"{"permissions": {"extraDirectories": ["/"]}}"#,
            "extraDirectories",
        ),
    ] {
        let err = shared(written).unwrap_err();

        let said = err.to_string();
        assert!(matches!(err, ConfigError::Widening { .. }), "got {err:?}");
        assert!(said.contains(&format!("permissions.{path}")), "got {said}");
        assert!(said.contains("line 1"), "got {said}");

        // Where it does work, or the next move is to delete the line rather
        // than to move it.
        assert!(said.contains("config.local.json"), "got {said}");
    }
}

#[test]
fn a_checked_in_file_can_still_tighten_its_own_rules() {
    // The case the layering exists for, and the half that must keep working: a
    // repository saying what nobody working in it should be allowed to do,
    // for everybody who clones it. `deny` and `ask` only ever put more in
    // front of the user, so neither is authority a file can hand itself.
    shared(r#"{"permissions": {"deny": ["read(.env)"], "ask": ["bash(git push)"]}}"#).unwrap();
}

#[test]
fn a_widening_key_is_read_from_the_two_files_that_did_not_travel() {
    // The refusal is about the one file git carries by design, not about the
    // keys. crucible writes an `allow` the user answered `always` to into
    // `.crucible/config.local.json`, so refusing one there would be crucible
    // writing a file it then refuses to open.
    for read in [mine as fn(&str) -> Result<Document, ConfigError>, local] {
        read(r#"{"permissions": {"mode": "fullAccess", "allow": ["bash(cargo test)"]}}"#).unwrap();
    }
}

#[test]
fn a_key_refused_by_its_layer_is_refused_before_its_value_is_read() {
    // Where the key is written is what is wrong, and that is true whatever it
    // was set to. Told the value is not an answer, a reader would fix the
    // value and meet the real refusal on the next run.
    let err = shared(r#"{"permissions": {"mode": "beige"}}"#).unwrap_err();
    assert!(matches!(err, ConfigError::Widening { .. }), "got {err:?}");
}

#[test]
fn a_dollar_key_the_standard_does_not_reserve_is_still_an_unknown_key() {
    // Two reserved names, not any name beginning with a dollar. The schema
    // generated from the shape names exactly these two, so accepting more
    // here would let through a document the reader's editor marks red —
    // and would swallow `$schemas` as a typo nobody is ever told about.
    let err = local(r#"{"$schemas": "x"}"#).unwrap_err();
    assert!(matches!(err, ConfigError::UnknownKey { .. }), "got {err:?}");
}

#[test]
fn a_refusal_names_which_of_the_files_it_came_from() {
    // Three layers can all hold the same key, so a position on its own sends
    // the reader to line 3 of whichever one they happened to open. The name is
    // what makes the rest of the sentence actionable.
    for (read, named) in [
        (
            mine as fn(&str) -> Result<Document, ConfigError>,
            "~/.crucible/config.json",
        ),
        (shared, ".crucible/config.json"),
        (local, ".crucible/config.local.json"),
    ] {
        let said = read(r#"{"output": {"color": "beige"}}"#)
            .unwrap_err()
            .to_string();

        assert!(said.contains(named), "got {said}");
    }
}

#[test]
fn a_refusal_points_at_the_line_the_key_is_on() {
    let err = shared("{\n  \"output\": {\n    \"colour\": \"never\"\n  }\n}").unwrap_err();
    let said = err.to_string();
    assert!(said.contains("line 3"), "got {said}");
}

#[test]
fn a_key_that_appears_twice_is_reported_without_a_position() {
    // Two providers both setting `model` means two places the token is
    // found, and naming one of them sends the reader to a line that is
    // correct. No position is better than the wrong position.
    let err = shared(
        r#"{"providers": {"a": {"model": "x", "nope": 1}, "b": {"model": "y", "nope": 2}}}"#,
    )
    .unwrap_err();
    let said = err.to_string();
    assert!(matches!(err, ConfigError::UnknownKey { .. }), "got {err:?}");
    assert!(!said.contains("line"), "got {said}");
}

#[test]
fn a_file_that_is_not_json_says_where_it_stopped_being_json() {
    let err = shared("{\n  \"output\": {,\n}").unwrap_err();
    let said = err.to_string();
    assert!(matches!(err, ConfigError::Malformed { .. }), "got {err:?}");
    assert!(said.contains("line 2"), "got {said}");
}
