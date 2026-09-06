//! What one request body says, field by field.
//!
//! Separate from the builder next door only because the builder reached the
//! per-file cap.

use crucible_core::{
    Attached, Change, Changed, Content, Diff, Effort, Fragment, Line, Modality, ToolArgs, ToolId,
    ToolOutput, Transcript,
};
use serde_json::json;

use super::*;
use crate::fake::{cached, failed, found, observed, picture};

/// What a pointer finds when there is nothing there.
const NOTHING: Value = Value::Null;

#[test]
fn recap_is_fresh_visible_text_without_executable_history() {
    let mut request = request(crate::fake::recap_history());
    request.purpose = crucible_core::RequestPurpose::Recap;
    let body = build(&request);
    assert!(
        body.get("input")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message.get("role").unwrap() == "user"
                && message.get("content").unwrap().is_string())
    );
    let text = body.to_string();
    for visible in [
        "old question",
        "old answer",
        "lookup",
        "call-1",
        "old result",
        "summarize",
    ] {
        assert!(text.contains(visible));
    }
    assert!(!text.contains("private-signature-canary"));
    assert!(!text.contains("function_call"));
}

fn request(transcript: Transcript) -> Request<'static> {
    Request {
        purpose: crucible_core::RequestPurpose::Turn,
        model: "gpt-test",
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        attached: &[],
        max_tokens: 1024,
        system: None,
        effort: None,
        prompt_cache: None,
    }
}

fn said(text: &str) -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said(text))
        .expect("valid fixture transcript");
    transcript
}

/// The four bytes a PNG starts with, which encode to `iVBORw==`.
const PIXEL: &[u8] = &[0x89, b'P', b'N', b'G'];

/// The line the runner writes in place of a file it did not send.
const INSTEAD: &str = "holiday.png is not attached to this request, to keep the request \
     within its size limit: read it again if you need it.";

/// The five bytes a PDF starts with, which encode to `JVBERi0=`.
const PAGES: &[u8] = b"%PDF-";

/// A prompt the runner resolved one picture for.
fn holding(text: &str, content: Content<'static>) -> Request<'static> {
    carrying(text, "image/png", Modality::Image, content)
}

/// A prompt the runner resolved one attachment of a stated kind for.
fn carrying(
    text: &str,
    media_type: &'static str,
    modality: Modality,
    content: Content<'static>,
) -> Request<'static> {
    // The transcript's own reference is deliberately absent: a provider reads
    // what the runner resolved and never a path.
    let mut request = request(said(text));
    request.attached = Box::leak(Box::new([Attached {
        message: 0,
        index: 0,
        media_type,
        modality,
        content,
    }]));
    request
}

/// One value by JSON pointer.
///
/// Indexing a `Value` panics on a shape that is not what it expected, which
/// turns a wrong assertion into a stack trace instead of a diff.
fn at<'a>(body: &'a Value, path: &str) -> &'a Value {
    body.pointer(path).unwrap_or(&NOTHING)
}

#[test]
fn a_request_streams_and_names_its_model() {
    // Not streaming would mean the answer appears all at once at the end,
    // which is the whole experience this harness is built around.
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/model"), "gpt-test");
    assert_eq!(at(&body, "/stream"), true);
}

#[test]
fn provider_default_automatic_caching_preserves_the_request_bytes() {
    let plain = request(said("hello"));
    let cached = cached(
        request(said("hello")),
        PromptCacheMechanism::AutomaticPrefix,
        PromptCacheRetentionClass::ProviderDefault,
        false,
    );

    assert_eq!(
        serialize(&cached, Serving::Api),
        serialize(&plain, Serving::Api)
    );
}

#[test]
fn observe_only_preserves_the_request_bytes() {
    let plain = request(said("hello"));
    let observed = observed(request(said("hello")));

    assert_eq!(
        serialize(&observed, Serving::Api),
        serialize(&plain, Serving::Api)
    );
}

#[test]
fn a_routing_key_is_encoded_only_when_the_prepared_request_supplies_one() {
    let without = cached(
        request(said("hello")),
        PromptCacheMechanism::AutomaticPrefix,
        PromptCacheRetentionClass::ProviderDefault,
        false,
    );
    let with = cached(
        request(said("hello")),
        PromptCacheMechanism::AutomaticPrefix,
        PromptCacheRetentionClass::ProviderDefault,
        true,
    );

    assert_eq!(at(&build(&without), "/prompt_cache_key"), &NOTHING);
    assert_eq!(
        at(&build(&with), "/prompt_cache_key"),
        &json!("44".repeat(32))
    );
}

#[test]
fn reviewed_retention_hints_use_the_model_specific_wire_shape() {
    let mut current = request(said("hello"));
    current.model = "gpt-5.6-sol";
    let current = cached(
        current,
        PromptCacheMechanism::AutomaticPrefix,
        PromptCacheRetentionClass::Ephemeral,
        false,
    );
    let mut older = request(said("hello"));
    older.model = "gpt-5.5";
    let older = cached(
        older,
        PromptCacheMechanism::AutomaticPrefix,
        PromptCacheRetentionClass::Extended,
        false,
    );

    assert_eq!(
        at(&build(&current), "/prompt_cache_options"),
        &json!({"mode": "implicit", "ttl": "30m"})
    );
    assert_eq!(at(&build(&older), "/prompt_cache_retention"), &json!("24h"));
}

#[test]
fn recap_uses_the_responses_breakpoint_field() {
    let mut transcript = Transcript::new();
    transcript.push(Message::said("stable history")).unwrap();
    transcript.push(Message::said("summarize")).unwrap();
    let mut request = request(transcript);
    request.model = "gpt-5.6-sol";
    request.purpose = crucible_core::RequestPurpose::Recap;
    let request = cached(
        request,
        PromptCacheMechanism::ExplicitBreakpoints,
        PromptCacheRetentionClass::ProviderDefault,
        false,
    );
    let body = build(&request);
    assert_eq!(
        at(&body, "/input/0/content/0/prompt_cache_breakpoint"),
        &json!({"mode":"explicit"})
    );
    assert_eq!(
        at(&body, "/input/0/content/0/text"),
        "User:\nstable history"
    );
    assert_eq!(at(&body, "/input/0/content/0/cache_control"), &Value::Null);
}

#[test]
fn explicit_caching_marks_one_legal_text_boundary_and_disables_implicit_writes() {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::Context(Fragment::new(
            "reference",
            "stable prefix",
        )))
        .expect("valid fixture transcript");
    transcript
        .push(Message::said("changing question"))
        .expect("valid fixture transcript");
    let mut explicit = request(transcript);
    explicit.model = "gpt-5.6-sol";
    let explicit = cached(
        explicit,
        PromptCacheMechanism::ExplicitBreakpoints,
        PromptCacheRetentionClass::ProviderDefault,
        false,
    );
    let body = build(&explicit);

    assert_eq!(
        at(&body, "/prompt_cache_options"),
        &json!({"mode": "explicit"})
    );
    assert_eq!(
        at(&body, "/input/0/content/0/prompt_cache_breakpoint"),
        &json!({"mode": "explicit"})
    );
    assert_eq!(
        prompt_cache_encoding(&explicit, Serving::Api),
        crucible_core::PromptCacheEncoding::BreakpointsEncoded(1)
    );
}

#[test]
fn explicit_caching_does_not_claim_a_later_unmarkable_tool_result_was_cached() {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::Context(Fragment::new("reference", "earlier text")))
        .expect("valid fixture transcript");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: Box::default(),
            calls: vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call_1"),
            output: found("result", Vec::new()),
        }]))
        .expect("valid fixture transcript");
    transcript
        .push(Message::said("changing question"))
        .expect("valid fixture transcript");
    let mut explicit = request(transcript);
    explicit.model = "gpt-5.6-sol";
    let explicit = cached(
        explicit,
        PromptCacheMechanism::ExplicitBreakpoints,
        PromptCacheRetentionClass::ProviderDefault,
        false,
    );

    assert_eq!(
        prompt_cache_encoding(&explicit, Serving::Api),
        PromptCacheEncoding::Failed(PromptCacheIneligibleReason::UnsupportedBoundary)
    );
    assert_eq!(
        serialize(&explicit, Serving::Api)
            .matches("prompt_cache_breakpoint")
            .count(),
        0
    );
}

#[test]
fn a_request_asks_the_vendor_not_to_keep_it() {
    // This endpoint retains a response for later retrieval unless told
    // otherwise, and what a coding agent sends is somebody's source. The
    // default is the one setting here that has to be overridden rather than
    // accepted.
    assert_eq!(at(&build(&request(said("hello"))), "/store"), false);
}

#[test]
fn the_generated_token_ceiling_reaches_the_endpoint() {
    // This endpoint counts reasoning and visible output together, which is
    // exactly what the domain request's generated-token ceiling bounds.
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/max_output_tokens"), 1024);
}

#[test]
fn the_plan_backend_is_not_sent_a_ceiling_it_refuses_the_whole_request_over() {
    // It does not implement the field and answers `Unsupported parameter:
    // max_output_tokens` with a 400, so this was every turn of every session
    // signed in with a plan rather than a key -- the whole provider, unusable,
    // on a field the other address requires.
    let body = served(&request(said("hello")), Serving::Subscription);

    assert_eq!(at(&body, "/max_output_tokens"), &NOTHING);

    // The rest of the request is unchanged: one field is missing there, not a
    // second body.
    assert_eq!(at(&body, "/model"), "gpt-test");
    assert_eq!(at(&body, "/stream"), true);
    assert_eq!(at(&body, "/store"), false);
    assert_eq!(at(&body, "/input/0/content"), "hello");
}

#[test]
fn a_session_nobody_told_how_hard_to_think_says_nothing_about_it() {
    // Which rungs a model serves is the model's business here, and one it does
    // not serve is a refusal. Leaving the field off is what keeps a session
    // nobody has an opinion about working on every model on the list.
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/reasoning"), &NOTHING);
}

#[test]
fn an_effort_somebody_chose_reaches_the_model_under_reasoning() {
    let mut asking = request(said("hello"));
    asking.effort = Some(Effort::Xhigh);

    assert_eq!(at(&build(&asking), "/reasoning/effort"), "xhigh");
}

#[test]
fn a_system_prompt_is_a_field_rather_than_a_message() {
    // Sent as a message it would be one more thing the model may answer,
    // rather than the instructions it answers under.
    let mut asking = request(said("hello"));
    asking.system = Some("be brief");

    let body = build(&asking);

    assert_eq!(at(&body, "/instructions"), "be brief");
    assert_eq!(at(&body, "/input/0/role"), "user");
    assert_eq!(at(&body, "/input/0/content"), "hello");
    assert_eq!(at(&body, "/input/1"), &NOTHING);
}

#[test]
fn typed_context_is_sent_as_retained_model_input() {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::Context(Fragment::new(
            "workspace",
            "Workspace: /src",
        )))
        .expect("valid fixture transcript");
    transcript
        .push(Message::said("continue"))
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/0/role"), "user");
    assert_eq!(at(&body, "/input/0/content"), "Workspace: /src");
    assert_eq!(at(&body, "/input/1/content"), "continue");
}

#[test]
fn a_session_without_a_system_prompt_sends_no_instructions() {
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/instructions"), &NOTHING);
    assert_eq!(at(&body, "/input/0/content"), "hello");
}

#[test]
fn a_tool_call_is_an_item_of_its_own_beside_what_was_said() {
    // The shape that differs most from the older endpoint: a call is not part
    // of the message that made it, and its result is not part of a message
    // either. Both are items in one flat list.
    let mut transcript = said("read it");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "reading".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "read".into(),
                args: ToolArgs::new("{\"path\":\"a.rs\"}"),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/1/role"), "assistant");
    assert_eq!(at(&body, "/input/1/content"), "reading");
    assert_eq!(at(&body, "/input/2/type"), "function_call");
    assert_eq!(at(&body, "/input/2/call_id"), "call_1");
    assert_eq!(at(&body, "/input/2/name"), "read");

    // Text, not an object. Re-encoding would hand the model back something it
    // did not write.
    assert_eq!(at(&body, "/input/2/arguments"), "{\"path\":\"a.rs\"}");
}

#[test]
fn a_tool_call_with_no_words_before_it_sends_no_message_at_all() {
    // A model that goes straight to a tool says nothing first, and an empty
    // message is an item with no content for it to read back.
    let mut transcript = said("read it");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/1/type"), "function_call");
}

#[test]
fn a_turn_that_produced_nothing_at_all_still_sends_something_to_hold() {
    // A cancelled turn and a filtered one both reach here with no text and no
    // calls. Sending neither would leave a gap in the transcript where a turn
    // was.
    let mut transcript = said("hello");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: Vec::new(),
            stop: Some(StopReason::Cancelled),
        })
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/1"), &NOTHING);
}

#[test]
fn a_turn_that_was_cut_off_is_not_sent_back_as_one_the_model_finished() {
    // The live notice tells the user; nothing told the model. So the next turn
    // — and every turn of a continued session — showed it its own half-sentence
    // as an answer it had chosen to end there.
    let mut transcript = said("write it all out");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "as I was say".into(),
            calls: Vec::new(),
            stop: Some(StopReason::OutOfTokens),
        })
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/1/content"), "as I was say");
    assert_eq!(
        at(&body, "/input/2/content"),
        StopReason::cut(Some(StopReason::OutOfTokens)).expect("a cut-off turn")
    );
}

#[test]
fn a_turn_the_model_ended_itself_carries_no_note() {
    // The path taken every time. A note under each answer would be spent on
    // the ordinary ending and teach the model nothing.
    let mut transcript = said("hello");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "hello back".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        })
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/2"), &NOTHING);
}

#[test]
fn a_tool_that_takes_no_arguments_still_sends_parsable_text() {
    // An empty string is not JSON on the other side, and the model is handed
    // this back as the arguments it wrote.
    let mut transcript = said("what time is it");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "clock".into(),
                args: ToolArgs::new("  "),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/1/arguments"), "{}");
}

#[test]
fn every_result_of_a_turn_is_its_own_item_naming_the_call_it_answers() {
    let mut transcript = said("read them");
    transcript
        .push(Message::ToolResults(vec![
            ToolResult {
                id: ToolId::new("call_1"),
                output: ToolOutput::ok("first"),
            },
            ToolResult {
                id: ToolId::new("call_2"),
                output: ToolOutput::ok("second"),
            },
        ]))
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/1/type"), "function_call_output");
    assert_eq!(at(&body, "/input/1/call_id"), "call_1");
    assert_eq!(at(&body, "/input/1/output"), "first");
    assert_eq!(at(&body, "/input/2/call_id"), "call_2");
    assert_eq!(at(&body, "/input/2/output"), "second");
}

#[test]
fn a_failed_result_says_so_in_the_only_place_this_wire_has() {
    // There is no field for it. Unmarked, "no such file: x" reads as the
    // contents of a file that was read successfully.
    let mut transcript = said("read it");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call_1"),
            output: ToolOutput::failed("no such file: a.rs"),
        }]))
        .expect("valid fixture transcript");

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/input/1/output"), "error: no such file: a.rs");
}

#[test]
fn a_tool_is_advertised_flat_with_its_schema_and_its_description() {
    // Flat is the difference from the older endpoint, where the same three
    // fields sit nested under a `function` object. Sent that way here the
    // request is refused outright.
    let mut asking = request(said("hello"));
    asking.tools = Box::leak(Box::new([ToolSchema {
        name: "read",
        schema: r#"{"description":"Reads a file","type":"object",
                    "properties":{"path":{"type":"string"}}}"#,
    }]));

    let body = build(&asking);

    assert_eq!(at(&body, "/tools/0/type"), "function");
    assert_eq!(at(&body, "/tools/0/name"), "read");
    assert_eq!(at(&body, "/tools/0/description"), "Reads a file");
    assert_eq!(at(&body, "/tools/0/parameters/type"), "object");
    assert_eq!(
        at(&body, "/tools/0/parameters/properties/path/type"),
        "string"
    );

    // The description is the tool's, not the schema's: a schema that carried
    // one into `parameters` would describe the arguments object.
    assert_eq!(at(&body, "/tools/0/parameters/description"), &NOTHING);

    // Said rather than left out. Strict mode requires every property to be
    // required and additional ones refused, which is not what these schemas
    // say, so a default that changed would change how a tool is validated.
    assert_eq!(at(&body, "/tools/0/strict"), false);
}

#[test]
fn a_session_with_no_tools_sends_no_tools_field() {
    // An empty array is refused rather than read as a session with no tools.
    assert_eq!(at(&build(&request(said("hello"))), "/tools"), &NOTHING);
}

/// The shape this vendor's documentation described on 2026-08-23: an
/// `input_image` part whose `image_url` is a `data:` URL, ahead of the prompt's
/// own `input_text` part. `detail` is optional and left off, so the endpoint
/// applies the default it documents rather than one this harness invented.
#[test]
fn an_image_is_a_data_url_part_before_the_prompt() {
    let body = build(&holding("what is in this", Content::Bytes(PIXEL)));

    assert_eq!(
        at(&body, "/input/0/content"),
        &json!([
            {
                "type": "input_image",
                "image_url": "data:image/png;base64,iVBORw=="
            },
            { "type": "input_text", "text": "what is in this" }
        ]),
        "{body}"
    );
}

/// The shape this vendor's documentation described on 2026-08-23: an
/// `input_file` part carrying the same `data:` URL a picture travels in, under
/// `file_data` rather than `image_url`, beside a `filename` the endpoint
/// requires of base64 and reads the kind from.
///
/// The name is the attachment's place in the transcript. A provider is handed
/// what the runner resolved and never a path, so the file's own name is not
/// here to send -- and the prompt beside it already says which file the person
/// meant.
#[test]
fn a_pdf_is_an_input_file_part_before_the_prompt() {
    let body = build(&carrying(
        "what does this say",
        "application/pdf",
        Modality::Pdf,
        Content::Bytes(PAGES),
    ));

    assert_eq!(
        at(&body, "/input/0/content"),
        &json!([
            {
                "type": "input_file",
                "filename": "attachment-0-0.pdf",
                "file_data": "data:application/pdf;base64,JVBERi0="
            },
            { "type": "input_text", "text": "what does this say" }
        ]),
        "{body}"
    );
}

#[test]
fn a_file_that_was_not_sent_is_the_runners_sentence_in_its_place() {
    let body = build(&holding("what is in this", Content::Instead(INSTEAD)));

    assert_eq!(
        at(&body, "/input/0/content"),
        &json!([
            { "type": "input_text", "text": INSTEAD },
            { "type": "input_text", "text": "what is in this" }
        ]),
        "the sentence is printed, not composed: {body}"
    );
}

#[test]
fn a_prompt_with_nothing_attached_is_the_string_it_always_was() {
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/input/0/content"), "hello", "{body}");
}

/// A prompt that named a file and said nothing else. The picture is the whole
/// message, and an empty text part beside it is one this vendor refuses the
/// request over rather than ignores.
#[test]
fn a_prompt_that_is_only_a_file_sends_no_empty_text_part() {
    let body = build(&holding("", Content::Bytes(PIXEL)));

    assert_eq!(
        at(&body, "/input/0/content"),
        &json!([{
            "type": "input_image",
            "image_url": "data:image/png;base64,iVBORw=="
        }]),
        "{body}"
    );
}

/// A turn whose tool results the runner resolved these attachments for.
fn answering(results: Vec<ToolResult>, attached: Vec<Attached<'static>>) -> Request<'static> {
    let mut transcript = said("find me one");
    transcript
        .push(Message::ToolResults(results))
        .expect("valid fixture transcript");
    let mut request = request(transcript);
    request.attached = Box::leak(attached.into_boxed_slice());
    request
}

/// One file the runner resolved, at its place across the message above.
fn resolved(
    index: usize,
    media_type: &'static str,
    modality: Modality,
    content: Content<'static>,
) -> Attached<'static> {
    Attached {
        message: 1,
        index,
        media_type,
        modality,
        content,
    }
}

/// The picture a tool found, resolved.
fn resolved_picture(index: usize) -> Attached<'static> {
    resolved(index, "image/png", Modality::Image, Content::Bytes(PIXEL))
}

/// One call, answered with the words and files a tool came back with.
fn one(output: ToolOutput) -> Vec<ToolResult> {
    vec![ToolResult {
        id: ToolId::new("call_1"),
        output,
    }]
}

#[test]
fn a_tool_that_found_a_picture_sends_it_inside_the_result() {
    let body = build(&answering(
        one(found("one match: holiday.png", vec![picture()])),
        vec![resolved_picture(0)],
    ));

    assert_eq!(
        at(&body, "/input/1"),
        &json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": [
                { "type": "input_text", "text": "one match: holiday.png" },
                { "type": "input_image", "image_url": "data:image/png;base64,iVBORw==" }
            ]
        }),
        "{body}"
    );
}

#[test]
fn a_tool_that_only_found_a_picture_sends_no_empty_text_part() {
    let body = build(&answering(
        one(found("", vec![picture()])),
        vec![resolved_picture(0)],
    ));

    assert_eq!(
        at(&body, "/input/1/output"),
        &json!([{ "type": "input_image", "image_url": "data:image/png;base64,iVBORw==" }]),
        "{body}"
    );
}

#[test]
fn a_failed_result_that_found_a_file_marks_the_words_it_leads_with() {
    let body = build(&answering(
        one(failed("could not open it", vec![picture()])),
        vec![resolved_picture(0)],
    ));

    assert_eq!(
        at(&body, "/input/1/output/0"),
        &json!({ "type": "input_text", "text": "error: could not open it" }),
        "{body}"
    );
}

#[test]
fn each_result_gets_the_files_its_own_call_found() {
    let body = build(&answering(
        vec![
            ToolResult {
                id: ToolId::new("call_1"),
                output: found("first", vec![picture()]),
            },
            ToolResult {
                id: ToolId::new("call_2"),
                output: found("second", vec![picture()]),
            },
        ],
        vec![
            resolved_picture(0),
            resolved(1, "application/pdf", Modality::Pdf, Content::Bytes(PAGES)),
        ],
    ));

    assert_eq!(
        at(&body, "/input/1/output/1/type"),
        &json!("input_image"),
        "the first call found the picture: {body}"
    );
    assert_eq!(
        at(&body, "/input/2/output/1/type"),
        &json!("input_file"),
        "the second found the document: {body}"
    );
}

/// What the reader was shown is for the reader, and the request must not be
/// able to tell.
///
/// The bytes rather than the value: two bodies that parse alike could still
/// have been written differently, and what is promised is the request itself
/// rather than a reading of it. Green before an output carries anything
/// reader-side and green afterwards — a guard written beside the change it
/// exists to catch would be recording that change instead of catching it.
///
/// Both shapes, because only one of them is the shape a request is ever built
/// from. The lines are dropped where the call answers, before the result joins
/// the transcript, so a result on its way to a provider carries the counts and
/// never the diff — and a guard that only varied the diff would be pinning the
/// shape this path cannot produce while leaving the shape it always produces
/// free to move.
#[test]
fn the_request_body_is_the_same_whatever_the_reader_was_shown() {
    let text = "fn main() {}";
    let plain = serialize(
        &answering(one(ToolOutput::ok(text)), Vec::new()),
        Serving::Api,
    );
    let shown = serialize(
        &answering(
            one(ToolOutput::ok(text).showing(Diff::new([Line::new(1, Change::Added, text)]))),
            Vec::new(),
        ),
        Serving::Api,
    );
    let counted = serialize(
        &answering(
            one(ToolOutput::ok(text).counting(Changed::new(2, 1))),
            Vec::new(),
        ),
        Serving::Api,
    );

    assert_eq!(plain, shown, "the reader's copy reached the wire");
    assert_eq!(plain, counted, "the reader's counts reached the wire");
}
