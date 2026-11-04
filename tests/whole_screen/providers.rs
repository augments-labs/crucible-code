//! The vendors this release adds, at the key box: a key that does not fit the
//! row it was typed on is refused there, with nothing stored and nothing sent,
//! and the box stands again for the one that does. And an agent's sign-in by a
//! command it runs, at the panel `/login` carries it through: what the command
//! says is shown, what is typed reaches it, and a hidden line is never drawn.

use std::path::PathBuf;

use crucible_external_harnesses::fake_peer::scripts::acp;
use crucible_external_harnesses::fake_peer::{self, Reply, Script};

use crate::takes;
use crate::vendor::Vendor;
use crate::warning::{NOTHING_CHOSEN, said};
use crate::watched::{self, Published, Watched};

#[test]
fn a_key_of_another_kind_is_refused_in_the_box_and_the_box_stands_again() {
    // `MiMo` takes a pay-as-you-go key; a key from its Token Plan starts
    // `tp-` and is answered with a 401 at this address, a turn later and in
    // words that say only that the key is wrong.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::keyless("mimo-key-marks", 80, 24, &vendor);

    window.types_until("/login mimo\r", "paste or type your API key");
    window.types_until(
        "tp-fabricated-token-plan-key\r",
        "not a key for this row; keys starting tp- are another kind",
    );
    assert!(
        window.picture().contains("paste or type your API key"),
        "{}",
        window.picture()
    );
    window.types_until("sk-fabricated-mimo-key\r", "login successful");

    let held = std::fs::read_to_string(window.home().join("auth.json")).unwrap_or_default();
    assert!(held.contains("mimo@xiaomimimo.com"), "{held}");
    assert!(
        held.contains("sk-fabricated-mimo-key"),
        "the key that fits is kept"
    );
    assert!(
        !held.contains("tp-fabricated"),
        "the refused key is never stored"
    );
    assert!(
        !window.picture().contains("tp-fabricated"),
        "{}",
        window.picture()
    );
}

/// Where an agent the catalog offers can be spoken with on the agents this
/// release ships, whose list names none: added from the catalog, chosen, asked
/// about, and answering. A restart of what that wrote reaches the prompt with
/// nothing asked of the catalog's host.
#[test]
fn an_agent_added_from_the_catalog_answers_a_turn_on_the_registry_that_ships() {
    let case = "catalog-conversation";
    let initialized = acp::offering("goose-acp", "1.0.0", &serde_json::json!([]));
    let peer = watched::scratch(case).join("released");
    let program = fake_peer::install(
        &peer,
        "goose",
        &Script::of([
            Reply::new()
                .hear(1)
                .say(&initialized)
                .hear(1)
                .say(acp::opened(1)),
            acp::opening(&initialized)
                .say(acp::said("Answered by the agent."))
                .say(acp::stopped(2, "end_turn")),
        ]),
    )
    .expect("the fake peer installs");
    let published = Published::peer(&program, true);

    let mut window = Watched::shipped(case, (80, 40), (NOTHING_CHOSEN, &[]), &published);
    added_and_chosen(&mut window);
    takes(&mut window, "Connect");
    window.types_until("", "The vendor may use what you send to improve goose.");
    assert_eq!(fake_peer::argued(&peer).len(), 0, "started before the yes");
    takes(&mut window, "Continue");
    window.types_until("Say something.\r", "Answered by the agent.");
    let picture = window.picture();
    assert!(picture.contains("execution: goose"), "{picture}");

    let asked = published.asked();
    let document = said(&window);
    window.ends_on("TERM");
    let restarted = Watched::shipped(case, (80, 40), (&document, &["--continue"]), &published);
    let picture = restarted.picture();
    assert!(picture.contains("execution: goose"), "{picture}");
    assert_eq!(
        published.asked(),
        asked,
        "a restart asked the catalog's host"
    );
}

/// Adds goose from the catalog and walks `/login` to the screen of the instance
/// that wrote.
fn added_and_chosen(window: &mut Watched) {
    window.types_until("/catalog\r", "1 of 1 offered");
    window.types_until("goose", "Search  goose");
    window.types_until("\r", "esc to go back");
    takes(window, "Add");
    window.types_until("", "bytes to download");
    takes(window, "Add");
    window.types_until("", "Apache-2.0 · installed");
    window.types_until("\x1b", "an agent, or a licence");
    window.types_until("\x1b", "Added goose 1.0.0");

    window.types_until("/login\r", "Use an installed coding agent");
    takes(window, "Use an installed coding agent");
    takes(window, "goose");
    window.types_until("", "Connect");
}

/// A connect the catalog gives no way to hold says why and that nothing changed,
/// never that it connected, and starts nothing.
#[test]
fn a_catalog_agent_that_cannot_be_held_is_refused_and_is_not_called_connected() {
    let case = "catalog-conversation-unstated";
    let peer = watched::scratch(case).join("released");
    let program =
        fake_peer::install(&peer, "goose", &Script::of([])).expect("the fake peer installs");
    let published = Published::peer(&program, false);

    let mut window = Watched::shipped(case, (80, 40), (NOTHING_CHOSEN, &[]), &published);
    added_and_chosen(&mut window);
    let before = said(&window);
    takes(&mut window, "Connect");
    window.types_until("", "goose cannot be connected.");
    let picture = window.picture();
    assert!(picture.contains("gives no statement"), "{picture}");
    assert!(
        picture.contains("sent.") && !picture.contains("sent.."),
        "the fact line names the missing statement and ends with one full stop: {picture}"
    );
    takes(&mut window, "Go back");
    assert!(
        !window.picture().contains("Connected."),
        "{}",
        window.picture()
    );
    assert_eq!(said(&window), before, "the refusal wrote to the user file");
    assert!(fake_peer::argued(&peer).is_empty(), "the agent was started");
}

/// The agent every sign-in case below is offered, by a name no built-in driver
/// takes.
const AGENT: &str = "goose";

/// What the panel is asked to send where a line is to stay hidden. Its letters
/// are in no other text the screen draws, so finding them means it was shown.
const HIDDEN_LINE: &str = "zqxjkvbw-not-a-real-password";

/// What the agent says of its method on the wire, which the catalog records
/// as the same: the program is whatever the release calls the peer.
fn terminal_method() -> serde_json::Value {
    serde_json::json!({
        "type": "terminal",
        "id": "terminal",
        "name": "Sign in at a terminal",
        "command": AGENT,
        "args": ["login"],
        "env": {"SIGNING": "in"},
    })
}

/// The agent's answer to initializing, offering `method`.
fn offering(method: &serde_json::Value) -> String {
    acp::offering("goose-acp", "1.0.0", &serde_json::json!([method]))
}

/// The start that reads what the agent says of its method, before the command.
fn read_first() -> Reply {
    Reply::new().hear(1).say(offering(&terminal_method()))
}

/// The start that reads the agent signed in, after the command.
fn read_signed_in() -> Reply {
    Reply::new()
        .hear(1)
        .say(offering(&terminal_method()))
        .hear(1)
        .say(acp::opened(1))
}

/// A window on a host offering [`AGENT`] as the fake peer playing `replies`,
/// the recorded method being `recorded`, with the agent installed from there
/// as the instance `goose` and its sign-in by that method chosen in `/login`,
/// its route given its yes. The peer's program stands in the directory beside
/// it, and the host is handed back to stay up as long as the window.
fn signing_in(
    case: &str,
    native: bool,
    recorded: &serde_json::Value,
    replies: Vec<Reply>,
) -> (Watched, Published, PathBuf) {
    let built = watched::agents(&format!("{case}-built"));
    std::fs::create_dir_all(&built).expect("a directory to build the release in");
    let published = Published::signing_in_by(AGENT, &Script::of(replies), recorded, &built);
    let _ = std::fs::remove_dir_all(&built);
    let vendor = Vendor::answering("Not asked.");
    let mut window =
        Watched::capturing_agents_published(case, ((80, 40), native), &vendor, None, &published);
    window.types_until("/catalog\r", "1 of 1 offered");
    window.types_until(AGENT, "Search  goose");
    window.types_until("\r", "esc to go back");
    takes(&mut window, "Add");
    window.types_until("", "bytes to download");
    takes(&mut window, "Add");
    window.types_until("", "installed");
    window.types_until("\x1b", "an agent, or a licence");
    window.types_until("\x1b", "Added goose 1.0.0");
    let root = window.home().join("programs").join(AGENT).join("1.0.0");
    window.types_until("/login\r", "Use an installed coding agent");
    takes(&mut window, "Use an installed coding agent");
    window.types_until("", "Which installed agent");
    takes(&mut window, AGENT);
    window.types_until("", "Sign in at a terminal");
    takes(&mut window, "Sign in at a terminal");
    window.types_until("", "Nothing starts");
    takes(&mut window, "Continue");
    (window, published, root)
}

/// Fails where `/login` will not run the method, shows nothing the command
/// says, delivers nothing typed, or draws success from anything but the door.
#[test]
fn a_sign_in_command_runs_through_a_panel_showing_it_and_taking_what_is_typed() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("sign-in-panel-{mode}");
        let (mut window, _published, root) = signing_in(
            &case,
            native,
            &terminal_method(),
            vec![
                read_first(),
                Reply::new()
                    .say("hello from the sign-in command")
                    .prompt("your code: ")
                    .hear(1)
                    .say("thank you"),
                read_signed_in(),
            ],
        );
        window.types_until("", "hello from the sign-in command");
        window.types_until("", "your code: ");
        let panel = window.picture();
        assert!(panel.contains("tab hides"), "{case}: {panel}");
        assert!(panel.contains("esc stops"), "{case}: {panel}");
        window.types_until("one-two-three", "one-two-three");
        window.types_until("\r", "opened a session by terminal");
        assert!(
            fake_peer::heard(&root)
                .iter()
                .any(|line| line == "one-two-three"),
            "{case}: the command was not given the line"
        );
    }
}

/// Fails where a line the person hid is drawn, comes back through what the
/// command says, or the next line stays hidden too.
#[test]
fn a_line_put_in_the_secret_box_reaches_the_command_and_is_never_drawn() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("sign-in-secret-{mode}");
        let (mut window, _published, root) = signing_in(
            &case,
            native,
            &terminal_method(),
            vec![
                read_first(),
                Reply::new()
                    .prompt("password: ")
                    .echo(1)
                    .say("done with the password")
                    .prompt("your name: ")
                    .hear(1),
                read_signed_in(),
            ],
        );
        window.types_until("", "password: ");
        window.types_until("\t", "hidden");
        window.types(HIDDEN_LINE);
        let marks = "•".repeat(HIDDEN_LINE.chars().count());
        window.types_until("", &marks);
        assert!(
            !window.picture().contains(HIDDEN_LINE),
            "{case}: the hidden line was drawn"
        );
        window.types_until("\r", "done with the password");
        assert!(
            fake_peer::heard(&root)
                .iter()
                .any(|line| line == HIDDEN_LINE),
            "{case}: the command was not given the line"
        );
        assert!(
            !window.picture().contains(HIDDEN_LINE),
            "{case}: what the command said brought the hidden line back"
        );

        // Back to plain once it was sent: this one is shown as it is typed.
        window.types_until("anne", "anne");
        window.types_until("\r", "opened a session by terminal");
        assert!(
            !window.picture().contains(HIDDEN_LINE),
            "{case}: the hidden line was drawn once it was over"
        );
    }
}

/// Fails where a paste holding a line ending is split, sent or kept in the box
/// without a word, which a door refusing it would turn into a lost sign-in.
#[test]
fn a_paste_holding_a_line_ending_is_refused_in_the_box_and_nothing_is_sent() {
    let case = "sign-in-paste";
    let (mut window, _published, root) = signing_in(
        case,
        false,
        &terminal_method(),
        vec![read_first(), Reply::new().prompt("password: ").hear(1)],
    );
    window.types_until("", "password: ");
    window.types_until("\t", "hidden");
    window.types_until("\x1b[200~first\nsecond\x1b[201~", "holds a line ending");
    let refused = window.picture();
    assert!(refused.contains("nothing was sent"), "{case}: {refused}");
    assert!(!refused.contains("second"), "{case}: {refused}");
    assert!(
        fake_peer::heard(&root)
            .iter()
            .all(|line| !line.contains("second")),
        "{case}: the command was given the paste"
    );
    window.types_until("\x1b", "the sign-in was stopped");
}

/// Fails where the method the agent says it offers differs from the catalog's
/// record and the command runs anyway, or the panel opens over it.
#[test]
fn a_method_the_agent_says_differently_from_its_record_is_refused_unrun() {
    let case = "sign-in-differs";
    let mut said = terminal_method();
    said.as_object_mut()
        .expect("a method is a JSON object")
        .insert("args".to_owned(), serde_json::json!(["login", "--more"]));
    let (mut window, _published, root) = signing_in(
        case,
        false,
        &terminal_method(),
        vec![
            Reply::new().hear(1).say(offering(&said)),
            Reply::new().say("the command ran"),
        ],
    );
    window.types_until("", "so it was not run");
    let picture = window.picture();
    assert!(!picture.contains("the command ran"), "{case}: {picture}");
    assert_eq!(
        fake_peer::played(&root),
        1,
        "{case}: the command was started"
    );
}

/// Fails where Escape does not stop the command, or the panel leaves before
/// the door has said what came of it.
#[test]
fn escape_stops_a_sign_in_command_and_says_what_the_door_answered() {
    for native in [false, true] {
        let mode = if native { "native" } else { "fullscreen" };
        let case = format!("sign-in-stopped-{mode}");
        let (mut window, _published, _root) = signing_in(
            &case,
            native,
            &terminal_method(),
            vec![read_first(), Reply::new().say("waiting for you").hear(1)],
        );
        window.types_until("", "waiting for you");
        window.types_until("\x1b", "the sign-in was stopped");
        let picture = window.picture();
        assert!(
            !picture.contains("opened a session by terminal"),
            "{case}: {picture}"
        );
    }
}

/// Fails where a command that ended well is taken for a signed-in agent: only
/// what the door answers says that.
#[test]
fn a_command_that_ended_well_is_no_sign_in_until_the_door_says_so() {
    let case = "sign-in-unverified";
    let (mut window, _published, _root) = signing_in(
        case,
        false,
        &terminal_method(),
        vec![
            read_first(),
            Reply::new().say("all done, you are signed in"),
            Reply::new()
                .hear(1)
                .say(offering(&terminal_method()))
                .hear(1)
                .say(acp::sign_in_required(1)),
        ],
    );
    window.types_until("", "did not read as");
    let picture = window.picture();
    assert!(
        !picture.contains("opened a session by terminal"),
        "{case}: {picture}"
    );
}

/// Fails where what a command writes reaches the terminal as it was written:
/// a sequence in it would move the cursor or clear the screen under the panel.
#[test]
fn what_a_command_writes_cannot_draw_outside_the_panel() {
    let case = "sign-in-hostile";
    let (mut window, _published, _root) = signing_in(
        case,
        false,
        &terminal_method(),
        vec![
            read_first(),
            Reply::new().say("before\u{1b}[2J\u{1b}[1;1Hafter").hear(1),
        ],
    );
    window.types_until("", "beforeafter");
    let picture = window.picture();
    assert!(picture.contains("Recent sessions"), "{case}: {picture}");
    window.types_until("\x1b", "the sign-in was stopped");
}
