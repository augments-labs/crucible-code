//! The conformance suite run the way an adapter outside this tree runs it.
//!
//! The unit tests reach into the module and check the judgement it makes. This
//! one deliberately cannot: it sees only what a container, a remote executor or
//! another operating system's backend would see after adding this crate as a
//! dependency. If the suite stops being usable from there — a type left
//! private, a verdict that cannot be read, a report that needs a fixture only
//! this repository has — this test stops compiling, and that is the point of
//! it. Publishing a suite nobody outside can run would be the same empty claim
//! the suite itself exists to catch.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_core::{SandboxError, SandboxFeature};
use crucible_tools::LocalSandbox;
use crucible_tools::conformance::{Conformance, SandboxClaim};

/// The variable a job sets to say the enforcing backend must be there.
///
/// An adapter outside this tree decides for itself whether a host with no
/// backend is a skip or a failure, so the suite reports that host as an error
/// and says nothing about which it is. This test makes the same decision the
/// in-crate ones do, and spells the name rather than importing it because the
/// crate does not publish a test harness: the other two spellings are
/// `crates/crucible-tools/src/sample.rs`, which pins this string, and
/// `.github/workflows/rust-ci.yml`, which sets it on the one job that installs
/// a backend.
const REQUIRE_ENFORCING_SANDBOX: &str = "CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX";

/// A directory the caller owns, which is all the suite asks for.
struct Owned(PathBuf);

impl Owned {
    // A directory that cannot be made is not a conformance result, and there is
    // nothing to report over without one.
    #[allow(clippy::expect_used)]
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let at =
            std::env::temp_dir().join(format!("crucible-{name}-{unique}-{}", std::process::id()));
        fs::create_dir_all(&at).expect("a directory");
        // Policies hold canonical absolute paths, and a temporary directory is
        // a symbolic link to somewhere else on more than one platform.
        Self(fs::canonicalize(&at).expect("a canonical directory"))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[allow(clippy::expect_used)]
fn the_published_suite_runs_from_outside_and_answers_the_whole_table() {
    let at = Owned::new("conformance");
    let service = LocalSandbox::new();
    let audited = match Conformance::audit(&service, &at.0) {
        Ok(audited) => audited,
        // No backend is not a backend that lies, and the suite is right to
        // refuse rather than report a table of faults over nothing. Where the
        // job has said a backend must exist, that refusal is the failure.
        Err(SandboxError::BackendUnavailable { reason }) => {
            assert!(
                std::env::var_os(REQUIRE_ENFORCING_SANDBOX).is_none(),
                "the enforcing sandbox backend is required by this job but unavailable: {reason}"
            );
            return;
        }
        Err(other) => panic!("a probe: {other}"),
    };

    // Whatever this host selected, every feature is answered and no answer
    // contradicts the claim it was read against.
    assert_eq!(audited.findings().len(), SandboxFeature::COUNT);
    let faults: Vec<_> = audited
        .faults()
        .map(|finding| {
            format!(
                "{} {}",
                finding.feature().as_str(),
                finding.verdict().as_str()
            )
        })
        .collect();
    assert!(faults.is_empty(), "{faults:?}\n{}", audited.report());

    // The families are the unit an adapter is judged in, and each one is
    // reachable and complete on its own.
    let mut within = 0;
    for claim in SandboxClaim::ALL {
        assert!(
            audited.holds(claim),
            "{}\n{}",
            claim.as_str(),
            audited.report()
        );
        within += audited.within(claim).count();
    }
    assert_eq!(within, SandboxFeature::COUNT);

    // The report is what an adapter author reads, so it names the backend and
    // every row rather than only the ones that went wrong.
    let said = audited.report();
    assert!(said.contains(audited.backend().version()), "{said}");
    for feature in SandboxFeature::ALL {
        assert!(
            said.contains(feature.as_str()),
            "{}\n{said}",
            feature.as_str()
        );
    }
    println!("{said}");
}
