//! The catalog built into the binary: the floor every fetched catalog is
//! measured against, and what is offered when no fetched one is believed.
//!
//! The published snapshot is checked by [`Catalog::compiled`] under
//! [`KEYS`](super::KEYS), and never expires, since it is the floor itself. The
//! catalog's owner has not published their keys yet, so there is no signed
//! catalog to compile in and nothing for that check to run over. Until there
//! is, the snapshot is built in from data written out in this file, and is
//! **not** signed or verified: it is as trustworthy as this source file. The
//! data is what 0.48 shipped for the reviewed Antigravity: the release, the
//! archive published for each platform and the two files that archive holds, by
//! name and size, so a build offers what 0.48 offered and no more, and the
//! argument 0.48 starts it with on Linux. It is the one copy of that data: the
//! Antigravity module reads its releases, what each must call itself and what
//! it is started with from the catalog, this snapshot included, and a test
//! holds this one to what 0.48 shipped. It records no digest of the two files,
//! because 0.48 pinned the archive and an unpack checks sizes, and it leaves
//! empty the licence, the review, the hosts and the sign-in facts an entry
//! carries once the owner's review fills them in, because 0.48 held none of
//! them.

use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use crucible_types::AgentKey;

use super::{
    Admits, Artifact, Catalog, Entry, File, FileKind, Files, Format, Kind, Launch, Target,
};
use crate::antigravity::AGENT;

/// The release reviewed.
const VERSION: &str = "1.3.0";

/// The name the agent gives itself when it is initialized.
const NAME: &str = "antigravity-acp";

/// The serial of the snapshot built in: the lowest a fetched catalog may carry.
const SERIAL: u64 = 1;

/// The archive published for one platform, with its two files, each as a name
/// and a size in bytes.
struct Archive {
    platform: &'static str,
    url: &'static str,
    sha256: &'static str,
    size: u64,
    executable: (&'static str, u64),
    companion: (&'static str, u64),
}

/// Where the release was published, platform by platform.
const ARCHIVES: [Archive; 6] = [
    Archive {
        platform: "darwin-aarch64",
        url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-darwin-arm64.zip",
        sha256: "7cd97045f7b4fe81175a107cdf16f9c51484e3c78a5162cae415338bb6aa5b88",
        size: 111_456_962,
        executable: ("agy_acp_server.par", 278_535_456),
        companion: ("localharness_external", 118_611_392),
    },
    Archive {
        platform: "darwin-x86_64",
        url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-darwin-x86_64.zip",
        sha256: "bb23956b89984bf5d354af2c3725e6c57f0cc1b7228e77a0e91c9c2bc1d47646",
        size: 117_245_544,
        executable: ("agy_acp_server.par", 282_840_688),
        companion: ("localharness_external", 124_175_392),
    },
    Archive {
        platform: "linux-x86_64",
        url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-x86_64.zip",
        sha256: "9fb60956af0a9d76220a4db91ca9ac88e2a2372ad68f985ab5fceace6b825b96",
        size: 333_727_150,
        executable: ("agy_acp_server.par", 926_533_965),
        companion: ("localharness_external", 130_388_040),
    },
    Archive {
        platform: "linux-aarch64",
        url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-arm64.zip",
        sha256: "500b0bc0fb858e88f4df404d4cedf80bf9298c178291e39e383d6c50b111cbdf",
        size: 321_690_363,
        executable: ("agy_acp_server.par", 930_848_992),
        companion: ("localharness_external", 123_224_968),
    },
    Archive {
        platform: "windows-x86_64",
        url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.3.0-windows-x86_64.zip",
        sha256: "65215e0688681fa3116e048a9eab27ef53af1bbd6f3da3f1c52bd4911d8b17f9",
        size: 124_509_787,
        executable: ("agy_acp_server.exe", 81_437_336),
        companion: ("localharness_external.exe", 145_548_952),
    },
    Archive {
        platform: "windows-aarch64",
        url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.3.0-windows-arm64.zip",
        sha256: "4a0f469720e9beb9438a979f543fdbfad5022ebe0992c052c590bd78b3144ca3",
        size: 124_654_803,
        executable: ("agy_acp_server.exe", 85_893_472),
        companion: ("localharness_external.exe", 135_640_216),
    },
];

/// The catalog built into the binary.
pub static SNAPSHOT: LazyLock<Catalog> = LazyLock::new(built_in);

/// The catalog the data above describes. It was never signed, so it has no
/// dates, and a snapshot is never old.
fn built_in() -> Catalog {
    // A key this tree spelled wrongly would list nothing, which a test sees.
    let entries = AgentKey::new(AGENT)
        .map(|key| entry(&key))
        .into_iter()
        .collect();
    Catalog {
        serial: SERIAL,
        signed_at: UNIX_EPOCH,
        expires: UNIX_EPOCH,
        entries,
        revoked: Vec::new(),
    }
}

/// The reviewed release of the agent `key`, with the archive published for
/// each platform.
fn entry(key: &AgentKey) -> Entry {
    Entry {
        key: key.clone(),
        registry_id: Box::default(),
        version: VERSION.into(),
        lock_revision: None,
        kind: Kind::Binary,
        license: Box::default(),
        terms: None,
        caution: None,
        review: Box::default(),
        content_use: None,
        admits: Admits {
            name: NAME.into(),
            version: VERSION.into(),
        },
        reach: Vec::new(),
        sign_in_hosts: Vec::new(),
        updater_off: None,
        profile_env: None,
        state_paths: Vec::new(),
        auth_methods: Vec::new(),
        targets: ARCHIVES.iter().map(target).collect(),
    }
}

/// The agent as `archive` publishes it.
fn target(archive: &Archive) -> Target {
    // The unpack writes both files so that they can be run.
    let file = |(name, size): (&str, u64)| File {
        path: name.into(),
        kind: FileKind::File {
            size,
            sha256: Box::default(),
            exec: true,
        },
    };
    let unpacked = archive.executable.1.saturating_add(archive.companion.1);
    Target {
        platform: archive.platform.into(),
        cmd: archive.executable.0.into(),
        // 0.48 starts it on Linux with an empty user id, as its published
        // integration does, and with no arguments elsewhere.
        args: if archive.platform.starts_with("linux-") {
            vec!["--uid=".into()]
        } else {
            Vec::new()
        },
        env: Vec::new(),
        hosts: Vec::new(),
        artifacts: vec![Artifact {
            url: archive.url.into(),
            mirror: None,
            sha256: archive.sha256.into(),
            size: archive.size,
            format: Format::Zip,
            unpacked,
            decompressed: unpacked,
            files: Files::Listed(vec![file(archive.executable), file(archive.companion)]),
            kept: Vec::new(),
            replaced_by: None,
        }],
        launch: Launch::Binary,
    }
}
