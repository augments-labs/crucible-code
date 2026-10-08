//! What the tests of an install build one from: its files, its receipt and
//! the archive a release ships, as the installer and the release pack them.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest as _, Sha256};
use tar::{Builder, EntryType, Header};

use super::layout::{BROKER, CRUCIBLE, CURRENT, PREFIX, RECEIPT, RELEASES};
use super::{Digest, Target};

/// The installation each test install's receipt names.
pub(crate) const INSTALLATION: &str = "5c0f9d2e8a4b47e1b3d6a09f7c21e845";

pub(crate) fn directory(at: &Path) {
    fs::create_dir(at).expect("a directory");
    fs::set_permissions(at, fs::Permissions::from_mode(0o755)).expect("its mode");
}

pub(crate) fn file(at: &Path, bytes: &[u8], mode: u32) {
    fs::write(at, bytes).expect("a file");
    fs::set_permissions(at, fs::Permissions::from_mode(mode)).expect("its mode");
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    Digest::new(Sha256::digest(bytes).into()).to_string()
}

pub(crate) fn target() -> &'static str {
    Target::running().expect("a supported target").as_str()
}

pub(crate) fn archive_name(version: &str) -> String {
    format!("crucible-{version}-{}.tar.gz", target())
}

/// A `crucible` that says it is `version`, as the installer asks it to.
pub(crate) fn executable(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'crucible {version}'\n").into_bytes()
}

/// A broker of `version`.
pub(crate) fn broker(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\n# crucible-sandbox-broker {version}\n").into_bytes()
}

/// The receipt of a unit of `version` holding `crucible` and its broker.
pub(crate) fn receipt(prefix: &Path, version: &str, crucible: &[u8]) -> String {
    format!(
        "crucible-installer-receipt 1\n\
         manager=crucible-installer\n\
         installation={INSTALLATION}\n\
         target={}\n\
         layout=versioned\n\
         prefix={}\n\
         version={version}\n\
         sha256.crucible={}\n\
         sha256.crucible-sandbox-broker={}\n",
        target(),
        prefix.display(),
        hex(crucible),
        hex(&broker(version)),
    )
}

/// Writes a whole unit of `version` under `prefix` whose `crucible` holds
/// `crucible`.
pub(crate) fn unit(prefix: &Path, version: &str, crucible: &[u8]) {
    let unit = prefix.join(RELEASES).join(version);
    directory(&unit);
    file(&unit.join(CRUCIBLE), crucible, 0o755);
    file(&unit.join(BROKER), &broker(version), 0o755);
    let receipt = receipt(prefix, version, crucible);
    file(&unit.join(RECEIPT), receipt.as_bytes(), 0o644);
}

/// Lays an install of `version` out in `dir`, a canonical directory, as the
/// installer lays one out.
pub(crate) fn installed(dir: &Path, version: &str) {
    let prefix = dir.join(PREFIX);
    directory(&prefix);
    directory(&prefix.join(RELEASES));
    unit(&prefix, version, &executable(version));
    symlink(format!("{RELEASES}/{version}"), prefix.join(CURRENT))
        .expect("the active-release link");
    symlink(format!("{PREFIX}/{CURRENT}/{CRUCIBLE}"), dir.join(CRUCIBLE))
        .expect("the command's link");
    symlink(CRUCIBLE, dir.join("cru")).expect("the alias");
}

/// The archive `version` ships, as the release packs it.
pub(crate) fn release_archive(version: &str) -> Vec<u8> {
    packed(version, &executable(version), Some(&broker(version)))
}

/// An archive of `version` whose `crucible` holds `crucible`, with `broker`
/// beside it where there is one, and the documents a release ships.
pub(crate) fn packed(version: &str, crucible: &[u8], broker: Option<&[u8]>) -> Vec<u8> {
    let stem = format!("crucible-{version}-{}", target());
    let mut builder = Builder::new(Vec::new());
    let mut entry = |name: &str, kind: EntryType, data: &[u8]| {
        let mut header = Header::new_gnu();
        header
            .set_path(format!("{stem}/{name}"))
            .expect("a member's name");
        header.set_entry_type(kind);
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, data).expect("a member");
    };
    entry("", EntryType::Directory, b"");
    entry(CRUCIBLE, EntryType::Regular, crucible);
    if let Some(broker) = broker {
        entry(BROKER, EntryType::Regular, broker);
    }
    entry("README.md", EntryType::Regular, b"# crucible\n");
    entry("LICENSE", EntryType::Regular, b"MIT\n");
    entry("install.sh", EntryType::Regular, b"#!/bin/sh\n");
    entry("uninstall.sh", EntryType::Regular, b"#!/bin/sh\n");
    let tar = builder.into_inner().expect("a finished archive");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&tar).expect("compressed");
    encoder.finish().expect("a finished gzip stream")
}
