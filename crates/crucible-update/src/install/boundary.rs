//! The durable boundaries of staging, activation and recovery, named where
//! each is crossed.
//!
//! A boundary is the moment just after one change to the install is made,
//! and synced when it must outlive the process: the lock taken, a leftover
//! removed, a staged file written, the unit moved into place, the active
//! release switched. Its name is all a
//! shipped build does with it. The tests start a process that is killed the
//! moment it crosses a chosen boundary, once for each boundary there is, and
//! hold what that process left to the same rule a person relies on: one whole
//! release active, and an install the next update recovers.

/// Says that `boundary` has just been crossed.
#[cfg(not(test))]
pub(super) fn crossed(boundary: &'static str) {
    let _ = boundary;
}

/// Says that `boundary` has just been crossed: written to the file
/// [`CROSSED`] names when it is set, and then, at the crossing [`KILL_AT`]
/// counts to, the process kills itself.
#[cfg(test)]
pub(super) fn crossed(boundary: &'static str) {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNT: AtomicUsize = AtomicUsize::new(0);

    if let Some(record) = std::env::var_os(CROSSED) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(record)
            .expect("the record of boundaries crossed");
        writeln!(file, "{boundary}").expect("a boundary recorded");
    }
    let Some(at) = std::env::var_os(KILL_AT) else {
        return;
    };
    let at: usize = at
        .to_str()
        .and_then(|at| at.parse().ok())
        .expect("a crossing to kill at");
    if COUNT.fetch_add(1, Ordering::SeqCst) + 1 == at {
        let _ =
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL);
    }
}

/// The file each boundary crossed is written to, one name a line.
#[cfg(test)]
pub(super) const CROSSED: &str = "CRUCIBLE_TEST_UPDATE_CROSSED";

/// The crossing, counted from one, at which the process kills itself.
#[cfg(test)]
pub(super) const KILL_AT: &str = "CRUCIBLE_TEST_UPDATE_KILL_AT";
