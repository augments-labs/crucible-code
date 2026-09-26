//! What the resource ceilings do once the workload is inside the sandbox.
//!
//! The plan tests next door prove the numbers reach the broker's argument list.
//! These start a real command and ask the kernel, from inside the namespace,
//! what it was actually given — the half an argument list cannot show.
//!
//! The process ceiling is asked twice over, because one kind of host answers
//! only one of the two questions: the kernel's own `Max processes` line where
//! the host renders a ceiling, and a workload's fork loop being refused where
//! the host renders none.

use std::os::unix::process::ExitStatusExt as _;

use crucible_sandbox::{
    SandboxCapability, SandboxError, SandboxFeature, SandboxManifest, SandboxNetworkPolicy,
    SandboxPolicy, SandboxRequest, SandboxResourceLimits, SandboxService,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

use super::tests::{command, finish};
use crate::sample::{Sample, skipped_without_enforcement};

/// What `/proc/self/limits` says the scope may hold, read back from inside it.
///
/// The file is the kernel's own answer rather than the shell's, so a shell that
/// spells `ulimit` differently on another host cannot change what this reads.
fn stated_process_ceiling(limits: &str) -> Option<(u64, u64)> {
    let line = limits
        .lines()
        .find(|line| line.starts_with("Max processes"))?;
    let mut fields = line.split_whitespace().skip(2);
    let mut number = || fields.next()?.parse::<u64>().ok();
    Some((number()?, number()?))
}

#[test]
fn stated_process_ceiling_distinguishes_absent_and_unlimited_limits() {
    assert_eq!(stated_process_ceiling(""), None);
    assert_eq!(
        stated_process_ceiling("Max processes             unlimited            unlimited"),
        None
    );
    assert_eq!(
        stated_process_ceiling("Max processes             1024                 1024"),
        Some((1024, 1024))
    );
}

/// Whether the confined scope reached the process ceiling a policy stated.
///
/// Two readings, and the second stands in for the first where the first is
/// unavailable. `/proc/self/limits` states the ceiling the kernel holds where
/// the host renders a `Max processes` line, and states none where the host
/// renders none — the line missing, or `unlimited` where the ceiling is not
/// spelled — and a host that cannot be asked is not a scope that was given the
/// wrong ceiling. What every host answers is the kernel's behaviour instead:
/// the workload's loop asks for two hundred children, which the broker's own
/// ceiling would have let through, so a loop refused before its own end is the
/// stated ceiling being enforced.
///
/// A host that does state the ceiling is held to it exactly, so a ceiling that
/// is merely low fails here rather than passing as one that could not be read.
fn stated_ceiling_reached_the_scope(limits: &str, stated: (u64, u64), fork_refused: bool) -> bool {
    stated_process_ceiling(limits).map_or(fork_refused, |ceiling| ceiling == stated)
}

#[test]
fn an_unstated_process_ceiling_is_answered_by_a_refused_fork_loop() {
    // A limits dump with no `Max processes` line in it, which is the whole of
    // what a host that renders no process ceiling hands the workload.
    let unstated = "Limit                     Soft Limit           Hard Limit           Units     \n\
                    Max cpu time              3600                 3600                 seconds   \n\
                    Max open files            4096                 4096                 files     \n";
    // The workload asked for two hundred children and was refused, which the
    // broker's own ceiling would not have done. That is a stated ceiling
    // enforced, so a host that states no ceiling still proves one arrived.
    assert!(stated_ceiling_reached_the_scope(unstated, (16, 16), true));
    // Nothing stated and nothing refused is no evidence either way, so the
    // ceiling is unproven rather than reached.
    assert!(!stated_ceiling_reached_the_scope(unstated, (16, 16), false));
    // The broker's own ceiling travelling in place of the stated one: the
    // limits line is there and it is wrong, so it fails whether or not the
    // loop happened to be refused.
    let owned = "Max processes             1024                 1024                 processes \n\
                 Max open files            4096                 4096                 files     \n";
    assert!(!stated_ceiling_reached_the_scope(owned, (16, 16), true));
    assert!(!stated_ceiling_reached_the_scope(owned, (16, 16), false));
    // A host that states the ceiling exactly is proven by stating it.
    let exact = "Max processes             16                   16                   processes \n";
    assert!(stated_ceiling_reached_the_scope(exact, (16, 16), false));
}

#[test]
fn a_confined_command_holds_the_process_ceiling_the_broker_owns() {
    let service = crate::sample::service();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-process-ceiling-owned");
    let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");
    // Nothing here states a process ceiling. The broker owns one anyway, the
    // way it owns the core-dump ceiling, so the scope beneath PID 1 is bounded
    // whether or not a caller thought to ask.
    assert_eq!(policy.limits().processes, None);
    let mut session = crucible_runtime::answered!(service.prepare(SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("processes"),
        policy,
        SandboxManifest::empty(),
    )))
    .expect("prepared sandbox");
    crucible_runtime::answered!(session.materialize()).expect("materialized workspace");

    let (status, output, errors) = finish(
        crucible_runtime::answered!(session.start(command("cat /proc/self/limits")))
            .expect("started command"),
    );

    assert!(
        status.success(),
        "{status} {}",
        String::from_utf8_lossy(&errors)
    );
    let limits = String::from_utf8(output).expect("utf8");
    // A host that states no ceiling cannot be asked to hold one; the broker still
    // sets one in the scope, which the second test covers.
    let Some(ceiling) = stated_process_ceiling(&limits) else {
        return;
    };
    assert_eq!(ceiling, (1024, 1024), "{limits}");
}

#[test]
fn a_stated_process_ceiling_stops_the_command_forking_past_it() {
    let service = crate::sample::service();
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
            crucible_runtime::answered!(service.prepare(request)),
            Err(SandboxError::Unsupported {
                feature: SandboxFeature::ProcessLimit
            })
        ));
        return;
    }

    let mut session =
        crucible_runtime::answered!(service.prepare(request)).expect("prepared sandbox");
    crucible_runtime::answered!(session.materialize()).expect("materialized workspace");
    let (status, output, errors) = finish(
        crucible_runtime::answered!(session.start(command(
            "cat /proc/self/limits; i=0; \
                 while [ \"$i\" -lt 200 ]; do sleep 1 & i=$((i+1)); done; echo unbounded",
        )))
        .expect("started command"),
    );

    let output = String::from_utf8(output).expect("utf8");
    let errors = String::from_utf8(errors).expect("utf8");
    // Whether the stated ceiling reached the scope, asked the way this host can
    // answer: the kernel's `Max processes` line where the host states one, and
    // the refusal of the loop below where it states none. The parsed value
    // rather than the whole `/proc/self/limits` text, because a panic message
    // keeps only its first 128 characters and would cut the very line this
    // reads. The line itself is checked in the tests above.
    let refused =
        !output.contains("unbounded") && errors.contains("Cannot fork") && !status.success();
    assert!(
        stated_ceiling_reached_the_scope(&output, (16, 16), refused),
        "a stated process ceiling of sixteen is neither stated nor enforced: \
         {status} {errors} {output}"
    );
    // The ceiling is what the kernel hands back and also what it enforces: the
    // loop asks for 200 children and never reaches the end of its own script.
    // Each way it can escape the ceiling is named, so a failure says which one.
    assert!(!output.contains("unbounded"), "{output}");
    assert!(errors.contains("Cannot fork"), "{errors}");
    assert!(!status.success(), "{status}");
}

#[test]
fn requested_open_file_limit_is_hard_before_workload_exec() {
    let service = crate::sample::service();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-open-file-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        true,
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
    let mut session =
        crucible_runtime::answered!(service.prepare(request)).expect("supported hard limit");
    crucible_runtime::answered!(session.materialize()).expect("materialized workspace");

    let (status, output, errors) = finish(
        crucible_runtime::answered!(session.start(command("ulimit -n")))
            .expect("started limited command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8(output).expect("utf8").trim(), "32");
}

#[test]
fn requested_address_space_limit_is_hard_before_workload_exec() {
    let service = crate::sample::service();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-address-space-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        true,
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
    let mut session =
        crucible_runtime::answered!(service.prepare(request)).expect("supported hard limit");
    crucible_runtime::answered!(session.materialize()).expect("materialized workspace");

    let (status, output, errors) = finish(
        crucible_runtime::answered!(session.start(command("ulimit -v")))
            .expect("started limited command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8(output).expect("utf8").trim(), "65536");
}

#[test]
fn requested_cpu_limit_terminates_the_workload_scope() {
    let service = crate::sample::service();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-cpu-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        true,
        base.filesystem().iter().cloned(),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            cpu_seconds: Some(1),
            ..SandboxResourceLimits::default()
        },
    )
    .expect("policy");
    // Nothing else here ends a command: no deadline, so the kill below is the
    // CPU ceiling's and not a clock's.
    assert_eq!(policy.limits().command_time, None);
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("cpu-limit"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session =
        crucible_runtime::answered!(service.prepare(request)).expect("supported hard limit");
    crucible_runtime::answered!(session.materialize()).expect("materialized workspace");

    let (status, output, _) = finish(
        crucible_runtime::answered!(session.start(command("ulimit -t; while :; do :; done")))
            .expect("started limited command"),
    );

    // The ceiling is counted in CPU time, so how long it takes on the clock is
    // this host's CPU share, not a property of the ceiling: a wall-clock bound
    // here failed a correct build on a busy host and passed one handed twice
    // the ceiling. What is read instead is the kernel's own answer, from inside
    // the workload, that it runs under the one second asked for, and the kill
    // the kernel sends when that second is spent — at the hard limit, which
    // the broker sets equal to the soft one.
    assert_eq!(String::from_utf8(output).expect("utf8").trim(), "1");
    assert_eq!(
        status.signal(),
        Some(rustix::process::Signal::KILL.as_raw()),
        "CPU-bound workload escaped its ceiling: {status}"
    );
}
