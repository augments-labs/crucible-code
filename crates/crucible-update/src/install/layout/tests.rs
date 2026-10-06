use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::*;

/// What each test install's executable holds.
const EXECUTABLE: &[u8] = b"crucible, as a release would ship it";

/// What each test install's broker holds.
const HELPER: &[u8] = b"crucible-sandbox-broker, as a release would ship it";

/// The release each test install has active.
const ACTIVE: &str = "0.46.0";

/// A managed install made the way the installer lays one out, in a directory
/// of its own that is deleted with the value.
struct Install {
    /// The directory the installer was given, canonical.
    dir: PathBuf,
}

impl Install {
    fn new(name: &str) -> Self {
        let at =
            std::env::temp_dir().join(format!("crucible-layout-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&at);
        fs::create_dir_all(&at).expect("a temporary directory");
        let dir = at.canonicalize().expect("a canonical temporary directory");
        let install = Self { dir };
        directory(&install.prefix());
        directory(&install.prefix().join(RELEASES));
        install.release(ACTIVE, true);
        symlink(
            format!("{RELEASES}/{ACTIVE}"),
            install.prefix().join(CURRENT),
        )
        .expect("the active-release link");
        symlink(
            format!("{PREFIX}/{CURRENT}/{CRUCIBLE}"),
            install.dir.join(CRUCIBLE),
        )
        .expect("the command's link");
        symlink(CRUCIBLE, install.dir.join("cru")).expect("the alias");
        install
    }

    fn prefix(&self) -> PathBuf {
        self.dir.join(PREFIX)
    }

    fn unit(&self, version: &str) -> PathBuf {
        self.prefix().join(RELEASES).join(version)
    }

    /// Writes a complete unit for `version`, with a broker when `broker`.
    fn release(&self, version: &str, broker: bool) {
        let unit = self.unit(version);
        directory(&unit);
        file(&unit.join(CRUCIBLE), EXECUTABLE, 0o755);
        if broker {
            file(&unit.join(BROKER), HELPER, 0o755);
        }
        let receipt = receipt(&self.prefix(), version, broker.then_some(HELPER));
        file(&unit.join(RECEIPT), receipt.as_bytes(), 0o644);
    }

    /// Replaces the active unit's receipt.
    fn write_receipt(&self, text: &str) {
        let at = self.unit(ACTIVE).join(RECEIPT);
        fs::remove_file(&at).expect("the old receipt removed");
        file(&at, text.as_bytes(), 0o644);
    }

    /// Takes the layout the way the running executable would.
    fn take(&self) -> Result<ReceiptLayout, LayoutError> {
        ReceiptLayout::of_executable(&self.dir.join(CRUCIBLE))
    }

    /// Reads the layout for `owner`, as the executable's owner would be found.
    fn read_for(&self, owner: Owner) -> Result<ReceiptLayout, LayoutError> {
        ReceiptLayout::read(&self.prefix(), owner)
    }

    fn read(&self) -> Result<ReceiptLayout, LayoutError> {
        self.read_for(Owner(
            fs::symlink_metadata(&self.dir)
                .expect("the directory")
                .uid(),
        ))
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn directory(at: &Path) {
    fs::create_dir(at).expect("a directory");
    fs::set_permissions(at, fs::Permissions::from_mode(0o755)).expect("its mode");
}

fn file(at: &Path, bytes: &[u8], mode: u32) {
    fs::write(at, bytes).expect("a file");
    fs::set_permissions(at, fs::Permissions::from_mode(mode)).expect("its mode");
}

fn hex(bytes: &[u8]) -> String {
    Digest::new(Sha256::digest(bytes).into()).to_string()
}

fn receipt(prefix: &Path, version: &str, broker: Option<&[u8]>) -> String {
    receipt_for(
        prefix,
        version,
        Target::running().expect("a supported target").as_str(),
        broker,
    )
}

fn receipt_for(prefix: &Path, version: &str, target: &str, broker: Option<&[u8]>) -> String {
    let mut text = format!(
        "crucible-installer-receipt 1\n\
         manager=crucible-installer\n\
         installation=5c0f9d2e8a4b47e1b3d6a09f7c21e845\n\
         target={target}\n\
         layout=versioned\n\
         prefix={}\n\
         version={version}\n\
         sha256.crucible={}\n",
        prefix.display(),
        hex(EXECUTABLE),
    );
    if let Some(broker) = broker {
        text.push_str("sha256.crucible-sandbox-broker=");
        text.push_str(&hex(broker));
        text.push('\n');
    }
    text
}

/// Who runs the tests, as the owner of a file they make.
fn me() -> u32 {
    let probe = std::env::temp_dir().join(format!("crucible-layout-uid-{}", std::process::id()));
    fs::write(&probe, b"").expect("a probe file");
    let uid = fs::symlink_metadata(&probe).expect("the probe").uid();
    let _ = fs::remove_file(&probe);
    uid
}

/// An owner who is neither root nor whoever runs the tests.
fn somebody_else() -> Owner {
    Owner(if me() == 4242 { 4243 } else { 4242 })
}

#[test]
fn a_managed_install_is_taken_through_the_link_it_was_started_by() {
    let install = Install::new("taken");

    let layout = install.take().expect("the layout");

    assert_eq!(layout.prefix(), install.prefix());
    assert_eq!(layout.unit(), install.unit(ACTIVE));
    assert_eq!(layout.receipt().version().as_str(), ACTIVE);
    assert_eq!(layout.receipt().prefix(), install.prefix());
    assert_eq!(layout.receipt().crucible().to_string(), hex(EXECUTABLE));
    assert_eq!(
        layout.receipt().broker().map(ToString::to_string),
        Some(hex(HELPER))
    );
    assert!(install.dir.join("cru").exists());
}

#[test]
fn a_release_without_a_broker_is_taken_when_its_receipt_names_none() {
    let install = Install::new("brokerless");
    fs::remove_file(install.unit(ACTIVE).join(BROKER)).expect("the broker removed");
    install.write_receipt(&receipt(&install.prefix(), ACTIVE, None));

    let layout = install.take().expect("the layout");

    assert_eq!(layout.receipt().broker(), None);
}

#[test]
fn an_executable_outside_a_managed_layout_is_not_one() {
    let install = Install::new("flat");
    let flat = install.dir.join("flat");
    file(&flat, EXECUTABLE, 0o755);

    assert!(matches!(
        ReceiptLayout::of_executable(&flat),
        Err(LayoutError::Unmanaged)
    ));
}

#[test]
fn a_process_left_running_from_a_release_no_longer_active_is_refused() {
    let install = Install::new("inactive");
    install.release("0.45.3", true);

    let stale = install.unit("0.45.3").join(CRUCIBLE);

    assert!(matches!(
        ReceiptLayout::of_executable(&stale),
        Err(LayoutError::Inactive)
    ));
}

#[test]
fn a_receipt_that_does_not_follow_the_format_is_refused() {
    let install = Install::new("garbled");
    install.write_receipt("crucible-installer-receipt 1\nmanaged-by=somebody-else\n");

    assert!(matches!(
        install.take(),
        Err(LayoutError::Receipt(ReceiptError::Expected { .. }))
    ));
}

#[test]
fn a_receipt_naming_another_prefix_is_refused() {
    let install = Install::new("moved");
    install.write_receipt(&receipt(
        &install.dir.join("elsewhere"),
        ACTIVE,
        Some(HELPER),
    ));

    assert!(matches!(
        install.take(),
        Err(LayoutError::Mismatch { what: "prefix" })
    ));
}

#[test]
fn a_receipt_naming_another_release_than_its_unit_is_refused() {
    let install = Install::new("relabelled");
    install.write_receipt(&receipt(&install.prefix(), "0.47.0", Some(HELPER)));

    assert!(matches!(
        install.take(),
        Err(LayoutError::Mismatch { what: "release" })
    ));
}

#[test]
fn a_receipt_naming_another_platform_is_refused() {
    let install = Install::new("foreign-platform");
    let running = Target::running().expect("a supported target").as_str();
    let other = if running == "linux-x86_64" {
        "macos-aarch64"
    } else {
        "linux-x86_64"
    };
    install.write_receipt(&receipt_for(&install.prefix(), ACTIVE, other, Some(HELPER)));

    assert!(matches!(
        install.take(),
        Err(LayoutError::Mismatch { what: "platform" })
    ));
}

#[test]
fn an_executable_whose_hash_is_not_the_recorded_one_is_refused() {
    let install = Install::new("edited-executable");
    let at = install.unit(ACTIVE).join(CRUCIBLE);
    file(&at, b"crucible, edited after it was installed", 0o755);

    assert!(matches!(
        install.take(),
        Err(LayoutError::Digest { what: "executable" })
    ));
}

#[test]
fn a_broker_whose_hash_is_not_the_recorded_one_is_refused() {
    let install = Install::new("edited-broker");
    let at = install.unit(ACTIVE).join(BROKER);
    file(&at, b"another broker", 0o755);

    assert!(matches!(
        install.take(),
        Err(LayoutError::Digest { what: "broker" })
    ));
}

#[test]
fn an_install_owned_by_somebody_else_is_refused() {
    let install = Install::new("foreign");

    if me() == 0 {
        // Root's entries are trusted whoever runs, so the foreign entry has to
        // be made: one file handed to an ordinary user.
        let at = install.unit(ACTIVE).join(CRUCIBLE);
        std::os::unix::fs::chown(&at, Some(4242), None).expect("the executable handed over");
        assert!(matches!(
            install.read_for(Owner(0)),
            Err(LayoutError::Foreign { what: "executable" })
        ));
    } else {
        assert!(matches!(
            install.read_for(somebody_else()),
            Err(LayoutError::Foreign { what: "prefix" })
        ));
    }
}

#[test]
fn root_and_the_executable_owner_are_trusted_and_nobody_else_is() {
    let owner = Owner(1000);

    assert!(trusted(owner, 0, 0o755));
    assert!(trusted(owner, 1000, 0o755));
    assert!(trusted(owner, 1000, 0o700));
    assert!(!trusted(owner, 1001, 0o755));
    assert!(!trusted(owner, 1000, 0o775));
    assert!(!trusted(owner, 0, 0o757));
}

#[test]
fn a_release_directory_others_may_write_is_refused() {
    let install = Install::new("open-unit");
    fs::set_permissions(install.unit(ACTIVE), fs::Permissions::from_mode(0o775)).expect("mode");

    assert!(matches!(
        install.take(),
        Err(LayoutError::Writable {
            what: "active release"
        })
    ));
}

#[test]
fn a_receipt_others_may_write_is_refused() {
    let install = Install::new("open-receipt");
    let at = install.unit(ACTIVE).join(RECEIPT);
    fs::set_permissions(at, fs::Permissions::from_mode(0o666)).expect("mode");

    assert!(matches!(
        install.take(),
        Err(LayoutError::Writable { what: "receipt" })
    ));
}

#[test]
fn an_active_release_link_that_is_absolute_is_refused() {
    let install = Install::new("absolute-current");
    let current = install.prefix().join(CURRENT);
    fs::remove_file(&current).expect("the link removed");
    symlink(install.unit(ACTIVE), &current).expect("an absolute link");

    assert!(matches!(install.read(), Err(LayoutError::Current)));
}

#[test]
fn an_active_release_link_that_climbs_is_refused() {
    let install = Install::new("climbing-current");
    let current = install.prefix().join(CURRENT);
    fs::remove_file(&current).expect("the link removed");
    symlink(format!("{RELEASES}/../{RELEASES}/{ACTIVE}"), &current).expect("a climbing link");

    assert!(matches!(install.read(), Err(LayoutError::Current)));
}

#[test]
fn an_active_release_that_is_not_a_link_is_refused() {
    let install = Install::new("real-current");
    let current = install.prefix().join(CURRENT);
    fs::remove_file(&current).expect("the link removed");
    directory(&current);

    assert!(matches!(
        install.read(),
        Err(LayoutError::Kind {
            what: "active-release link",
            ..
        })
    ));
}

#[test]
fn a_release_directory_that_is_a_link_out_of_the_prefix_is_refused() {
    let install = Install::new("linked-unit");
    let outside = install.dir.join("outside");
    fs::rename(install.unit(ACTIVE), &outside).expect("the unit moved out");
    symlink(&outside, install.unit(ACTIVE)).expect("a link to it");

    assert!(matches!(
        install.read(),
        Err(LayoutError::Link {
            what: "active release"
        })
    ));
    assert!(install.take().is_err());
}

#[test]
fn a_releases_directory_that_is_a_link_is_refused() {
    let install = Install::new("linked-releases");
    let outside = install.dir.join("outside");
    fs::rename(install.prefix().join(RELEASES), &outside).expect("releases moved out");
    symlink(&outside, install.prefix().join(RELEASES)).expect("a link to it");

    assert!(matches!(
        install.read(),
        Err(LayoutError::Link {
            what: "releases directory"
        })
    ));
}

#[test]
fn an_executable_that_is_a_link_out_of_its_unit_is_refused() {
    let install = Install::new("linked-executable");
    let outside = install.dir.join("outside");
    file(&outside, EXECUTABLE, 0o755);
    let at = install.unit(ACTIVE).join(CRUCIBLE);
    fs::remove_file(&at).expect("the executable removed");
    symlink(&outside, &at).expect("a link out");

    assert!(matches!(
        install.read(),
        Err(LayoutError::Link { what: "executable" })
    ));
    assert!(install.take().is_err());
}

#[test]
fn a_receipt_that_is_a_link_is_refused() {
    let install = Install::new("linked-receipt");
    let outside = install.dir.join("outside");
    let at = install.unit(ACTIVE).join(RECEIPT);
    fs::rename(&at, &outside).expect("the receipt moved out");
    symlink(&outside, &at).expect("a link to it");

    assert!(matches!(
        install.take(),
        Err(LayoutError::Link { what: "receipt" })
    ));
}

#[test]
fn a_prefix_reached_through_a_link_is_refused() {
    let install = Install::new("linked-prefix");
    let real = install.dir.join("real-prefix");
    fs::rename(install.prefix(), &real).expect("the prefix moved");
    symlink(&real, install.prefix()).expect("a link to it");

    assert!(matches!(install.read(), Err(LayoutError::NotCanonical)));
}

#[test]
fn an_executable_with_a_second_name_is_refused() {
    let install = Install::new("hard-linked");
    fs::hard_link(
        install.unit(ACTIVE).join(CRUCIBLE),
        install.dir.join("second"),
    )
    .expect("a second name");

    assert!(matches!(
        install.take(),
        Err(LayoutError::HardLink { what: "executable" })
    ));
}

#[test]
fn a_unit_holding_a_file_its_receipt_does_not_name_is_refused() {
    let install = Install::new("extra-file");
    file(&install.unit(ACTIVE).join("payload"), b"", 0o644);

    assert!(matches!(install.take(), Err(LayoutError::Contents)));
}

#[test]
fn a_broker_the_receipt_does_not_name_is_refused() {
    let install = Install::new("unnamed-broker");
    install.write_receipt(&receipt(&install.prefix(), ACTIVE, None));

    assert!(matches!(install.take(), Err(LayoutError::Contents)));
}

#[test]
fn a_broker_the_receipt_names_but_the_unit_lacks_is_refused() {
    let install = Install::new("missing-broker");
    fs::remove_file(install.unit(ACTIVE).join(BROKER)).expect("the broker removed");

    assert!(matches!(install.take(), Err(LayoutError::Contents)));
}
