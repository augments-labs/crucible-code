//! Asking what would confine a command starts nothing on this machine.
//!
//! The witness is the kernel's count of the minor page faults of every child
//! this process has waited for, `cminflt` in `/proc/self/stat`. No program runs
//! without faulting a page in, and every query the backend makes waits for the
//! process it ran, so a count that has not moved is a process that was not
//! started. The control beside it shows the count moving when the backend is
//! probed, so a witness that could not see a start would fail there rather than
//! pass here.
//!
//! The count is the whole process's, and other tests in this binary start
//! processes of their own. So each measurement runs in a fresh copy of this
//! test binary that runs that one test and nothing else.

use std::process::Command;

use crucible_sandbox::{SandboxManifest, SandboxPolicy, SandboxRequest, SandboxService};
use crucible_types::{Ancestry, SandboxId, ToolId};

use crate::sample::{REQUIRE_ENFORCING_SANDBOX, Sample};
use crate::{LocalSandbox, ObservedVersion};

/// Set in the copy of this binary a measurement runs in.
const ALONE: &str = "CRUCIBLE_TEST_OBSERVE_ALONE";

/// How many page faults the children this process has waited for took.
fn reaped() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("this process's status");
    // The command name is in parentheses and may hold anything; every field
    // after the last parenthesis is a number, and `cminflt` is the ninth.
    let (_, after) = stat.rsplit_once(')').expect("a command name");
    after
        .split_ascii_whitespace()
        .nth(8)
        .expect("a count of children's faults")
        .parse()
        .expect("a number")
}

/// Runs `test` again in a copy of this binary that runs nothing else, and
/// fails where it failed. Inside that copy, `measured` is what runs.
fn alone(test: &str, measured: impl FnOnce()) {
    if std::env::var_os(ALONE).is_some() {
        measured();
        return;
    }
    let name = format!("linux::observe_tests::{test}");
    let ran = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", &name, "--test-threads=1", "--nocapture"])
        .env(ALONE, "1")
        .output()
        .expect("a copy of this test binary");
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(
        ran.status.success() && said.contains("1 passed"),
        "{test} failed alone: {said}{}",
        String::from_utf8_lossy(&ran.stderr)
    );
}

/// A request for `policy`, as an inspection would build one.
fn request(policy: SandboxPolicy) -> SandboxRequest {
    SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("sandbox"),
        policy,
        SandboxManifest::empty(),
    )
}

#[test]
fn observing_this_machine_starts_no_process() {
    alone("observing_this_machine_starts_no_process", || {
        let sample = Sample::new("sandbox-observe-quiet");
        let enabled = SandboxPolicy::standard(&sample.workspace()).expect("policy");
        let disabled = enabled.clone().with_enabled(false);

        for policy in [enabled, disabled] {
            let asked = policy.enabled();
            let before = reaped();
            let observed = LocalSandbox::observe(&request(policy));
            let after = reaped();
            assert_eq!(
                before, after,
                "observing with confinement {asked} started a process"
            );

            // A native backend found without being run cannot have said its
            // version; only the wrapper crucible is itself can.
            match observed {
                Ok(observed) => assert_eq!(
                    matches!(observed.version(), ObservedVersion::Unverified(_)),
                    asked,
                    "{observed:?}"
                ),
                Err(problem) => assert!(
                    asked && std::env::var_os(REQUIRE_ENFORCING_SANDBOX).is_none(),
                    "no backend was observed: {problem}"
                ),
            }
        }
        // Nothing was staged either: the workspace is as the fixture made it.
        assert!(
            std::fs::read_dir(sample.root())
                .expect("the workspace")
                .next()
                .is_none()
        );
    });
}

#[test]
fn probing_this_machine_is_seen_starting_one() {
    alone("probing_this_machine_is_seen_starting_one", || {
        let before = reaped();
        let probed = crucible_runtime::answered!(LocalSandbox::new().probe());
        let after = reaped();
        match probed {
            Ok(_) => assert!(after > before, "a probe ran Bubblewrap unseen"),
            Err(problem) => assert!(
                std::env::var_os(REQUIRE_ENFORCING_SANDBOX).is_none(),
                "the enforcing sandbox backend is required by this job but unavailable: {problem}"
            ),
        }
    });
}
