//! Publishing bounded sandbox lifecycle facts.
//!
//! A fact crosses the runner boundary only after its fixed call and ancestry
//! attribution is checked, then reaches the event stream and durable journal
//! as the same typed value.

use crucible_core::{
    Ancestry, Event, JournalStore, Reporter, RunItem, SandboxAudit, SandboxAuditRecord,
    SandboxAuditRegistry, ToolContext, ToolError, ToolId,
};

pub(super) fn report_sandbox_facts(
    context: &ToolContext<'_>,
    events: Reporter<'_>,
    journal: &dyn JournalStore,
) -> Result<(), ToolError> {
    report_sandbox_audit(
        &context.sandbox_audit(),
        context.ancestry(),
        context.call(),
        events,
        journal,
    )
}

pub(super) fn report_sandbox_audit(
    audit: &SandboxAudit,
    ancestry: Ancestry,
    call: &ToolId,
    events: Reporter<'_>,
    journal: &dyn JournalStore,
) -> Result<(), ToolError> {
    let records = audit.take_records().map_err(|problem| ToolError::Io {
        tool: "sandbox audit".into(),
        problem: "could not retain the bounded lifecycle record".into(),
        source: std::io::Error::other(problem),
    })?;
    for record in records {
        if record.ancestry() != ancestry || record.call() != call {
            return Err(ToolError::Io {
                tool: "sandbox audit".into(),
                problem: "sandbox fact attribution did not match its host tool context".into(),
                source: std::io::Error::other("sandbox audit attribution mismatch"),
            });
        }
        report_sandbox_record(&record, events, journal)?;
    }
    Ok(())
}

pub(in crate::runner) fn report_sandbox_registry(
    audits: &SandboxAuditRegistry,
    events: Reporter<'_>,
    journal: &dyn JournalStore,
) -> Result<(), ToolError> {
    let records = audits.take_records().map_err(|problem| ToolError::Io {
        tool: "sandbox audit".into(),
        problem: "could not drain detached sandbox lifecycle facts".into(),
        source: std::io::Error::other(problem),
    })?;
    for record in records {
        report_sandbox_record(&record, events, journal)?;
    }
    Ok(())
}

fn report_sandbox_record(
    record: &SandboxAuditRecord,
    events: Reporter<'_>,
    journal: &dyn JournalStore,
) -> Result<(), ToolError> {
    let ancestry = record.ancestry();
    let call = record.call().clone();
    let fact = record.fact().clone();
    let item = RunItem::sandbox(ancestry, call.clone(), fact.clone()).map_err(|problem| {
        ToolError::Io {
            tool: "sandbox audit".into(),
            problem: "sandbox fact crossed the framework journal boundary".into(),
            source: std::io::Error::other(problem),
        }
    })?;
    journal.append_run_item(&item);
    events
        .attributed_to(ancestry)
        .post(Event::Sandbox { call, fact });
    Ok(())
}
