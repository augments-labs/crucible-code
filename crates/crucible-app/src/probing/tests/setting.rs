//! The probe a registry install takes follows the sandbox setting: with it off
//! the release is started as a plain local process and admitted, with it on a
//! sandbox that cannot close the network starts nothing, and the policy the
//! release is started under closes the network; the policy the sandbox is
//! handed is enabled with the network closed in the sandbox and disabled with
//! the setting off.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crucible_config::Settings;
use crucible_external_harnesses::fake_peer::scripts::acp;
use crucible_external_harnesses::fake_peer::{self, Reply, Script};
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxError, SandboxFeature, SandboxFilesystemProvenance,
    SandboxNetworkPolicy, SandboxPolicy, SandboxRequest, SandboxService, SandboxSession,
};
use tokio::runtime::Handle;

use super::super::{Confinement, Mode, Unprobed, local, policy, probe};
use crate::launching::Unprobeable;
use crate::sample::Sample;

/// How long the fake peer is given to answer.
const ANSWERING: Duration = Duration::from_secs(20);

/// A sandbox that runs what it is given as the local one does, but whose
/// backend cannot be proven to close the network.
struct Open(Arc<dyn SandboxService>);

impl SandboxService for Open {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async {
            Err(SandboxError::BackendUnavailable {
                reason: "no network sandbox on this machine".into(),
            })
        })
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        self.0.prepare(request)
    }
}

fn open(runtime: Handle) -> Arc<dyn SandboxService> {
    Arc::new(Open(local(runtime)))
}

/// A sandbox that says it closes the network and holds the filesystem, keeps
/// the policy each request hands it, and starts nothing.
struct Recording(Arc<Mutex<Vec<SandboxPolicy>>>);

impl SandboxService for Recording {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async {
            let unnamed = || SandboxError::BackendUnavailable {
                reason: "no identity".into(),
            };
            let name = SandboxBackendId::new("a-backend").map_err(|_| unnamed())?;
            let identity = SandboxBackendIdentity::new(
                name,
                "1.2.3",
                SandboxBackendProvenance::System,
                Some([0x5a; 32]),
            )
            .map_err(|_| unnamed())?;
            let holding = [SandboxFeature::NetworkDeny, SandboxFeature::Filesystem]
                .into_iter()
                .fold(SandboxCapabilities::none(), |claims, feature| {
                    claims.with(feature, SandboxCapability::Enforced)
                });
            Ok((identity, holding))
        })
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.policy().clone());
        Box::pin(async {
            Err(SandboxError::BackendUnavailable {
                reason: "this sandbox only records".into(),
            })
        })
    }
}

/// A release unpacked with the fake peer as its program, answering
/// `initialize` and counting its starts, and the entry that lists it.
struct Release {
    sample: Sample,
    root: PathBuf,
    entry: PathBuf,
}

impl Release {
    fn new(name: &str) -> Self {
        let sample = Sample::new(name);
        let root = sample.root().join("release");
        let program = format!("agent{}", std::env::consts::EXE_SUFFIX);
        let reply = Reply::new().hear(1).say(acp::probed());
        fake_peer::install(&root, "agent", &Script::of([reply])).expect("the fake peer is built");
        let target = |platform: &str| {
            serde_json::json!({
                "platform": platform,
                "cmd": program,
                "args": ["acp"],
                "env": {},
                "hosts": ["github.com"],
                "artifacts": [{
                    "url": "https://github.com/example/agent/releases/download/v1/agent.zip",
                    "sha256": "a".repeat(64),
                    "size": 1000,
                    "format": "zip",
                    "unpacked": 3000,
                    "decompressed": 4000,
                    "files": [{
                        "path": program, "size": 2900, "sha256": "b".repeat(64), "exec": true
                    }],
                    "kept": [program],
                }],
            })
        };
        let platforms = [
            "linux-x86_64",
            "linux-aarch64",
            "darwin-x86_64",
            "darwin-aarch64",
            "windows-x86_64",
            "windows-aarch64",
            "freebsd-x86_64",
        ];
        let entry = serde_json::json!({
            "key": "fake-agent",
            "registry_id": "fake-agent-acp",
            "version": "1.0.0",
            "kind": "binary",
            "license": "MIT",
            "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/1",
            "admits": {"name": "fake-agent", "version": "1.0.0"},
            "reach": [],
            "sign_in_hosts": [],
            "state_paths": [],
            "auth_methods": [],
            "targets": platforms.map(target),
        });
        let entry_file = sample.home().join("entry.json");
        fs::create_dir_all(sample.home()).expect("a home");
        fs::write(&entry_file, entry.to_string()).expect("the entry");
        Self {
            sample,
            root,
            entry: entry_file,
        }
    }

    /// The user's settings as `document` writes them, read through their
    /// owner.
    fn settings(&self, document: &str) -> Settings {
        self.sample.user(document)
    }

    /// What the probe gives under the mode `settings` sets, in a sandbox that
    /// cannot close the network.
    fn probed(&self, settings: &Settings) -> Result<String, Unprobed> {
        let mode = Mode::under(settings, false, &|_| None)?;
        probe(&self.root, &self.entry, mode, &open, ANSWERING)
    }
}

#[test]
fn an_install_probe_with_the_sandbox_off_starts_the_release_unconfined() {
    let release = Release::new("probe-setting-off");
    let off = release.settings(r#"{"sandbox": {"enabled": false}}"#);

    let printed = release.probed(&off).expect("started with the sandbox off");

    let answered: serde_json::Value = serde_json::from_str(&printed).expect("JSON");
    assert_eq!(answered.get("sandbox"), Some(&serde_json::json!("off")));
    assert_eq!(fake_peer::played(&release.root), 1, "{printed}");
    let admitted = crate::installing::record(&printed, "linux-x86_64").expect("admitted");
    assert_eq!(&*admitted.admits.name, "fake-agent");
}

#[test]
fn an_install_probe_with_the_sandbox_on_and_no_closing_sandbox_is_refused() {
    let release = Release::new("probe-setting-on");
    let on = release.settings(r#"{"sandbox": {"enabled": true}}"#);

    let refused = release
        .probed(&on)
        .expect_err("no sandbox that closes the network");

    assert!(
        matches!(refused, Unprobed::Start(Unprobeable::Unclosable(_))),
        "{refused:?}"
    );
    assert_eq!(
        fake_peer::played(&release.root),
        0,
        "nothing was started before the refusal"
    );
}

#[test]
fn an_install_probe_with_the_sandbox_on_closes_the_network() {
    let release = Release::new("probe-setting-network");
    let on = release.settings(r#"{"sandbox": {"enabled": true}}"#);
    let mode = Mode::under(&on, false, &|_| None).expect("the sandbox needs no word");
    assert_eq!(mode.confinement(), Confinement::Required);

    // The policy a probe in the closed sandbox is started under.
    let held = policy(&release.root, SandboxFilesystemProvenance::Workspace, true)
        .expect("an install probe policy");

    assert!(held.enabled());
    assert_eq!(held.network(), &SandboxNetworkPolicy::Closed);
}

#[test]
fn the_sandbox_is_handed_a_closed_policy_in_the_sandbox_and_a_disabled_one_with_it_off() {
    let release = Release::new("probe-setting-handed");
    let off = release.settings(r#"{"sandbox": {"enabled": false}}"#);
    let off = Mode::under(&off, false, &|_| None).expect("the setting off needs no word");
    let handed = |mode: Mode| {
        let kept = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&kept);
        let sandbox = move |_: Handle| -> Arc<dyn SandboxService> {
            Arc::new(Recording(Arc::clone(&recording)))
        };
        let _ = probe(&release.root, &release.entry, mode, &sandbox, ANSWERING)
            .expect_err("the recording sandbox starts nothing");
        let kept = kept.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(kept.len(), 1, "one request reached the sandbox");
        kept.into_iter().next().expect("the request's policy")
    };

    let closed = handed(Mode::CONFINED);
    assert!(
        closed.enabled(),
        "the sandbox is enabled for a confined probe"
    );
    assert_eq!(closed.network(), &SandboxNetworkPolicy::Closed);

    let plain = handed(off);
    assert!(
        !plain.enabled(),
        "the sandbox is disabled with the setting off"
    );
    assert_eq!(fake_peer::played(&release.root), 0, "nothing was started");
}
