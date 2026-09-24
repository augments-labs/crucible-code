//! A value a server was given through `envFrom` stays out of everything it
//! says back, however the server's JSON spelled it.

use crucible_sandbox::{
    SandboxCredentialHandle, SandboxCredentialProjection, SandboxCredentialProvenance,
};

use super::*;

/// A value JSON has to escape: a quote and a newline, which a server's
/// serializer writes as `\"` and `\n`, so the bytes on the pipe are never the
/// value's own.
const CANARY: &str = "sk-\"canary\"\nvalue-9c41";

/// The `docs` server, given `value` as its one `envFrom` secret.
fn given(value: &str) -> Chosen {
    let credential = SandboxCredentialProjection::new(
        SandboxCredentialHandle::new("env:0", SandboxCredentialProvenance::User)
            .expect("a credential handle"),
        "DOCS_TOKEN",
        value,
    )
    .expect("a credential projection");
    Chosen::new("docs", PROGRAM, [], policy())
        .given(
            SandboxEnvironment::with_credentials([], [credential])
                .expect("an environment carrying one credential"),
        )
        .waiting(PATIENCE, PATIENCE, GRACE)
        .required(true)
}

/// Starts the server given `value`, which says `frames`, and calls its
/// `search` tool once.
fn called(value: &str, frames: Vec<Value>) -> Result<ToolOutput, ToolError> {
    let sandbox = Pretend::new([Answers::Says(frames)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![given(value)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");
    let ran = awaited(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", "{}"),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &Cancel::new(),
            None,
            &Nothing,
        ),
    ));
    // Settled answers leave the conversation able to stop politely.
    drop(awaited(hosting.dispose(&context)));
    ran
}

/// Everything an error says, down its whole chain of causes.
fn told(error: &ToolError) -> String {
    let mut said = format!("{error} / {error:?}");
    let mut cause = std::error::Error::source(error);
    while let Some(one) = cause {
        said = format!("{said} / {one}");
        cause = one.source();
    }
    said
}

#[test]
fn an_envfrom_value_a_server_echoes_in_a_tool_result_never_reaches_it() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced(&format!("token {CANARY} accepted"), false));

    let output = called(CANARY, frames).expect("the server answered");

    let text = output.text();
    assert!(
        !text.contains(CANARY),
        "an envFrom value a server echoed in a tool result reached it: {text:?}"
    );
    assert_eq!(
        text,
        format!("token {} accepted", "*".repeat(CANARY.len())),
        "the rest of the result is kept, and the value reads as one `*` per byte"
    );
}

#[test]
fn an_envfrom_value_a_server_echoes_in_an_error_never_reaches_it() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "error": { "code": -32000, "message": format!("token {CANARY} refused") },
    }));

    let error = called(CANARY, frames).expect_err("the server refused the call");

    let said = told(&error);
    assert!(
        !said.contains(CANARY),
        "an envFrom value a server echoed in an error reached it: {said}"
    );
    assert!(
        said.contains(&format!("token {} refused", "*".repeat(CANARY.len()))),
        "the rest of the server's words are kept: {said}"
    );
}

#[test]
fn an_envfrom_value_a_server_echoes_in_its_catalogue_never_reaches_a_schema() {
    let frames = opening(
        "docs",
        &json!([{
            "name": "search",
            "description": format!("uses {CANARY}"),
            "inputSchema": {
                "type": "object",
                "properties": { CANARY: { "type": "string", "default": CANARY } },
            },
        }]),
    );
    let sandbox = Pretend::new([Answers::Says(frames)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![given(CANARY)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let schema = snapshot
        .find("mcp:docs/search")
        .expect("the offered tool")
        .descriptor()
        .schema()
        .to_owned();
    awaited(hosting.dispose(&context)).expect("the server stopped");

    // The schema is JSON the model reads, so the value would be in it escaped.
    let escaped = serde_json::to_string(CANARY).expect("a string serializes");
    let escaped = escaped.trim_matches('"');
    assert!(
        !schema.contains(CANARY) && !schema.contains(escaped),
        "an envFrom value a server echoed in its catalogue reached a schema: {schema}"
    );
}

#[test]
fn an_envfrom_value_of_1_leaves_every_frame_read_and_hides_only_what_it_said() {
    // Every frame here carries `"id":1` or another number with a 1 in it, and
    // an error code does too; none of them is anything the server said, so
    // all of it has to be read as it was sent.
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("v1 done", false));

    let output = called("1", frames).expect("every frame was read");

    assert_eq!(
        output.text(),
        "v* done",
        "an envFrom value of 1 in what a server said was not masked"
    );
}

#[test]
fn an_envfrom_value_a_server_answers_as_its_version_never_reaches_the_refusal() {
    let sandbox = Pretend::new([Answers::Says(vec![json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "protocolVersion": CANARY, "capabilities": { "tools": {} } },
    })])]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![given(CANARY)],
        crate::testing::runtime(),
    );
    let context = lifecycle();

    let refused =
        awaited(hosting.prepare(&context)).expect_err("a version crucible does not speak");
    drop(awaited(hosting.dispose(&context)));

    let said = format!("{refused} / {refused:?}");
    assert!(
        !said.contains(CANARY),
        "an envFrom value a server answered as its version reached the refusal: {said}"
    );
    assert!(
        said.contains(&"*".repeat(CANARY.len())),
        "the refusal still says what the server answered: {said}"
    );
}

#[test]
fn an_envfrom_value_a_server_echoes_overlapping_itself_never_reaches_a_result() {
    // Two copies sharing their middle: the second starts inside the first, so
    // a search that resumes after each match never sees it whole.
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("tokentokentoken", false));

    let output = called("tokentoken", frames).expect("the server answered");

    assert_eq!(
        output.text(),
        "*".repeat(15),
        "an envFrom value a server echoed overlapping itself was left partly shown"
    );
}
