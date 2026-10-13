//! The frames Claude Code writes in its stream protocol, read once.
//!
//! Every frame is one JSON object on a line, told apart by its `type`, and
//! system frames further by their `subtype`. Each kind the reviewed releases
//! write is named below and either read for what it means or passed over as
//! saying nothing a turn reports; a kind named nowhere is refused, which ends
//! the turn incomplete, so a release that says something new is not read as
//! if it had said something known.

use crucible_execution::{Cost, Currency, Failure};
use serde_json::Value;

/// The largest amount a cost may report, in dollars: far beyond any session,
/// and small enough that its millionths fit in a `u64`.
const AMOUNT_CEILING: f64 = 1e12;

/// The kinds of frame that report nothing a turn carries: partial messages,
/// progress, status, suggestions and the program's own bookkeeping.
const PASSED_OVER: [&str; 11] = [
    "stream_event",
    "tool_progress",
    "tool_use_summary",
    "auth_status",
    "rate_limit_event",
    "prompt_suggestion",
    "conversation_reset",
    "keep_alive",
    "transcript_mirror",
    "active_goal",
    "autocompact_state",
];

/// The system frames that report nothing a turn carries.
const SYSTEM_PASSED_OVER: [&str; 30] = [
    "api_retry",
    "compact_boundary",
    "status",
    "hook_started",
    "hook_progress",
    "hook_response",
    "plugin_install",
    "task_started",
    "task_progress",
    "task_updated",
    "task_notification",
    "task_summary",
    "background_tasks_changed",
    "thinking_tokens",
    "session_state_changed",
    "session_metadata",
    "worker_shutting_down",
    "commands_changed",
    "notification",
    "files_persisted",
    "memory_recall",
    "permission_denied",
    "model_refusal_fallback",
    "model_refusal_no_fallback",
    "local_command_output",
    "control_request_progress",
    "elicitation_complete",
    "mirror_error",
    "informational",
    "post_turn_summary",
];

/// One frame, as far as a turn is concerned.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Frame {
    /// The session began or went on, as the program runs it.
    Session(Init),
    /// A message from the model.
    Assistant(Assistant),
    /// What the tools the model called reported, each a call's id and
    /// whether it failed.
    Reported(Vec<(String, bool)>),
    /// The turn ended.
    Result(Ended),
    /// The program asks whether a tool may run.
    Ask(Ask),
    /// The program asks something else of its host, under this request id.
    Asked(String),
    /// The program answered the request with this id.
    Answered(String),
    /// The program withdrew the question with this id.
    Withdrawn(String),
    /// Nothing a turn reports.
    Nothing,
}

/// How the program runs the session a turn opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Init {
    /// The session's name.
    pub(crate) session: String,
    /// Where the key it sends requests with came from, `none` where it
    /// sends none.
    pub(crate) key: String,
    /// The permission mode it runs in.
    pub(crate) mode: String,
    /// Its version.
    pub(crate) version: String,
}

/// A message from the model.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Assistant {
    /// Whether the main loop said it, rather than a task the model started.
    pub(crate) main: bool,
    /// Its text, block by block.
    pub(crate) text: Vec<String>,
    /// The tools it called, each its id, name and input.
    pub(crate) calls: Vec<(String, String, Value)>,
    /// Whether it is the program saying the account is not signed in.
    pub(crate) signed_out: bool,
}

/// How a turn ended, as its `result` frame says.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ended {
    /// Its subtype.
    pub(crate) subtype: Subtype,
    /// Whether the program counts it a failure.
    pub(crate) failed: bool,
    /// What it said, where it is a success with words of its own.
    pub(crate) said: Option<String>,
    /// The turn's token figures: uncached input, cache read, cache write and
    /// output, each where the frame gives it.
    pub(crate) tokens: [Option<u64>; 4],
    /// The session's running cost, where the frame gives one.
    pub(crate) cost: Option<Cost>,
}

/// What kind of end a `result` frame reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subtype {
    /// The turn answered.
    Success,
    /// The turn stopped while it ran, stopped by the host or failing.
    DuringExecution,
    /// The turn hit a limit of its own.
    Limit,
}

/// A question whether a tool may run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ask {
    /// The id its answer carries.
    pub(crate) request: String,
    /// The tool.
    pub(crate) tool: String,
    /// The call it is about.
    pub(crate) call: String,
    /// The call's input, which an allow sends back.
    pub(crate) input: Value,
    /// The rules the program would add for an answer that lasts, where it
    /// offers any.
    pub(crate) lasting: Option<Value>,
    /// What the program says the call does, where it says.
    pub(crate) title: Option<String>,
}

/// What `line` means.
///
/// # Errors
///
/// A [`Failure`] where it is not a frame of a reviewed kind, or one whose
/// kind it is lacks what that kind must carry.
pub(crate) fn frame(line: &str) -> Result<Frame, Failure> {
    let frame: Value = serde_json::from_str(line)
        .map_err(|_| Failure::new("The agent wrote a line that is not a frame of its protocol."))?;
    let kind = text(&frame, "type")?;
    match kind {
        "system" => system(&frame),
        "assistant" => Ok(Frame::Assistant(assistant(&frame)?)),
        "user" => Ok(Frame::Reported(reported(&frame))),
        "result" => Ok(Frame::Result(ended(&frame)?)),
        "control_request" => control(&frame),
        "control_response" => {
            let response = frame.get("response").ok_or_else(missing)?;
            Ok(Frame::Answered(text(response, "request_id")?.to_owned()))
        }
        "control_cancel_request" => Ok(Frame::Withdrawn(text(&frame, "request_id")?.to_owned())),
        kind if PASSED_OVER.contains(&kind) => Ok(Frame::Nothing),
        _ => Err(unreviewed()),
    }
}

/// A system frame.
fn system(frame: &Value) -> Result<Frame, Failure> {
    match text(frame, "subtype")? {
        "init" => Ok(Frame::Session(Init {
            session: text(frame, "session_id")?.to_owned(),
            key: text(frame, "apiKeySource")?.to_owned(),
            mode: text(frame, "permissionMode")?.to_owned(),
            version: text(frame, "claude_code_version")?.to_owned(),
        })),
        subtype if SYSTEM_PASSED_OVER.contains(&subtype) => Ok(Frame::Nothing),
        _ => Err(unreviewed()),
    }
}

/// A message from the model.
fn assistant(frame: &Value) -> Result<Assistant, Failure> {
    let message = frame.get("message").ok_or_else(missing)?;
    let mut said = Assistant {
        main: frame.get("parent_tool_use_id").is_none_or(Value::is_null),
        signed_out: frame.get("error").and_then(Value::as_str) == Some("authentication_failed"),
        ..Assistant::default()
    };
    for block in blocks(message) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => said.text.push(text(block, "text")?.to_owned()),
            Some("tool_use") => said.calls.push((
                text(block, "id")?.to_owned(),
                text(block, "name")?.to_owned(),
                block.get("input").cloned().unwrap_or(Value::Null),
            )),
            // Thinking, and the blocks of tools the model's own service ran,
            // say nothing this side reports.
            _ => {}
        }
    }
    Ok(said)
}

/// The tool results a user frame carries, each a call's id and whether it
/// failed; a frame of anything else carries none.
fn reported(frame: &Value) -> Vec<(String, bool)> {
    frame
        .get("message")
        .map(blocks)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|block| {
            let call = block.get("tool_use_id")?.as_str()?.to_owned();
            let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
            Some((call, failed))
        })
        .collect()
}

/// The end a `result` frame reports.
fn ended(frame: &Value) -> Result<Ended, Failure> {
    let subtype = match text(frame, "subtype")? {
        "success" => Subtype::Success,
        "error_during_execution" => Subtype::DuringExecution,
        "error_max_turns" | "error_max_budget_usd" | "error_max_structured_output_retries" => {
            Subtype::Limit
        }
        _ => return Err(unreviewed()),
    };
    let usage = frame.get("usage");
    let figure = |name: &str| {
        usage
            .and_then(|usage| usage.get(name))
            .and_then(Value::as_u64)
    };
    Ok(Ended {
        subtype,
        failed: frame.get("is_error").and_then(Value::as_bool) == Some(true),
        said: frame
            .get("result")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tokens: [
            figure("input_tokens"),
            figure("cache_read_input_tokens"),
            figure("cache_creation_input_tokens"),
            figure("output_tokens"),
        ],
        cost: frame.get("total_cost_usd").and_then(dollars),
    })
}

/// A request the program makes of its host.
fn control(frame: &Value) -> Result<Frame, Failure> {
    let request_id = text(frame, "request_id")?.to_owned();
    let request = frame.get("request").ok_or_else(missing)?;
    if text(request, "subtype")? != "can_use_tool" {
        return Ok(Frame::Asked(request_id));
    }
    let title = ["title", "description", "display_name"]
        .into_iter()
        .find_map(|name| request.get(name).and_then(Value::as_str))
        .map(str::to_owned);
    Ok(Frame::Ask(Ask {
        request: request_id,
        tool: text(request, "tool_name")?.to_owned(),
        call: text(request, "tool_use_id")?.to_owned(),
        input: request.get("input").cloned().ok_or_else(missing)?,
        // The agent marks an ask whose lasting rule would allow more than
        // the ask itself; it offers no rule to keep.
        lasting: request
            .get("permission_suggestions")
            .filter(|rules| rules.is_array())
            .filter(|_| {
                request
                    .get("suppress_always_allow_rule")
                    .and_then(Value::as_bool)
                    != Some(true)
            })
            .cloned(),
        title,
    }))
}

/// The content blocks of `message`.
fn blocks(message: &Value) -> impl Iterator<Item = &Value> {
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The text `object` holds under `name`.
fn text<'a>(object: &'a Value, name: &str) -> Result<&'a str, Failure> {
    object.get(name).and_then(Value::as_str).ok_or_else(missing)
}

/// An amount of dollars, as a cost.
fn dollars(amount: &Value) -> Option<Cost> {
    let amount = amount.as_f64()?;
    if !amount.is_finite() || !(0.0..=AMOUNT_CEILING).contains(&amount) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the amount is finite, not negative and at most the ceiling, so its millionths, rounded, fit"
    )]
    let millionths = (amount * 1e6).round() as u64;
    Some(Cost::new(millionths, Currency::new("USD")?))
}

/// A frame of a kind the reviewed releases do not write.
fn unreviewed() -> Failure {
    Failure::new("The agent wrote a kind of frame its reviewed release does not write.")
}

/// A frame that lacks what its kind carries.
fn missing() -> Failure {
    Failure::new("The agent wrote a frame that lacks what its kind carries.")
}
