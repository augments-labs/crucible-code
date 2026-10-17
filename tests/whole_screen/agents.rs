//! The screens an installed agent is reached through, each captured at eighty
//! and forty columns, on the full screen and on the terminal's own.
//!
//! Every case offers every agent this build can run, with the fake peer
//! installed as `claude` and nothing at all installed as `antigravity`, so the
//! one agent is found and the other is not, whatever this machine has on its
//! own `PATH`. A case walks the screens a person would to reach its own, and
//! the picture taken there is the snapshot.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use crucible_external_harnesses::fake_peer::scripts::claude;
use crucible_external_harnesses::fake_peer::{self, Reply, Script};

use crate::vendor::Vendor;
use crate::watched::{self, Watched};
use crate::{scrubbed, takes, timeless};

/// The widths every screen is captured at, each with rows enough for the
/// longest panel to stand whole below the opening card.
const SIZES: [(u16, u16); 2] = [(80, 40), (40, 40)];

/// One configured instance of the agent the fake peer stands in for.
const WORK: &str =
    "  \"externalAgents\": {\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}},\n";

/// The instance a `/login` to the agent writes when none was configured.
const LOGGED_IN: &str =
    "  \"externalAgents\": {\"claude\": {\"driver\": \"claude\", \"executable\": \"claude\"}},\n";

/// Two configured instances of it.
const TWO: &str = concat!(
    "  \"externalAgents\": {\n",
    "    \"work\": {\"driver\": \"claude\", \"executable\": \"claude\"},\n",
    "    \"home\": {\"driver\": \"claude\", \"executable\": \"claude\"}\n",
    "  },\n"
);

/// Anthropic routed through the installed agent `work` while the
/// conversation starts on openai, which holds no key.
const ROUTED: &str = concat!(
    "  \"provider\": \"openai\",\n",
    "  \"externalAgents\": {\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}},\n",
    "  \"providers\": {\"anthropic\": {\"access\": ",
    "{\"type\": \"externalAgent\", \"instance\": \"work\"}}},\n",
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

/// The list of installed agents, each reaching its own provider.
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

/// The transcript once the signed-in agent is let start, its route written
/// and the conversation on it, nothing more asked.
fn connected(window: &mut Watched) {
    checked(window, "Connected.");
}

/// The box a key for the provider on `row` is typed into, at any width.
fn keyed(window: &mut Watched, row: &str) {
    window.types_until("/login\r", "Provide your own API key");
    takes(window, "Provide your own API key");
    window.types_until("", "DeepSeek");
    takes(window, row);
    if window.picture().contains("Use it anyway") {
        window.types("\r");
    }
    window.types_until("", "API key ─");
}

/// A signed-in status, then the runs `then` plays.
fn signed_in(then: impl IntoIterator<Item = Reply>) -> Vec<Reply> {
    std::iter::once(claude::signed_in()).chain(then).collect()
}

/// Fails where the conversation was not moved onto `work`, or a key was
/// asked for instead.
fn moved_onto(window: &Watched) {
    let picture = window.picture();
    assert!(
        picture.contains("anthropic is reached through the agent work"),
        "{picture}"
    );
    assert!(!picture.contains("no credential"), "{picture}");
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
fn the_installed_agents_are_listed_each_with_its_own_provider() {
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
                    .mutter(format!("\u{1b}[31mstatus \u{7}unavailable{RETITLED}"))
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
            untitled(window);
        },
    );
}

/// An escape sequence that, written as it stands, sets the window's title to a
/// word of its own.
const RETITLED: &str = "\u{1b}]0;retitled\u{7}";

/// Fails where text carrying [`RETITLED`] reached the terminal as the command
/// it spells rather than as words.
fn untitled(window: &Watched) {
    let commands = window.commands();
    assert!(
        !commands.iter().any(|said| said.contains("retitled")),
        "{commands:?}"
    );
}

/// `text` inside a string of the configuration file, with the two control
/// characters [`RETITLED`] holds spelled as JSON escapes.
fn quoted(text: &Path) -> String {
    text.display()
        .to_string()
        .replace('\u{1b}', "\\u001b")
        .replace('\u{7}', "\\u0007")
}

/// Opens `name` at eighty columns in each screen mode, the fake peer playing
/// `script` where it is installed, and holds the window `reaching` leaves to
/// [`untitled`]. `configured` turns the directory agents are installed in into
/// members of the configuration before launch; `reaching` runs after it.
fn retitling(
    name: &str,
    script: Option<&Script>,
    configured: impl Fn(&Path) -> String,
    reaching: impl Fn(&mut Watched),
) {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-{name}-{mode}");
        let configured = configured(&watched::agents(&case));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (script, &configured));
        reaching(&mut window);
        untitled(&window);
    }
}

#[test]
fn a_program_path_that_spells_a_command_never_reaches_the_terminal_as_one() {
    retitling(
        "retitled-path",
        None,
        |installed| {
            let program = quoted(&installed.join(RETITLED).join("claude"));
            format!(
                "  \"externalAgents\": {{\"work\": {{\"driver\": \"claude\", \
                 \"executable\": \"{program}\"}}}},\n"
            )
        },
        |window| {
            let directory = window.workspace().join("agents").join(RETITLED);
            std::fs::create_dir_all(&directory).expect("a directory named for a command");
            let program = directory.join("claude");
            std::fs::write(&program, "not a program\n").expect("a file that is no program");
            let mut permissions = std::fs::metadata(&program)
                .expect("its metadata")
                .permissions();
            permissions.set_mode(0o644);
            std::fs::set_permissions(&program, permissions).expect("a file nobody may run");
            agents(window);
            takes(window, "Claude Code");
            window.types_until("", "is not executable");
        },
    );
}

#[test]
fn a_profile_path_that_spells_a_command_never_reaches_the_terminal_as_one() {
    retitling(
        "retitled-profile",
        Some(&Script::of([claude::signed_out()])),
        |installed| {
            let profile = quoted(&installed.join(RETITLED));
            format!(
                "  \"externalAgents\": {{\"work\": {{\"driver\": \"claude\", \
                 \"executable\": \"claude\", \"configDir\": \"{profile}\"}}}},\n"
            )
        },
        |window| {
            let profile = window.workspace().join("agents").join(RETITLED);
            std::fs::create_dir_all(profile).expect("a profile named for a command");
            checked(window, "auth login");
        },
    );
}

/// Fails where leaving a list of agents, or what to do with one, with nothing
/// taken starts an agent or writes anything.
#[test]
fn leaving_an_agent_chooser_with_nothing_taken_sends_nothing() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-left-{mode}");
        let script = Script::of(signed_in([]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), WORK));
        let configuration = window.home().join("config.json");
        let before = std::fs::read_to_string(&configuration).expect("the configuration");
        agents(&mut window);
        window.types_until("\x1b", "Choose how usage is paid for.");
        window.types_until("\x1b", "cancelled, nothing signed in");
        window.types_until("/logout\r", "external agent instance");
        takes(&mut window, "work");
        window.types_until("", "Choose what to do with this agent.");
        window.types_until("\x1b", "cancelled, nothing signed out");
        assert_eq!(fake_peer::played(&watched::agents(&case)), 0);
        assert!(!window.home().join("auth.json").exists());
        let after = std::fs::read_to_string(&configuration).expect("the configuration");
        assert_eq!(after, before);
    }
}

/// Fails where `/login`, with the provider chosen holding no credential and
/// so none being asked, does not offer an installed agent, which reaches a
/// provider of its own.
#[test]
fn login_with_no_provider_asked_still_offers_an_installed_agent() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-unasked-{mode}");
        let chosen = "  \"provider\": \"openai\",\n";
        let mut window =
            Watched::capturing_agents_asking(&case, (80, 40), native, None, (None, chosen));
        window.types_until("/login\r", "Provide your own API key");
        let picture = window.picture();
        assert!(
            picture.contains("Use an installed coding agent"),
            "{picture}"
        );
    }
}

/// Fails where `/model`, naming a model of a provider the configuration
/// reaches through an installed agent while no agent runs the conversation,
/// asks for that provider's key instead of moving onto the agent.
#[test]
fn a_model_of_a_provider_reached_through_an_agent_moves_onto_it() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-model-onto-{mode}");
        let script = Script::of(Vec::new());
        let mut window = Watched::capturing_agents_asking(
            &case,
            (80, 40),
            native,
            None,
            (Some(&script), ROUTED),
        );
        window.types_until(
            "/model anthropic/claude-sonnet-5-5\r",
            "anthropic is reached through",
        );
        moved_onto(&window);
    }
}

/// Fails where the same model, taken off the `/model` shelf rather than named
/// on the line, does not move the conversation onto the agent.
#[test]
fn a_model_taken_off_the_shelf_of_a_provider_reached_through_an_agent_moves_onto_it() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-shelf-onto-{mode}");
        let script = Script::of(Vec::new());
        let mut window = Watched::capturing_agents_asking(
            &case,
            (80, 40),
            native,
            None,
            (Some(&script), ROUTED),
        );
        window.types_until("/model\r", "claude-sonnet-5-5");
        window.types_until("claude-sonnet-5-5", "claude-sonnet-5-5");
        window.types_until("\r", "anthropic is reached through");
        moved_onto(&window);
    }
}

/// Fails where `/login` with no agent's program installed stops offering an
/// account or a key.
#[test]
fn login_with_no_agent_installed_still_offers_an_account_and_a_key() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-none-{mode}");
        let vendor = Vendor::answering("Not asked.");
        let mut window = Watched::capturing_agents(&case, (80, 40), native, &vendor, (None, ""));
        window.types_until("/login\r", "Provide your own API key");
        let picture = window.picture();
        assert!(
            picture.contains("Your account with subscription"),
            "{picture}"
        );
    }
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

/// Fails where `/logout`, once a connect has installed the agent during the
/// run, does not offer it, or where disconnecting it from there is not
/// written down.
#[test]
fn an_agent_a_connect_installed_is_offered_by_logout_in_the_same_run() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-logout-installed-{mode}");
        let script = Script::of(signed_in([]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut window);

        window.types_and_catches_where("/logout\r", "an agent or nothing offered", |picture| {
            picture.contains("external agent instance") || picture.contains("nothing is stored")
        });
        let offered = window.picture();
        assert!(
            offered.contains("external agent instance"),
            "{case}: {offered}"
        );
        takes(&mut window, "claude");
        window.types_until("", "Disconnect from Crucible");
        takes(&mut window, "Disconnect from Crucible");
        window.types_until("", "Disconnect claude from Crucible?");
        takes(&mut window, "Disconnect from Crucible");
        window.types_until("", "Disconnected from Crucible.");

        let file = std::fs::read_to_string(window.home().join("config.json"))
            .expect("the configuration file");
        let written: serde_json::Value =
            serde_json::from_str(&file).expect("a configuration in its format");
        let enabled = written
            .pointer("/externalAgents/claude/enabled")
            .and_then(serde_json::Value::as_bool);
        assert_eq!(enabled, Some(false), "{case}: {written}");
    }
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

/// Fails where the status row, once the conversation moves onto an agent, does
/// not say who runs its turns, on which model, and who is paid for them in
/// place of the provider, between turns and under a running one.
#[test]
fn the_status_row_says_who_runs_the_turns_on_an_agent_route() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-status-route-{mode}");
        let script = Script::of(signed_in([claude::held_open()]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        let status = |window: &Watched| {
            window
                .picture()
                .lines()
                .rev()
                .find(|row| row.contains("mode on"))
                .unwrap_or_default()
                .to_owned()
        };
        window.types_until("", "mode on");
        let before = status(&window);
        assert!(before.contains("anthropic · "), "{case}: {before}");
        assert!(!before.contains("execution:"), "{case}: {before}");

        connected(&mut window);
        let after = status(&window);

        assert!(
            after.contains("execution: claude, model: claude-opus-5-5 · billing: external"),
            "{case}: {after}"
        );
        assert!(!after.contains("anthropic"), "{case}: {after}");

        window.types_and_catches_where("Keep working.\r", "a turn under way", |picture| {
            picture.contains("Working.") && picture.contains("esc to interrupt")
        });
        let running = status(&window);
        window.types_until("\x1b", "! stopped");

        assert!(
            running.contains("execution: claude, model: claude-opus-5-5 · billing: external"),
            "{case}: {running}"
        );
    }
}

/// Fails where a move onto the agent's route does not name the route and say
/// the conversation starts fresh there, where `/login` on it does not offer the
/// installed agents, or where a key stored while on it for the provider the
/// agent reaches does not move the conversation off the agent onto it and say
/// so.
#[test]
fn a_route_switch_and_a_key_stored_on_an_agent_route_say_where_the_conversation_is() {
    for native in [false, true] {
        for columns in [80, 40] {
            let mode = if native { "native" } else { "fullscreen" };
            let case = format!("agent-route-lines-{columns}-{mode}");
            let script = Script::of(signed_in([]));
            let vendor = Vendor::answering("Not asked.");
            let mut window = Watched::capturing_agents(
                &case,
                (columns, 40),
                native,
                &vendor,
                (Some(&script), ""),
            );
            connected(&mut window);
            let said = |window: &Watched| {
                window
                    .picture()
                    .replace("|\n|", " ")
                    // The full screen's scroll bar, and its thumb, stand
                    // between a folded line's halves.
                    .replace(['┃', '●'], " ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let moved = said(&window);
            assert!(
                moved.contains("now runs on the agent claude and starts fresh"),
                "{case}: {moved}"
            );

            window.types_until("/login\r", "Choose how usage is paid for");
            let offered = said(&window);
            assert!(
                offered.contains("Use an installed coding agent"),
                "{case}: {offered}"
            );
            window.types_until("\u{1b}", "cancelled, nothing signed in");

            keyed(&mut window, "Anthropic");
            window.types_until("not-a-key-and-nothing-reads-it\r", "the one it left");
            let moved_off = said(&window);
            assert!(
                moved_off.contains(
                    "this conversation left its agent for anthropic, in a new session; \
                     /resume lists the one it left"
                ),
                "{case}: {moved_off}"
            );
            let file = std::fs::read_to_string(window.home().join("config.json"))
                .expect("the configuration file");
            let written: serde_json::Value =
                serde_json::from_str(&file).expect("a configuration in its format");
            let access = written
                .pointer("/providers/anthropic/access/type")
                .and_then(serde_json::Value::as_str);
            assert_eq!(access, Some("apiKey"), "{case}: {written}");
        }
    }
}

/// Fails where `/model` on a Claude agent's route offers no other provider's
/// models beside its own, or says the agent chooses its own, where the model
/// and rung taken there do not stand over the ladder `/effort` opens next, or
/// where a model of a provider with a key in hand does not move the
/// conversation off the agent onto it.
#[test]
fn model_and_effort_on_an_agent_route_offer_every_provider_s_models() {
    for native in [false, true] {
        for columns in [80, 40] {
            let mode = if native { "native" } else { "fullscreen" };
            let case = format!("agent-model-{columns}-{mode}");
            let script = Script::of(signed_in([claude::held_open()]));
            let vendor = Vendor::answering("Not asked.");
            let mut window = Watched::capturing_agents_keyed(
                &case,
                (columns, 40),
                native,
                &vendor,
                (
                    Some(&script),
                    &[("OPENAI_API_KEY", "not-a-key-and-nothing-reads-it")],
                ),
            );
            connected(&mut window);
            let joined = |picture: &str| {
                picture
                    .replace("|\n|", " ")
                    // The full screen's scroll bar, and its thumb, stand
                    // between a folded line's halves.
                    .replace(['┃', '●'], " ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let said = |window: &Watched| joined(&window.picture());
            // A vendor's name, or one of its models where a narrow window
            // clips the row of vendors.
            let others = [("DeepSeek", "deepseek-flash")];
            let offers = |picture: &str, (vendor, model): (&str, &str)| {
                picture.contains(vendor) || picture.contains(model)
            };
            window.types_until("/model\r", "Model");
            let shelf = said(&window);
            assert!(shelf.contains("claude-opus-5-5"), "{case}: {shelf}");
            assert!(!shelf.contains("chooses its own model"), "{case}: {shelf}");
            for other in others {
                assert!(offers(&shelf, other), "{case}: no {other:?} in {shelf}");
            }
            window.types_until("\x1b", "no model taken");

            window.types_until("/model claude-sonnet-5-5\r", "claude-sonnet-5-5 ·");
            let file = std::fs::read_to_string(window.home().join("config.json"))
                .expect("the configuration file");
            let written: serde_json::Value =
                serde_json::from_str(&file).expect("a configuration in its format");
            let model = written
                .pointer("/externalAgents/claude/model")
                .and_then(serde_json::Value::as_str);
            assert_eq!(model, Some("claude-sonnet-5-5"), "{case}: {written}");

            // Too narrow a window for the ladder has it listed instead.
            if columns == 80 {
                window.types_until("/effort\r", "Effort · claude-sonnet-5-5");
                let ladder = said(&window);
                assert!(ladder.contains("Smarter"), "{case}: {ladder}");
                window.types_until("\x1b", "no rung taken");
            } else {
                window.types_until("/effort\r", "/effort max");
            }

            // Mid-turn the same shelf stands over the running turn, its pick
            // held for the agent's next run.
            window.types_and_catches_where("Keep working.\r", "a turn under way", |picture| {
                picture.contains("esc to interrupt")
            });
            window.types_until("/model\r", "Search");
            let running = said(&window);
            assert!(running.contains("claude-sonnet-5-5"), "{case}: {running}");
            for other in others {
                assert!(offers(&running, other), "{case}: no {other:?} in {running}");
            }
            window.types_and_catches_where("\x1b", "the shelf left", |picture| {
                !picture.contains("a model, or a vendor")
            });
            window.types_until("\x1b", "! stopped");

            window.types_until("/model openai/gpt-5.6-sol\r", "the one it left");
            let moved_off = said(&window);
            assert!(
                moved_off.contains(
                    "this conversation left its agent for openai, in a new session; \
                     /resume lists the one it left"
                ),
                "{case}: {moved_off}"
            );
            let file = std::fs::read_to_string(window.home().join("config.json"))
                .expect("the configuration file");
            let written: serde_json::Value =
                serde_json::from_str(&file).expect("a configuration in its format");
            let provider = written
                .pointer("/provider")
                .and_then(serde_json::Value::as_str);
            assert_eq!(provider, Some("openai"), "{case}: {written}");
            let model = written
                .pointer("/providers/openai/model")
                .and_then(serde_json::Value::as_str);
            assert_eq!(model, Some("gpt-5.6-sol"), "{case}: {written}");
        }
    }
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

/// Fails where the prompt on an agent's route does not read what the agent
/// said its context holds against its model's window, or where the agent
/// making room is not written into the transcript with what it held either
/// side and the reading does not drop to what was left.
#[test]
fn an_agent_route_reads_its_context_against_the_window_and_says_when_it_made_room() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-window-{mode}");
        let script = Script::of(signed_in([claude::carrying_and_compacting()]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut window);
        window.types_until("First.\r", "Done.");
        let picture = window.picture();
        assert!(
            picture
                .lines()
                .any(|line| line.contains("41k/") && line.contains("% window left")),
            "the reading in {mode} holds what the agent carried\n{picture}"
        );

        window.types_until("Second.\r", "Made room.");
        let picture = window.picture();
        for said in [
            "compacted",
            "the window was full",
            "the agent made room",
            "41k → 9k carried",
        ] {
            assert!(picture.contains(said), "{mode} says {said:?}\n{picture}");
        }
        assert!(
            picture
                .lines()
                .any(|line| line.contains("9k/") && line.contains("% window left")),
            "the reading in {mode} drops to what was left\n{picture}"
        );
    }
}

/// Fails where the calls an agent made with its own tools are not drawn as a
/// native call is, in either screen mode: the tool's name with what the call
/// was about, and under it what the call came back with, a failure included,
/// rather than a line saying the agent used a tool.
#[test]
fn an_agent_s_own_calls_are_drawn_as_call_rows_with_what_they_came_back_with() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-calls-{mode}");
        let script = Script::of(signed_in([claude::tooled()]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut window);
        window.types_until("List them.\r", "Done.");
        let picture = window.picture();

        let listed = format!("Bash({})", claude::LISTED);
        let read = format!("Read({})", claude::MISSING);
        for called in [&listed, &read] {
            assert!(
                picture
                    .lines()
                    .any(|line| line.trim_start_matches('|').starts_with(&format!(
                        "{} {called}",
                        char::from_u32(0x25CF).unwrap_or('*')
                    ))),
                "{mode} draws {called:?} as a call row\n{picture}"
            );
        }
        let came_back = |said: &str| {
            picture
                .lines()
                .any(|line| line.contains('\u{23BF}') && line.contains(said))
        };
        assert!(
            came_back(claude::LISTING[0]) && picture.contains("ctrl+o to expand"),
            "{mode} draws the listing under its call\n{picture}"
        );
        assert!(
            came_back(claude::NOT_THERE),
            "{mode} draws the failed read under its call\n{picture}"
        );
        assert!(
            !picture.contains("the agent used"),
            "{mode} still draws the agent's calls as activity\n{picture}"
        );
    }
}

/// An edit the agent made is drawn as a native one is: the call, what it
/// changed counted beneath it, and the changed lines, in either screen mode,
/// with no quiet row naming the file a second time.
#[test]
fn an_agent_s_edit_is_drawn_as_the_change_it_made() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-edit-{mode}");
        let script = Script::of(signed_in([claude::edited()]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut window);
        window.types_until("Change it.\r", "Done.");
        let picture = window.picture();

        let called = format!(
            "{} Edit({})",
            char::from_u32(0x25CF).unwrap_or('*'),
            claude::EDITED
        );
        assert!(
            picture
                .lines()
                .any(|line| line.trim_start_matches('|').starts_with(&called)),
            "{mode} draws the edit as a call row\n{picture}"
        );
        assert!(
            picture
                .lines()
                .any(|line| line.contains('\u{23BF}') && line.contains("Added 1 line")),
            "{mode} counts the change under its call\n{picture}"
        );
        for (mark, changed) in [("4 -", 'a'), ("4 +", 'b')] {
            assert!(
                picture.lines().any(|line| line.contains(mark)
                    && line.trim_end_matches([' ', '|']).ends_with(changed)),
                "{mode} draws the changed line {mark} {changed}\n{picture}"
            );
        }
        assert_eq!(
            picture.matches(claude::EDITED).count(),
            1,
            "{mode} names the file once\n{picture}"
        );
    }
}

/// Calls the agent made that only looked around are counted on one line, as
/// a native run of them is, in either screen mode.
#[test]
fn an_agent_s_lookups_are_counted_on_one_line() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-lookups-{mode}");
        let script = Script::of(signed_in([claude::looking()]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut window);
        window.types_until("Look around.\r", "Done.");
        let picture = window.picture();

        assert!(
            picture.contains("Searched for 1 pattern, read 2 files"),
            "{mode} counts the lookups on one line\n{picture}"
        );
        for looked in claude::LOOKED_AT {
            assert!(
                !picture.contains(&format!("Read({looked})")),
                "{mode} still draws {looked} as a row of its own\n{picture}"
            );
        }
    }
}

/// A session the agent ran, opened again in a new window, draws the agent's
/// calls as it drew them while they ran: the call rows and what they came
/// back with, the change an edit made, and the lookups counted on one line,
/// in either screen mode.
#[test]
fn a_resumed_agent_session_draws_the_agent_s_calls_as_they_were_drawn() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-resumed-{mode}");
        let script = Script::of(signed_in([claude::worked()]));
        let vendor = Vendor::answering("Not asked.");
        let mut first =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut first);
        first.types_until("Do it all.\r", "Done.");
        assert!(
            first.picture().contains("Searched for 1 pattern"),
            "{mode} drew the turn as it ran\n{}",
            first.picture()
        );
        first.ends_on("TERM");
        let id = crate::native::recorded_holding(&first, "Do it all.");

        let again = Script::of(signed_in([]));
        let mut window =
            Watched::capturing_agents(&case, (80, 60), native, &vendor, (Some(&again), ""));
        connected(&mut window);
        window.types_until(&format!("/resume {id}\r"), "Done.");
        let picture = window.picture();

        let row = |called: &str| {
            picture.lines().any(|line| {
                line.trim_start_matches('|').starts_with(&format!(
                    "{} {called}",
                    char::from_u32(0x25CF).unwrap_or('*')
                ))
            })
        };
        let came_back = |said: &str| {
            picture
                .lines()
                .any(|line| line.contains('\u{23BF}') && line.contains(said))
        };
        assert!(
            row(&format!("Bash({})", claude::LISTED)) && came_back(claude::LISTING[0]),
            "{mode} draws the agent's command and what it printed again\n{picture}"
        );
        assert!(
            row(&format!("Edit({})", claude::EDITED)) && came_back("Added 1 line"),
            "{mode} draws the agent's edit and its change again\n{picture}"
        );
        assert!(
            picture.contains("Searched for 1 pattern, read 2 files"),
            "{mode} counts the agent's lookups on one line again\n{picture}"
        );
    }
}

/// A session the agent ran, resumed in a window whose file holds no yes to
/// the agent's route, asks that route at the next message, before the agent
/// is started, and says what starting it runs; the yes lets the message go
/// to the agent, in either screen mode.
#[test]
fn a_message_that_would_start_an_unanswered_agent_asks_its_route_first() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-resumed-unanswered-{mode}");
        let script = Script::of(signed_in([claude::clean()]));
        let vendor = Vendor::answering("Not asked.");
        let mut first =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));
        connected(&mut first);
        first.types_until("Do it all.\r", "Done.");
        first.ends_on("TERM");
        let id = crate::native::recorded_holding(&first, "Do it all.");

        let went_on = Reply::new()
            .hear(1)
            .say(claude::init())
            .say(claude::said("Went on."))
            .say(claude::answered(Some(claude::PRICE)));
        let again = Script::of([went_on]);
        let mut window =
            Watched::capturing_agents(&case, (80, 60), native, &vendor, (Some(&again), LOGGED_IN));
        window.types_until(&format!("/resume {id}\r"), "Done.");
        window.types_until("Go on.\r", "Use it anyway");
        let asked = window.picture();
        assert!(
            asked.contains("Starting Claude Code runs"),
            "{mode} says what starting the agent runs\n{asked}"
        );
        window.types_until("\r", "Went on.");
    }
}

#[test]
fn with_no_terminal_a_message_that_would_start_an_unanswered_agent_ends_the_run() {
    let case = "agent-resumed-unanswered-piped";
    let script = Script::of(signed_in([claude::clean()]));
    let vendor = Vendor::answering("Not asked.");
    let mut first = Watched::capturing_agents(case, (80, 40), false, &vendor, (Some(&script), ""));
    connected(&mut first);
    first.types_until("Do it all.\r", "Done.");
    first.ends_on("TERM");
    let id = crate::native::recorded_holding(&first, "Do it all.");
    let started = fake_peer::argued(&watched::agents(case)).len();

    let ended = watched::piped_agents(case, &vendor, LOGGED_IN, &format!("/resume {id}\nGo on.\n"));

    let said = String::from_utf8_lossy(&ended.stderr);
    assert_eq!(ended.status.code(), Some(1), "{said}");
    assert!(
        said.contains("Claude Code: "),
        "names the agent's route\n{said}"
    );
    assert!(said.contains("answer it once in a terminal"), "{said}");
    assert_eq!(
        fake_peer::argued(&watched::agents(case)).len(),
        started,
        "the agent is not started again\n{said}"
    );
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

/// Fails where an agent's leave to act is not put to the person as a panel, or
/// where that panel does not say who runs the action and how it is held in.
#[test]
fn an_agent_asking_leave_to_act_is_put_as_a_panel() {
    captured(
        &Capture {
            name: "permission",
            replies: Some(signed_in([claude::permission()])),
            configured: "",
        },
        |window| {
            connected(window);
            window.types_until("List them.\r", "Do you want to proceed?");
        },
    );
}

/// Fails where a yes on an agent's panel does not reach the agent, so that
/// its turn goes on to its answer.
#[test]
fn a_yes_on_an_agent_panel_lets_the_agent_go_on() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("agent-permission-yes-{mode}");
        let script = Script::of(signed_in([claude::permission()]));
        let vendor = Vendor::answering("Not asked.");
        let mut window =
            Watched::capturing_agents(&case, (80, 40), native, &vendor, (Some(&script), ""));

        connected(&mut window);
        window.types_until("List them.\r", "Do you want to proceed?");
        let asked = window.picture();
        window.types_until("\r", "Done.");

        // The connect line above says which agent runs the turns; only the
        // panel says how far what it runs is held in.
        assert!(
            asked.contains("execution: claude · confinement: unsupported"),
            "{case}: {asked}"
        );
        assert!(asked.contains("Bash command"), "{case}: {asked}");
        assert!(asked.contains("3. No "), "{case}: {asked}");
    }
}
