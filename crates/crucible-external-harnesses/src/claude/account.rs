//! What the agent's account commands report.
//!
//! Each account step is a command of its own, run in the same environment a
//! turn is and read once it has ended. Whether the profile is signed in is the
//! agent's own `auth status`, read only where what it printed and how it
//! exited agree; signing out is its own `auth logout`. Signing in is
//! interactive in the agent's own terminal and is never driven from here: the
//! user is shown the command that signs the profile in, runs it, and checks
//! again. Which models it offers is the one step spoken over its stream: it is
//! asked to `initialize`, and the model rows of its answer are read, its
//! default as the agent's own choice.

use std::path::Path;

use crucible_execution::{Authentication, Failure, Text};
use serde_json::{Value, json};

use super::Manifest;
use crate::{Account, OFFERED, Offered, Ran, Reading, Told};

/// The id the models step's `initialize` is sent under.
const INITIALIZE: &str = "crucible-models";

/// The value the agent names its own default by, which it is launched
/// without naming.
const DEFAULT: &str = "default";

/// The arguments the models step starts the agent's stream with: no turn, no
/// permission mode and no model, since it is only asked what it offers.
pub(crate) const STREAM: [&str; 6] = [
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
];

/// The arguments the step `step` runs with, where the agent has a command
/// for it.
pub(crate) fn arguments(step: Account) -> Option<&'static [&'static str]> {
    match step {
        Account::Version => Some(&["--version"]),
        Account::Status => Some(&["auth", "status"]),
        Account::SignOut => Some(&["auth", "logout"]),
        Account::SignIn | Account::Models => None,
    }
}

/// The frame that opens the models step.
pub(crate) fn initialize() -> String {
    json!({
        "type": "control_request",
        "request_id": INITIALIZE,
        "request": {"subtype": "initialize"},
    })
    .to_string()
}

/// The text at `key` in `value`, where it is text.
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// What `frame`, heard during the models step, means: the step's end where it
/// answers the step's `initialize`, else nothing it waits for.
///
/// # Errors
///
/// A [`Failure`] where `frame` is not JSON, which no line of the stream is.
pub(crate) fn told(frame: &str) -> Result<Told, Failure> {
    let frame: Value = serde_json::from_str(frame)
        .map_err(|_| Failure::new("The agent wrote a line that is not of its stream."))?;
    let response = frame.get("response").unwrap_or(&Value::Null);
    if field(&frame, "type") != Some("control_response")
        || field(response, "request_id") != Some(INITIALIZE)
    {
        return Ok(Told::Nothing);
    }
    if field(response, "subtype") != Some("success") {
        return Ok(Told::Done(Reading::Unreadable(Failure::new(
            "The agent refused to say which models it offers.",
        ))));
    }
    Ok(Told::Done(
        offered(response.pointer("/response/models").unwrap_or(&Value::Null)).map_or_else(
            || {
                Reading::Unreadable(Failure::new(
                    "The agent's list of models could not be read.",
                ))
            },
            Reading::Models,
        ),
    ))
}

/// The rows of `models`, where each names its value and what it is shown as
/// and there are at most [`OFFERED`].
fn offered(models: &Value) -> Option<Vec<Offered>> {
    let rows = models.as_array().filter(|rows| rows.len() <= OFFERED)?;
    rows.iter()
        .map(|row| {
            let value = row.get("value")?.as_str()?;
            let shown = row.get("displayName")?.as_str()?;
            Some(Offered {
                model: (value != DEFAULT).then(|| Text::new(value)),
                shown: Text::new(shown),
            })
        })
        .collect()
}

/// What the step `step`'s run `ran` reported.
pub(crate) fn read(step: Account, ran: &Ran) -> Reading {
    if ran.cut {
        return Reading::Unreadable(Failure::new(
            "The agent printed more than its account steps print.",
        ));
    }
    match step {
        Account::Version => match ran.said.first() {
            Some(said) if ran.code == Some(0) => match Manifest::REVIEWED.admits(said) {
                Ok(version) => Reading::Version(version),
                Err(failure) => Reading::Unreadable(failure),
            },
            _ => Reading::Unreadable(Failure::new("The agent did not say what version it is.")),
        },
        Account::Status => status(ran),
        Account::SignOut if ran.code == Some(0) => Reading::Signed(Authentication::SignedOut),
        Account::SignOut => {
            Reading::Unreadable(Failure::new("The agent's sign-out did not succeed."))
        }
        Account::SignIn => Reading::Unreadable(Failure::new(
            "The agent signs in in its own terminal, not from here.",
        )),
        Account::Models => Reading::Unreadable(Failure::new(
            "The agent's models are read over its stream, not from a command.",
        )),
    }
}

/// What `auth status` reported: signed in where it printed so and exited
/// zero, signed out where it printed so and exited one. Anything else is
/// unread, saying how the agent exited and the first thing it said, which the
/// run has already stripped of terminal control and withheld values.
fn status(ran: &Ran) -> Reading {
    let printed: Option<Value> = serde_json::from_str(&ran.said.join("\n")).ok();
    let signed = printed
        .as_ref()
        .and_then(|printed| printed.get("loggedIn"))
        .and_then(Value::as_bool);
    match (signed, ran.code) {
        (Some(true), Some(0)) => Reading::Signed(Authentication::SignedIn),
        (Some(false), Some(1)) => Reading::Signed(Authentication::SignedOut),
        _ => Reading::Unreadable(Failure::new(&unread(ran))),
    }
}

/// Why a status run that said nothing readable was not read.
fn unread(ran: &Ran) -> String {
    let first = |lines: &mut dyn Iterator<Item = &str>| {
        lines
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    };
    let said = first(&mut ran.muttered.lines())
        .or_else(|| first(&mut ran.said.iter().map(String::as_str)));
    match (ran.code, said) {
        (Some(code), Some(said)) if code != 0 => {
            format!("The agent exited with status {code}: {said}")
        }
        (Some(code), None) if code != 0 => format!("The agent exited with status {code}."),
        _ => "The agent's sign-in status could not be read.".to_owned(),
    }
}

/// The command the user runs to sign the profile in `profile` in, where the
/// instance has one, else the agent's usual configuration.
#[must_use]
pub fn sign_in(profile: Option<&Path>) -> Text {
    let Some(profile) = profile else {
        return Text::new("claude auth login");
    };
    let profile = profile.display().to_string();
    if cfg!(windows) {
        Text::new(&format!(
            "$env:CLAUDE_CONFIG_DIR={}; claude auth login",
            quoted_for_powershell(&profile)
        ))
    } else {
        Text::new(&format!(
            "CLAUDE_CONFIG_DIR='{}' claude auth login",
            profile.replace('\'', r"'\''")
        ))
    }
}

/// `text` as a single-quoted PowerShell string: every character PowerShell
/// reads as a single quote, the typographic ones too, is doubled.
pub(crate) fn quoted_for_powershell(text: &str) -> String {
    let mut quoted = String::from("'");
    for character in text.chars() {
        if matches!(
            character,
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}'
        ) {
            quoted.push(character);
        }
        quoted.push(character);
    }
    quoted.push('\'');
    quoted
}
