//! What the resource ceilings do once the workload is inside the sandbox.
//!
//! The plan tests next door prove the numbers reach the broker's argument list.
//! These start a real command and ask the kernel, from inside the namespace,
//! what it was actually given — the half an argument list cannot show.
//!
//! The process ceiling is asked twice over, because one kind of host answers
//! only one of the two questions: the kernel's own `Max processes` line where
//! the host renders a ceiling, and a workload's fork loop being refused where
//! the host renders none. Both ceilings are read through that one rule, so the
//! test that states one and the test that lets the broker own one cannot answer
//! differently about the same host.

use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;

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

/// The name each child a fork loop starts leaves behind in the workspace.
const CHILD: &str = "child-";

/// The name the probe leaves behind to say it reached its loop.
const BEGAN: &str = "began";

/// A command that reports the scope's own limits and then asks for `asked`
/// children, leaving a [`CHILD`] marker behind for each one it started, and a
/// [`BEGAN`] marker behind if it got as far as the loop at all.
///
/// The children outlive the loop on purpose, because a ceiling counts the
/// processes a scope holds at once: children that exit as fast as they are
/// forked never fill one, and a shell that reaps them and one that leaves them
/// to become zombies would then disagree about how many were started. Sixty
/// seconds is past the longest a command here may run, so no child can expire
/// inside the loop however slow the host, and the broker kills the whole group
/// as soon as the workload ends.
///
/// [`BEGAN`] is written before the loop and before the limits are read, so a
/// workload that ran and then printed nothing is told apart from one that never
/// started at all. A count of children cannot do that on its own: a scope that
/// never ran and a scope the ceiling refused at its very first fork both leave
/// no marker behind, and only the first of them is a failure.
fn fork_loop_probe(asked: u64) -> String {
    format!(
        ": > {BEGAN}; cat /proc/self/limits; i=0; while [ \"$i\" -lt {asked} ]; do \
         sleep 60 > {CHILD}$i & i=$((i+1)); done; echo unbounded"
    )
}

/// Whether the probe reached its loop, read from the marker rather than the
/// shell's word for it.
///
/// An unreadable path counts as not begun, which is the direction this rule
/// fails in: a marker this test cannot see is a claim about the workload it
/// cannot support.
fn workload_began(root: &Path) -> bool {
    root.join(BEGAN).exists()
}

/// How many children the probe's loop actually started.
///
/// Counted from the workspace rather than read out of what the shell said,
/// because a refused fork is where shells differ most. dash ends the script
/// and prints `Cannot fork`. bash retries the fork with a backoff totalling
/// fifteen seconds before it gives up, prints `fork: retry: Resource
/// temporarily unavailable` instead, and then either ends the script or carries
/// on to the end of the loop, printing `unbounded` and succeeding. What it
/// started is the one reading both agree on, and it is the same under each: 14
/// children under a ceiling of sixteen, 1021 or 1022 under the broker's own
/// 1024.
fn children_started(root: &Path) -> u64 {
    let children = std::fs::read_dir(root)
        .expect("a workspace whose children this test can count")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(CHILD))
        })
        .count();
    u64::try_from(children).expect("a child count that fits a u64")
}

/// Whether the confined scope reached the process ceiling a policy stated, and
/// held it.
///
/// Where the host renders a `Max processes` line the ceiling is held to it
/// exactly, so a ceiling that is merely low fails here rather than passing as
/// one that could not be read. Where the host renders none — the line missing,
/// or `unlimited` where the ceiling is not spelled — a host that cannot be
/// asked is not a scope that was given the wrong ceiling, and the kernel's
/// behaviour answers instead: a workload that began asked for more children than
/// the ceiling allows, and fewer than it asked for came to exist.
///
/// All four conditions are required, and each rules out a different way past
/// the ceiling. The workload began at all: a count of children is not evidence
/// of a ceiling when the workload that would have started them never ran, and a
/// broker that cannot exec the shell its caller asked for leaves an empty dump,
/// no children and a status nobody should be reading — which is a scope that
/// started nothing, not one that was refused. Fewer children than the loop asked
/// for, counted from a workload that began, is a loop the ceiling refused; a
/// loop refused at its first fork is still a refusal, and a kernel that counts
/// processes for the whole real user refuses one on a busy host. No more
/// children than the ceiling allows rules out a ceiling larger than the stated
/// one passing for it: a loop refused at 199 under a stated sixteen was
/// refused, and was not this ceiling. The two tests that read a process ceiling
/// differ in the ceiling they state and in how many children the loop asks for,
/// and in nothing else.
fn stated_ceiling_reached_the_scope(
    limits: &str,
    stated: (u64, u64),
    began: bool,
    children: u64,
    asked: u64,
) -> bool {
    began
        && children < asked
        && children <= stated.0
        && stated_process_ceiling(limits).is_none_or(|ceiling| ceiling == stated)
}

#[test]
fn an_unstated_process_ceiling_is_answered_by_a_refused_fork_loop() {
    // A limits dump with no `Max processes` line in it, which is the whole of
    // what a host that renders no process ceiling hands the workload.
    let unstated = "Limit                     Soft Limit           Hard Limit           Units     \n\
                    Max cpu time              3600                 3600                 seconds   \n\
                    Max open files            4096                 4096                 files     \n";
    // The workload asked for two hundred children and the ceiling allowed
    // fourteen, which the broker's own ceiling would not have done. That is a
    // stated ceiling enforced, so a host that states no ceiling still proves
    // one arrived.
    assert!(stated_ceiling_reached_the_scope(
        unstated,
        (16, 16),
        true,
        14,
        200
    ));
    // A loop that began every child it asked for is no evidence of a ceiling
    // at all, so the ceiling is unproven rather than reached.
    assert!(!stated_ceiling_reached_the_scope(
        unstated,
        (16, 16),
        true,
        200,
        200
    ));
    // A ceiling that is merely low refuses the loop as surely as the stated
    // one, and is not it. This is the case the count rules out, and a bare
    // refusal could not.
    assert!(!stated_ceiling_reached_the_scope(
        unstated,
        (16, 16),
        true,
        199,
        200
    ));
    // The broker's own ceiling travelling in place of the stated one: the
    // limits line is there and it is wrong, so it fails whether or not the
    // loop was refused.
    let owned = "Max processes             1024                 1024                 processes \n\
                 Max open files            4096                 4096                 files     \n";
    assert!(!stated_ceiling_reached_the_scope(
        owned,
        (16, 16),
        true,
        14,
        200
    ));
    assert!(!stated_ceiling_reached_the_scope(
        owned,
        (16, 16),
        true,
        200,
        200
    ));
    // A host that states the ceiling exactly is proven by stating it, and the
    // loop it refused is proven by the children it was allowed to start.
    let exact = "Max processes             16                   16                   processes \n";
    assert!(stated_ceiling_reached_the_scope(
        exact,
        (16, 16),
        true,
        14,
        200
    ));
    // A stated ceiling the loop was never refused by is stated and not held.
    assert!(!stated_ceiling_reached_the_scope(
        exact,
        (16, 16),
        true,
        200,
        200
    ));
    // No children at all, from a workload that never began, satisfies both count
    // clauses and leaves the dump unreadable: an empty dump, a count of zero and
    // no refusal is what a broker that cannot exec the shell leaves behind, and
    // it is the case the count alone cannot see. This is the case a bare
    // `children < asked` fallback would pass.
    assert!(!stated_ceiling_reached_the_scope(
        unstated,
        (16, 16),
        false,
        0,
        200
    ));
    assert!(!stated_ceiling_reached_the_scope(
        owned,
        (1024, 1024),
        false,
        0,
        1280
    ));
    // The marker is not a second reading of the count. A workload that began
    // and was refused at its very first fork started no children either, and
    // that is a ceiling holding: a kernel counting processes for the whole real
    // user refuses one on a busy host, so no marker is left to say so.
    assert!(stated_ceiling_reached_the_scope(
        unstated,
        (16, 16),
        true,
        0,
        200
    ));
    // And a workload that began is required whatever the count says, so a probe
    // whose loop ran without leaving the marker behind proves nothing.
    assert!(!stated_ceiling_reached_the_scope(
        unstated,
        (16, 16),
        false,
        14,
        200
    ));
}

/// How many children the loop in the broker-owned probe asks for.
///
/// The broker's own ceiling is `PROCESSES` — 1024, in `broker.rs` — and the
/// scope beneath PID 1 holds the broker and this shell before the first child,
/// so a loop that asks for more than the ceiling can ever allow is one the
/// ceiling refuses on every host and under either shell.
///
/// The ask is a quarter above the ceiling rather than two above it, because the
/// children are counted after the scope is torn down and a child the broker
/// killed before it opened its marker is one the count misses: that cost 7 of
/// 200 markers in one run here and 7 of 1280 in another. A margin of two would
/// let a loop that was never refused read as one that was, and a quarter leaves
/// that race far smaller than the margin it has to fit inside. It costs nothing
/// when the ceiling holds, because a refused loop stops at the ceiling however
/// far past it the ask goes — 1021 or 1022 of 1280 across the runs measured
/// here.
const OWNED_PROBE_ASKS: u64 = 1024 + 1024 / 4;

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
        crucible_runtime::answered!(session.start(command(&fork_loop_probe(OWNED_PROBE_ASKS))))
            .expect("started command"),
    );

    let output = String::from_utf8(output).expect("utf8");
    let errors = String::from_utf8(errors).expect("utf8");
    // The same two readings the stated ceiling is read through, and for the
    // same reason: the kernel's own `Max processes` line where this host
    // renders one, and the children the loop was allowed to start where it
    // renders none. A host that renders no ceiling cannot be asked to hold one
    // by reading, so the loop is what answers — and it asks for more children
    // than the broker's own ceiling allows, which an unbounded scope would
    // have started every one of. The marker the probe writes before its loop is
    // read as well, because a scope that started nothing at all is not one the
    // ceiling refused anything.
    let began = workload_began(sample.root());
    let children = children_started(sample.root());
    assert!(
        stated_ceiling_reached_the_scope(&output, (1024, 1024), began, children, OWNED_PROBE_ASKS),
        "the broker's own process ceiling of 1024 is neither stated nor enforced: \
         {children} of {OWNED_PROBE_ASKS} children started; the workload began: {began}; \
         {status} {errors} {output}"
    );
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
    // Far above the ceiling the broker holds when nothing states one, so a loop
    // that began every child it asked for is a ceiling that never travelled.
    let asked = 200;
    let (status, output, errors) = finish(
        crucible_runtime::answered!(session.start(command(&fork_loop_probe(asked))))
            .expect("started command"),
    );

    let output = String::from_utf8(output).expect("utf8");
    let errors = String::from_utf8(errors).expect("utf8");
    // Whether the stated ceiling reached the scope, and whether it held, asked
    // the two ways this host can answer: the kernel's `Max processes` line
    // where the host states one, and the children the loop was allowed to start
    // where it states none. The parsed ceiling and the count rather than the
    // whole `/proc/self/limits` text, because a panic message keeps only its
    // first 128 characters and would cut the very line this reads. The line
    // itself is checked in the tests above.
    //
    // Nothing here reads the shell's own report, because a refused fork is
    // where shells differ most. Under a bash `/bin/sh` the message is not
    // `Cannot fork`, and whether the script ends there or runs on to print
    // `unbounded` and exit successfully is bash's choice rather than the
    // ceiling's: both were measured here. The children the loop was allowed to
    // start, and the marker the probe left before it began, are what every
    // shell agrees happened.
    let began = workload_began(sample.root());
    let children = children_started(sample.root());
    assert!(
        stated_ceiling_reached_the_scope(&output, (16, 16), began, children, asked),
        "a stated process ceiling of sixteen is neither stated nor enforced: \
         {children} of {asked} children started; the workload began: {began}; \
         {status} {errors} {output}"
    );
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
