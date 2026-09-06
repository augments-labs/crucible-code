//! Public provider -> runner -> protected session -> provider acceptance.
//!
//! Six exact models use synthetic HTTP responses on loopback. The fixture
//! never reads credentials or reaches a vendor. Assertions observe requests,
//! durable replay, permission decisions and tool effects, not parser internals.

// Test-only helpers fail the owning case when its controlled fixture is invalid.
#![allow(clippy::expect_used, clippy::panic)]

#[path = "provider_lifecycle/support.rs"]
mod support;

use crucible_core::{Compacting, Message, Room, Spend, StopReason, Transcript};
use crucible_runner::Session;
use support::*;

#[test]
fn every_new_model_replays_two_tool_passes_after_restart_and_compaction() {
    for model in MODELS {
        let sample = Sample::new();
        let vendor = Vendor::new(
            model,
            [
                response(model, Some(1), "before"),
                response(model, Some(2), "between"),
                response(model, None, "finished"),
                response(model, None, "resumed"),
                response(model, None, &recap()),
                response(model, None, "after recap"),
                response(model, None, "cleared"),
            ],
        );
        let mut run = sample.runner(
            model,
            &vendor,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        assert_eq!(
            turn(&mut run, "use both tools", &sample),
            StopReason::Yielded
        );
        assert_eq!(sample.executed(), ["1", "2"]);
        assert_eq!(sample.approved(), 2);
        let before = run.transcript().clone();
        assert_eq!(
            before
                .messages()
                .iter()
                .filter(|message| message.continuation_bytes() > 0)
                .count(),
            3
        );
        drop(run);

        let (session, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert_eq!(
            history.messages(),
            before.messages(),
            "{model}: durable history changed"
        );
        let mut run = sample.runner(model, &vendor, session).resuming(history);
        assert_eq!(turn(&mut run, "continue", &sample), StopReason::Yielded);
        assert_eq!(
            sample.executed(),
            ["1", "2"],
            "{model}: tools executed again on resume"
        );
        assert_eq!(sample.approved(), 2);

        let cancel = crucible_core::Cancel::new();
        let steer = crucible_core::Steer::new();
        let aside = crucible_core::Aside::new();
        let context = run.starting(&sample, &cancel, &steer, &aside);
        assert!(
            matches!(
                run.compact(Compacting::Asked, &context, &mut Spend::default())
                    .unwrap(),
                Room::Made(_)
            ),
            "{model}"
        );
        assert_eq!(
            turn(&mut run, "after compaction", &sample),
            StopReason::Yielded
        );
        drop(run);
        let (session, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert!(history.messages().iter().any(|message| matches!(message, Message::User { text, .. } if text.contains("fixture checkpoint"))));
        let mut run = sample.runner(model, &vendor, session).resuming(history);
        let empty = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
        drop(run.pick_up(empty, Transcript::new()));
        assert_eq!(
            turn(&mut run, "fresh session", &sample),
            StopReason::Yielded
        );
        assert_eq!(sample.executed(), ["1", "2"]);

        let requests = vendor.requests();
        assert_eq!(requests.len(), 7, "{model}: unexpected retry/request");
        for request in &requests {
            assert_request(model, request);
        }
        assert_native_history(model, &requests.get(1).unwrap().body, 1);
        assert_native_history(model, &requests.get(2).unwrap().body, 2);
        assert_native_history(model, &requests.get(3).unwrap().body, 2);
        assert!(
            !requests
                .get(4)
                .unwrap()
                .body
                .to_string()
                .contains("private-state"),
            "{model}: recap leaked opaque state"
        );
        assert!(
            !requests
                .get(6)
                .unwrap()
                .body
                .to_string()
                .contains("private-state"),
            "{model}: clear leaked old state"
        );
        assert!(
            !requests
                .get(6)
                .unwrap()
                .body
                .to_string()
                .contains("fixture-result")
        );
        assert!(
            !sample.events().contains("private-state"),
            "{model}: private output reached UI events"
        );
    }
}

#[test]
fn failed_native_streams_never_persist_state_or_execute_their_calls() {
    for model in MODELS {
        for failure in [Failure::MissingStop, Failure::LateError] {
            let sample = Sample::new();
            let vendor = Vendor::new(model, [broken(model, failure)]);
            let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
            let mut run = sample.runner(model, &vendor, session);
            let _ = try_turn(&mut run, "do the work", &sample);
            assert!(
                sample.executed().is_empty(),
                "{model}: failed response executed a tool"
            );
            assert_eq!(sample.approved(), 0);
            assert!(
                run.transcript()
                    .messages()
                    .iter()
                    .all(|message| message.continuation_bytes() == 0)
            );
            drop(run);
            let (_, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
            assert!(
                history
                    .messages()
                    .iter()
                    .all(|message| message.continuation_bytes() == 0),
                "{model}: failed state survived restart"
            );
            assert_eq!(
                vendor.requests().len(),
                1,
                "{model}: failed private output retried as empty"
            );
            assert!(!sample.events().contains("private-state"));
        }
    }
}

#[test]
fn cancellation_at_eof_drops_completed_native_state_before_tool_execution() {
    for model in MODELS {
        let sample = Sample::new();
        let vendor = Vendor::new(model, []);
        let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
        let mut run = sample.runner(model, &vendor, session);
        run.serve(cancelling(
            model,
            vendor.endpoint.clone(),
            response(model, Some(1), "cancelled text"),
        ));
        assert_eq!(turn(&mut run, "work", &sample), StopReason::Cancelled);
        assert_eq!(sample.approved(), 0);
        assert!(sample.executed().is_empty());
        assert!(
            run.transcript()
                .messages()
                .iter()
                .all(|message| message.continuation_bytes() == 0)
        );
        drop(run);
        let (_, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert!(
            history
                .messages()
                .iter()
                .all(|message| message.continuation_bytes() == 0)
        );
    }
}

#[test]
fn automatic_compaction_continues_each_model_and_resumes_its_new_history() {
    for model in MODELS {
        let sample = Sample::new();
        let vendor = Vendor::new(
            model,
            [
                response(model, None, &"old visible text ".repeat(10_000)),
                response(model, None, &recap()),
                response(model, None, "continued automatically"),
                response(model, None, "resumed after automatic recap"),
            ],
        );
        let mut run = sample.runner(
            model,
            &vendor,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        turn(&mut run, "first", &sample);
        run.ask(model, 4096, Some(65_000), None);
        assert_eq!(turn(&mut run, "carry on", &sample), StopReason::Yielded);
        let compacted = run.transcript().clone();
        assert!(compacted.messages().iter().any(|message| matches!(message, Message::User { text, .. } if text.contains("fixture checkpoint"))), "{model}");
        drop(run);
        let (session, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert_eq!(history.messages(), compacted.messages());
        let mut run = sample.runner(model, &vendor, session).resuming(history);
        turn(&mut run, "after restart", &sample);
        let requests = vendor.requests();
        assert_eq!(requests.len(), 4);
        assert!(
            !requests
                .get(1)
                .unwrap()
                .body
                .to_string()
                .contains("private-state")
        );
        assert!(
            requests
                .get(2)
                .unwrap()
                .body
                .to_string()
                .contains("fixture checkpoint")
        );
        assert!(
            !requests
                .get(2)
                .unwrap()
                .body
                .to_string()
                .contains("old visible text")
        );
    }
}

#[test]
fn rotating_the_key_after_restart_keeps_visible_history_but_not_native_authority() {
    for model in MODELS {
        let sample = Sample::new();
        let vendor = Vendor::new(
            model,
            [
                response(model, Some(1), "first"),
                response(model, None, "done"),
                response(model, None, "rotated"),
            ],
        );
        let mut run = sample.runner(
            model,
            &vendor,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        turn(&mut run, "work", &sample);
        drop(run);
        let (session, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        let mut run = sample.runner(model, &vendor, session).resuming(history);
        run.serve(provider(
            model,
            vendor.endpoint.clone(),
            "rotated-fixture-key",
        ));
        turn(&mut run, "continue with new authority", &sample);
        let requests = vendor.requests();
        let sent = &requests.last().unwrap().body.to_string();
        assert!(
            !sent.contains("private-state"),
            "{model}: old key's state crossed scope"
        );
        assert!(sent.contains("Historical tool request"));
        assert!(sent.contains("fixture-result-1"));
        assert_eq!(sample.executed(), ["1"]);
        assert_eq!(sample.approved(), 1);
    }
}

#[test]
fn cancelling_a_recap_at_eof_preserves_the_original_durable_history() {
    for model in MODELS {
        let sample = Sample::new();
        let vendor = Vendor::new(
            model,
            [
                response(model, None, "first"),
                response(model, None, "second"),
            ],
        );
        let mut run = sample.runner(
            model,
            &vendor,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        turn(&mut run, "first", &sample);
        turn(&mut run, "second", &sample);
        let before = run.transcript().clone();
        run.serve(cancelling(
            model,
            vendor.endpoint.clone(),
            response(model, None, &recap()),
        ));
        let cancel = crucible_core::Cancel::new();
        let steer = crucible_core::Steer::new();
        let aside = crucible_core::Aside::new();
        let context = run.starting(&sample, &cancel, &steer, &aside);
        assert_eq!(
            run.compact(Compacting::Asked, &context, &mut Spend::default())
                .unwrap(),
            Room::Stopped,
            "{model}: cancelled recap committed"
        );
        assert_eq!(run.transcript().messages(), before.messages());
        drop(run);
        let (_, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert_eq!(history.messages(), before.messages());
    }
}

#[test]
fn a_late_recap_failure_never_replaces_the_original_session() {
    for model in MODELS {
        let sample = Sample::new();
        let failed = format!(
            "{}event: error\ndata: {{\"type\":\"error\",\"event_type\":\"error\",\"error\":{{\"message\":\"private-state\"}}}}\n\n",
            response(model, None, &recap())
        );
        let vendor = Vendor::new(
            model,
            [
                response(model, None, "first"),
                response(model, None, "second"),
                failed,
            ],
        );
        let mut run = sample.runner(
            model,
            &vendor,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        turn(&mut run, "first", &sample);
        turn(&mut run, "second", &sample);
        let before = run.transcript().clone();
        let cancel = crucible_core::Cancel::new();
        let steer = crucible_core::Steer::new();
        let aside = crucible_core::Aside::new();
        let context = run.starting(&sample, &cancel, &steer, &aside);
        assert!(
            run.compact(Compacting::Asked, &context, &mut Spend::default())
                .is_err()
        );
        assert_eq!(run.transcript().messages(), before.messages());
        drop(run);
        let (_, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert_eq!(history.messages(), before.messages());
        assert!(!sample.events().contains("private-state"));
    }
}

#[test]
fn changing_the_endpoint_after_restart_preserves_only_neutral_history() {
    for model in MODELS {
        let sample = Sample::new();
        let first = Vendor::new(
            model,
            [
                response(model, Some(1), "first"),
                response(model, None, "done"),
            ],
        );
        let mut run = sample.runner(
            model,
            &first,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        turn(&mut run, "work", &sample);
        drop(run);
        let second = Vendor::new(model, [response(model, None, "new recipient")]);
        let (session, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        let mut run = sample.runner(model, &second, session).resuming(history);
        turn(&mut run, "continue", &sample);
        let requests = second.requests();
        assert_eq!(requests.len(), 1);
        assert_request(model, requests.first().unwrap());
        let wire = requests.first().unwrap().body.to_string();
        assert!(
            !wire.contains("private-state"),
            "{model}: state crossed recipient"
        );
        assert!(wire.contains("Historical tool request"));
        assert!(wire.contains("fixture-result-1"));
        assert_eq!(sample.executed(), ["1"]);
        assert_eq!(sample.approved(), 1);
    }
}

#[test]
fn model_and_provider_switches_after_restart_respect_native_compatibility() {
    for model in MODELS {
        let target = match model {
            "gemini-3.8-flash" => "gemini-3.7-flash",
            "gemini-3.7-flash" => "gemini-3.6-flash",
            "gemini-3.6-flash" => "gemini-3.1-pro-preview",
            "gemini-3.1-pro-preview" => "claude-fable-5-1",
            "claude-fable-5-1" => "gpt-6-astra",
            _ => "gemini-3.8-flash",
        };
        let sample = Sample::new();
        // Keep the same recipient and key so only model/protocol compatibility
        // can account for retaining or withholding the native state.
        let vendor = Vendor::new(
            model,
            [
                response(model, Some(1), "first"),
                response(model, None, "done"),
                response(target, None, "switched"),
            ],
        );
        let mut run = sample.runner(
            model,
            &vendor,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        turn(&mut run, "work", &sample);
        drop(run);
        let (session, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        let mut run = sample.runner(target, &vendor, session).resuming(history);
        turn(&mut run, "continue", &sample);
        let requests = vendor.requests();
        assert_eq!(requests.len(), 3);
        let last = requests.last().unwrap();
        assert_request(target, last);
        if model.starts_with("gemini-") && target.starts_with("gemini-") {
            assert_native_history(target, &last.body, 1);
        } else {
            let wire = last.body.to_string();
            assert!(!wire.contains("private-state"), "{model} -> {target}");
            assert!(wire.contains("Historical tool request"));
            assert!(wire.contains("fixture-result-1"));
        }
        assert_eq!(sample.executed(), ["1"]);
        assert_eq!(sample.approved(), 1);
    }
}

#[test]
fn pruning_native_tool_results_survives_restart_without_reexecution() {
    for model in MODELS {
        let sample = Sample::new().padded();
        let vendor = Vendor::new(
            model,
            [
                response(model, Some(1), "first"),
                response(model, Some(2), "second"),
                response(model, Some(3), "third"),
                response(model, None, "done"),
                response(model, None, "resumed"),
            ],
        );
        let mut run = sample.runner(
            model,
            &vendor,
            Session::start(&sample.logs(), &sample.workspace(), None).unwrap(),
        );
        turn(&mut run, "use three tools", &sample);
        let cancel = crucible_core::Cancel::new();
        let steer = crucible_core::Steer::new();
        let aside = crucible_core::Aside::new();
        let context = run.starting(&sample, &cancel, &steer, &aside);
        assert!(
            matches!(
                run.compact(Compacting::Asked, &context, &mut Spend::default())
                    .unwrap(),
                Room::Made(_)
            ),
            "{model}"
        );
        let pruned = run.transcript().clone();
        let sizes: Vec<_> = pruned
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults(results) => {
                    Some(results.iter().map(|result| result.output.text().len()))
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(sizes.len(), 3);
        assert!(sizes.first().unwrap() < &200);
        assert!(sizes.get(1).unwrap() < &200);
        assert!(sizes.last().unwrap() > &20_000);
        assert_eq!(vendor.requests().len(), 4, "prune must not request a recap");
        drop(run);
        let (session, history) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
        assert_eq!(history.messages(), pruned.messages());
        let mut run = sample.runner(model, &vendor, session).resuming(history);
        turn(&mut run, "continue", &sample);
        let requests = vendor.requests();
        assert_eq!(requests.len(), 5);
        let wire = requests.last().unwrap().body.to_string();
        assert!(!wire.contains("fixture-result-1"));
        assert!(!wire.contains("fixture-result-2"));
        assert!(wire.contains("fixture-result-3"));
        assert_eq!(
            wire.contains("private-state"),
            !model.starts_with("claude-")
        );
        for step in 1..=3 {
            assert_eq!(
                wire.matches(&format!("\"call-{step}\"")).count(),
                2,
                "{model}: pruned result lost call identity"
            );
        }
        assert_eq!(sample.executed(), ["1", "2", "3"]);
        assert_eq!(sample.approved(), 3);
    }
}
