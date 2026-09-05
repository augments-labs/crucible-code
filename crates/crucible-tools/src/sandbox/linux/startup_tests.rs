//! The real post-spawn handler owns a distinct projection tree and journal.
//! No Bubblewrap availability fault or fabricated process handle is needed.

use super::*;
use crate::sandbox::process::{Stage, testing_plan};

#[test]
fn startup_unconfirmed_cleanup_retains_linux_projection_and_audits_failed() -> io::Result<()> {
    let sample = crate::sample::Sample::new("linux-startup-cleanup");
    let materialization = sample.root().join("materialization-file");
    std::fs::write(&materialization, "not a removable directory")?;
    let mut plan = testing_plan(
        crucible_core::SandboxSpeech::Closed,
        Some(Stage::new(materialization.clone())),
    )
    .map_err(io::Error::other)?;
    let rule = crucible_core::SandboxFilesystemRule::new(
        sample.root(),
        SandboxFilesystemAccess::ReadOnly,
        crucible_core::SandboxFilesystemProvenance::Workspace,
    )
    .map_err(io::Error::other)?;
    let policy = crucible_core::SandboxPolicy::new(
        crucible_core::SandboxMode::Required,
        [rule],
        sample.root(),
        crucible_core::SandboxNetworkPolicy::Closed,
        crucible_core::SandboxResourceLimits::default(),
    )
    .map_err(io::Error::other)?;
    let request = SandboxRequest::new(
        plan.sandbox,
        crucible_core::Ancestry::new(),
        crucible_core::ToolId::new("startup-cleanup"),
        policy,
        crucible_core::SandboxManifest::empty(),
    );
    plan.audit = request.audit().clone();
    let root = transaction::state_directory(&request)
        .map_err(io::Error::other)?
        .join(transaction::stage_name(request.id()));
    // This separate fixture owner removes only our unique root after every
    // assertion, including when the intentionally broken candidate panics.
    let _rescue = Stage::new(root.clone());
    let view = command::prepare(&request).map_err(io::Error::other)?;
    let projection =
        projection::Projection::prepare(&request, &view, None, None).map_err(io::Error::other)?;
    let audit = request.audit().clone();
    let mut launch = LinuxLaunch {
        process: None,
        projection: Some(projection),
        status_channel: Some(broker::StatusChannel::pair()?),
        inspection: plan.inspection.clone(),
        audit: audit.clone(),
        sandbox: plan.sandbox,
        invocation: plan.invocation,
        call_result_key: None,
        owner_transferred: false,
        released: false,
    };
    let problem = super::super::process::spawn(
        std::process::Command::new(sample.root().join("absent-program")),
        plan,
    )
    .expect_err("actual spawn and staging cleanup both fail");
    launch.startup_failed(&problem);
    drop(launch);
    let facts = audit.records().map_err(io::Error::other)?;
    assert!(
        root.exists(),
        "projection evidence must survive failed startup cleanup"
    );
    assert!(
        root.join("transaction.wal").is_file(),
        "durable evidence must survive"
    );
    assert!(materialization.exists());
    assert!(facts.iter().any(|record| matches!(
        record.fact().kind(),
        SandboxFactKind::Lifecycle(SandboxLifecycle::Quarantined)
    )));
    assert!(facts.iter().any(|record| matches!(
        record.fact().kind(),
        SandboxFactKind::Cleanup(SandboxCleanup::Failed)
    )));
    assert!(!facts.iter().any(|record| matches!(
        record.fact().kind(),
        SandboxFactKind::Cleanup(SandboxCleanup::Complete)
    )));
    Ok(())
}
