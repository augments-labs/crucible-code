//! The screens an installed agent is reached through, each captured at eighty
//! and forty columns, on the full screen and on the terminal's own.
//!
//! Every case offers every agent this build can run, with the fake peer
//! installed as `claude` and nothing at all installed as `antigravity`, so the
//! one agent is found and the other is not, whatever this machine has on its
//! own `PATH`. A case walks the screens a person would to reach its own, and
//! the picture taken there is the snapshot.

use std::os::unix::fs::PermissionsExt as _;

use crucible_external_harnesses::fake_peer::scripts::claude;
use crucible_external_harnesses::fake_peer::{Reply, Script};

use crate::vendor::Vendor;
use crate::watched::Watched;
use crate::{scrubbed, takes, timeless};

/// The widths every screen is captured at, each with rows enough for the
/// longest panel to stand whole below the opening card.
const SIZES: [(u16, u16); 2] = [(80, 40), (40, 40)];

/// One configured instance of the agent the fake peer stands in for.
const WORK: &str =
    "  \"externalAgents\": {\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}},\n";

/// Two configured instances of it.
const TWO: &str = concat!(
    "  \"externalAgents\": {\n",
    "    \"work\": {\"driver\": \"claude\", \"executable\": \"claude\"},\n",
    "    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\"}\n",
    "  },\n"
);

/// How one capture is set up.
struct Capture<'a> {
    /// What the snapshot and the case are named after.
    name: &'a str,
    /// What the fake peer plays, start by start; nothing where it is not
    /// installed at all.
    replies: Option<Vec<Reply>>,
    /// Members of the configuration object, each ending in a comma.
    configured: &'a str,
}

/// Takes `capture` at every size in both screen modes, `reaching` walking
/// each window from its first frame to the screen captured.
fn captured(capture: &Capture<'_>, reaching: impl Fn(&mut Watched)) {
    for native in [false, true] {
        for size in SIZES {
            let mode = if native { "native" } else { "fullscreen" };
            let case = format!("agent-{}-{}-{mode}", capture.name, size.0);
            let script = capture.replies.clone().map(Script::of);
            let vendor = Vendor::answering("Not asked.");
            let mut window = Watched::capturing_agents(
                &case,
                size,
                native,
                &vendor,
                (script.as_ref(), capture.configured),
            );
            reaching(&mut window);
            let picture = scrubbed(&timeless(&window.picture()), &case);
            insta::assert_snapshot!(format!("agent_{}_{}_{mode}", capture.name, size.0), picture);
        }
    }
}

/// The first `/login` panel, its third row reaching an installed agent.
fn first(window: &mut Watched) {
    window.types_until("/login\r", "Use an installed coding agent");
}

/// The list of agents the provider can be reached through.
fn agents(window: &mut Watched) {
    first(window);
    takes(window, "Use an installed coding agent");
    window.types_until("", "Antigravity");
}

/// The question put before the agent's program is first started.
fn held(window: &mut Watched) {
    agents(window);
    takes(window, "Claude Code");
    window.types_until("", "Nothing starts");
}

/// What the agent said of its sign-in, once it was let start: `wanted` is on
/// that screen.
fn checked(window: &mut Watched, wanted: &str) {
    held(window);
    takes(window, "Continue");
    window.types_until("", wanted);
}

/// The route asked about, once the account it is signed in with is taken.
fn routed(window: &mut Watched) {
    checked(window, "Use this account");
    takes(window, "Use this account");
    window.types_until("", "Connect");
}

/// The transcript once the route is written and the conversation is on it.
fn connected(window: &mut Watched) {
    routed(window);
    takes(window, "Connect");
    window.types_until("", "Connected.");
}

/// A signed-in status, then the runs `then` plays.
fn signed_in(then: impl IntoIterator<Item = Reply>) -> Vec<Reply> {
    std::iter::once(claude::signed_in()).chain(then).collect()
}

#[test]
fn the_first_login_panel_offers_an_installed_agent() {
    captured(
        &Capture {
            name: "first",
            replies: Some(Vec::new()),
            configured: "",
        },
        first,
    );
}

#[test]
fn the_agents_a_provider_can_be_reached_through_are_listed() {
    captured(
        &Capture {
            name: "agents",
            replies: Some(Vec::new()),
            configured: "",
        },
        agents,
    );
}

#[test]
fn an_agent_whose_program_is_not_installed_says_so() {
    captured(
        &Capture {
            name: "missing",
            replies: Some(Vec::new()),
            configured: "",
        },
        |window| {
            agents(window);
            takes(window, "Antigravity");
            window.types_until("", "Not installed");
        },
    );
}

#[test]
fn an_agent_whose_program_cannot_be_run_says_so() {
    captured(
        &Capture {
            name: "not-executable",
            replies: None,
            configured: "",
        },
        |window| {
            let program = window.workspace().join("agents").join("claude");
            std::fs::write(&program, "not a program\n").expect("a file that is no program");
            let mut permissions = std::fs::metadata(&program)
                .expect("its metadata")
                .permissions();
            permissions.set_mode(0o644);
            std::fs::set_permissions(&program, permissions).expect("a file nobody may run");
            agents(window);
            takes(window, "Claude Code");
            // Why it cannot run is the panel's grounds, kept however short
            // the window is.
            window.types_until("", "is not executable");
        },
    );
}

#[test]
fn an_agent_with_several_instances_asks_which() {
    captured(
        &Capture {
            name: "instances",
            replies: Some(Vec::new()),
            configured: TWO,
        },
        |window| {
            agents(window);
            takes(window, "Claude Code");
            window.types_until("", "home");
        },
    );
}

#[test]
fn the_question_before_an_agent_first_starts_says_what_it_runs() {
    captured(
        &Capture {
            name: "hold",
            replies: Some(Vec::new()),
            configured: "",
        },
        held,
    );
}

#[test]
fn a_signed_in_agent_offers_its_account() {
    captured(
        &Capture {
            name: "signed-in",
            replies: Some(signed_in([])),
            configured: "",
        },
        |window| checked(window, "Use this account"),
    );
}

#[test]
fn a_signed_out_agent_says_how_to_sign_in() {
    captured(
        &Capture {
            name: "signed-out",
            replies: Some(vec![claude::signed_out()]),
            configured: "",
        },
        |window| checked(window, "is not signed in"),
    );
}

#[test]
fn an_agent_whose_sign_in_cannot_be_read_says_so_in_one_reason() {
    captured(
        &Capture {
            name: "unverified",
            replies: Some(vec![
                Reply::new()
                    .mutter("\u{1b}[31mstatus \u{7}unavailable\u{1b}]0;title\u{7}")
                    .exit(2),
            ]),
            configured: "",
        },
        |window| {
            checked(window, "Reason: The agent exited");
            let picture = window.picture();
            assert!(picture.contains("unavailable"), "{picture}");
            assert!(
                !picture.contains("[31m") && !picture.contains("]0;"),
                "{picture}"
            );
        },
    );
}

#[test]
fn the_route_is_confirmed_before_anything_is_written() {
    captured(
        &Capture {
            name: "route",
            replies: Some(signed_in([])),
            configured: "",
        },
        routed,
    );
}

#[test]
fn a_connect_moves_the_conversation_onto_the_agent() {
    captured(
        &Capture {
            name: "connected",
            replies: Some(signed_in([])),
            configured: "",
        },
        connected,
    );
}

#[test]
fn the_logout_panel_offers_what_can_be_done_with_an_agent() {
    captured(
        &Capture {
            name: "logout",
            replies: Some(Vec::new()),
            configured: WORK,
        },
        |window| {
            window.types_until("/logout\r", "external agent instance");
            takes(window, "work");
            window.types_until("", "Disconnect from Crucible");
        },
    );
}

#[test]
fn a_run_lost_mid_effect_says_its_work_is_uncertain() {
    captured(
        &Capture {
            name: "uncertain",
            replies: Some(signed_in([claude::crash()])),
            configured: "",
        },
        |window| {
            connected(window);
            window.types_until("Change the file.\r", "have happened.");
        },
    );
}

/// Fails where `/usage`, or the Usage tab of `/settings`, over a running turn
/// on an agent's route draws a native row, a plan limit or nothing spent,
/// rather than what the agent reported of the turn before, headed as its
/// report.
#[test]
fn usage_over_a_running_agent_turn_draws_what_the_agent_reported() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-usage-mid-turn-{mode}");
        let script = Script::of(signed_in([claude::priced_and_held()]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut window);
        window.types_until("First.\r", "Done.");
        // The turn under way redraws its spinner, so the screen never
        // settles: each step is caught as it is drawn.
        window.types_and_catches("Second.\r", "Working.");

        window.types_and_catches("/usage\r", "Session · as claude reported it");
        let panel = window.picture();
        reported(&panel, &format!("/usage in {mode}"));

        window.types_and_catches_where("\u{1b}", "the panel closed", |picture| {
            !picture.contains("as claude reported it")
        });
        window.types_and_catches("/settings\r", "esc to close");
        window.types_and_catches("\t", "Session · as claude reported it");
        reported(&window.picture(), &format!("the Usage tab in {mode}"));
    }
}

/// Asserts `picture` shows the priced turn the agent reported, headed as its
/// report, and nothing only a native route has.
fn reported(picture: &str, drawn: &str) {
    let dollars = format!(
        "${}.{:02}",
        claude::PRICE / 1_000_000,
        claude::PRICE / 10_000 % 100
    );
    let cost = picture
        .lines()
        .find(|line| line.contains("Total cost"))
        .unwrap_or_else(|| panic!("{drawn} has a cost row\n{picture}"));
    assert!(
        cost.contains(&dollars),
        "{drawn} costs {dollars}\n{picture}"
    );
    let output = format!("{} out", claude::OUTPUT);
    assert!(
        picture.contains(&output),
        "{drawn} has the tokens\n{picture}"
    );
    for unwanted in ["nothing asked yet", "API time", "Lines changed"] {
        assert!(
            !picture.contains(unwanted),
            "{drawn} draws {unwanted:?}\n{picture}"
        );
    }
    assert!(
        picture.contains("not reported by agents"),
        "{drawn}\n{picture}"
    );
}
