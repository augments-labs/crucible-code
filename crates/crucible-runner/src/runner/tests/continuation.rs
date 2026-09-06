//! Private response state survives only complete turns, never partial streams.

use super::*;
use crucible_core::{Continuation, ContinuationData, ContinuationPart, ContinuationScope};

fn state() -> Continuation {
    let mut state = Continuation::new(
        "fixture-v1",
        "claude-test",
        ContinuationScope::from_digest([0; 32]),
    )
    .unwrap();
    state
        .push(ContinuationPart::Opaque(
            ContinuationData::new("private-signature-canary").unwrap(),
        ))
        .unwrap();
    state
}

#[test]
fn complete_reasoning_only_answer_is_retained_and_recorded() {
    let sample = Sample::new("continuation-recorded");
    let script = Script::new(vec![vec![
        Delta::Progress,
        Delta::Continuation(state()),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);
    scripted.turn("think").unwrap();
    assert!(matches!(
        scripted.runner.transcript().messages().last(),
        Some(Message::Agent {
            continuation: Some(_),
            ..
        })
    ));
    let visible = format!("{:?}", scripted.seen.try_iter().collect::<Vec<_>>());
    assert!(!visible.contains("private-signature-canary"));
    drop(scripted);
    let (_session, transcript) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    assert!(matches!(
        transcript.messages().last(),
        Some(Message::Agent {
            continuation: Some(_),
            ..
        })
    ));
}

#[test]
fn broken_and_unfinished_private_output_never_becomes_replayable_or_retries() {
    for (name, script) in [
        (
            "transport",
            Script::breaking(vec![vec![Delta::Continuation(state())]]),
        ),
        (
            "transport-after-stop",
            Script::breaking(vec![vec![
                Delta::Continuation(state()),
                Delta::Stopped(StopReason::Yielded),
            ]]),
        ),
        (
            "missing-stop",
            Script::new(vec![vec![Delta::Continuation(state())]]),
        ),
        (
            "protocol",
            Script::new(vec![vec![
                Delta::Continuation(state()),
                Delta::Stopped(StopReason::Yielded),
                Delta::Progress,
            ]]),
        ),
        ("partial", Script::breaking(vec![vec![Delta::Progress]])),
        (
            "partial-call",
            Script::breaking(vec![vec![
                Delta::ToolStarted {
                    id: ToolId::new("partial-call"),
                    name: "write".into(),
                },
                Delta::ToolArgs("{\"path\":".into()),
                Delta::Continuation(state()),
            ]]),
        ),
        (
            "invalid-call-reference",
            Script::new(vec![vec![
                Delta::ToolStarted {
                    id: ToolId::new("invalid-call"),
                    name: "write".into(),
                },
                Delta::ToolArgs("{}".into()),
                Delta::Continuation(state()),
                Delta::Stopped(StopReason::WantsTools),
            ]]),
        ),
        (
            "cancelled",
            Script::new(vec![vec![
                Delta::Continuation(state()),
                Delta::Stopped(StopReason::Cancelled),
            ]]),
        ),
    ] {
        let sample = Sample::new(name);
        let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
        let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);
        let _ = scripted.turn("think");
        assert_eq!(
            scripted.sent.lock().unwrap().len(),
            1,
            "{name} must not be retried as empty output"
        );
        assert!(
            scripted
                .runner
                .transcript()
                .messages()
                .iter()
                .all(|message| message.continuation_bytes() == 0),
            "{name} cannot retain private state"
        );
        let events = scripted.seen.try_iter().collect::<Vec<_>>();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::ToolRequested { .. })),
            "{name} cannot release incomplete calls"
        );
        assert!(!format!("{events:?}").contains("private-signature-canary"));
        drop(scripted);
        let (_session, replayed) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert!(
            replayed
                .messages()
                .iter()
                .all(|message| message.continuation_bytes() == 0),
            "{name} cannot persist private state"
        );
    }
}
