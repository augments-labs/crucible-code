//! What a projected stop reports when the process it owns cannot be stopped.

use super::*;

/// A stop hung in its mediator fails the stop and the cleanup, rather than
/// joining without end or reporting a silent success: the synchronous stop
/// owns the process's own stop directly, with no bridge left to drop the
/// wait at.
#[test]
fn a_stop_hung_in_its_mediator_is_failed_cleanup_not_silent_success() {
    use crucible_sandbox::{SandboxDomainPolicy, SandboxNetworkProvenance};

    let policy =
        SandboxDomainPolicy::new([], [], false, [], SandboxNetworkProvenance::User).unwrap();
    let mut proxy = crate::network::Mediator::tcp(policy, SandboxId::new(), None).unwrap();
    proxy.hang_listener();
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "exit 0"]);
    let mut plan =
        crate::process::testing_plan(crucible_sandbox::SandboxSpeech::Closed, None).unwrap();
    plan.network = Some(proxy);
    let audit = plan.audit.clone();
    let sandbox = plan.sandbox;
    let (process, stop_mark) = crate::process::spawn_marked(command, plan).unwrap();
    let inspection = process.inspection().clone();
    let (control, _broker) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut projected = ProjectedProcess {
        process: share_process(process),
        output_boundary: Arc::new(OutputBoundary::default()),
        projection: None,
        publications: BoundedPublication::default(),
        receiver: None,
        status: None,
        terminal: false,
        reported: None,
        concluding: None,
        failure: None,
        unrecorded: None,
        publication: None,
        audit: audit.clone(),
        sandbox,
        control: Some(control),
        invocation: SandboxInvocationMode::Foreground,
        call_result_key: None,
        acceptance_pending: false,
        inspection,
        cleanup: crucible_sandbox::SandboxCleanup::Pending,
        stop_mark: Some(stop_mark),
        on_cancel: None,
        _serial: None,
    };

    let stopped = projected.stop();

    assert!(
        stopped.is_err(),
        "a stop hung in its mediator reported success"
    );
    assert!(
        projected.cleanup == crucible_sandbox::SandboxCleanup::Failed,
        "a stop hung in its mediator was not failed cleanup"
    );
    let facts = audit.records().expect("facts");
    assert!(facts.iter().any(|record| matches!(
        record.fact().kind(),
        SandboxFactKind::Cleanup(crucible_sandbox::SandboxCleanup::Failed)
    )));
    assert!(!facts.iter().any(|record| matches!(
        record.fact().kind(),
        SandboxFactKind::Cleanup(crucible_sandbox::SandboxCleanup::Complete)
    )));
}
