//! Each account step reads only what the agent's own command established.

use std::path::Path;

use crucible_execution::Authentication;

use crate::claude::{Claude, sign_in};
use crate::{Account, Dialect, Ran, Reading, Shape};

/// A run that exited with `code` having printed `said`.
fn ran(code: Option<i32>, said: &[&str]) -> Ran {
    Ran {
        code,
        said: said.iter().map(|line| (*line).to_owned()).collect(),
        cut: false,
        muttered: String::new(),
    }
}

#[test]
fn the_steps_run_the_agents_own_commands() {
    let mut agent = Claude::default();
    let arguments = |agent: &mut Claude, step| {
        agent
            .account(step, Shape::default())
            .map(|request| request.arguments)
    };

    assert_eq!(
        arguments(&mut agent, Account::Version),
        Some(vec!["--version".to_owned()])
    );
    assert_eq!(
        arguments(&mut agent, Account::Status),
        Some(vec!["auth".to_owned(), "status".to_owned()])
    );
    assert_eq!(
        arguments(&mut agent, Account::SignOut),
        Some(vec!["auth".to_owned(), "logout".to_owned()])
    );
    assert_eq!(arguments(&mut agent, Account::SignIn), None);
}

/// A step runs in the profile, as a turn does.
#[test]
fn a_step_runs_in_the_profile() {
    let request = Claude::default()
        .account(
            Account::Status,
            Shape {
                profile: Some(Path::new("/profiles/work")),
                ..Shape::default()
            },
        )
        .unwrap();

    assert!(
        request
            .environment
            .contains(&("CLAUDE_CONFIG_DIR".to_owned(), "/profiles/work".to_owned()))
    );
}

/// The status is read only where what it printed and how it exited agree.
#[test]
fn the_status_is_read_only_where_its_words_and_its_exit_agree() {
    let agent = Claude::default();
    let read = |code, said: &[&str]| agent.read(Account::Status, &ran(code, said));

    assert_eq!(
        read(
            Some(0),
            &[r#"{"loggedIn": true, "authMethod": "claude.ai"}"#]
        ),
        Reading::Signed(Authentication::SignedIn)
    );
    assert_eq!(
        read(Some(1), &["{", r#"  "loggedIn": false"#, "}"]),
        Reading::Signed(Authentication::SignedOut)
    );
    for (code, said) in [
        (Some(1), r#"{"loggedIn": true}"#),
        (Some(0), r#"{"loggedIn": false}"#),
        (Some(0), "Logged in"),
        (Some(0), r#"{"loggedIn": "yes"}"#),
        (None, r#"{"loggedIn": true}"#),
    ] {
        assert!(
            matches!(read(code, &[said]), Reading::Unreadable(_)),
            "{code:?} {said}"
        );
    }
}

/// A run whose output was cut is read as nothing.
#[test]
fn a_cut_run_is_unreadable() {
    let mut cut = ran(Some(0), &[r#"{"loggedIn": true}"#]);
    cut.cut = true;

    assert!(matches!(
        Claude::default().read(Account::Status, &cut),
        Reading::Unreadable(_)
    ));
}

#[test]
fn the_version_is_read_against_the_manifest() {
    let agent = Claude::default();

    assert!(matches!(
        agent.read(Account::Version, &ran(Some(0), &["2.1.296 (Claude Code)"])),
        Reading::Version(version) if version.as_str() == "2.1.296"
    ));
    assert!(matches!(
        agent.read(Account::Version, &ran(Some(0), &["1.0.0 (Claude Code)"])),
        Reading::Unreadable(_)
    ));
    assert!(matches!(
        agent.read(Account::Version, &ran(Some(1), &["2.1.296 (Claude Code)"])),
        Reading::Unreadable(_)
    ));
}

#[test]
fn a_sign_out_is_read_from_its_exit() {
    let agent = Claude::default();

    assert_eq!(
        agent.read(Account::SignOut, &ran(Some(0), &[])),
        Reading::Signed(Authentication::SignedOut)
    );
    assert!(matches!(
        agent.read(Account::SignOut, &ran(Some(1), &[])),
        Reading::Unreadable(_)
    ));
}

/// The command shown for signing in names the profile, quoted so a path
/// with a quote in it is still one word.
#[test]
fn the_sign_in_command_names_the_profile() {
    assert_eq!(sign_in(None).as_str(), "claude auth login");
    if cfg!(windows) {
        assert_eq!(
            sign_in(Some(Path::new(r"C:\p\o'k"))).as_str(),
            r"$env:CLAUDE_CONFIG_DIR='C:\p\o''k'; claude auth login"
        );
    } else {
        assert_eq!(
            sign_in(Some(Path::new("/p/o'k"))).as_str(),
            r"CLAUDE_CONFIG_DIR='/p/o'\''k' claude auth login"
        );
    }
}
