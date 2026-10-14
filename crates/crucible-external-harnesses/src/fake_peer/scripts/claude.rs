//! Replies in Claude Code's stream protocol, as the program would give.
//!
//! Each [`Reply`] is one start of the program: the `--version` probe, a status
//! check, or a run, which hears one `user` frame per turn and ends each turn
//! with its `result` frame. A run's `result` frames carry the session's
//! running cost, so a run's second priced turn reports a larger total than its
//! first. One run holds its turn open until it hears the stop, so a case can
//! read a conversation while a turn is under way. The named sequences open as
//! a connect does, with the probe and the status check, and then play their
//! runs in launch order.

use serde_json::{Value, json};

use crate::conformance::Set;
use crate::fake_peer::{Reply, Script};
use crucible_execution::{Decision, Lasting};

/// The session every run names.
pub const SESSION: &str = "1b6a4c0e-5f3d-4e8a-9c2b-7d1e0f9a3b42";

/// What the probe prints: a reviewed release.
pub const VERSION: &str = "2.1.296 (Claude Code)";

/// A priced turn's uncached input tokens.
pub const UNCACHED: u64 = 100;

/// A priced turn's cache-read input tokens.
pub const CACHE_READ: u64 = 20;

/// A priced turn's cache-written input tokens.
pub const CACHE_WRITE: u64 = 5;

/// A priced turn's output tokens.
pub const OUTPUT: u64 = 40;

/// What one priced turn costs, in millionths of a dollar.
pub const PRICE: u64 = 250_000;

/// The file a turn's effect edits.
pub const EDITED: &str = "src/lib.rs";

/// The id this side's interrupt is sent under.
const INTERRUPT: &str = "crucible-interrupt";

/// The frame that names the session as a turn begins.
#[must_use]
pub fn init() -> String {
    json!({
        "type": "system",
        "subtype": "init",
        "session_id": SESSION,
        "apiKeySource": "none",
        "permissionMode": "default",
        "claude_code_version": "2.1.296",
    })
    .to_string()
}

/// The model saying `text`.
#[must_use]
pub fn said(text: &str) -> String {
    assistant(&json!([{"type": "text", "text": text}]))
}

/// The model calling for [`EDITED`] to be changed: an effect under way.
#[must_use]
pub fn effect() -> String {
    assistant(&json!([{
        "type": "tool_use",
        "id": "toolu_edit",
        "name": "Edit",
        "input": {"file_path": EDITED, "old_string": "a", "new_string": "b"},
    }]))
}

/// The end of a turn answered, its figures those of a priced turn and the
/// session's running cost `total` millionths, where it reports one.
#[must_use]
pub fn answered(total: Option<u64>) -> String {
    let mut ended = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": "Done.",
        "session_id": SESSION,
        "usage": {
            "input_tokens": UNCACHED,
            "cache_read_input_tokens": CACHE_READ,
            "cache_creation_input_tokens": CACHE_WRITE,
            "output_tokens": OUTPUT,
        },
    });
    if let Some(total) = total
        && let Some(ended) = ended.as_object_mut()
    {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a script's totals are a few dollars, exact in a double"
        )]
        let dollars = total as f64 / 1e6;
        ended.insert("total_cost_usd".to_owned(), json!(dollars));
    }
    ended.to_string()
}

/// The end of a turn that was stopped, reporting no figures.
#[must_use]
pub fn interrupted() -> String {
    json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
        "session_id": SESSION,
    })
    .to_string()
}

/// What the program hears when its turn is stopped.
#[must_use]
pub fn stop() -> String {
    json!({
        "type": "control_request",
        "request_id": INTERRUPT,
        "request": {"subtype": "interrupt"},
    })
    .to_string()
}

/// The program's answer to the stop.
#[must_use]
pub fn stopped() -> String {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": INTERRUPT},
    })
    .to_string()
}

/// A question, under the request id `native`, whether to run a command.
#[must_use]
pub fn asked(native: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": native,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": command(),
            "tool_use_id": call(native),
            "title": "List the files",
            "permission_suggestions": [
                {
                    "type": "addRules",
                    "rules": [{"toolName": "Bash", "ruleContent": "ls"}],
                    "behavior": "allow",
                    "destination": "localSettings",
                },
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            ],
        },
    })
    .to_string()
}

/// The program taking back the question `native`, as it does for one open
/// when its turn is stopped.
#[must_use]
pub fn withdrawn(native: &str) -> String {
    json!({"type": "control_cancel_request", "request_id": native}).to_string()
}

/// What the program must hear when the question `native` is decided
/// `decision`: the command as it was asked about for an allow, with the rule
/// it proposed kept for the session alone where the allow lasts; a refusal
/// for anything else.
#[must_use]
pub fn answer(native: &str, decision: &Decision) -> String {
    let response = match decision {
        Decision::Allow(Lasting::Once) => {
            json!({"behavior": "allow", "updatedInput": command(), "toolUseID": call(native)})
        }
        Decision::Allow(Lasting::Session) => json!({
            "behavior": "allow",
            "updatedInput": command(),
            "toolUseID": call(native),
            "updatedPermissions": [{
                "type": "addRules",
                "rules": [{"toolName": "Bash", "ruleContent": "ls"}],
                "behavior": "allow",
                "destination": "session",
            }],
        }),
        Decision::Deny | Decision::Answer(_) => json!({
            "behavior": "deny",
            "message": "The user did not allow this.",
            "toolUseID": call(native),
        }),
    };
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": native, "response": response},
    })
    .to_string()
}

/// The probe's reply: a reviewed release.
#[must_use]
pub fn probe() -> Reply {
    Reply::new().say(VERSION)
}

/// A status check's reply for a signed-in profile.
#[must_use]
pub fn signed_in() -> Reply {
    Reply::new().say(
        json!({"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"})
            .to_string(),
    )
}

/// A status check's reply for a signed-out profile.
#[must_use]
pub fn signed_out() -> Reply {
    Reply::new()
        .say(json!({"loggedIn": false, "authMethod": "none"}).to_string())
        .exit(1)
}

/// A run of one clean turn, answered and priced.
#[must_use]
pub fn clean() -> Reply {
    priced_turn(Reply::new(), PRICE)
}

/// A run of one turn that asks one question before it answers.
#[must_use]
pub fn permission() -> Reply {
    permission_turn(Reply::new(), PRICE)
}

/// A run of one turn that is stopped once its effect is under way, and
/// reports no cost.
#[must_use]
pub fn interrupt() -> Reply {
    interrupt_turn(Reply::new())
}

/// A run of one turn that says something and then holds the turn open, with
/// no effect under way, until it hears the stop, which it answers.
#[must_use]
pub fn held_open() -> Reply {
    held_turn(Reply::new())
}

/// A run that crashes once its effect is under way.
#[must_use]
pub fn crash() -> Reply {
    turn(Reply::new()).say(effect()).exit(9)
}

/// A run of one turn whose `result` frame reports a cost.
#[must_use]
pub fn priced() -> Reply {
    priced_turn(Reply::new(), PRICE)
}

/// A run of one turn whose `result` frame reports none.
#[must_use]
pub fn unpriced() -> Reply {
    turn(Reply::new()).say(said("Done.")).say(answered(None))
}

/// Two runs of one turn each, the second resuming the first's session: each
/// `result` frame carries the session's running cost, so the second's is the
/// two turns' together.
#[must_use]
pub fn cumulative() -> [Reply; 2] {
    [priced(), priced_turn(Reply::new(), 2 * PRICE)]
}

/// A connect, then a run of a priced turn and an unpriced one.
#[must_use]
pub fn priced_then_unreported() -> Script {
    connected(turn(priced_turn(Reply::new(), PRICE)).say(answered(None)))
}

/// A connect, then a run of two priced turns.
#[must_use]
pub fn two_priced() -> Script {
    connected(priced_turn(priced_turn(Reply::new(), PRICE), 2 * PRICE))
}

/// A connect, then a run of a priced turn and one that is stopped.
#[must_use]
pub fn priced_then_interrupt() -> Script {
    connected(interrupt_turn(priced_turn(Reply::new(), PRICE)))
}

/// A connect, then a run of a priced turn and one held open until it is
/// stopped.
#[must_use]
pub fn priced_then_held() -> Script {
    connected(held_turn(priced_turn(Reply::new(), PRICE)))
}

/// A connect, then a run whose first turn is stopped.
#[must_use]
pub fn stopped_first() -> Script {
    connected(interrupt())
}

/// A connect, then a run of a priced turn, one that is stopped, and another
/// priced one.
#[must_use]
pub fn priced_interrupt_priced() -> Script {
    connected(priced_turn(
        interrupt_turn(priced_turn(Reply::new(), PRICE)),
        2 * PRICE,
    ))
}

/// A connect, then the two runs of [`cumulative`].
#[must_use]
pub fn cumulative_pair() -> Script {
    let [first, second] = cumulative();
    Script::of([probe(), signed_in(), first, second])
}

/// A connect, then a run of one turn that asks a question and one that is
/// stopped; a disconnect starts nothing.
#[must_use]
pub fn headless() -> Script {
    connected(interrupt_turn(permission_turn(Reply::new(), PRICE)))
}

/// The conformance set of the reviewed release.
#[must_use]
pub fn set() -> Set {
    Set {
        opening: vec![(1, vec![init()])],
        effect: effect(),
        ask: asked,
        answer,
        answered: answered(Some(PRICE)),
        cancelled: interrupted(),
        stop: Some(stop()),
    }
}

/// The probe, a signed-in status check, then `run`.
fn connected(run: Reply) -> Script {
    Script::of([probe(), signed_in(), run])
}

/// `reply`, then a turn begun: the program hears its `user` frame and names
/// the session.
fn turn(reply: Reply) -> Reply {
    reply.hear(1).say(init())
}

/// `reply`, then a turn answered with the session's running cost at `total`.
fn priced_turn(reply: Reply, total: u64) -> Reply {
    turn(reply).say(said("Done.")).say(answered(Some(total)))
}

/// `reply`, then a turn that asks the question `q`, hears its answer, and is
/// answered with the session's running cost at `total`.
fn permission_turn(reply: Reply, total: u64) -> Reply {
    turn(reply)
        .say(asked("q"))
        .hear(1)
        .say(said("Done."))
        .say(answered(Some(total)))
}

/// `reply`, then a turn whose effect is under way when it hears the stop,
/// which it answers, ending the turn with no figures.
fn interrupt_turn(reply: Reply) -> Reply {
    turn(reply)
        .say(effect())
        .hear(1)
        .say(stopped())
        .say(interrupted())
}

/// `reply`, then a turn that says something and waits to hear the stop, which
/// it answers, ending the turn with no figures.
fn held_turn(reply: Reply) -> Reply {
    turn(reply)
        .say(said("Working."))
        .hear(1)
        .say(stopped())
        .say(interrupted())
}

/// The model's message holding `content`.
fn assistant(content: &Value) -> String {
    json!({
        "type": "assistant",
        "session_id": SESSION,
        "parent_tool_use_id": null,
        "message": {"role": "assistant", "content": content},
    })
    .to_string()
}

/// The command every question asks to run.
fn command() -> Value {
    json!({"command": "ls", "description": "List the files"})
}

/// The id of the call the question `native` is about.
fn call(native: &str) -> String {
    format!("toolu_{native}")
}
