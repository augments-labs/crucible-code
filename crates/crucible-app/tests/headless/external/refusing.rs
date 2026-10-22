//! What a launch that cannot happen is asked about.
//!
//! Each case here holds a conversation whose user file names an instance of
//! Claude Code whose program is nowhere on the `PATH`, and whose route either
//! has no yes or has one already. A command that would start that program is
//! refused for it being missing, before the route is put and before anything
//! is written; nobody is asked to agree to a launch that cannot happen, and
//! the command ends undecided, as it does where a person declines. The consent
//! here has no file to write a yes to, so what a yes would have recorded is
//! not asserted; only that the route is not put where it has no yes, that the
//! file is left as it was, and the command's outcome.

use crucible_app::client;
use crucible_client_api::{
    Capabilities as Asking, Command, LoginOutcome, LogoutOutcome, Name, Outcome, Pending,
};
use crucible_external_harnesses::fake_peer::{self, Reply, scripts::claude};

use super::{Asked, Outside, Picking, SHARED, accounting, connected};
use crate::{Failed, Tree};

/// The user file naming one instance of Claude Code, run as a program that is
/// not installed.
const MISSING: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "no-such-claude-program"}},
  "updates": {"check": "auto"}
}
"#;

/// Whether `put` holds the question for the route of an installed agent.
fn asked_route(put: &[Pending]) -> bool {
    put.iter().any(|pending| {
        matches!(pending, Pending::Warning { route, .. } if route.as_str() == "agent:claude")
    })
}

/// A connect through, or a sign-out of, an instance whose program is not
/// installed, with its route unanswered, does not put the route to the client
/// and ends undecided. Before the refusal came first the route was put, its
/// accept could not be kept, and the command ended some other way.
#[test]
fn a_launch_that_cannot_happen_is_refused_before_the_route_is_asked() -> Result<(), Failed> {
    type Made = fn(Name) -> Command;
    let cases: [(&str, Made, Vec<Option<&'static str>>, Outcome); 2] = [
        (
            "connect",
            |instance| Command::Connect {
                instance: Some(instance),
            },
            vec![Some("Accept"), None],
            Outcome::Login(LoginOutcome::Undecided),
        ),
        (
            "sign-out",
            |instance| Command::SignOut { instance },
            vec![Some(client::accounts::SIGN_OUT), Some("Accept"), None],
            Outcome::Logout(LogoutOutcome::Undecided),
        ),
    ];
    let mut failures = Vec::new();
    for (case, command, choices, undecided) in cases {
        let tree = Tree::new(&format!("external-refused-first-{case}"))?;
        let script = connected([claude::clean(), claude::signed_in()]);
        let mut outside = Outside::new(&tree, &script)?;
        outside.unanswer();
        let asked = Asked {
            installed: (MISSING, "work"),
            command: &command,
            asking: Asking::every(),
            after: None,
        };
        let mut picking = Picking::of(choices);

        let (outcome, _file) = accounting(&tree, &mut outside, &asked, &mut picking)?;

        if asked_route(&picking.put) {
            failures.push(format!(
                "{case}: the agent's route was asked about for a program that is not installed"
            ));
        }
        if outcome != undecided {
            failures.push(format!(
                "{case}: ended {outcome:?} rather than undecided, with the route unanswered"
            ));
        }
    }
    // Every case is run, so that a failure in one does not hide the other.
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}

/// A sign-out of an instance whose program is not installed, with its route
/// already answered, is refused before it writes anything, as it is where the
/// route has no yes: nothing could be signed out, so no instance on the
/// profile is disabled and nothing is started.
#[test]
fn a_sign_out_whose_program_is_missing_writes_nothing_on_an_answered_route() -> Result<(), Failed> {
    let missing = SHARED.replace(
        r#""executable": "claude""#,
        r#""executable": "no-such-claude-program""#,
    );
    let tree = Tree::new("external-sign-out-missing-answered")?;
    let script = connected([claude::clean(), Reply::new(), claude::signed_out()]);
    let mut outside = Outside::new(&tree, &script)?;
    let asked = Asked {
        installed: (&missing, "work"),
        command: &|instance| Command::SignOut { instance },
        asking: Asking::every(),
        after: None,
    };
    // Signs out, then goes back from whatever it is asked next.
    let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
    for _ in 0..500 {
        if fake_peer::played(&outside.peer) >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let before = fake_peer::played(&outside.peer);

    let (outcome, file) = accounting(&tree, &mut outside, &asked, &mut picking)?;

    assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided));
    assert_eq!(file, missing, "nothing was written");
    assert_eq!(
        fake_peer::played(&outside.peer) - before,
        0,
        "the agent's program was started"
    );
    let told = picking.put.get(1).is_some_and(|put| match put {
        Pending::Account { asks, facts, .. } => {
            asks.as_str() == "Claude Code cannot be started."
                && facts.iter().any(|fact| {
                    fact.as_str()
                        .starts_with("Agent: work, run as no-such-claude-program")
                })
        }
        _ => false,
    });
    assert!(told, "{:?}", picking.put);
    Ok(())
}
