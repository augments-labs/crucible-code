//! The probe that takes in a release follows the sandbox setting: with it off
//! the release is started as a plain local process and admitted, with it on a
//! sandbox that cannot close the network starts nothing.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crucible_config::Settings;
use crucible_external_harnesses::fake_peer::scripts::acp;
use crucible_external_harnesses::fake_peer::{self, Reply, Script};
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxError, SandboxRequest, SandboxService,
    SandboxSession,
};
use tokio::runtime::Handle;

use super::super::{Mode, Unprobed, local, probe};
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
