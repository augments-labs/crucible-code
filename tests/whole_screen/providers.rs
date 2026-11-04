//! The vendors this release adds, at the key box: a key that does not fit the
//! row it was typed on is refused there, with nothing stored and nothing sent,
//! and the box stands again for the one that does.

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
    window.types_until("/agents\r", "1 of 1 offered");
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
