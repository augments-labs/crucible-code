//! The display record survives model compaction without becoming model input.

use super::*;
use crate::Session;
use crate::sample::Sample;
use crucible_storage::{InvocationRecord, RunItem, ToolEffect};
use crucible_tools::{ToolOutcome, ToolOutput};
use crucible_types::{
    Ancestry, Changed, RecordedToolOutput, StopReason, ToolArgs, ToolCall, ToolResult,
};

fn conversation(session: &Session) -> (ToolCall, Diff) {
    session.append(&Message::said("original question"));
    let call = ToolCall {
        id: ToolId::new("edit-1"),
        name: "edit".into(),
        args: ToolArgs::new(r#"{"path":"gone.txt"}"#),
    };
    session.append(&Message::Agent {
        continuation: None,
        text: "original answer".into(),
        calls: vec![call.clone()],
        stop: Some(StopReason::WantsTools),
    });
    let diff = Diff::new(
        (1..=80).map(|number| Line::new(number, Change::Added, "private-preview-canary")),
    );
    (call, diff)
}

/// Records the call as finished, as the runner does before the result line,
/// and hands back the result that line would carry.
fn recorded(session: &Session, call: &ToolCall, diff: Diff) -> ToolResult {
    let output = ToolOutput::ok("edited").showing(diff);
    let preview = output.diff().cloned();
    let recorded = output.into_recorded();
    let mut invocation =
        InvocationRecord::new(call.clone(), Ancestry::new(), ToolEffect::ReadOnly, None);
    invocation
        .finish(ToolOutcome::Succeeded, recorded.clone())
        .unwrap();
    session.append_journal(&RunItem::Invocation {
        record: invocation,
        preview,
    });
    ToolResult {
        id: call.id.clone(),
        output: recorded,
    }
}

fn finished(session: &Session, call: &ToolCall, diff: Diff) {
    let result = recorded(session, call, diff);
    session.append(&Message::ToolResults(vec![result]));
}

fn items(session: &Session) -> Vec<DisplayItem> {
    session
        .display_history()
        .unwrap()
        .unwrap()
        .collect::<io::Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn full_history_and_private_preview_survive_without_changing_model_context() {
    let sample = Sample::new("display-model-separation");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let (call, diff) = conversation(&session);
    finished(&session, &call, diff.clone());
    session.compacted(3, "model recap");
    let notice = Compacted {
        why: Compacting::Asked,
        replaced: 3,
        before: 100,
        after: 10,
        kept: 0,
    };
    session.display_compacted(notice, false);
    session.append(&Message::said("later question"));
    let display = items(&session);
    assert_eq!(display.len(), 5);
    assert!(matches!(
        display.first().unwrap(),
        DisplayItem::Message {
            message: Message::User { .. },
            ..
        }
    ));
    let DisplayItem::Message {
        message: Message::ToolResults(results),
        previews,
    } = display.get(2).unwrap()
    else {
        panic!("missing result")
    };
    assert_eq!(previews.get(&results.first().unwrap().id), Some(&diff));
    assert!(
        matches!(display.get(3).unwrap(), DisplayItem::Compacted(details) if *details == notice)
    );
    drop(session);
    let (session, model) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    assert_eq!(model.messages().len(), 2);
    assert!(!format!("{model:?}").contains("private-preview-canary"));
    assert_eq!(items(&session).len(), 5);
}

#[test]
fn ordinary_model_replay_does_not_restore_display_diff() {
    let sample = Sample::new("display-no-model-diff");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let (call, diff) = conversation(&session);
    finished(&session, &call, diff);
    drop(session);
    let (_, model) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    let Message::ToolResults(results) = model.messages().get(2).unwrap() else {
        panic!("missing result")
    };
    // What crosses into the model's copy is the header its row can be drawn
    // from again; the lines themselves stay in the display journal.
    let output = &results.first().unwrap().output;
    // Not that the lines are absent -- the model's copy has nowhere to put
    // them, which is the type's job and not this test's -- but that the header
    // they were counted into arrived.
    assert_eq!(output.changed(), Some(Changed::new(80, 0)));
}

#[test]
fn consecutive_completed_compactions_remain_distinct() {
    let sample = Sample::new("display-consecutive-compactions");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    session.append(&Message::said("question"));
    for why in [Compacting::Asked, Compacting::Resumed] {
        session.pruned(10, &[]);
        session.display_compacted(
            Compacted {
                why,
                replaced: 0,
                before: 20,
                after: 10,
                kept: 1,
            },
            true,
        );
    }
    let display = items(&session);
    assert_eq!(display.len(), 3);
    assert!(matches!(
        *display.get(1).unwrap(),
        DisplayItem::Compacted(Compacted {
            why: Compacting::Asked,
            ..
        })
    ));
    assert!(matches!(
        *display.get(2).unwrap(),
        DisplayItem::Compacted(Compacted {
            why: Compacting::Resumed,
            ..
        })
    ));
}

#[test]
fn old_logs_restore_original_messages_and_truthful_compaction_without_previews() {
    let sample = Sample::new("display-legacy");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let (call, _) = conversation(&session);
    session.append(&Message::ToolResults(vec![ToolResult {
        id: call.id,
        output: RecordedToolOutput::ok("old output"),
    }]));
    session.compacted(3, "old recap");
    let display = items(&session);
    let DisplayItem::Message {
        message: Message::ToolResults(_),
        previews,
    } = display.get(2).unwrap()
    else {
        panic!("missing result")
    };
    assert!(previews.is_empty());
    assert!(matches!(
        *display.get(3).unwrap(),
        DisplayItem::LegacyCompacted { replaced: 3 }
    ));
}

#[test]
fn display_reader_is_a_fixed_view_and_debug_never_exposes_private_bytes() {
    let sample = Sample::new("display-fixed-view");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    session.append(&Message::said("private-preview-canary"));
    let mut history = session.display_history().unwrap().unwrap();
    session.append(&Message::said("later append"));
    assert!(history.next().unwrap().is_ok());
    assert!(!format!("{history:?}").contains("private-preview-canary"));
    assert!(history.next().is_none());
    assert_eq!(items(&session).len(), 2);
}

#[test]
fn preview_decoder_rejects_overflow_inconsistent_counts_and_oversized_unicode() {
    let diff = Diff::new([Line::new(1, Change::Added, "private-preview-canary")]);
    let original = preview(&diff);
    assert_eq!(read_preview(&original), Some(diff));
    for (field, value) in [
        ("added", json!(0)),
        ("removed", json!(1)),
        ("dropped", json!(1)),
        ("added", json!(u64::MAX)),
        ("lines", json!([])),
    ] {
        let mut damaged = original.clone();
        *damaged.get_mut(field).unwrap() = value;
        assert!(read_preview(&damaged).is_none());
    }
    let mut damaged = original.clone();
    *damaged
        .get_mut("lines")
        .unwrap()
        .get_mut(0)
        .unwrap()
        .get_mut("text")
        .unwrap() = json!("🦀".repeat(Line::TEXT + 1));
    assert!(read_preview(&damaged).is_none());
    *damaged
        .get_mut("lines")
        .unwrap()
        .get_mut(0)
        .unwrap()
        .get_mut("text")
        .unwrap() = json!("🦀".repeat(Line::TEXT));
    assert!(read_preview(&damaged).is_some());
    *damaged
        .get_mut("lines")
        .unwrap()
        .get_mut(0)
        .unwrap()
        .get_mut("change")
        .unwrap() = json!("private-preview-canary");
    assert!(read_preview(&damaged).is_none());
    assert!(!invalid().to_string().contains("private-preview-canary"));
}

#[test]
fn a_full_live_batch_of_maximum_unicode_previews_remains_replayable() {
    let sample = Sample::new("display-full-preview-batch");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let calls: Vec<_> = (0..128)
        .map(|n| ToolCall {
            id: ToolId::new(format!("edit-{n}")),
            name: "edit".into(),
            args: ToolArgs::new("{}"),
        })
        .collect();
    session.append(&Message::Agent {
        continuation: None,
        text: "".into(),
        calls: calls.clone(),
        stop: Some(StopReason::WantsTools),
    });
    let diff = Diff::new(
        (0..Diff::LINES).map(|n| Line::new(n + 1, Change::Removed, "🦀".repeat(Line::TEXT))),
    );
    let mut results = Vec::new();
    for call in calls {
        let output = ToolOutput::ok("edited").showing(diff.clone());
        let preview = output.diff().cloned();
        let recorded = output.into_recorded();
        let mut record =
            InvocationRecord::new(call.clone(), Ancestry::new(), ToolEffect::ReadOnly, None);
        record
            .finish(ToolOutcome::Succeeded, recorded.clone())
            .unwrap();
        session.append_journal(&RunItem::Invocation { record, preview });
        results.push(ToolResult {
            id: call.id,
            output: recorded,
        });
    }
    session.append(&Message::ToolResults(results));
    let display = items(&session);
    let DisplayItem::Message {
        message: Message::ToolResults(results),
        previews,
    } = display.last().unwrap()
    else {
        panic!("missing batch")
    };
    assert_eq!(results.len(), 128);
    assert!(
        results
            .iter()
            .all(|result| previews.get(&result.id) == Some(&diff))
    );
}

#[test]
fn legacy_compaction_markers_are_not_replaced_by_later_operations() {
    let sample = Sample::new("display-legacy-consecutive");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    session.append(&Message::said("question"));
    session.compacted(1, "first recap");
    session.compacted(1, "second recap");
    session.pruned(10, &[]);
    session.display_compacted(
        Compacted {
            why: Compacting::Asked,
            replaced: 0,
            before: 20,
            after: 10,
            kept: 1,
        },
        true,
    );
    let display = items(&session);
    assert_eq!(
        display.len(),
        4,
        "each completed operation keeps its marker"
    );
    for item in display.iter().skip(1).take(2) {
        assert!(matches!(item, DisplayItem::LegacyCompacted { replaced: 1 }));
    }
    assert!(matches!(display.last().unwrap(), DisplayItem::Compacted(_)));
}

#[test]
fn pruning_and_recapping_one_operation_restore_one_live_marker() {
    let sample = Sample::new("display-prune-and-recap");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    session.append(&Message::said("question"));
    session.pruned(10, &[]);
    session.compacted(1, "recap");
    session.display_compacted(
        Compacted {
            why: Compacting::Asked,
            replaced: 1,
            before: 30,
            after: 10,
            kept: 0,
        },
        true,
    );
    let display = items(&session);
    assert_eq!(
        display.len(),
        2,
        "pruning and recap share one live completion"
    );
    assert!(matches!(display.last().unwrap(), DisplayItem::Compacted(_)));
}

#[test]
fn a_call_finished_before_its_result_line_comes_back_answered_with_its_preview() {
    // The process died after the call's record and before its result line.
    // The pick-up writes the line the record stood for, so the model is told
    // what the call did instead of being free to ask for it again, and the
    // reader is shown the change the call made.
    let sample = Sample::new("display-finished-unanswered");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let path = session.path().to_owned();
    let (call, diff) = conversation(&session);
    let answered = Message::ToolResults(vec![recorded(&session, &call, diff.clone())]);
    drop(session);

    let (session, model) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    assert_eq!(model.messages().get(2), Some(&answered));
    let log = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        log.lines().last(),
        Some(wire::line(&answered).as_str()),
        "the line written is the one the turn would have written"
    );
    let display = items(&session);
    let Some(DisplayItem::Message {
        message: Message::ToolResults(_),
        previews,
    }) = display.get(2)
    else {
        panic!("missing result: {display:?}")
    };
    assert_eq!(previews.get(&call.id), Some(&diff));
    drop(session);

    // Picked up again, it is an ordinary answered call.
    let (_, again) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    assert_eq!(again.messages(), model.messages());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), log);
}

#[test]
fn a_finished_call_that_changed_no_line_comes_back_as_its_result_line_says() {
    // The line written carries no count for a call that changed nothing, so
    // the call the first pick-up answers is the one every later one reads.
    let sample = Sample::new("display-finished-unchanged");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let path = session.path().to_owned();
    let (call, _) = conversation(&session);
    let answered = Message::ToolResults(vec![recorded(
        &session,
        &call,
        Diff::new(std::iter::empty()),
    )]);
    drop(session);

    let (_, first) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    let log = std::fs::read_to_string(&path).unwrap();
    assert_eq!(log.lines().last(), Some(wire::line(&answered).as_str()));
    let (_, again) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    assert_eq!(first.messages(), again.messages());
}
