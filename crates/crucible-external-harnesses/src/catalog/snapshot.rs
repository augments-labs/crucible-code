//! The catalog built into the binary: the floor every fetched catalog is
//! measured against, and what is offered when no fetched one is believed.
//!
//! The published snapshot is checked by [`Catalog::compiled`] under
//! [`KEYS`](super::KEYS), and never expires, since it is the floor itself. The
//! catalog's owner has not published their keys yet, so there is no signed
//! catalog to compile in and nothing for that check to run over. Until there
//! is, the snapshot is built in from the one release 0.48 shipped with, the
//! reviewed Antigravity in its manifest, and is **not** signed or verified: it
//! is as trustworthy as this source file. It says only what the manifest
//! records, which is the release, the archive published for each platform and
//! the two files that archive holds, by name and size, so a build offers what
//! 0.48 offered and no more. It records no digest of those two files, because
//! the manifest pins the archive and an unpack checks sizes, and it leaves
//! empty the licence, the review, the hosts and the sign-in facts an entry
//! carries once the owner's review fills them in, because the manifest holds
//! none of them.

use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use crucible_types::AgentKey;

use super::trust::platform;
use super::{
    Admits, Artifact, Catalog, Entry, File, FileKind, Files, Format, Kind, Launch, Target,
};
use crate::antigravity::Manifest;
use crate::release::{Member, Pin};

/// The agent the manifest reviewed.
const AGENT: &str = "antigravity";

/// The serial of the snapshot built in: the lowest a fetched catalog may carry.
const SERIAL: u64 = 1;

/// The catalog built into the binary.
pub static SNAPSHOT: LazyLock<Catalog> = LazyLock::new(built_in);

/// The catalog the reviewed manifest describes. It was never signed, so it has
/// no dates, and a snapshot is never old.
fn built_in() -> Catalog {
    let mut entries = Vec::new();
    // A key this tree spelled wrongly would list nothing, which a test sees.
    if let Ok(key) = AgentKey::new(AGENT) {
        for version in Manifest::REVIEWED.versions {
            entries.push(entry(&key, version));
        }
    }
    Catalog {
        serial: SERIAL,
        signed_at: UNIX_EPOCH,
        expires: UNIX_EPOCH,
        entries,
        revoked: Vec::new(),
    }
}

/// `version` of the agent `key`, with the archive the manifest pins for each
/// platform it was published for.
fn entry(key: &AgentKey, version: &str) -> Entry {
    Entry {
        key: key.clone(),
        registry_id: Box::default(),
        version: version.into(),
        lock_revision: None,
        kind: Kind::Binary,
        license: Box::default(),
        terms: None,
        caution: None,
        review: Box::default(),
        content_use: None,
        admits: Admits {
            name: Manifest::REVIEWED.name.into(),
            version: version.into(),
        },
        reach: Vec::new(),
        sign_in_hosts: Vec::new(),
        updater_off: None,
        profile_env: None,
        state_paths: Vec::new(),
        auth_methods: Vec::new(),
        targets: Manifest::REVIEWED
            .platforms
            .iter()
            .filter(|(_, _, pin)| pin.version == version)
            .map(|(os, arch, pin)| target(os, arch, pin))
            .collect(),
    }
}

/// The agent as `pin` publishes it for the platform `os` and `arch`.
fn target(os: &str, arch: &str, pin: &Pin) -> Target {
    // The unpack writes both files so that they can be run.
    let file = |member: Member| File {
        path: member.name.into(),
        kind: FileKind::File {
            size: member.bytes,
            sha256: Box::default(),
            exec: true,
        },
    };
    let unpacked = pin.executable.bytes.saturating_add(pin.companion.bytes);
    Target {
        platform: platform(os, arch).into(),
        cmd: pin.executable.name.into(),
        args: Vec::new(),
        env: Vec::new(),
        hosts: Vec::new(),
        artifacts: vec![Artifact {
            url: pin.url.into(),
            mirror: None,
            sha256: pin.sha256.into(),
            size: pin.archive,
            format: Format::Zip,
            unpacked,
            decompressed: unpacked,
            files: Files::Listed(vec![file(pin.executable), file(pin.companion)]),
            kept: Vec::new(),
            replaced_by: None,
        }],
        launch: Launch::Binary,
    }
}
