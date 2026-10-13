//! What the agent's account commands report.
//!
//! Each account step is a command of its own, run in the same environment a
//! turn is and read once it has ended. Whether the profile is signed in is the
//! agent's own `auth status`, read only where what it printed and how it
//! exited agree; signing out is its own `auth logout`. Signing in is
//! interactive in the agent's own terminal and is never driven from here: the
//! user is shown the command that signs the profile in, runs it, and checks
//! again.

use std::path::Path;

use crucible_execution::{Authentication, Failure, Text};
use serde_json::Value;

use super::Manifest;
use crate::{Account, Ran, Reading};

/// The arguments the step `step` runs with, where the agent has a command
/// for it.
pub(crate) fn arguments(step: Account) -> Option<&'static [&'static str]> {
    match step {
        Account::Version => Some(&["--version"]),
        Account::Status => Some(&["auth", "status"]),
        Account::SignOut => Some(&["auth", "logout"]),
        Account::SignIn => None,
    }
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
    }
}

/// What `auth status` reported: signed in where it printed so and exited
/// zero, signed out where it printed so and exited one.
fn status(ran: &Ran) -> Reading {
    let printed: Option<Value> = serde_json::from_str(&ran.said.join("\n")).ok();
    let signed = printed
        .as_ref()
        .and_then(|printed| printed.get("loggedIn"))
        .and_then(Value::as_bool);
    match (signed, ran.code) {
        (Some(true), Some(0)) => Reading::Signed(Authentication::SignedIn),
        (Some(false), Some(1)) => Reading::Signed(Authentication::SignedOut),
        _ => Reading::Unreadable(Failure::new(
            "The agent's sign-in status could not be read.",
        )),
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
