use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// Reads the layout for `owner`, as the running user would be found.
    fn read_for(&self, owner: Owner) -> Result<ReceiptLayout, LayoutError> {
        ReceiptLayout::read(&self.prefix(), owner)
    }

    fn read(&self) -> Result<ReceiptLayout, LayoutError> {
        self.read_for(Owner::running())
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
    static PROBES: AtomicUsize = AtomicUsize::new(0);
    let probe = std::env::temp_dir().join(format!(
        "crucible-layout-uid-{}-{}",
        std::process::id(),
        PROBES.fetch_add(1, Ordering::Relaxed)
    ));
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
        Err(LayoutError::Mismatch {
            claim: ReceiptClaim::Prefix
        })
    ));
}

#[test]
fn a_receipt_naming_another_release_than_its_unit_is_refused() {
    let install = Install::new("relabelled");
    install.write_receipt(&receipt(&install.prefix(), "0.47.0", Some(HELPER)));

    assert!(matches!(
        install.take(),
        Err(LayoutError::Mismatch {
            claim: ReceiptClaim::Release
        })
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
        Err(LayoutError::Mismatch {
            claim: ReceiptClaim::Platform
        })
    ));
}

#[test]
fn an_executable_whose_hash_is_not_the_recorded_one_is_refused() {
    let install = Install::new("edited-executable");
    let at = install.unit(ACTIVE).join(CRUCIBLE);
    file(&at, b"crucible, edited after it was installed", 0o755);

    assert!(matches!(
        install.take(),
        Err(LayoutError::Digest {
            entry: LayoutEntry::Executable
        })
    ));
}

#[test]
fn a_broker_whose_hash_is_not_the_recorded_one_is_refused() {
    let install = Install::new("edited-broker");
    let at = install.unit(ACTIVE).join(BROKER);
    file(&at, b"another broker", 0o755);

    assert!(matches!(
        install.take(),
        Err(LayoutError::Digest {
            entry: LayoutEntry::Broker
        })
    ));
}

#[test]
fn the_owner_an_install_is_held_to_is_the_user_running_crucible() {
    assert_eq!(Owner::running(), Owner(me()));
}

#[test]
fn an_install_somebody_else_made_is_refused_on_its_first_entry() {
    let install = Install::new("foreign");

    // The install is this user's, so to a process another user runs it is
    // somebody else's tree.
    assert!(matches!(
        ReceiptLayout::taken(&install.dir.join(CRUCIBLE), somebody_else()),
        Err(LayoutError::Foreign {
            entry: LayoutEntry::Prefix
        })
    ));
}

#[test]
fn an_install_reached_through_a_link_to_another_tree_is_held_to_that_tree() {
    let install = Install::new("redirected");
    let elsewhere = install.dir.join("elsewhere");
    directory(&elsewhere);
    let other = elsewhere.join(PREFIX);
    fs::rename(install.prefix(), &other).expect("the prefix moved");
    symlink(&other, install.prefix()).expect("the prefix linked to it");

    assert!(matches!(
        ReceiptLayout::taken(&install.dir.join(CRUCIBLE), somebody_else()),
        Err(LayoutError::Foreign {
            entry: LayoutEntry::Prefix
        })
    ));
}

#[test]
fn each_entry_handed_to_somebody_else_is_refused_when_root_runs() {
    if me() != 0 {
        // Only root can hand an entry to another user; an ordinary run holds
        // the whole install to an owner it is not, above.
        return;
    }
    for (entry, place) in places() {
        let install = Install::new("handed-over");
        std::os::unix::fs::lchown(place(&install), Some(4242), None).expect("handed over");

        let refused = install.read_for(Owner(0));

        assert!(
            matches!(refused, Err(LayoutError::Foreign { entry: found }) if found == entry),
            "{entry}: {refused:?}"
        );
    }
}

/// Where an entry is in an install.
type Place = fn(&Install) -> PathBuf;

/// Each directory and file the trust rule covers, with where it is in an
/// install.
fn places() -> [(LayoutEntry, Place); 6] {
    [
        (LayoutEntry::Prefix, |install| install.prefix()),
        (LayoutEntry::Releases, |install| {
            install.prefix().join(RELEASES)
        }),
        (LayoutEntry::Unit, |install| install.unit(ACTIVE)),
        (LayoutEntry::Receipt, |install| {
            install.unit(ACTIVE).join(RECEIPT)
        }),
        (LayoutEntry::Executable, |install| {
            install.unit(ACTIVE).join(CRUCIBLE)
        }),
        (LayoutEntry::Broker, |install| {
            install.unit(ACTIVE).join(BROKER)
        }),
    ]
}

#[test]
fn root_and_the_running_user_are_trusted_and_nobody_else_is() {
    let owner = Owner(1000);

    assert!(trusted(owner, 0, 0o755));
    assert!(trusted(owner, 1000, 0o755));
    assert!(trusted(owner, 1000, 0o700));
    assert!(!trusted(owner, 1001, 0o755));
    assert!(!trusted(owner, 1000, 0o775));
    assert!(!trusted(owner, 0, 0o757));
}

#[test]
fn each_entry_its_group_or_anybody_may_write_is_refused() {
    for (entry, place) in places() {
        let modes = if entry == LayoutEntry::Receipt {
            [0o664, 0o646]
        } else {
            [0o775, 0o757]
        };
        for mode in modes {
            let install = Install::new("open");
            fs::set_permissions(place(&install), fs::Permissions::from_mode(mode)).expect("mode");

            let refused = install.take();

            assert!(
                matches!(refused, Err(LayoutError::Writable { entry: found }) if found == entry),
                "{entry} at {mode:o}: {refused:?}"
            );
        }
    }
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
            entry: LayoutEntry::Current,
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
            entry: LayoutEntry::Unit
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
            entry: LayoutEntry::Releases
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
        Err(LayoutError::Link {
            entry: LayoutEntry::Executable
        })
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
        Err(LayoutError::Link {
            entry: LayoutEntry::Receipt
        })
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
        Err(LayoutError::HardLink {
            entry: LayoutEntry::Executable
        })
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

#[test]
fn a_unit_holding_several_files_its_receipt_does_not_name_is_refused() {
    let install = Install::new("extra-files");
    for name in ["one", "two", "three"] {
        file(&install.unit(ACTIVE).join(name), b"", 0o644);
    }

    assert!(matches!(install.take(), Err(LayoutError::Contents)));
}

#[test]
fn a_receipt_that_is_a_directory_is_refused() {
    let install = Install::new("receipt-directory");
    let at = install.unit(ACTIVE).join(RECEIPT);
    fs::remove_file(&at).expect("the receipt removed");
    directory(&at);

    assert!(matches!(
        install.read(),
        Err(LayoutError::Kind {
            entry: LayoutEntry::Receipt,
            kind: EntryKind::File
        })
    ));
}

#[test]
fn an_executable_that_is_a_directory_is_refused() {
    let install = Install::new("executable-directory");
    let at = install.unit(ACTIVE).join(CRUCIBLE);
    fs::remove_file(&at).expect("the executable removed");
    directory(&at);

    assert!(matches!(
        install.read(),
        Err(LayoutError::Kind {
            entry: LayoutEntry::Executable,
            kind: EntryKind::File
        })
    ));
}

#[test]
fn a_releases_directory_that_is_a_file_is_refused() {
    let install = Install::new("releases-file");
    let at = install.prefix().join(RELEASES);
    fs::remove_dir_all(&at).expect("the releases removed");
    file(&at, b"", 0o644);

    assert!(matches!(
        install.read(),
        Err(LayoutError::Kind {
            entry: LayoutEntry::Releases,
            kind: EntryKind::Directory
        })
    ));
}

#[test]
fn a_broker_that_is_a_fifo_is_refused_without_being_opened() {
    let install = Install::new("broker-fifo");
    let at = install.unit(ACTIVE).join(BROKER);
    fs::remove_file(&at).expect("the broker removed");
    let made = std::process::Command::new("mkfifo")
        .arg(&at)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());

    // Opening a FIFO to read waits for a writer, so a refusal that came only
    // after the open would never come.
    assert!(matches!(
        install.read(),
        Err(LayoutError::Kind {
            entry: LayoutEntry::Broker,
            kind: EntryKind::File
        })
    ));
}

#[test]
fn an_executable_that_is_not_there_is_refused() {
    let install = Install::new("missing");

    assert!(matches!(
        ReceiptLayout::of_executable(&install.dir.join("nothing")),
        Err(LayoutError::Io {
            entry: LayoutEntry::Executable,
            ..
        })
    ));
}

#[test]
fn a_receipt_larger_than_the_ceiling_is_refused() {
    let install = Install::new("large-receipt");
    let mut text = receipt(&install.prefix(), ACTIVE, Some(HELPER));
    text.insert_str(0, &"x".repeat(receipt::MAX_BYTES + 1 - text.len()));

    install.write_receipt(&text);

    assert!(matches!(
        install.take(),
        Err(LayoutError::Receipt(ReceiptError::TooLarge))
    ));
}

#[test]
fn a_file_is_hashed_up_to_its_ceiling_and_refused_past_it() {
    let install = Install::new("ceiling");
    let at = install.dir.join("sized");
    file(&at, &[7; 16], 0o644);

    let within = digest(File::open(&at).expect("the file"), LayoutEntry::Broker, 16);

    assert_eq!(within.expect("the digest").to_string(), hex(&[7; 16]));
    file(&at, &[7; 17], 0o644);
    assert!(matches!(
        digest(File::open(&at).expect("the file"), LayoutEntry::Broker, 16),
        Err(LayoutError::TooLarge {
            entry: LayoutEntry::Broker
        })
    ));
}
