//! Each frame the agent writes is read for what it says and no more.

use std::path::Path;

use crucible_execution::{
    Cost, Decision, Detail, Event, Lasting, Outcome, Pending, TurnRequest, Usage,
};
use crucible_types::TurnId;
use serde_json::{Value, json};

use crate::claude::Claude;
use crate::{Act, Dialect, Shape};

/// The agent with a turn open, for a fresh session or a resumed one.
fn turning(resume: Option<&str>) -> Claude {
    let mut agent = Claude::default();
    let _ = agent.request(Shape {
        resume,
        workspace: Some(Path::new("/work")),
        ..Shape::default()
    });
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST, "hello"))
        .unwrap();
    agent
}

/// A `result` frame of `subtype` with these figures.
fn result(subtype: &str, failed: bool, input: u64, cost: f64) -> String {
    json!({
        "type": "result",
        "subtype": subtype,
        "is_error": failed,
        "result": "done",
        "usage": {
            "input_tokens": input,
            "cache_read_input_tokens": 10,
            "cache_creation_input_tokens": 5,
            "output_tokens": 7,
        },
        "total_cost_usd": cost,
    })
    .to_string()
}

/// The frame a turn opens with, naming the session `4f0c`.
fn init(key: &str, mode: &str, version: &str) -> String {
    json!({
        "type": "system",
        "subtype": "init",
        "session_id": "4f0c",
        "apiKeySource": key,
        "permissionMode": mode,
        "claude_code_version": version,
    })
    .to_string()
}

/// The usage among `acts`.
fn usage(acts: &[Act]) -> Option<&Usage> {
    acts.iter().find_map(|act| match act {
        Act::Say(Event::Usage(observed)) => Some(observed.value()),
        _ => None,
    })
}

/// A question whether `tool` may run with `input`.
fn asking(request: &str, tool: &str, input: &Value) -> String {
    json!({
        "type": "control_request",
        "request_id": request,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": tool,
            "input": input,
            "tool_use_id": "toolu_1",
            "permission_suggestions": [
                {"type": "addRules", "rules": [{"toolName": tool, "ruleContent": "ls"}], "behavior": "allow", "destination": "localSettings"},
                {"type": "setMode", "mode": "bypassPermissions", "destination": "session"},
                {"type": "addDirectories", "directories": ["/"], "destination": "session"},
            ],
        },
    })
    .to_string()
}

#[test]
fn the_prompt_is_one_user_frame() {
    let mut agent = Claude::default();
    let frames = agent
        .open(&TurnRequest::new(TurnId::FIRST, "hello"))
        .unwrap();

    let [frame] = frames.as_slice() else {
        panic!("{frames:?}")
    };
    let frame: Value = serde_json::from_str(frame).unwrap();
    assert_eq!(frame.pointer("/type"), Some(&json!("user")));
    assert_eq!(
        frame.pointer("/message/content/0/text"),
        Some(&json!("hello"))
    );
}

#[test]
fn a_prompt_too_long_for_a_frame_is_refused() {
    let long = "a".repeat(crucible_transport::FRAME_BYTES);

    assert!(
        Claude::default()
            .open(&TurnRequest::new(TurnId::FIRST, &long))
            .is_err()
    );
}

/// A run for a session the agent kept resumes it; a fresh one does not.
#[test]
fn only_a_kept_session_is_resumed() {
    let mut agent = Claude::default();
    let fresh = agent.request(Shape::default()).arguments;
    let resumed = agent
        .request(Shape {
            resume: Some("4f0c"),
            ..Shape::default()
        })
        .arguments;

    assert!(!fresh.iter().any(|argument| argument == "--resume"));
    assert!(resumed.windows(2).any(|pair| pair == ["--resume", "4f0c"]));
}

#[test]
fn the_session_is_named_from_its_first_frame() {
    let acts = turning(None)
        .heard(&init("none", "default", "2.1.296"))
        .unwrap();

    assert!(matches!(acts.as_slice(), [Act::Session(session)] if session == "4f0c"));
}

/// A turn whose agent says it runs on a key nobody signed the profile in
/// with, in a mode that asks less than it was started in, or as a release
/// nobody reviewed, ends before anything it says is read.
#[test]
fn a_turn_the_agent_runs_otherwise_than_it_was_started_ends() {
    for (key, mode, version) in [
        ("ANTHROPIC_API_KEY", "default", "2.1.296"),
        ("apiKeyHelper", "default", "2.1.296"),
        ("none", "bypassPermissions", "2.1.296"),
        ("none", "acceptEdits", "2.1.296"),
        ("none", "auto", "2.1.296"),
        ("none", "default", "2.2.0"),
        ("none", "default", "2.1.276"),
    ] {
        assert!(
            turning(None).heard(&init(key, mode, version)).is_err(),
            "{key} {mode} {version}"
        );
    }
    for (key, mode) in [
        ("none", "plan"),
        ("none", "dontAsk"),
        ("/login managed key", "default"),
    ] {
        assert!(
            turning(None).heard(&init(key, mode, "2.1.277")).is_ok(),
            "{key} {mode}"
        );
    }
}

/// The main loop's text is said; a task's is not.
#[test]
fn only_the_main_loops_text_is_said() {
    let mut agent = turning(None);
    let said = |parent: Value| {
        json!({
            "type": "assistant",
            "parent_tool_use_id": parent,
            "message": {"content": [{"type": "text", "text": "hi"}, {"type": "thinking", "thinking": "hm"}]},
        })
        .to_string()
    };

    let main = agent.heard(&said(Value::Null)).unwrap();
    let task = agent.heard(&said(json!("toolu_9"))).unwrap();

    assert!(matches!(main.as_slice(), [Act::Say(Event::Delta(text))] if text.as_str() == "hi"));
    assert!(task.is_empty(), "{task:?}");
}

/// An edit is reported changed once its call reports, and not when it
/// failed.
#[test]
fn an_edit_that_reports_success_changes_its_file() {
    let mut agent = turning(None);
    let call = |id: &str| {
        json!({
            "type": "assistant",
            "parent_tool_use_id": null,
            "message": {"content": [{"type": "tool_use", "id": id, "name": "Edit", "input": {"file_path": "/work/a.rs"}}]},
        })
        .to_string()
    };
    let report = |id: &str, failed: bool| {
        json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": failed}]},
        })
        .to_string()
    };

    let activity = agent.heard(&call("toolu_1")).unwrap();
    let _ = agent.heard(&call("toolu_2")).unwrap();
    let changed = agent.heard(&report("toolu_1", false)).unwrap();
    let failed = agent.heard(&report("toolu_2", true)).unwrap();

    assert!(matches!(
        activity.as_slice(),
        [Act::Say(Event::ToolActivity { tool, summary })] if tool.as_str() == "Edit" && summary.as_str() == "/work/a.rs"
    ));
    assert!(matches!(
        changed.as_slice(),
        [Act::Say(Event::FileChanged { path })] if path.as_str() == "/work/a.rs"
    ));
    assert!(failed.is_empty(), "{failed:?}");
}

/// The turn's tokens are its own, the input summed from its parts, and its
/// cost is what the session's running total grew by.
#[test]
fn a_turn_reports_its_tokens_and_what_the_session_cost_grew_by() {
    let mut agent = turning(None);

    let first = agent.heard(&result("success", false, 100, 0.25)).unwrap();
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST.next(), "again"))
        .unwrap();
    let second = agent.heard(&result("success", false, 40, 0.375)).unwrap();

    let first_usage = usage(&first).unwrap();
    assert_eq!(first_usage.input_tokens(), Some(115));
    assert_eq!(first_usage.cache_read_tokens(), Some(10));
    assert_eq!(first_usage.cache_write_tokens(), Some(5));
    assert_eq!(first_usage.output_tokens(), Some(7));
    assert_eq!(first_usage.spent().map(Cost::millionths), Some(250_000));
    assert_eq!(
        usage(&second).unwrap().spent().map(Cost::millionths),
        Some(125_000)
    );
    assert!(matches!(first.last(), Some(Act::End(Outcome::Answered))));
}

/// A resumed session cost an amount this run never heard, so its first turn
/// reports tokens and no cost.
#[test]
fn a_resumed_sessions_first_turn_reports_no_cost() {
    let mut agent = turning(Some("4f0c"));

    let acts = agent.heard(&result("success", false, 100, 3.0)).unwrap();

    let usage = usage(&acts).unwrap();
    assert_eq!(usage.input_tokens(), Some(115));
    assert_eq!(usage.spent(), None);
}

/// After a turn that did not say what the session cost, the running total is
/// not known, so the next priced turn reports no cost rather than the whole
/// total as its own.
#[test]
fn a_priced_turn_after_an_unpriced_one_reports_no_cost() {
    let mut agent = turning(None);
    let unpriced = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": "done",
        "usage": {"input_tokens": 1, "output_tokens": 1},
    });
    let _ = agent.heard(&unpriced.to_string()).unwrap();
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST.next(), "b"))
        .unwrap();
    let priced = agent.heard(&result("success", false, 1, 0.5)).unwrap();
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST.next().next(), "c"))
        .unwrap();
    let after = agent.heard(&result("success", false, 1, 0.75)).unwrap();

    assert_eq!(usage(&priced).unwrap().spent(), None);
    assert_eq!(
        usage(&after).unwrap().spent().map(Cost::millionths),
        Some(250_000)
    );
}

/// A running total that fell, as after the session was cleared, reports no
/// cost, and the turn after it is differenced from it.
#[test]
fn a_running_total_that_fell_reports_no_cost() {
    let mut agent = turning(None);
    let _ = agent.heard(&result("success", false, 1, 2.0)).unwrap();
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST.next(), "b"))
        .unwrap();
    let fell = agent.heard(&result("success", false, 1, 0.5)).unwrap();
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST.next().next(), "c"))
        .unwrap();
    let after = agent.heard(&result("success", false, 1, 0.75)).unwrap();

    assert_eq!(usage(&fell).unwrap().spent(), None);
    assert_eq!(
        usage(&after).unwrap().spent().map(Cost::millionths),
        Some(250_000)
    );
}

#[test]
fn each_end_is_read_as_what_it_is() {
    let end = |stopping: bool, frame: &str| {
        let mut agent = turning(None);
        if stopping {
            let _ = agent.cancel();
        }
        agent.heard(frame).unwrap().pop()
    };

    assert!(matches!(
        end(true, &result("error_during_execution", true, 1, 0.0)),
        Some(Act::End(Outcome::Cancelled))
    ));
    assert!(matches!(
        end(false, &result("error_max_turns", true, 1, 0.0)),
        Some(Act::End(Outcome::CutShort))
    ));
    assert!(matches!(
        end(false, &result("error_during_execution", true, 1, 0.0)),
        Some(Act::Fail(_))
    ));
    assert!(matches!(
        end(false, &result("success", true, 1, 0.0)),
        Some(Act::Fail(failure)) if failure.reason().as_str().contains("done")
    ));
}

/// The agent saying it is not signed in fails the turn saying how to sign
/// in, and its words are not said as an answer.
#[test]
fn a_signed_out_turn_fails_saying_how_to_sign_in() {
    let mut agent = turning(None);
    let said = agent
        .heard(
            &json!({
                "type": "assistant",
                "parent_tool_use_id": null,
                "error": "authentication_failed",
                "message": {"content": [{"type": "text", "text": "Not logged in · Please run /login"}]},
            })
            .to_string(),
        )
        .unwrap();
    let ended = agent.heard(&result("success", true, 0, 0.0)).unwrap();

    assert!(said.is_empty(), "{said:?}");
    assert!(matches!(
        ended.last(),
        Some(Act::Fail(failure)) if failure.reason().as_str().contains("/login")
    ));
}

/// A kind of frame, or of system frame, the reviewed releases do not write
/// ends the turn incomplete.
#[test]
fn an_unreviewed_frame_is_refused() {
    let mut agent = turning(None);

    for frame in [
        json!({"type": "brand_new"}),
        json!({"type": "system", "subtype": "brand_new"}),
        json!({"type": "result", "subtype": "brand_new"}),
        json!({"no": "type"}),
    ] {
        assert!(agent.heard(&frame.to_string()).is_err(), "{frame}");
    }
    assert!(agent.heard("not json").is_err());
    for said_nothing in [
        json!({"type": "keep_alive"}),
        json!({"type": "active_goal", "value": null}),
        json!({"type": "autocompact_state"}),
    ] {
        assert!(
            agent.heard(&said_nothing.to_string()).unwrap().is_empty(),
            "{said_nothing}"
        );
    }
}

#[test]
fn a_question_is_raised_as_what_the_tool_would_do() {
    let mut agent = turning(None);
    let detail = |agent: &mut Claude, tool: &str, input: Value| match agent
        .heard(&asking("r1", tool, &input))
        .unwrap()
        .pop()
    {
        Some(Act::Ask {
            native,
            pending: Pending::Permission { detail, .. },
        }) => {
            let _ = agent.answer(&native, &Decision::Deny).unwrap();
            detail
        }
        other => panic!("{other:?}"),
    };

    assert!(matches!(
        detail(&mut agent, "Bash", json!({"command": "ls", "run_in_background": true})),
        Detail::Command { sent, left_running: true } if sent.as_str() == "ls"
    ));
    assert_eq!(
        detail(&mut agent, "Read", json!({"file_path": "/work/a"})),
        Detail::Reads
    );
    assert_eq!(
        detail(&mut agent, "Read", json!({"file_path": "src/a"})),
        Detail::Reads
    );
    for outside in [
        "/etc/passwd",
        "/work/../etc/passwd",
        "../a",
        "~/.ssh/id_rsa",
        "~",
    ] {
        assert_eq!(
            detail(&mut agent, "Read", json!({"file_path": outside})),
            Detail::ReadsOutside
        );
    }
    assert_eq!(
        detail(
            &mut agent,
            "Glob",
            json!({"pattern": "src/**/*.rs", "path": "/work"})
        ),
        Detail::Reads
    );
    for pattern in ["/home/u/.ssh/*", "~/.ssh/*", "../**"] {
        assert_eq!(
            detail(
                &mut agent,
                "Glob",
                json!({"pattern": pattern, "path": "/work"})
            ),
            Detail::ReadsOutside,
            "{pattern}"
        );
    }
    assert_eq!(
        detail(&mut agent, "Write", json!({"file_path": "/work/a"})),
        Detail::MutatesFile
    );
    assert!(matches!(
        detail(&mut agent, "WebFetch", json!({"url": "https://example.com"})),
        Detail::Network { sent } if sent.as_str() == "https://example.com"
    ));
    assert!(matches!(
        detail(&mut agent, "mcp__server__tool", json!({"x": 1})),
        Detail::Command { sent, .. } if sent.as_str().contains("\"x\"")
    ));
}

/// An allow sends the call's input back as it was asked about; only an
/// allow for the session carries rules, and only the agent's rules that
/// allow the tool, kept for this session.
#[test]
fn only_an_allow_for_the_session_carries_rules_and_only_for_the_session() {
    let input = json!({"command": "ls"});
    let answered = |decision: &Decision| {
        let mut agent = turning(None);
        let _ = agent.heard(&asking("r1", "Bash", &input)).unwrap();
        let frame: Value = serde_json::from_str(&agent.answer("r1", decision).unwrap()).unwrap();
        frame.pointer("/response").cloned().unwrap_or_default()
    };

    let once = answered(&Decision::Allow(Lasting::Once));
    let session = answered(&Decision::Allow(Lasting::Session));
    let denied = answered(&Decision::Deny);

    assert_eq!(once.pointer("/request_id"), Some(&json!("r1")));
    assert_eq!(once.pointer("/response/behavior"), Some(&json!("allow")));
    assert_eq!(once.pointer("/response/updatedInput"), Some(&input));
    assert_eq!(once.pointer("/response/toolUseID"), Some(&json!("toolu_1")));
    assert!(once.pointer("/response/updatedPermissions").is_none());
    assert_eq!(
        session.pointer("/response/updatedPermissions"),
        Some(
            &json!([{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "ls"}], "behavior": "allow", "destination": "session"}])
        )
    );
    assert_eq!(denied.pointer("/response/behavior"), Some(&json!("deny")));
    assert!(denied.pointer("/response/updatedInput").is_none());
}

/// A session allow carries only the proposed rules for the tool asked about
/// that name what they allow, so a yes to one command grants neither the
/// whole tool nor another tool.
#[test]
fn a_session_allow_keeps_only_the_rules_for_the_tool_asked_about() {
    let mut agent = turning(None);
    let ask = json!({
        "type": "control_request",
        "request_id": "r1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": {"command": "ls"},
            "tool_use_id": "toolu_1",
            "permission_suggestions": [
                {
                    "type": "addRules",
                    "rules": [
                        {"toolName": "Bash", "ruleContent": "ls"},
                        {"toolName": "Bash"},
                        {"toolName": "Write"},
                    ],
                    "behavior": "allow",
                    "destination": "session",
                },
                {"type": "addRules", "rules": [{"toolName": "WebFetch"}], "behavior": "allow", "destination": "session"},
            ],
        },
    });
    let _ = agent.heard(&ask.to_string()).unwrap();

    let frame: Value = serde_json::from_str(
        &agent
            .answer("r1", &Decision::Allow(Lasting::Session))
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        frame.pointer("/response/response/updatedPermissions"),
        Some(&json!([{
            "type": "addRules",
            "rules": [{"toolName": "Bash", "ruleContent": "ls"}],
            "behavior": "allow",
            "destination": "session",
        }]))
    );
}

/// Where the agent says a lasting rule would allow more than the ask, a
/// session allow is sent as an allow for this call alone.
#[test]
fn a_session_allow_the_agent_says_is_too_broad_lasts_for_the_call_alone() {
    let mut agent = turning(None);
    let mut ask: Value =
        serde_json::from_str(&asking("r1", "Bash", &json!({"command": "ls"}))).unwrap();
    if let Some(request) = ask.pointer_mut("/request").and_then(Value::as_object_mut) {
        request.insert("suppress_always_allow_rule".to_owned(), json!(true));
    }
    let _ = agent.heard(&ask.to_string()).unwrap();

    let frame: Value = serde_json::from_str(
        &agent
            .answer("r1", &Decision::Allow(Lasting::Session))
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        frame.pointer("/response/response/behavior"),
        Some(&json!("allow"))
    );
    assert!(
        frame
            .pointer("/response/response/updatedPermissions")
            .is_none(),
        "{frame}"
    );
}

/// A question is answered once, and never with written words.
#[test]
fn a_question_is_answered_once_and_only_as_a_permission() {
    let mut agent = turning(None);
    let _ = agent
        .heard(&asking("r1", "Bash", &json!({"command": "ls"})))
        .unwrap();

    assert!(
        agent
            .answer(
                "r1",
                &Decision::Answer(crucible_execution::Text::new("yes"))
            )
            .is_err()
    );
    assert!(agent.answer("r1", &Decision::Deny).is_ok());
    assert!(agent.answer("r1", &Decision::Deny).is_err());
    assert!(agent.answer("r2", &Decision::Deny).is_err());
}

/// A request this host does not serve is refused under its own id.
#[test]
fn any_other_request_is_refused() {
    let mut agent = turning(None);
    let acts = agent
        .heard(
            &json!({"type": "control_request", "request_id": "r7", "request": {"subtype": "mcp_message"}})
                .to_string(),
        )
        .unwrap();

    let [Act::Send(frame)] = acts.as_slice() else {
        panic!("{acts:?}")
    };
    let frame: Value = serde_json::from_str(frame).unwrap();
    assert_eq!(frame.pointer("/response/subtype"), Some(&json!("error")));
    assert_eq!(frame.pointer("/response/request_id"), Some(&json!("r7")));
}

/// The answer to this side's interrupt is passed over; an answer to anything
/// else this side never asked is refused.
#[test]
fn only_the_interrupt_is_answered() {
    let mut agent = turning(None);
    let interrupt: Value = serde_json::from_str(&agent.cancel().unwrap()).unwrap();
    let answer = |id: &Value| {
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": id}})
            .to_string()
    };

    assert_eq!(
        interrupt.pointer("/request/subtype"),
        Some(&json!("interrupt"))
    );
    assert!(
        agent
            .heard(&answer(
                interrupt.pointer("/request_id").unwrap_or(&Value::Null)
            ))
            .unwrap()
            .is_empty()
    );
    assert!(agent.heard(&answer(&json!("other"))).is_err());
}

/// A read through a link inside the workspace to a place outside it is asked
/// about as a read outside.
#[cfg(unix)]
#[test]
fn a_read_through_a_link_out_of_the_workspace_is_a_read_outside() {
    let root = std::env::temp_dir().join(format!("crucible-claude-link-{}", std::process::id()));
    let workspace = root.join("work");
    std::fs::create_dir_all(&workspace).unwrap();
    let _ = std::fs::remove_file(workspace.join("out"));
    std::os::unix::fs::symlink(&root, workspace.join("out")).unwrap();
    std::fs::write(root.join("secret"), "x").unwrap();
    let mut agent = Claude::default();
    let _ = agent.request(Shape {
        workspace: Some(&workspace),
        ..Shape::default()
    });
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST, "hello"))
        .unwrap();

    let acts = agent
        .heard(&asking("r1", "Read", &json!({"file_path": "out/secret"})))
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        matches!(
            acts.last(),
            Some(Act::Ask {
                pending: Pending::Permission {
                    detail: Detail::ReadsOutside,
                    ..
                },
                ..
            })
        ),
        "{acts:?}"
    );
}
