//! A terminated leader is reaped once the operating system has finished
//! ending it, which a loaded machine can take a while to do.

use super::*;

/// When the leader below becomes reapable, counted from its termination.
///
/// A loaded Windows host has been seen to take longer than a quarter of a
/// second to make a leader whose job it had emptied reapable, and a stop that
/// gave up then reported a command that had in fact been stopped as one it
/// could not stop.
const REAPABLE_AFTER: Duration = Duration::from_millis(500);

#[cfg(unix)]
fn exit_status() -> ExitStatus {
    std::os::unix::process::ExitStatusExt::from_raw(0)
}

#[cfg(windows)]
fn exit_status() -> ExitStatus {
    std::os::windows::process::ExitStatusExt::from_raw(0)
}

#[test]
fn a_leader_the_system_is_slow_to_finish_ending_is_reaped() {
    let terminated = Instant::now();

    let reaped = reaped_within(REAP, || {
        Ok((terminated.elapsed() >= REAPABLE_AFTER).then(exit_status))
    });

    assert!(
        reaped.is_ok(),
        "a leader reapable {REAPABLE_AFTER:?} after termination was given up on: {reaped:?}"
    );
}

#[test]
fn a_leader_that_never_becomes_reapable_is_given_up_on() {
    let asked = Instant::now();

    let reaped = reaped_within(REAP, || Ok(None));

    assert_eq!(
        reaped.map_err(|problem| problem.kind()).err(),
        Some(io::ErrorKind::TimedOut),
        "an unreapable leader was reported reaped"
    );
    assert!(asked.elapsed() >= REAP, "it gave up before its bound");
}
