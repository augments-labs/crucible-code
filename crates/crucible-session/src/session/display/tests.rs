//! The display record survives model compaction without becoming model input.

use super::*;
use crate::Session;
use crate::sample::Sample;
use crucible_core::{
    Ancestry, InvocationRecord, RunItem, StopReason, ToolArgs, ToolCall, ToolEffect, ToolOutcome,
    ToolOutput, ToolResult,
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

fn finished(session: &Session, call: ToolCall, diff: Diff) {
    let output = ToolOutput::ok("edited").showing(diff);
    let mut invocation =
        InvocationRecord::new(call.clone(), Ancestry::new(), ToolEffect::ReadOnly, None);
    invocation
        .finish(ToolOutcome::Succeeded, output.clone())
        .unwrap();
    session.append_journal(&RunItem::Invocation(invocation));
    session.append(&Message::ToolResults(vec![ToolResult {
        id: call.id,
        output,
    }]));
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
    finished(&session, call, diff.clone());
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
        DisplayItem::Message(Message::User { .. })
    ));
    let DisplayItem::Message(Message::ToolResults(results)) = display.get(2).unwrap() else {
        panic!("missing result")
    };
    assert_eq!(results.first().unwrap().output.diff(), Some(&diff));
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
    finished(&session, call, diff);
    drop(session);
    let (_, model) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    let Message::ToolResults(results) = model.messages().get(2).unwrap() else {
        panic!("missing result")
    };
    assert!(results.first().unwrap().output.diff().is_none());
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
        output: ToolOutput::ok("old output"),
    }]));
    session.compacted(3, "old recap");
    let display = items(&session);
    let DisplayItem::Message(Message::ToolResults(results)) = display.get(2).unwrap() else {
        panic!("missing result")
    };
    assert!(results.first().unwrap().output.diff().is_none());
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
        let mut record =
            InvocationRecord::new(call.clone(), Ancestry::new(), ToolEffect::ReadOnly, None);
        record
            .finish(ToolOutcome::Succeeded, output.clone())
            .unwrap();
        session.append_journal(&RunItem::Invocation(record));
        results.push(ToolResult {
            id: call.id,
            output,
        });
    }
    session.append(&Message::ToolResults(results));
    let display = items(&session);
    let DisplayItem::Message(Message::ToolResults(results)) = display.last().unwrap() else {
        panic!("missing batch")
    };
    assert_eq!(results.len(), 128);
    assert!(
        results
            .iter()
            .all(|result| result.output.diff() == Some(&diff))
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
