//! What the resource ceilings do once the workload is inside the sandbox.
//!
//! The plan tests next door prove the numbers reach the broker's argument list.
//! These start a real command and ask the kernel, from inside the namespace,
//! what it was actually given — the half an argument list cannot show.

use std::time::{Duration, Instant};

use crucible_core::{
    Ancestry, SandboxCapability, SandboxError, SandboxFeature, SandboxId, SandboxManifest,
    SandboxMode, SandboxNetworkPolicy, SandboxPolicy, SandboxRequest, SandboxResourceLimits,
    SandboxService, ToolId,
};

use super::tests::{command, finish};
use crate::LocalSandbox;
use crate::sample::{Sample, skipped_without_enforcement};

/// What `/proc/self/limits` says the scope may hold, read back from inside it.
///
/// The file is the kernel's own answer rather than the shell's, so a shell that
/// spells `ulimit` differently on another host cannot change what this reads.
fn stated_process_ceiling(limits: &str) -> (u64, u64) {
    let line = limits
        .lines()
        .find(|line| line.starts_with("Max processes"))
        .expect("a process ceiling in /proc/self/limits");
    let mut fields = line.split_whitespace().skip(2);
    let mut number = || {
        fields
            .next()
            .expect("a ceiling")
            .parse::<u64>()
            .expect("a number")
    };
    (number(), number())
}

#[test]
fn a_confined_command_holds_the_process_ceiling_the_broker_owns() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-process-ceiling-owned");
    let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");
    // Nothing here states a process ceiling. The broker owns one anyway, the
    // way it owns the core-dump ceiling, so the scope beneath PID 1 is bounded
    // whether or not a caller thought to ask.
    assert_eq!(policy.limits().processes, None);
    let mut session = service
        .prepare(SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("processes"),
            policy,
            SandboxManifest::empty(),
        ))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let (status, output, errors) = finish(
        session
            .start(command("cat /proc/self/limits"))
            .expect("started command"),
    );

    assert!(
        status.success(),
        "{status} {}",
        String::from_utf8_lossy(&errors)
    );
    let limits = String::from_utf8(output).expect("utf8");
    assert_eq!(stated_process_ceiling(&limits), (1024, 1024), "{limits}");
}

#[test]
fn a_stated_process_ceiling_stops_the_command_forking_past_it() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-process-ceiling-stated");
    let standard = SandboxPolicy::standard(&sample.workspace()).expect("policy");
    // Narrow enough that a loop reaches it at once, and far under the ceiling
    // the broker holds when nothing states one, so a ceiling that never
    // travelled would read as 1024 rather than as this.
    let limits = SandboxResourceLimits {
        processes: Some(16),
        ..standard.limits()
    };
    let policy = standard.with_limits(limits).expect("a narrowed ceiling");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("processes"),
        policy,
        SandboxManifest::empty(),
    );

    // On a kernel that counts processes for the whole real user rather than for
    // this namespace, a stated ceiling is one the backend cannot keep, and the
    // sandbox says so instead of running the command under a ceiling that would
    // bound the host's other work.
    if super::probe::process_limit() != SandboxCapability::Enforced {
        assert!(matches!(
            service.prepare(request),
            Err(SandboxError::Unsupported {
                feature: SandboxFeature::ProcessLimit
            })
        ));
        return;
    }

    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, output, errors) = finish(
        session
            .start(command(
                "cat /proc/self/limits; i=0; \
                 while [ \"$i\" -lt 200 ]; do sleep 1 & i=$((i+1)); done; echo unbounded",
            ))
            .expect("started command"),
    );

    let output = String::from_utf8(output).expect("utf8");
    let errors = String::from_utf8(errors).expect("utf8");
    assert_eq!(stated_process_ceiling(&output), (16, 16), "{output}");
    // The ceiling is what the kernel hands back and also what it enforces: the
    // loop asks for 200 children and never reaches the end of its own script.
    assert!(!output.contains("unbounded"), "{output}");
    assert!(errors.contains("Cannot fork"), "{errors}");
    assert!(!status.success(), "{status}");
}

#[test]
fn requested_open_file_limit_is_hard_before_workload_exec() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-open-file-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned(),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            open_files: Some(32),
            ..SandboxResourceLimits::default()
        },
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("open-file-limit"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("supported hard limit");
    session.materialize().expect("materialized workspace");

    let (status, output, errors) = finish(
        session
            .start(command("ulimit -n"))
            .expect("started limited command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8(output).expect("utf8").trim(), "32");
}

#[test]
fn requested_address_space_limit_is_hard_before_workload_exec() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-address-space-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned(),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            memory_bytes: Some(64 * 1024 * 1024),
            ..SandboxResourceLimits::default()
        },
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("address-space-limit"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("supported hard limit");
    session.materialize().expect("materialized workspace");

    let (status, output, errors) = finish(
        session
            .start(command("ulimit -v"))
            .expect("started limited command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8(output).expect("utf8").trim(), "65536");
}

#[test]
fn requested_cpu_limit_terminates_the_workload_scope() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-cpu-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned(),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            cpu_seconds: Some(1),
            ..SandboxResourceLimits::default()
        },
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("cpu-limit"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("supported hard limit");
    session.materialize().expect("materialized workspace");
    let started = Instant::now();

    let (status, _, _) = finish(
        session
            .start(command("while :; do :; done"))
            .expect("started limited command"),
    );

    assert!(!status.success(), "CPU-bound workload escaped its ceiling");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "CPU ceiling did not terminate the workload promptly"
    );
}
