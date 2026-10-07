//! What the doctor says, and what asking it must never do: dial out, ask for
//! a credential it would have to renew, start a program, or write a byte.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crucible_auth::{
    LoginAttempt, LoginMethod, OAuthError, Store, StoredCredentials, SubscriptionLogin,
};
use crucible_client_api::Text;
use crucible_client_api::doctor::{Report, Status};
use crucible_config::{ConfigError, Home};
use crucible_credentials::Credential;
use crucible_provider::OpenAi;
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxError, SandboxFeature, SandboxRequest, SandboxService,
    SandboxSession,
};
use crucible_sandbox_local::{ObservedVersion, SandboxObservation};

use super::{CHECKS, Host, examining, human, said};
use crate::sample::{Sample, WRITTEN};
use crate::sandbox::Observing;
use crate::subscription::Subscriptions;

/// A key exported for the run, spelled so that one copied anywhere shows.
const CANARY: &str = "sk-doctor-canary-0d1c";

/// The checks a configuration that cannot be read leaves with nothing to
/// stand on.
const DEPENDENT: [&str; 4] = ["credentials", "provider", "sandbox-policy", "mcp"];

/// The checks that stand on the home directory and nothing in the checkout.
const INDEPENDENT: [&str; 7] = [
    "home",
    "workspace",
    "private-state",
    "credential-store",
    "sandbox-backend",
    "extension-discovery",
    "extension-trust",
];

/// A backend that holds everything a confined command has to rest on.
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

fn named() -> SandboxBackendId {
    SandboxBackendId::new("a-backend").expect("a name")
}

/// A sandbox service that counts what it is asked, and answers observing the
/// way a machine with a good backend would.
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
        Box::pin(async {
            Err(SandboxError::BackendUnavailable {
                reason: "a counting backend probes nothing".into(),
            })
        })
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
        Ok(SandboxObservation::new(
            named(),
            SandboxBackendProvenance::System,
            Some([0x5a; 32]),
            ObservedVersion::Unverified("reading it would start the backend"),
            holding(),
        ))
    }
}

/// An account login whose credential can only be had by dialling `to`: the
/// stand-in for a token that has to be renewed before it can be applied.
#[derive(Debug)]
struct Dialing {
    to: SocketAddr,
}

impl SubscriptionLogin for Dialing {
    fn provider(&self) -> &'static str {
        "openai"
    }

    fn start(&self, _method: LoginMethod, _store: Store) -> Result<LoginAttempt, OAuthError> {
        Err(OAuthError::Method)
    }

    fn credential(&self, _stored: &StoredCredentials) -> Option<Box<dyn Credential>> {
        drop(TcpStream::connect(self.to));
        None
    }
}

/// No account login at all, so that nothing a test does could reach one.
fn none() -> Subscriptions {
    Subscriptions::new(Vec::new(), Vec::new())
}

/// This tree's home directory as crucible would find it.
fn home(sample: &Sample) -> Result<Home, ConfigError> {
    Home::find(&|name: &str| (name == crucible_config::HOME).then(|| OsString::from(sample.home())))
}

/// An environment holding nothing.
fn bare(_name: &str) -> Option<String> {
    None
}

/// An environment holding the canary as OpenAI's key.
fn exported(name: &str) -> Option<String> {
    (name == "OPENAI_API_KEY").then(|| CANARY.to_owned())
}

/// What the doctor says of `sample`, asked of `service` and `subscriptions`.
fn examined(
    sample: &Sample,
    service: &Counting,
    subscriptions: &Subscriptions,
    from: &dyn Fn(&str) -> Option<String>,
) -> Report {
    let found = home(sample);
    let root = sample.root();
    examining(
        service,
        subscriptions,
        Host {
            here: Some(&root),
            home: found.as_ref(),
            from,
            running: env!("CARGO_PKG_VERSION"),
        },
    )
}

/// Each check's status, by id.
fn statuses(report: &Report) -> BTreeMap<&str, Status> {
    report
        .checks
        .iter()
        .map(|check| (check.id.as_str(), check.status))
        .collect()
}

/// The status of the check called `id`.
fn status(report: &Report, id: &str) -> Status {
    statuses(report)
        .get(id)
        .copied()
        .unwrap_or_else(|| panic!("no check called {id}"))
}

/// The reason the check called `id` gives.
fn reason<'a>(report: &'a Report, id: &str) -> &'a str {
    report
        .checks
        .iter()
        .find(|check| check.id.as_str() == id)
        .map_or_else(
            || panic!("no check called {id}"),
            |check| check.reason.as_str(),
        )
}

/// Writes `document` as `path`, making its directory.
fn put(path: &Path, document: &str) {
    fs::create_dir_all(path.parent().expect("a directory")).expect("a temporary directory");
    fs::write(path, document).expect("a temporary file");
}

/// A store holding one account login for OpenAI, written as bytes rather than
/// through a read, since every read but the doctor's tightens the file.
fn signed_in(sample: &Sample) {
    put(
        &sample.home().join("auth.json"),
        r#"{"version":2,"keys":{},"subscriptions":{"openai":{"access_token":"test-access","refresh_token":"test-refresh","details":{"account_id":"test-account"},"expires_at":18446744073709551615,"refreshed_at":1}},"identities":{}}"#,
    );
}

/// Every file under `root`, with its bytes, when it last changed and its
/// permissions, and every directory by its presence and permissions.
fn tree(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, Option<std::time::SystemTime>, u32)> {
    let mut seen = BTreeMap::new();
    let mut left = vec![root.to_path_buf()];
    while let Some(directory) = left.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let at = entry.expect("an entry").path();
            let metadata = fs::symlink_metadata(&at).expect("its metadata");
            let mode = permissions(&metadata);
            if metadata.is_dir() {
                seen.insert(at.clone(), (Vec::new(), None, mode));
                left.push(at);
            } else {
                let changed = metadata.modified().ok();
                seen.insert(
                    at.clone(),
                    (fs::read(&at).expect("its bytes"), changed, mode),
                );
            }
        }
    }
    seen
}

#[cfg(unix)]
fn permissions(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn permissions(metadata: &fs::Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("a mode");
}

#[test]
fn the_doctor_never_asks_for_an_applied_credential_so_nothing_is_dialled() {
    let sample = Sample::new("doctor-denied-network");
    signed_in(&sample);
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let to = listener.local_addr().expect("its address");
    let subscriptions = Subscriptions::new(
        vec![(Arc::new(Dialing { to }), OpenAi::SUBSCRIPTION)],
        Vec::new(),
    );

    let report = examined(&sample, &Counting::default(), &subscriptions, &bare);

    listener
        .set_nonblocking(true)
        .expect("a nonblocking listener");
    match listener.accept() {
        Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("the doctor dialled out: it asked for a credential to apply"),
        Err(problem) => panic!("the listener failed: {problem}"),
    }
    // The login is still seen for what it is: held, and the source a launch
    // would use, read off the store's names alone.
    assert_eq!(status(&report, "credentials"), Status::Ok);
    assert!(
        reason(&report, "credentials").contains("openai from a stored account login"),
        "{}",
        reason(&report, "credentials")
    );
}

#[cfg(unix)]
#[test]
fn the_doctor_writes_nothing_not_even_to_tighten_what_it_reports_as_open() {
    let sample = Sample::new("doctor-denied-write");
    signed_in(&sample);
    drop(sample.user(r#"{"provider":"openai"}"#));
    drop(sample.settings(r#"{"sandbox":{"enabled":true}}"#));
    chmod(&sample.home(), 0o755);
    chmod(&sample.home().join("auth.json"), 0o644);
    chmod(&sample.user_file(), 0o644);
    let before = (tree(&sample.root()), tree(&sample.home()));

    let report = examined(&sample, &Counting::default(), &none(), &bare);

    assert!(
        before == (tree(&sample.root()), tree(&sample.home())),
        "the doctor changed a file, a mode or a directory"
    );
    assert!(!sample.home().join("sessions").exists());
    // What it would have tightened is what it reports instead.
    assert_eq!(status(&report, "private-state"), Status::Warning);
}

#[cfg(unix)]
#[test]
fn the_doctor_starts_nothing_neither_a_backend_nor_a_declared_server() {
    let sample = Sample::new("doctor-denied-launch");
    let marker = sample.home().join("started");
    drop(sample.user(&format!(
        r#"{{"mcp":{{"servers":{{"marker":{{"command":"/bin/sh","args":["-c","touch {}"],"required":true}}}}}}}}"#,
        marker.display()
    )));
    let service = Counting::default();

    let report = examined(&sample, &service, &none(), &bare);

    assert_eq!(
        service.probed.load(Ordering::SeqCst),
        0,
        "the doctor probed"
    );
    assert_eq!(
        service.prepared.load(Ordering::SeqCst),
        0,
        "the doctor prepared"
    );
    assert!(
        service.observed.load(Ordering::SeqCst) >= 1,
        "nothing was observed"
    );
    assert!(!marker.exists(), "a declared server was started");
    // Declared, counted and not started — and said to be unchecked for it.
    assert_eq!(status(&report, "mcp"), Status::Ok);
    let said = reason(&report, "mcp");
    assert!(said.contains("1 declared, 1 required"), "{said}");
    assert!(!said.contains("marker"), "{said}");
}

#[test]
fn no_credential_value_reaches_either_form_of_the_report() {
    let sample = Sample::new("doctor-no-secret");
    drop(sample.stored("anthropic"));
    drop(sample.user(r#"{"provider":"openai","providers":{"openai":{"model":"gpt-5"}}}"#));

    let report = examined(&sample, &Counting::default(), &none(), &exported);
    let said = human(&report);
    let document = String::from_utf8(report.encode().expect("an encodable report"))
        .expect("a document in text");

    for (form, words) in [("text", &said), ("document", &document)] {
        assert!(
            !words.contains(CANARY),
            "an exported key reached the {form} form"
        );
        assert!(
            !words.contains(WRITTEN),
            "a stored key reached the {form} form"
        );
    }
    // Where each came from is named instead.
    let credentials = reason(&report, "credentials");
    assert!(
        credentials.contains("anthropic from a stored API key"),
        "{credentials}"
    );
    assert!(
        credentials.contains("openai from environment variable OPENAI_API_KEY"),
        "{credentials}"
    );
}

#[test]
fn a_configuration_that_cannot_be_read_leaves_the_checks_that_need_it_unavailable() {
    let sample = Sample::new("doctor-config-broken");
    put(&sample.root().join(".crucible/config.json"), "{ not json");

    let report = examined(&sample, &Counting::default(), &none(), &bare);
    let found = statuses(&report);

    assert_eq!(found.get("config"), Some(&Status::Failed), "{found:?}");
    for id in DEPENDENT {
        assert_eq!(found.get(id), Some(&Status::Unavailable), "{id}: {found:?}");
    }
    for id in INDEPENDENT {
        let got = found.get(id).copied();
        assert!(
            got.is_some() && got != Some(Status::Unavailable),
            "{id} did not run: {found:?}"
        );
    }
    assert_eq!(report.exit(), 2);
    // A failure names the layer and sends the reader to the command that
    // names the file, rather than printing a path from a checkout.
    assert!(
        reason(&report, "config").contains("project"),
        "{}",
        reason(&report, "config")
    );
    assert!(!human(&report).contains(&sample.root().display().to_string()));
}

#[test]
fn every_check_is_made_under_its_own_id_in_one_order_whatever_happens() {
    // The ids are the interface a script holds on to, so they are written
    // out here as they shipped rather than read from the list they test.
    assert_eq!(
        CHECKS,
        [
            "home",
            "workspace",
            "config",
            "private-state",
            "credential-store",
            "credentials",
            "provider",
            "sandbox-backend",
            "sandbox-policy",
            "extension-discovery",
            "extension-trust",
            "mcp",
        ]
    );

    let healthy = Sample::new("doctor-ids-healthy");
    let broken = Sample::new("doctor-ids-broken");
    put(&broken.root().join(".crucible/config.json"), "[]");
    for report in [
        examined(&healthy, &Counting::default(), &none(), &exported),
        examined(&broken, &Counting::default(), &none(), &bare),
    ] {
        let ids: Vec<&str> = report
            .checks
            .iter()
            .map(|check| check.id.as_str())
            .collect();
        assert_eq!(ids, CHECKS);
        assert!(report.encode().is_ok(), "a report its own contract refuses");
    }
}

#[test]
fn without_a_home_or_a_directory_what_rests_on_them_is_unavailable_and_the_rest_runs() {
    let sample = Sample::new("doctor-homeless");
    let homeless = Home::find(&|_: &str| None);
    let service = Counting::default();
    let root = sample.root();

    let report = examining(
        &service,
        &none(),
        Host {
            here: Some(&root),
            home: homeless.as_ref(),
            from: &bare,
            running: env!("CARGO_PKG_VERSION"),
        },
    );
    let found = statuses(&report);
    assert_eq!(found.get("home"), Some(&Status::Failed), "{found:?}");
    assert_eq!(found.get("workspace"), Some(&Status::Ok), "{found:?}");
    assert_eq!(found.get("sandbox-backend"), Some(&Status::Ok), "{found:?}");
    for id in [
        "config",
        "credential-store",
        "credentials",
        "provider",
        "sandbox-policy",
        "extension-discovery",
        "extension-trust",
        "mcp",
    ] {
        assert_eq!(found.get(id), Some(&Status::Unavailable), "{id}: {found:?}");
    }

    let found = home(&sample);
    let report = examining(
        &service,
        &none(),
        Host {
            here: None,
            home: found.as_ref(),
            from: &bare,
            running: env!("CARGO_PKG_VERSION"),
        },
    );
    let found = statuses(&report);
    assert_eq!(found.get("workspace"), Some(&Status::Failed), "{found:?}");
    for id in [
        "config",
        "sandbox-backend",
        "sandbox-policy",
        "credentials",
        "provider",
        "mcp",
    ] {
        assert_eq!(found.get(id), Some(&Status::Unavailable), "{id}: {found:?}");
    }
    for id in [
        "home",
        "credential-store",
        "extension-discovery",
        "extension-trust",
    ] {
        let got = found.get(id).copied();
        assert!(
            got.is_some() && got != Some(Status::Unavailable),
            "{id}: {found:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn the_exit_is_zero_when_healthy_one_for_a_warning_and_two_for_a_failure() {
    let ready = Sample::new("doctor-exit-healthy");
    drop(ready.user(r#"{"provider":"openai","providers":{"openai":{"model":"gpt-5"}}}"#));
    chmod(&ready.home(), 0o700);
    chmod(&ready.user_file(), 0o600);
    let report = examined(&ready, &Counting::default(), &none(), &exported);
    let left: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.status != Status::Ok)
        .map(|check| (check.id.as_str(), check.status, check.reason.as_str()))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!((report.exit(), report.status()), (0, "healthy"));
    assert!(human(&report).starts_with("crucible doctor: healthy\n"));

    // A provider with no model chosen still answers once one is named, so it
    // is a warning rather than a failure.
    let unchosen = Sample::new("doctor-exit-warning");
    drop(unchosen.user(r#"{"provider":"openai"}"#));
    chmod(&unchosen.home(), 0o700);
    chmod(&unchosen.user_file(), 0o600);
    let report = examined(&unchosen, &Counting::default(), &none(), &exported);
    assert_eq!(status(&report, "provider"), Status::Warning);
    assert_eq!((report.exit(), report.status()), (1, "warnings"));

    // Nothing to ask anyone with is a conversation that cannot start.
    let empty = Sample::new("doctor-exit-failed");
    let report = examined(&empty, &Counting::default(), &none(), &bare);
    assert_eq!(status(&report, "provider"), Status::Failed);
    assert_eq!((report.exit(), report.status()), (2, "failed"));
    for check in &report.checks {
        if matches!(check.status, Status::Warning | Status::Failed) {
            assert!(
                check.remedy.is_some(),
                "{} gives no remedy",
                check.id.as_str()
            );
        }
    }
}

#[test]
fn a_provider_this_build_does_not_serve_is_a_failure_that_names_the_ones_it_does() {
    let sample = Sample::new("doctor-unknown-provider");
    drop(sample.user(r#"{"provider":"nobody"}"#));

    let report = examined(&sample, &Counting::default(), &none(), &exported);

    assert_eq!(status(&report, "provider"), Status::Failed);
    assert!(
        reason(&report, "provider").contains("openai"),
        "{}",
        reason(&report, "provider")
    );
}

#[test]
fn an_extension_turned_on_without_a_digest_is_a_warning_that_names_nobody() {
    let sample = Sample::new("doctor-extension-unpinned");
    sample.installed(
        "reviewer",
        r#"{"id": "acme.reviewer", "version": "1.4.0", "protocol": "1.0",
            "entrypoint": "bin/reviewer", "minimumCrucible": "0.34.0",
            "capabilities": ["registerTools"], "contributions": ["tools"]}"#,
    );
    drop(sample.user(r#"{"extensions":{"acme.reviewer":{"enabled":true}}}"#));

    let report = examined(&sample, &Counting::default(), &none(), &exported);

    assert_eq!(status(&report, "extension-discovery"), Status::Ok);
    assert_eq!(status(&report, "extension-trust"), Status::Warning);
    assert!(!human(&report).contains("acme.reviewer"));
}

#[test]
fn words_from_a_configuration_file_reach_either_form_without_their_control_or_format_characters() {
    let sample = Sample::new("doctor-hostile-words");
    // An escape and a bell, then what reorders or hides text without being a
    // control character: a right-to-left override, an isolate, a zero-width
    // space, a byte order mark, and the line and paragraph separators.
    drop(sample.user(
        r#"{"provider":"openai","providers":{"openai":{"model":"red\u001b[31mmodel\u0007\u202eledom\u2066\u200b\ufeff\u2028\u2029end"}}}"#,
    ));

    let report = examined(&sample, &Counting::default(), &none(), &exported);
    let said = human(&report);
    let document = String::from_utf8(report.encode().expect("an encodable report"))
        .expect("a document in text");

    assert!(said.contains("provider:"), "{said:?}");
    assert!(said.contains("red"), "{said:?}");
    assert!(said.contains("end"), "{said:?}");
    for (form, words) in [("text", &said), ("document", &document)] {
        for hostile in [
            '\u{1b}', '\u{7}', '\u{202e}', '\u{2066}', '\u{200b}', '\u{feff}', '\u{2028}',
            '\u{2029}',
        ] {
            assert!(
                !words.contains(hostile),
                "U+{:04X} reached the {form} form: {words:?}",
                u32::from(hostile)
            );
        }
    }
}

#[test]
fn the_doctor_replaces_every_character_a_limit_name_drops_and_the_two_separators() {
    // The format characters are listed by hand, as a limit's name lists them,
    // since no Unicode table is in the standard library; this holds the two
    // lists to each other. The line and paragraph separators end a line in
    // some terminals and viewers, and a reason is one line.
    const SEPARATORS: [char; 2] = ['\u{2028}', '\u{2029}'];
    // A line feed is where a reason's lines are joined, which the test below
    // holds.
    for character in (0..=u32::from(char::MAX))
        .filter_map(char::from_u32)
        .filter(|character| *character != '\n')
    {
        let words = format!("a{character}b");
        let line = said(&words);
        let replaced = line.as_str() != words;
        assert!(
            !replaced || line.as_str() == "a\u{fffd}b",
            "U+{:04X}",
            u32::from(character)
        );
        let dropped =
            crucible_types::GroupName::new(&words).is_some_and(|name| name.as_str() == "ab");
        assert_eq!(
            replaced,
            dropped || SEPARATORS.contains(&character),
            "U+{:04X}",
            u32::from(character)
        );
    }
}

#[test]
fn words_cut_at_their_ceiling_are_marked_as_cut_in_the_text_form() {
    // A provider's name longer than a reason may be: the report is cut, and
    // a person reading it is told so, where the document says it in a field.
    let sample = Sample::new("doctor-cut-words");
    let long = format!("{}TAIL", "p".repeat(20_000));
    drop(sample.user(&format!(r#"{{"provider":"{long}"}}"#)));

    let report = examined(&sample, &Counting::default(), &none(), &exported);
    let said = human(&report);

    let provider = said
        .lines()
        .find(|line| line.contains(" provider: "))
        .expect("a line for the provider");
    assert!(!provider.contains("TAIL"), "the reason was not cut");
    assert!(
        provider.ends_with(" [cut]"),
        "{:?}",
        provider.get(provider.len().saturating_sub(80)..)
    );
    let mut lines = said.lines();
    assert_eq!(lines.next(), Some("crucible doctor: failed"));
    assert_eq!(
        lines.next(),
        Some("  some words were cut short, each where it says [cut]")
    );
    // Nothing that was not cut is marked: the opening line and the
    // provider's are the two.
    assert_eq!(said.matches(" [cut]").count(), 2);

    // A remedy cut is marked as a reason is, and a report with nothing cut
    // says nothing of cutting.
    let mut cut = report.clone();
    cut.checks.retain(|check| check.id.as_str() == "home");
    if let Some(home) = cut.checks.first_mut() {
        home.status = Status::Warning;
        home.remedy = Some(Text::cut(&"r".repeat(20_000)));
    }
    let said = human(&cut);
    assert!(
        said.lines()
            .nth(1)
            .is_some_and(|line| line.contains("[cut]")),
        "{said:?}"
    );
    assert!(
        said.lines()
            .nth(3)
            .is_some_and(|line| line.ends_with("r [cut]"))
    );
    let whole = human(
        &examined(&sample, &Counting::default(), &none(), &bare)
            .checks
            .first()
            .map(|home| Report {
                checks: vec![home.clone()],
            })
            .expect("a home check"),
    );
    assert!(!whole.contains("[cut]"), "{whole:?}");
}

#[test]
fn words_written_over_several_lines_are_one_line_of_the_report() {
    // The probe says what it refused one line each, indented under the
    // first; a check's reason is one line, so its lines are joined in order.
    let joined = said("not found; refused:\n  the first place\n  the second place\n");
    assert_eq!(
        joined.as_str(),
        "not found; refused: the first place; the second place"
    );
    // Any other control character is still replaced where it stands.
    assert_eq!(said("a\u{1b}b").as_str(), "a\u{fffd}b");
}

#[test]
fn the_command_line_reference_names_every_check_the_doctor_makes() {
    let reference = include_str!("../../../../docs/reference/cli.md");
    let section = reference
        .split_once("### `doctor [--json]`")
        .map(|(_, after)| {
            after
                .split_once("\n## ")
                .map_or(after, |(within, _)| within)
        })
        .expect("a section for the doctor");
    let missing: Vec<&str> = CHECKS
        .iter()
        .copied()
        .filter(|id| !section.contains(&format!("| `{id}` |")))
        .collect();
    assert_eq!(missing, Vec::<&str>::new());
}
