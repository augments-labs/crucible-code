//! What the confinement report says, what it must never say, and what asking
//! for it must never do.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crucible_client_api::inspection::{
    BackendVersion, Claim, Inspected, Inspection, Network, Requirement, Unit,
};
use crucible_config::Home;
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxEnablement, SandboxError, SandboxFeature, SandboxManifest,
    SandboxPolicy, SandboxRequest, SandboxResourceLimits, SandboxService, SandboxSession,
};
use crucible_sandbox_local::{ObservedVersion, SandboxObservation};
use crucible_types::{Ancestry, SandboxId, ToolId};

use super::{DISABLED, Found, Observed, Observing, Unchanged, admitted, choose, inspecting};
use crate::sample::{Sample, WRITTEN};

/// A backend that holds everything a confined report has to rest on.
///
/// Built up from nothing rather than from a preset, so a test that wants one
/// claim weakened weakens exactly that claim and nothing travels with it.
fn holding() -> SandboxCapabilities {
    [
        SandboxFeature::Filesystem,
        SandboxFeature::NetworkDeny,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
    ]
    .into_iter()
    .fold(SandboxCapabilities::none(), |claims, feature| {
        claims.with(feature, SandboxCapability::Enforced)
    })
}

/// The name every stand-in backend goes by.
fn named() -> SandboxBackendId {
    SandboxBackendId::new("a-backend").expect("a name")
}

/// Somebody's build of something, named the way a real probe would name it.
fn backend() -> SandboxBackendIdentity {
    SandboxBackendIdentity::new(
        named(),
        "1.2.3",
        SandboxBackendProvenance::System,
        Some([0x5a; 32]),
    )
    .expect("an identity")
}

/// Why a stand-in backend's version was not read.
const UNREAD: &str = "reading it would start the backend";

/// Somebody's build of something, found the way observing finds one: measured,
/// not run.
fn found(capabilities: SandboxCapabilities) -> SandboxObservation {
    SandboxObservation::new(
        named(),
        SandboxBackendProvenance::System,
        Some([0x5a; 32]),
        ObservedVersion::Unverified(UNREAD),
        capabilities,
    )
}

/// What `future` answers, waited for on a runtime of its own.
fn awaited<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a test runtime")
        .block_on(future)
}

/// The request an inspection builds for `policy`.
fn request(policy: SandboxPolicy) -> SandboxRequest {
    SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("sandbox"),
        policy,
        SandboxManifest::empty(),
    )
}

/// What an inspection of `sample` makes of `observation`, settled the way
/// [`super::inspection`] settles one.
fn observed(sample: &Sample, policy: &SandboxPolicy, observation: SandboxObservation) -> Observed {
    let request = request(policy.clone());
    Observed {
        at: sample.root(),
        enabled: policy.enabled(),
        required: false,
        found: Ok(Found::settled(observation, &request)),
        request,
    }
}

/// One report over `policy`, as `--sandbox` would have written it.
fn written(sample: &Sample, policy: &SandboxPolicy, capabilities: SandboxCapabilities) -> String {
    observed(sample, policy, found(capabilities)).human()
}

/// The report `observed` is, as the document `--json` writes, read back.
fn document(observed: &Observed) -> Inspected {
    let written = observed.json().expect("an encodable document");
    match Inspection::decode(&written).expect("a document that reads back") {
        Inspection::Inspected(inspected) => *inspected,
        Inspection::Failed(problem) => panic!("an inspection that failed: {problem:?}"),
    }
}

#[test]
fn a_ceiling_is_never_printed_without_the_claim_it_rests_on() {
    let sample = Sample::new("sandbox-report-ceilings");
    let workspace = sample.workspace();
    let standard = SandboxPolicy::standard(&workspace).expect("policy");
    // Narrowed from what the workspace already states rather than assembled
    // beside it, because a policy may only ever be tightened and dropping a
    // ceiling this one holds would be the widening it refuses.
    let limits = SandboxResourceLimits {
        cpu_seconds: Some(120),
        output_bytes: Some(1 << 20),
        // Deliberately not a whole number of anything. A ceiling is a number
        // somebody wrote down, and one printed as "1 MiB" would be a different
        // number from the one that was configured.
        memory_bytes: Some(1_500_000),
        command_time: Some(std::time::Duration::from_secs(90)),
        ..standard.limits()
    };
    let policy = standard.with_limits(limits).expect("limits");

    // One backend that enforces the first, only watches the second, and cannot
    // do the third at all. All three ceilings are the same kind of number in
    // the policy, and the difference between them is the whole answer.
    let capabilities = holding()
        .with(SandboxFeature::CpuLimit, SandboxCapability::Enforced)
        .with(SandboxFeature::OutputLimit, SandboxCapability::Observed);
    let said = written(&sample, &policy, capabilities.clone());

    let ends_with = |stated: &str, claimed: &str| {
        let line = said
            .lines()
            .find(|line| line.trim_start().starts_with(stated))
            .unwrap_or_else(|| panic!("no ceiling line for {stated}: {said}"));
        assert!(line.ends_with(claimed), "{line}");
    };

    // The one that is genuinely a ceiling carries no excuse after it, so the
    // qualified lines are the ones that stand out rather than the plain one.
    ends_with("cpu 2m", "enforced");
    ends_with(
        "1 MiB captured",
        "observed, so it is recorded rather than imposed",
    );
    ends_with(
        "90s per command",
        "unsupported, so this number does nothing",
    );
    ends_with(
        "memory 1500000 bytes",
        "unsupported, so this number does nothing",
    );

    // And the document a script reads says the same of the same numbers, in
    // units rather than words.
    let given = document(&observed(&sample, &policy, found(capabilities)));
    let ceiling = |feature: &str| {
        given
            .effective
            .ceilings
            .iter()
            .find(|ceiling| ceiling.feature.as_str() == feature)
            .map_or_else(
                || panic!("no {feature} ceiling: {given:?}"),
                |ceiling| (ceiling.amount, ceiling.nanos, ceiling.unit, ceiling.claim),
            )
    };
    assert_eq!(
        ceiling("cpu_limit"),
        (120, 0, Unit::Seconds, Claim::Enforced)
    );
    assert_eq!(
        ceiling("output_limit"),
        (1 << 20, 0, Unit::Bytes, Claim::Observed)
    );
    assert_eq!(
        ceiling("command_time_limit"),
        (90, 0, Unit::Seconds, Claim::Unsupported)
    );
    assert_eq!(
        ceiling("memory_limit"),
        (1_500_000, 0, Unit::Bytes, Claim::Unsupported)
    );
    assert_eq!(
        given.effective.ceilings.len(),
        said.lines()
            .skip_while(|line| line.trim() != "ceilings")
            .skip(1)
            .take_while(|line| line.starts_with("    "))
            .count(),
        "{said}"
    );
}

#[test]
fn fractional_time_ceilings_are_reported_without_rounding() {
    let sample = Sample::new("sandbox-report-fractional-time");
    let workspace = sample.workspace();
    for (span, expected) in [
        (std::time::Duration::from_nanos(1), "0.000000001s"),
        (std::time::Duration::from_millis(1500), "1.5s"),
        (std::time::Duration::new(60, 1000), "60.000001s"),
        (std::time::Duration::from_mins(1), "1m"),
        (
            std::time::Duration::new(u64::MAX, 999_999_999),
            "18446744073709551615.999999999s",
        ),
    ] {
        let standard = SandboxPolicy::standard(&workspace).expect("policy");
        let limits = SandboxResourceLimits {
            command_time: Some(span),
            session_time: Some(span),
            ..standard.limits()
        };
        let policy = standard.with_limits(limits).expect("limits");
        let capabilities = holding()
            .with(
                SandboxFeature::CommandTimeLimit,
                SandboxCapability::Enforced,
            )
            .with(
                SandboxFeature::SessionTimeLimit,
                SandboxCapability::Observed,
            );
        let said = written(&sample, &policy, capabilities.clone());
        for (scope, claim) in [
            ("per command", "enforced"),
            (
                "per session",
                "observed, so it is recorded rather than imposed",
            ),
        ] {
            let line = said
                .lines()
                .find(|line| line.contains(scope))
                .expect("time ceiling");
            assert!(
                line.trim_start()
                    .starts_with(&format!("{expected} {scope}")),
                "{line}"
            );
            assert!(line.ends_with(claim), "{line}");
        }

        let given = document(&observed(&sample, &policy, found(capabilities)));
        for feature in ["command_time_limit", "session_time_limit"] {
            let ceiling = given
                .effective
                .ceilings
                .iter()
                .find(|ceiling| ceiling.feature.as_str() == feature)
                .expect("time ceiling");
            assert_eq!(
                (ceiling.amount, ceiling.nanos, ceiling.unit),
                (span.as_secs(), span.subsec_nanos(), Unit::Seconds)
            );
        }
    }
}

#[test]
fn nothing_but_the_directory_that_was_asked_about_reaches_the_report() {
    // A component no digest could contain by accident and no fixed spelling in
    // the report could collide with.
    let sample = Sample::new("secret-tenant-zzqq");
    let workspace = sample.workspace();
    let policy = SandboxPolicy::standard(&workspace).expect("policy");
    let said = written(&sample, &policy, holding());

    // Once, on the line that says which directory the question was about. The
    // roots below it are the same paths and are named by digest, so a report
    // pasted into an issue carries the reach without carrying the tree.
    assert_eq!(
        said.matches("secret-tenant-zzqq").count(),
        1,
        "a path escaped the redaction: {said}"
    );
    let header = said.lines().next().expect("a first line");
    assert!(header.contains("secret-tenant-zzqq"), "{header}");
    assert!(
        said.lines()
            .skip(1)
            .all(|line| !line.contains("secret-tenant-zzqq")),
        "{said}"
    );

    // The document names no directory at all: a script knows where it ran.
    let written = observed(&sample, &policy, found(holding()))
        .json()
        .expect("an encodable document");
    assert!(
        !String::from_utf8_lossy(&written).contains("secret-tenant-zzqq"),
        "a path escaped the redaction"
    );
}

#[test]
fn a_report_that_settled_says_everything_it_settled_on() {
    let sample = Sample::new("sandbox-report-fields");
    let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");
    let observed = observed(
        &sample,
        &policy,
        found(holding()).leaving("whether the backend starts on this host"),
    );
    let said = observed.human();
    assert!(said.contains("  backend   a-backend, system"), "{said}");
    assert!(
        said.contains(&format!("  version   unverified; {UNREAD}")),
        "{said}"
    );
    assert!(
        said.contains(&format!("  build     sha256:{}", "5a".repeat(32))),
        "{said}"
    );
    assert!(said.contains("  mode      optional"), "{said}");
    assert!(said.contains("confined  yes"), "{said}");
    assert!(
        said.contains("not checked, since checking would start something:\n  whether the backend starts on this host"),
        "{said}"
    );
    // Nothing was prepared, so there is no cleanup to have been pending.
    assert!(!said.contains("cleanup"), "{said}");

    let given = document(&observed);
    assert!(given.enabled);
    assert_eq!(given.mode, Requirement::Optional);
    assert!(given.confined);
    assert_eq!(given.refusal, None);
    assert_eq!(
        given
            .unchecked
            .as_ref()
            .map(|said| said.as_str().to_owned()),
        Some(String::from("whether the backend starts on this host"))
    );
    let backend = given.backend.expect("a backend");
    assert_eq!(backend.name.as_str(), "a-backend");
    assert_eq!(backend.provenance.as_str(), "system");
    assert_eq!(backend.build.map(|digest| digest.0), Some([0x5a; 32]));
    assert!(
        matches!(&backend.version, BackendVersion::Unverified(why) if why.as_str() == UNREAD),
        "{:?}",
        backend.version
    );
    // The whole matrix, not just the part this policy leans on.
    assert_eq!(backend.capabilities.len(), holding().iter().count());
    let claimed = |feature: &str| {
        backend
            .capabilities
            .iter()
            .find(|held| held.feature.as_str() == feature)
            .map(|held| held.claim)
    };
    assert_eq!(claimed("filesystem"), Some(Claim::Enforced));
    assert_eq!(claimed("memory_limit"), Some(Claim::Unsupported));

    let plan = crucible_sandbox::plan_inspection(&policy, &SandboxManifest::empty());
    for drafted in [&given.requested, &given.effective] {
        assert!(drafted.enabled);
        assert_eq!(drafted.policy.0, policy.digest());
        assert_eq!(drafted.cwd.0, plan.working_directory());
        assert_eq!(drafted.roots.len(), plan.roots().len());
        assert_eq!(drafted.omitted, 0);
        assert_eq!(
            drafted.hidden,
            u64::try_from(plan.unreadable_patterns()).expect("a count")
        );
        assert_eq!(drafted.network, Network::Closed);
        assert_eq!(drafted.commands.0, plan.command_policy());
        assert_eq!(drafted.staged, 0);
        assert_eq!(drafted.persistent, plan.persistent());
        assert_eq!(drafted.snapshots, plan.snapshots());
    }
    for (root, planned) in given.effective.roots.iter().zip(plan.roots()) {
        assert_eq!(root.access.as_str(), planned.access().as_str());
        assert_eq!(root.provenance.as_str(), planned.provenance().as_str());
        assert_eq!(root.identity.0, planned.identity());
    }
}

#[test]
fn a_backend_that_would_not_take_the_policy_still_says_what_it_can_hold() {
    let sample = Sample::new("sandbox-report-refused");
    let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");
    // The refusal a policy requiring confinement meets on a machine whose
    // kernel will not give crucible its own namespaces.
    let capabilities = holding().with(
        SandboxFeature::ProcessIsolation,
        SandboxCapability::Unsupported,
    );
    let why = SandboxError::Unsupported {
        feature: SandboxFeature::ProcessIsolation,
    };
    let observed = observed(&sample, &policy, found(capabilities).refusing(why));
    let said = observed.human();

    // The matrix is the point of printing anything at all here: a refusal on
    // its own repeats what the failing command already said, and the line that
    // explains it is the one feature this backend says it cannot hold.
    assert!(said.contains("a-backend, system"), "{said}");
    assert!(said.contains("process_isolation     unsupported"), "{said}");
    assert!(said.contains("no command could be run here"), "{said}");
    assert!(said.contains("what was asked for:"), "{said}");
    // And nothing is claimed about a session nobody could have.
    assert!(!said.contains("what a command would run under"), "{said}");
    assert!(!said.contains("confined"), "{said}");

    let given = document(&observed);
    assert!(!given.confined);
    assert!(given.backend.is_some());
    assert!(given.refusal.is_some());
    assert_eq!(observed.contract().expect("a document").status(), "refused");
}

#[test]
fn nothing_found_is_said_as_that_rather_than_as_an_empty_report() {
    let sample = Sample::new("sandbox-report-absent");
    let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");
    let observed = Observed {
        at: sample.root(),
        enabled: true,
        required: true,
        request: request(policy),
        found: Err(SandboxError::BackendUnavailable {
            reason: "nothing was installed".into(),
        }),
    };
    let said = observed.human();

    assert!(said.contains("no sandbox backend was found"), "{said}");
    assert!(said.contains("nothing was installed"), "{said}");
    assert!(
        said.contains("  mode      required by project configuration"),
        "{said}"
    );
    // No matrix, because there is nobody whose claims those would be. An empty
    // one would read as a backend that holds nothing, which is a different and
    // much worse thing to be told.
    assert!(!said.contains("what this backend can hold"), "{said}");

    let given = document(&observed);
    assert_eq!(given.mode, Requirement::Required);
    assert!(given.backend.is_none());
    assert!(!given.confined);
    assert!(
        given
            .refusal
            .is_some_and(|why| why.as_str().contains("nothing was installed"))
    );
    // A ceiling nobody was found to hold is held by nobody.
    assert!(
        given
            .effective
            .ceilings
            .iter()
            .all(|ceiling| ceiling.claim == Claim::Unsupported)
    );
    assert_eq!(
        observed.contract().expect("a document").status(),
        "unavailable"
    );
}

#[test]
fn disabled_confinement_is_reported_as_an_explicit_choice() {
    let sample = Sample::new("sandbox-report-disabled");
    let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");
    let confined = written(&sample, &policy, holding());
    assert!(confined.contains("enabled   true"), "{confined}");
    assert!(confined.contains("confined  yes"), "{confined}");

    let policy = policy.with_enabled(false);
    let compatibility = SandboxObservation::new(
        named(),
        SandboxBackendProvenance::Compatibility,
        None,
        ObservedVersion::Stated("1"),
        SandboxCapabilities::none(),
    );
    let observed = observed(&sample, &policy, compatibility);
    let given = observed.human();
    assert!(given.contains("sandbox disabled in"), "{given}");
    assert!(given.contains("enabled   false"), "{given}");
    assert!(given.contains("confined  no"), "{given}");
    assert!(given.contains(&format!("disabled  {DISABLED}")), "{given}");
    assert!(given.contains("  version   1\n"), "{given}");
    assert!(given.contains("  build     not measured"), "{given}");

    // Compatibility is never called confined, in either form.
    let document = document(&observed);
    assert!(!document.enabled);
    assert!(!document.effective.enabled);
    assert!(!document.confined);
    assert_eq!(
        document
            .backend
            .map(|backend| backend.provenance.as_str().to_owned()),
        Some(String::from("compatibility"))
    );
}

/// A backend that counts what it is asked to do, and would rather be observed.
#[derive(Default)]
struct Counting {
    probed: AtomicUsize,
    prepared: AtomicUsize,
    observed: AtomicUsize,
}

impl SandboxService for Counting {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.probed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok((backend(), holding())) })
    }

    fn prepare(
        &self,
        _request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(SandboxError::BackendUnavailable {
                reason: "a counting backend prepares nothing".into(),
            })
        })
    }
}

impl Observing for Counting {
    fn observe(&self, _request: &SandboxRequest) -> Result<SandboxObservation, SandboxError> {
        self.observed.fetch_add(1, Ordering::SeqCst);
        Ok(found(holding()))
    }
}

/// Every file under `root`, with its bytes and when it last changed.
fn tree(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, Option<std::time::SystemTime>)> {
    let mut seen = BTreeMap::new();
    let mut left = vec![root.to_path_buf()];
    while let Some(directory) = left.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let entry = entry.expect("an entry");
            let at = entry.path();
            let metadata = entry.metadata().expect("its metadata");
            let changed = metadata.modified().ok();
            if metadata.is_dir() {
                seen.insert(at.clone(), (Vec::new(), changed));
                left.push(at);
            } else {
                seen.insert(
                    at.clone(),
                    (std::fs::read(&at).expect("its bytes"), changed),
                );
            }
        }
    }
    seen
}

/// This tree's home directory as crucible would find it.
fn home(sample: &Sample) -> Home {
    Home::find(&|name: &str| (name == crucible_config::HOME).then(|| OsString::from(sample.home())))
        .expect("an absolute path was given")
}

#[test]
fn inspecting_neither_probes_nor_prepares_nor_writes_nor_reads_a_credential() {
    let sample = Sample::new("sandbox-inspect-quiet");
    // A credential on the disk, so that one read into the report would show.
    drop(sample.stored("anthropic"));
    drop(sample.settings(r#"{"sandbox":{"enabled":true}}"#));
    let before = (tree(&sample.root()), tree(&sample.home()));

    let service = Counting::default();
    let observed = inspecting(&service, &sample.root(), &home(&sample)).expect("a report");

    assert_eq!(
        service.probed.load(Ordering::SeqCst),
        0,
        "inspecting probed"
    );
    assert_eq!(
        service.prepared.load(Ordering::SeqCst),
        0,
        "inspecting prepared"
    );
    assert_eq!(service.observed.load(Ordering::SeqCst), 1);

    let said = observed.human();
    let written = observed.json().expect("an encodable document");
    assert!(!said.contains(WRITTEN), "a credential reached the report");
    assert!(
        !String::from_utf8_lossy(&written).contains(WRITTEN),
        "a credential reached the document"
    );
    // A project that turns confinement on also forbids turning it off here.
    assert!(
        said.contains("  mode      required by project configuration"),
        "{said}"
    );
    assert!(said.contains("confined  yes"), "{said}");

    // Nothing was staged, written or touched.
    assert_eq!((tree(&sample.root()), tree(&sample.home())), before);
}

#[test]
fn inspecting_this_machine_writes_one_document_that_reads_back() {
    let sample = Sample::new("sandbox-inspect-native");
    let observed = super::inspection(&sample.root(), &home(&sample)).expect("a report");
    let written = observed.json().expect("an encodable document");
    let line = written.strip_suffix(b"\n").expect("a newline at the end");
    assert!(!line.contains(&b'\n'), "more than one line");
    let read = Inspection::decode(&written).expect("a document that reads back");
    assert!(
        ["ready", "refused", "unavailable"].contains(&read.status()),
        "{}",
        read.status()
    );
    assert!(observed.human().ends_with('\n'));
}

#[test]
fn an_unavailable_backend_cannot_change_the_choice() {
    let control = SandboxEnablement::new(false, false);
    assert!(
        awaited(choose(
            &control,
            true,
            async { Err(Unchanged::Stopped("native boundary unavailable".into())) },
            || panic!("an unavailable boundary cannot be saved")
        ))
        .is_err()
    );
    assert!(!control.enabled());
    awaited(choose(&control, true, async { Ok(()) }, || Ok(()))).unwrap();
    assert!(control.enabled());
}

#[test]
fn a_project_requirement_survives_interactive_disabling() {
    let control = SandboxEnablement::new(true, true);
    assert!(
        awaited(choose(
            &control,
            false,
            async { panic!("disabling must not probe") },
            || panic!("a required boundary cannot be disabled")
        ))
        .is_err()
    );
    assert!(control.enabled());
    let optional = SandboxEnablement::new(true, false);
    awaited(choose(
        &optional,
        false,
        async { panic!("disabling needs no enforcing backend") },
        || Ok(()),
    ))
    .unwrap();
    assert!(!optional.enabled());
}

/// A backend that answers every question only after it has once had to wait,
/// the way one reached over a pipe or a socket does.
struct Late;

/// Pending once, with the waker told to poll again, then ready.
async fn later() {
    let mut waited = false;
    std::future::poll_fn(|context| {
        if waited {
            return std::task::Poll::Ready(());
        }
        waited = true;
        context.waker().wake_by_ref();
        std::task::Poll::Pending
    })
    .await;
}

impl SandboxService for Late {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async {
            later().await;
            Ok((backend(), holding()))
        })
    }

    fn prepare(
        &self,
        _request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async {
            later().await;
            Err(SandboxError::BackendUnavailable {
                reason: "the late backend's own words".into(),
            })
        })
    }
}

#[test]
fn a_backend_that_answers_late_is_heard_rather_than_given_up_on() {
    let sample = Sample::new("sandbox-choice-late");
    let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");

    let answered = awaited(admitted(&Late, policy));
    assert!(
        matches!(
            &answered,
            Err(Unchanged::Stopped(said)) if said.contains("the late backend's own words")
        ),
        "{answered:?}"
    );
}
