//! The real post-spawn handler owns a distinct projection tree and journal.
//! No Bubblewrap availability fault or fabricated process handle is needed.

use super::*;
use crate::sandbox::process::{Stage, testing_plan};

fn domain_policy() -> crucible_core::SandboxDomainPolicy {
    crucible_core::SandboxDomainPolicy::new(
        [crucible_core::SandboxDomainPattern::new("127.0.0.1").expect("literal")],
        [],
        false,
        [],
        crucible_core::SandboxNetworkProvenance::User,
    )
    .expect("domain policy")
}

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
        true,
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
    let rescue = Stage::new(root.clone());
    let view = command::prepare(&request).map_err(io::Error::other)?;
    let projection =
        projection::Projection::prepare(&request, &view, None, None).map_err(io::Error::other)?;
    // This test deliberately closes the WAL while retaining a quarantined
    // stage. Keep other test admissions outside that interval. Tuple fields
    // drop in order, so even a panic removes our fixture before unlocking.
    let _fixture = (
        rescue,
        transaction::RegistryLease::acquire(&request).map_err(io::Error::other)?,
    );
    let audit = request.audit().clone();
    let mut launch = LinuxLaunch {
        process: None,
        projection: Some(projection),
        network: None,
        materialization: None,
        reservation: None,
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

#[test]
fn pretransfer_network_cleanup_failure_is_quarantined_and_never_complete() -> io::Result<()> {
    let sample = crate::sample::Sample::new("linux-startup-network-cleanup");
    let mut plan =
        testing_plan(crucible_core::SandboxSpeech::Closed, None).map_err(io::Error::other)?;
    let rule = crucible_core::SandboxFilesystemRule::new(
        sample.root(),
        SandboxFilesystemAccess::ReadOnly,
        crucible_core::SandboxFilesystemProvenance::Workspace,
    )
    .map_err(io::Error::other)?;
    let policy = crucible_core::SandboxPolicy::new(
        true,
        [rule],
        sample.root(),
        crucible_core::SandboxNetworkPolicy::Domains(domain_policy()),
        crucible_core::SandboxResourceLimits::default(),
    )
    .map_err(io::Error::other)?;
    let request = SandboxRequest::new(
        plan.sandbox,
        crucible_core::Ancestry::new(),
        crucible_core::ToolId::new("startup-network-cleanup"),
        policy,
        crucible_core::SandboxManifest::empty(),
    );
    plan.audit = request.audit().clone();
    let root = transaction::state_directory(&request)
        .map_err(io::Error::other)?
        .join(transaction::stage_name(request.id()));
    let rescue = Stage::new(root.clone());
    let view = command::prepare(&request).map_err(io::Error::other)?;
    let projection =
        projection::Projection::prepare(&request, &view, None, None).map_err(io::Error::other)?;
    // Retained WAL evidence must not be reconciled by parallel admissions
    // between dropping the launch owner and checking/removing this fixture.
    let _fixture = (
        rescue,
        transaction::RegistryLease::acquire(&request).map_err(io::Error::other)?,
    );
    let socket = projection.network_socket();
    let mediator = super::super::network::Mediator::unix(
        &socket,
        domain_policy(),
        request.id(),
        Some(std::time::Duration::from_secs(3)),
    )?;
    std::fs::remove_file(&socket)?;
    std::fs::write(&socket, b"replacement")?;
    let Err(problem) =
        network::SocketMount::open(&socket, std::path::Path::new(network::PROXY_PATH))
    else {
        panic!("replacement must fail real proxy socket preparation");
    };
    let audit = request.audit().clone();
    let mut launch = LinuxLaunch {
        process: None,
        projection: Some(projection),
        network: Some(mediator),
        materialization: None,
        reservation: None,
        status_channel: None,
        inspection: plan.inspection,
        audit: audit.clone(),
        sandbox: plan.sandbox,
        invocation: plan.invocation,
        call_result_key: None,
        owner_transferred: false,
        released: false,
    };
    launch.startup_failed(&problem);
    drop(launch);

    let facts = audit.records().map_err(io::Error::other)?;
    assert!(
        root.exists(),
        "failed mediator cleanup must retain evidence"
    );
    assert!(facts.iter().any(|record| matches!(
        record.fact().kind(),
        SandboxFactKind::Failed {
            phase: SandboxFailurePhase::Start,
            kind: SandboxFailureKind::Materialization,
        }
    )));
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

#[test]
fn pretransfer_materialization_cleanup_failure_retains_projection_evidence() -> io::Result<()> {
    let sample = crate::sample::Sample::new("linux-startup-materialization-cleanup");
    let mut plan =
        testing_plan(crucible_core::SandboxSpeech::Closed, None).map_err(io::Error::other)?;
    let rule = crucible_core::SandboxFilesystemRule::new(
        sample.root(),
        SandboxFilesystemAccess::ReadOnly,
        crucible_core::SandboxFilesystemProvenance::Workspace,
    )
    .map_err(io::Error::other)?;
    let manifest =
        crucible_core::SandboxManifest::new([crucible_core::SandboxManifestEntry::file(
            "input.txt",
            Box::<[u8]>::from(&b"input\n"[..]),
            crucible_core::SandboxFilesystemProvenance::Manifest,
        )
        .map_err(io::Error::other)?])
        .map_err(io::Error::other)?;
    let policy = crucible_core::SandboxPolicy::new(
        true,
        [rule],
        sample.root(),
        crucible_core::SandboxNetworkPolicy::Closed,
        crucible_core::SandboxResourceLimits::default(),
    )
    .map_err(io::Error::other)?;
    let request = SandboxRequest::new(
        plan.sandbox,
        crucible_core::Ancestry::new(),
        crucible_core::ToolId::new("startup-materialization-cleanup"),
        policy,
        manifest,
    );
    plan.audit = request.audit().clone();
    let projection_root = transaction::state_directory(&request)
        .map_err(io::Error::other)?
        .join(transaction::stage_name(request.id()));
    let projection_rescue = Stage::new(projection_root.clone());
    let view = command::prepare(&request).map_err(io::Error::other)?;
    let materialization = materialize::commit(&request)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("fixture manifest was not materialized"))?;
    let projection = projection::Projection::prepare(&request, &view, Some(&materialization), None)
        .map_err(io::Error::other)?;
    let _fixture = (
        projection_rescue,
        transaction::RegistryLease::acquire(&request).map_err(io::Error::other)?,
    );
    let materialization_root =
        std::path::PathBuf::from(format!("/tmp/crucible-sandbox-{}", request.id()));
    std::fs::remove_dir_all(&materialization_root)?;
    std::fs::write(&materialization_root, b"replacement")?;
    let _replacement = ReplacedStageFile(materialization_root.clone());
    let Err(problem) = network::SocketMount::open(
        &materialization_root,
        std::path::Path::new(network::PROXY_PATH),
    ) else {
        panic!("replacement must fail real setup");
    };
    let audit = request.audit().clone();
    let mut launch = LinuxLaunch {
        process: None,
        projection: Some(projection),
        network: None,
        materialization: Some(materialization),
        reservation: None,
        status_channel: None,
        inspection: plan.inspection,
        audit: audit.clone(),
        sandbox: plan.sandbox,
        invocation: plan.invocation,
        call_result_key: None,
        owner_transferred: false,
        released: false,
    };
    launch.startup_failed(&problem);
    drop(launch);

    let facts = audit.records().map_err(io::Error::other)?;
    assert!(
        projection_root.exists(),
        "materialization cleanup failure must retain the projection WAL"
    );
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

/// Removes the unique deliberate file replacement even when a regression
/// assertion fails; Stage itself only removes directories.
struct ReplacedStageFile(std::path::PathBuf);

impl Drop for ReplacedStageFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
