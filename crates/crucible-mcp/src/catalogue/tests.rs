//! Handshakes and catalogues a server could offer, including the ones it should
//! not.

use std::{fmt::Write as _, io::Cursor};

use serde_json::{Value, json};

use super::{
    CURSOR_BYTES, Greeting, NAME_BYTES, Offered, PAGES, Rebuffed, SCHEMA_BYTES, TOOLS, VERSIONS,
    hello, tools,
};
use crate::talking::Talking;

/// A server's side of the conversation, one frame per line.
///
/// Written out in full before crucible reads a byte, which is the shape a
/// server that answers eagerly would have and one this crate has to be right
/// about either way.
fn script(frames: &[Value]) -> String {
    let mut written = String::new();
    for frame in frames {
        writeln!(written, "{frame}").expect("a string accepts what is written to it");
    }
    written
}

/// One member of a message, or a panic saying which one was not there.
fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key} in {value}"))
}

/// Every `tools/list` crucible sent.
fn asking(said: &[Value]) -> Vec<Value> {
    said.iter()
        .filter(|message| message.get("method") == Some(&json!("tools/list")))
        .cloned()
        .collect()
}

/// One of the messages crucible sent.
fn nth(said: &[Value], which: usize) -> &Value {
    said.get(which)
        .unwrap_or_else(|| panic!("crucible sent no message {which}"))
}

/// An `initialize` answer that agrees with what crucible offers.
fn agreeable() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": VERSIONS[0],
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "docs", "version": "1.2.3" },
        },
    })
}

/// Greets a server against a script, and hands back the greeting and what
/// crucible said.
fn greet(frames: &[Value]) -> (Result<Greeting, Rebuffed>, Vec<Value>) {
    let mut said = Vec::new();
    let greeting = {
        let mut talking = Talking::new(Cursor::new(script(frames)), &mut said);
        hello(&mut talking)
    };
    (greeting, spoken(&said))
}

/// Greets a server and then reads its catalogue, against one script.
fn read(frames: &[Value]) -> (Result<Vec<Offered>, Rebuffed>, Vec<Value>) {
    let mut said = Vec::new();
    let read = {
        let mut talking = Talking::new(Cursor::new(script(frames)), &mut said);
        let greeting = hello(&mut talking).expect("the greeting in these scripts is agreeable");
        tools(&mut talking, &greeting)
    };
    (read, spoken(&said))
}

/// What crucible wrote, read back as messages.
fn spoken(said: &[u8]) -> Vec<Value> {
    String::from_utf8(said.to_vec())
        .expect("crucible writes text")
        .lines()
        .map(|line| serde_json::from_str(line).expect("crucible writes messages"))
        .collect()
}

#[test]
fn a_greeting_offers_the_newest_version_and_says_the_handshake_finished() {
    let (greeting, said) = greet(&[agreeable()]);

    let greeting = greeting.expect("the server agreed");
    assert_eq!(greeting.version(), VERSIONS[0]);
    assert_eq!(greeting.named(), Some("docs"));
    assert!(greeting.offers(), "the server said it has tools");

    let greeted = nth(&said, 0);
    assert_eq!(at(greeted, "method"), "initialize");
    assert_eq!(at(at(greeted, "params"), "protocolVersion"), VERSIONS[0]);
    assert_eq!(
        at(at(greeted, "params"), "capabilities"),
        &json!({}),
        "crucible offers a server nothing, so a server cannot plan on asking"
    );
    let finished = nth(&said, 1);
    assert_eq!(
        at(finished, "method"),
        "notifications/initialized",
        "the protocol's order: nothing else may be asked until this was sent"
    );
    assert!(
        finished.get("id").is_none(),
        "a notification is not a call and nothing answers it"
    );
    assert_eq!(
        said.len(),
        2,
        "a greeting is those two messages and no more"
    );
}

#[test]
fn an_older_version_both_ends_speak_is_agreed() {
    // The negotiation the protocol describes: crucible offers its newest, and a
    // server that does not have it names one it does.
    let older = *VERSIONS
        .last()
        .expect("crucible speaks at least one version");
    let (greeting, _) = greet(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "protocolVersion": older, "capabilities": {} },
    })]);

    let greeting = greeting.expect("crucible speaks that one too");
    assert_eq!(greeting.version(), older);
    assert_eq!(greeting.named(), None, "the server did not say");
    assert!(
        !greeting.offers(),
        "a server that claimed no tools capability offers none"
    );
}

#[test]
fn a_version_crucible_does_not_speak_ends_the_conversation() {
    let (greeting, said) = greet(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "protocolVersion": "2031-01-01", "capabilities": {} },
    })]);

    let Err(Rebuffed::Version { found, spoken }) = greeting else {
        panic!("expected the version to be refused, got {greeting:?}");
    };
    assert_eq!(&*found, "2031-01-01");
    assert!(
        spoken.contains(VERSIONS[0]),
        "the refusal says what it does speak"
    );
    assert_eq!(
        said.len(),
        1,
        "the handshake never finished, so crucible does not say it did"
    );
}

#[test]
fn an_answer_carrying_no_version_is_refused() {
    let (greeting, _) = greet(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "capabilities": {} },
    })]);

    let Err(Rebuffed::Missing { field, .. }) = greeting else {
        panic!("expected the missing version to be refused, got {greeting:?}");
    };
    assert_eq!(field, "protocolVersion");
}

#[test]
fn a_server_that_says_it_has_no_tools_is_not_asked_for_any() {
    let mut said = Vec::new();
    let read = {
        let mut talking = Talking::new(
            Cursor::new(script(&[json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": { "protocolVersion": VERSIONS[0], "capabilities": {} },
            })])),
            &mut said,
        );
        let greeting = hello(&mut talking).expect("the server agreed");
        tools(&mut talking, &greeting)
    };

    assert_eq!(read.expect("no tools is not a failure"), Vec::new());
    let said = spoken(&said);
    assert!(
        said.iter()
            .all(|message| message.get("method") != Some(&json!("tools/list"))),
        "the capability is the server's own answer, and asking past it ignores it"
    );
}

#[test]
fn every_page_of_a_catalogue_is_read() {
    let (read, said) = read(&[
        agreeable(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{
                    "name": "search",
                    "description": "Looks something up",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "q": { "type": "string" } },
                    },
                }],
                "nextCursor": "further",
            },
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": { "tools": [{ "name": "fetch" }] },
        }),
    ]);

    let read = read.expect("both pages are ordinary");
    assert_eq!(read.len(), 2);
    let searching = read.first().expect("the first page held one");
    assert_eq!(searching.name(), "search");
    assert_eq!(searching.about(), Some("Looks something up"));
    assert_eq!(
        at(at(at(searching.schema(), "properties"), "q"), "type"),
        "string"
    );
    let fetching = read.last().expect("the second page held one");
    assert_eq!(fetching.name(), "fetch");
    assert_eq!(fetching.about(), None);
    assert_eq!(
        fetching.schema(),
        &json!({}),
        "a tool that takes no arguments is an ordinary tool"
    );

    let listing = asking(&said);
    assert_eq!(listing.len(), 2);
    let first = at(nth(&listing, 0), "params");
    assert!(
        first.get("cursor").is_none(),
        "the first page is asked for without one"
    );
    assert_eq!(
        at(at(nth(&listing, 1), "params"), "cursor"),
        "further",
        "the second is asked for with the cursor the first handed back"
    );
}

#[test]
fn a_catalogue_that_never_ends_is_refused_rather_than_followed() {
    let mut frames = vec![agreeable()];
    for id in 2..2 + PAGES + 4 {
        frames.push(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": [], "nextCursor": "again" },
        }));
    }
    let (read, said) = read(&frames);

    let Err(Rebuffed::Endless { most }) = read else {
        panic!("expected an endless catalogue to be refused, got {read:?}");
    };
    assert_eq!(most, PAGES);
    assert_eq!(
        asking(&said).len(),
        PAGES,
        "it stops asking rather than reading forever"
    );
}

#[test]
fn a_catalogue_longer_than_crucible_reads_is_refused_whole() {
    // Refused rather than cut short. A shorter list that looked complete would
    // have the model told this server has these tools and not those.
    let listed: Vec<Value> = (0..=TOOLS)
        .map(|which| json!({ "name": format!("tool{which}") }))
        .collect();
    let (read, _) = read(&[
        agreeable(),
        json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": listed } }),
    ]);

    let Err(Rebuffed::TooMany { most }) = read else {
        panic!("expected the catalogue to be refused, got {read:?}");
    };
    assert_eq!(most, TOOLS);
}

#[test]
fn exactly_as_many_tools_as_the_bound_allows_are_read() {
    // The awkward legal case. A ceiling written with the comparison one out
    // would refuse this catalogue, and every test above would still pass.
    let listed: Vec<Value> = (0..TOOLS)
        .map(|which| json!({ "name": format!("tool{which}") }))
        .collect();
    let (read, _) = read(&[
        agreeable(),
        json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": listed } }),
    ]);

    assert_eq!(read.expect("a full catalogue is a legal one").len(), TOOLS);
}

#[test]
fn a_spelling_past_its_ceiling_is_refused() {
    for (field, held) in [
        ("name", json!({ "name": "n".repeat(NAME_BYTES + 1) })),
        (
            "inputSchema",
            json!({ "name": "big", "inputSchema": { "about": "s".repeat(SCHEMA_BYTES) } }),
        ),
    ] {
        let (read, _) = read(&[
            agreeable(),
            json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [held] } }),
        ]);

        let Err(Rebuffed::TooLong { field: refused, .. }) = read else {
            panic!("expected {field} to be refused, got {read:?}");
        };
        assert_eq!(refused, field);
    }
}

#[test]
fn a_tool_with_no_usable_name_is_refused() {
    for held in [json!({ "description": "nameless" }), json!({ "name": "" })] {
        let (read, _) = read(&[
            agreeable(),
            json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [held.clone()] } }),
        ]);

        let Err(Rebuffed::Missing { field, .. }) = read else {
            panic!("expected {held} to be refused, got {read:?}");
        };
        assert_eq!(field, "name");
    }
}

#[test]
fn two_tools_under_one_name_are_refused() {
    // A name is what the model acts on, so two meanings for one of them is a
    // call whose outcome depends on which the reader happened to keep.
    let (read, _) = read(&[
        agreeable(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": { "tools": [{ "name": "search" }, { "name": "search" }] },
        }),
    ]);

    let Err(Rebuffed::Twice { name }) = read else {
        panic!("expected the repeated name to be refused, got {read:?}");
    };
    assert_eq!(&*name, "search");
}

#[test]
fn a_page_that_is_not_a_listing_is_refused() {
    let (read, _) = read(&[
        agreeable(),
        json!({ "jsonrpc": "2.0", "id": 2, "result": { "nextCursor": "further" } }),
    ]);

    let Err(Rebuffed::Missing { field, .. }) = read else {
        panic!("expected a page without a listing to be refused, got {read:?}");
    };
    assert_eq!(field, "tools");
}

#[test]
fn a_conversation_that_fails_is_reported_as_the_conversation_failing() {
    // Nothing at all: the server closed before answering.
    let (greeting, _) = greet(&[]);

    assert!(
        matches!(greeting, Err(Rebuffed::Talking(_))),
        "expected the conversation itself to be blamed, got {greeting:?}"
    );
}

#[test]
fn two_tools_one_permission_rule_could_not_tell_apart_are_refused_as_one_name() {
    // A rule reads a tool's name without case, so these are two tools here and
    // one name to anything anybody could write about them: a verdict given for
    // the first would be spent on the second without being asked for.
    let (read, _) = read(&[
        agreeable(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": { "tools": [{ "name": "search" }, { "name": "SEARCH" }] },
        }),
    ]);

    let Err(Rebuffed::Twice { name }) = read else {
        panic!("expected the repeated name to be refused, got {read:?}");
    };
    assert_eq!(&*name, "SEARCH");
}

#[test]
fn a_cursor_past_its_ceiling_is_refused_rather_than_handed_back() {
    // The one string here crucible never reads: it is the server's own, kept
    // only to be given back. Something nothing looks at is something nothing
    // would notice growing, so it is held to a ceiling like every other
    // spelling off a pipe.
    let (read, _) = read(&[
        agreeable(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{ "name": "search" }],
                "nextCursor": "c".repeat(CURSOR_BYTES + 1),
            },
        }),
    ]);

    let Err(Rebuffed::TooLong { field, .. }) = read else {
        panic!("expected the cursor to be refused, got {read:?}");
    };
    assert_eq!(field, "nextCursor");
}
