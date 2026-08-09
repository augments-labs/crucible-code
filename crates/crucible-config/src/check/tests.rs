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
    local(r#"{"env": {"RUST_LOG": "warn"}}"#).unwrap();
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
fn someone_elses_variable_is_refused_in_the_file_that_travels_with_a_clone() {
    let err = shared(r#"{"env": {"TOKEN": "hunter2"}}"#).unwrap_err();

    // The refusal has to say where to put it instead, or the next move is
    // to delete the setting rather than to move it.
    let said = err.to_string();
    assert!(
        matches!(err, ConfigError::SecretLayer { .. }),
        "got {err:?}"
    );
    assert!(said.contains("config.local.json"), "got {said}");

    // And it must not quote what it refused. The whole point of refusing is
    // that the value might be a secret; echoing it into an error string
    // would put it exactly where G3 says it may never go.
    assert!(!said.contains("hunter2"), "got {said}");
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
fn env_takes_anybodys_variable_in_the_layers_that_do_not_travel() {
    local(r#"{"env": {"RUST_LOG": "warn", "PAGER": "cat"}}"#).unwrap();
    Document::parse(
        r#"{"env": {"RUST_LOG": "warn"}}"#,
        "~/.crucible/config.json",
        Origin::User,
    )
    .unwrap();
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
