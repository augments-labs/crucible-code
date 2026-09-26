//! Sandbox lifecycle reporting at the runner boundary.
//!
//! These tests keep the event stream and durable journal beside one another so
//! a tool cannot erase confinement facts by returning, panicking, or detaching
//! a process after its call has answered.

use super::*;

struct Audited;

impl Tool for Audited {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("audited")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let sandbox = SandboxId::new();
            context
                .sandbox_audit()
                .record(
                    sandbox,
                    SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
                )
                .unwrap();
            context
                .sandbox_audit()
                .record(sandbox, SandboxFactKind::Cleanup(SandboxCleanup::Complete))
                .unwrap();
            Ok(ToolOutput::ok("done"))
        })
    }
}

#[test]
fn sandbox_facts_are_evented_and_journaled_before_the_tool_finishes() {
    let descriptor = ToolDescriptor::new(
        "audited",
        "{}",
        ToolProvenance::new(ToolSourceKind::User, "test:audited", "audit test").unwrap(),
    )
    .unwrap();
    let mut tools = Tools::new();
    tools.add(descriptor, Arc::new(Audited)).unwrap();
    let snapshot = tools.snapshot().unwrap();
    let journal = KeepingJournal::default();
    let (events, seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let cancel = Cancel::new();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = Work {
        tools: &snapshot,
        permission: &mut permission,
        ask: &mut ask,
        events: Reporter::new(ancestry, &keeping),
        cancel: &cancel,
        ancestry,
        journal: &journal,
        audits: &SandboxAuditRegistry::new(),
        worker: None,
        concurrency: 1,
    }
    .pass(&[call("audited-call", "audited")], 0, usize::MAX)
    .awaited();

    assert!(matches!(went, Went::On));
    assert_eq!(results.len(), 1);
    let events = seen.try_iter().collect::<Vec<_>>();
    assert!(
        matches!(
            events.as_slice(),
            [
                Event::Sandbox { call: first, .. },
                Event::Sandbox { call: second, .. },
                Event::ToolFinished { call: finished, .. }
            ] if first == second && second == finished && first.as_str() == "audited-call"
        ),
        "{events:#?}"
    );

    let held = journal.0.lock().unwrap();
    assert!(
        matches!(held.as_slice(), [
        RunItem::Invocation { .. },
        RunItem::Invocation { .. },
        RunItem::Sandbox { call: first, .. },
        RunItem::Sandbox { call: second, .. },
        RunItem::Invocation { .. },
    ] if first == second && first.as_str() == "audited-call"),
        "{held:#?}"
    );
}

struct PanickingAudited;

impl Tool for PanickingAudited {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("panicking audited tool")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            context
                .sandbox_audit()
                .record(
                    SandboxId::new(),
                    SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
                )
                .unwrap();
            panic!("fixture panic after a sandbox lifecycle transition")
        })
    }
}

#[test]
fn a_panicking_tool_cannot_erase_its_sandbox_facts() {
    let descriptor = ToolDescriptor::new(
        "panicking-audited",
        "{}",
        ToolProvenance::new(
            ToolSourceKind::User,
            "test:panicking-audited",
            "panic audit test",
        )
        .unwrap(),
    )
    .unwrap();
    let mut tools = Tools::new();
    tools.add(descriptor, Arc::new(PanickingAudited)).unwrap();
    let snapshot = tools.snapshot().unwrap();
    let journal = KeepingJournal::default();
    let (events, seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let cancel = Cancel::new();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = Work {
        tools: &snapshot,
        permission: &mut permission,
        ask: &mut ask,
        events: Reporter::new(ancestry, &keeping),
        cancel: &cancel,
        ancestry,
        journal: &journal,
        audits: &SandboxAuditRegistry::new(),
        worker: None,
        concurrency: 1,
    }
    .pass(
        &[call("panicking-audited-call", "panicking-audited")],
        0,
        usize::MAX,
    )
    .awaited();

    assert!(matches!(went, Went::On));
    assert!(results.first().is_some_and(|result| {
        result.output.is_failed() && result.output.text().contains("failure was contained")
    }));
    let events = seen.try_iter().collect::<Vec<_>>();
    assert!(
        matches!(
            events.as_slice(),
            [
                Event::Sandbox { call: audited, .. },
                Event::ToolFinished {
                    call: finished,
                    receipt: Some(receipt),
                    ..
                }
            ] if audited == finished
                && audited.as_str() == "panicking-audited-call"
                && receipt.outcome() == ToolOutcome::Panicked
        ),
        "{events:#?}"
    );

    let held = journal.0.lock().unwrap();
    assert_eq!(
        held.iter()
            .filter(|item| matches!(item, RunItem::Sandbox { .. }))
            .count(),
        1,
        "{held:#?}"
    );
}

/// Records a sandbox fact, says once that it is not ready, and panics the
/// next time it is asked: a run that comes apart on a poll after its first.
struct PanickingLater;

impl Tool for PanickingLater {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("a tool that panics on a later poll")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            context
                .sandbox_audit()
                .record(
                    SandboxId::new(),
                    SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
                )
                .unwrap();
            let mut asked = false;
            std::future::poll_fn(|cx| {
                if asked {
                    std::task::Poll::Ready(())
                } else {
                    asked = true;
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            })
            .await;
            panic!("fixture panic on the poll after the first")
        })
    }
}

#[test]
fn a_run_that_panics_on_a_later_poll_is_contained_as_one_that_panics_at_once() {
    // A call that runs alone is awaited across polls, so it can come apart on
    // any of them. One that does after waiting is answered exactly as one
    // that comes apart on its first poll is: contained, with the fact its
    // sandbox recorded before it came apart reported once.
    let descriptor = ToolDescriptor::new(
        "panicking-later",
        "{}",
        ToolProvenance::new(
            ToolSourceKind::User,
            "test:panicking-later",
            "later panic audit test",
        )
        .unwrap(),
    )
    .unwrap();
    let mut tools = Tools::new();
    tools.add(descriptor, Arc::new(PanickingLater)).unwrap();
    let snapshot = tools.snapshot().unwrap();
    let journal = KeepingJournal::default();
    let (events, seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let cancel = Cancel::new();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = Work {
        tools: &snapshot,
        permission: &mut permission,
        ask: &mut ask,
        events: Reporter::new(ancestry, &keeping),
        cancel: &cancel,
        ancestry,
        journal: &journal,
        audits: &SandboxAuditRegistry::new(),
        worker: None,
        concurrency: 1,
    }
    .pass(
        &[call("panicking-later-call", "panicking-later")],
        0,
        usize::MAX,
    )
    .awaited();

    assert!(matches!(went, Went::On));
    assert!(results.first().is_some_and(|result| {
        result.output.is_failed() && result.output.text().contains("failure was contained")
    }));
    let events = seen.try_iter().collect::<Vec<_>>();
    assert!(
        matches!(
            events.as_slice(),
            [
                Event::Sandbox { call: audited, .. },
                Event::ToolFinished {
                    call: finished,
                    receipt: Some(receipt),
                    ..
                }
            ] if audited == finished
                && audited.as_str() == "panicking-later-call"
                && receipt.outcome() == ToolOutcome::Panicked
        ),
        "{events:#?}"
    );
    let held = journal.0.lock().unwrap();
    assert_eq!(
        held.iter()
            .filter(|item| matches!(item, RunItem::Sandbox { .. }))
            .count(),
        1,
        "{held:#?}"
    );
}

#[test]
fn a_panicking_tool_in_a_parallel_wave_cannot_erase_its_sandbox_facts() {
    // The calls of a parallel wave run as tasks of one batch's group, side by
    // side, and a panic in one is contained around its own run as a lone
    // call's is, without stopping the other. Each call is still answered as
    // contained, and each still has the fact its sandbox recorded before it
    // came apart reported once.
    let descriptor = ToolDescriptor::new(
        "panicking-audited",
        "{}",
        ToolProvenance::new(
            ToolSourceKind::User,
            "test:panicking-audited",
            "panic audit test",
        )
        .unwrap(),
    )
    .unwrap()
    .executing(ToolExecutionMode::Parallel);
    let mut tools = Tools::new();
    tools.add(descriptor, Arc::new(PanickingAudited)).unwrap();
    let snapshot = tools.snapshot().unwrap();
    let journal = KeepingJournal::default();
    let (events, seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let cancel = Cancel::new();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = Work {
        tools: &snapshot,
        permission: &mut permission,
        ask: &mut ask,
        events: Reporter::new(ancestry, &keeping),
        cancel: &cancel,
        ancestry,
        journal: &journal,
        audits: &SandboxAuditRegistry::new(),
        worker: None,
        concurrency: 2,
    }
    .pass(
        &[
            call("panicking-a", "panicking-audited"),
            call("panicking-b", "panicking-audited"),
        ],
        0,
        usize::MAX,
    )
    .awaited();

    assert!(matches!(went, Went::On));
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| {
        result.output.is_failed() && result.output.text().contains("failure was contained")
    }));
    let events = seen.try_iter().collect::<Vec<_>>();
    let mut audited: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Sandbox { call, .. } => Some(call.as_str()),
            _ => None,
        })
        .collect();
    audited.sort_unstable();
    assert_eq!(audited, ["panicking-a", "panicking-b"], "{events:#?}");
    let finished: Vec<(&str, Option<ToolOutcome>)> = events
        .iter()
        .filter_map(|event| match event {
            Event::ToolFinished { call, receipt, .. } => Some((
                call.as_str(),
                receipt.as_ref().map(crucible_core::ToolReceipt::outcome),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        [
            ("panicking-a", Some(ToolOutcome::Panicked)),
            ("panicking-b", Some(ToolOutcome::Panicked)),
        ]
    );

    let held = journal.0.lock().unwrap();
    assert_eq!(
        held.iter()
            .filter(|item| matches!(item, RunItem::Sandbox { .. }))
            .count(),
        2,
        "{held:#?}"
    );
}

struct DetachedAudited {
    release: Arc<Barrier>,
    done: Sender<()>,
}

impl Tool for DetachedAudited {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("detached audited tool")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let sandbox = SandboxId::new();
            let audit = context.sandbox_audit();
            audit
                .record(
                    sandbox,
                    SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
                )
                .unwrap();
            let release = Arc::clone(&self.release);
            let done = self.done.clone();
            thread::spawn(move || {
                release.wait();
                audit
                    .record(
                        sandbox,
                        SandboxFactKind::Lifecycle(SandboxLifecycle::CommandFinished),
                    )
                    .unwrap();
                done.send(()).unwrap();
            });
            Ok(ToolOutput::ok("detached"))
        })
    }
}

#[test]
fn detached_sandbox_facts_keep_the_original_call_until_the_next_runner_boundary() {
    let release = Arc::new(Barrier::new(2));
    let (done, finished) = channel();
    let descriptor = ToolDescriptor::new(
        "detached-audited",
        "{}",
        ToolProvenance::new(
            ToolSourceKind::User,
            "test:detached-audited",
            "detached audit test",
        )
        .unwrap(),
    )
    .unwrap();
    let mut tools = Tools::new();
    tools
        .add(
            descriptor,
            Arc::new(DetachedAudited {
                release: Arc::clone(&release),
                done,
            }),
        )
        .unwrap();
    let snapshot = tools.snapshot().unwrap();
    let journal = KeepingJournal::default();
    let audits = SandboxAuditRegistry::new();
    let (events, seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let cancel = Cancel::new();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = Work {
        tools: &snapshot,
        permission: &mut permission,
        ask: &mut ask,
        events: Reporter::new(ancestry, &keeping),
        cancel: &cancel,
        ancestry,
        journal: &journal,
        audits: &audits,
        worker: None,
        concurrency: 1,
    }
    .pass(
        &[call("detached-audited-call", "detached-audited")],
        0,
        usize::MAX,
    )
    .awaited();
    assert!(matches!(went, Went::On));
    assert_eq!(results.len(), 1);

    release.wait();
    finished
        .recv_timeout(Duration::from_secs(2))
        .expect("detached fact was recorded");
    report_sandbox_registry(&audits, Reporter::new(Ancestry::new(), &keeping), &journal)
        .awaited()
        .expect("next runner boundary");

    let events = seen.try_iter().collect::<Vec<_>>();
    assert!(matches!(
        events.as_slice(),
        [
            Event::Sandbox { call: initial, .. },
            Event::ToolFinished { call: finished, .. },
            Event::Sandbox {
                call: detached,
                fact,
            },
        ] if initial == finished
            && finished == detached
            && detached.as_str() == "detached-audited-call"
            && matches!(
                fact.kind(),
                SandboxFactKind::Lifecycle(SandboxLifecycle::CommandFinished)
            )
    ));
    let held = journal.0.lock().unwrap();
    assert!(matches!(
        held.last(),
        Some(RunItem::Sandbox {
            ancestry: retained,
            call,
            fact,
        }) if *retained == ancestry
            && call.as_str() == "detached-audited-call"
            && matches!(
                fact.kind(),
                SandboxFactKind::Lifecycle(SandboxLifecycle::CommandFinished)
            )
    ));
}
