use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest as _, Sha256};
use tar::{Builder, EntryType, Header};

use super::super::boundary::{CROSSED, KILL_AT, refusing_sync};
use super::super::layout::{BROKER, CRUCIBLE, PREFIX, RECEIPT};
use super::super::{Digest, Target};
use super::*;

/// The release each test install has active.
const ACTIVE: &str = "0.46.0";

/// The release each test stages and activates.
const NEXT: &str = "0.46.1";

/// The installation each test install's receipt names.
const INSTALLATION: &str = "5c0f9d2e8a4b47e1b3d6a09f7c21e845";

/// The test the kill-point tests start as the update they kill.
const UPDATE: &str = "install::activate::tests::the_update_the_kill_point_tests_stop";

/// The install the started update acts on.
const UPDATE_DIR: &str = "CRUCIBLE_TEST_UPDATE_DIR";

/// Set when the started update rolls back the release it activated.
const UPDATE_ROLLS_BACK: &str = "CRUCIBLE_TEST_UPDATE_ROLLS_BACK";

/// A managed install laid out as the installer lays one out, in a directory
/// of its own that is deleted with the value, with the next release's archive
/// and `SHA256SUMS` beside it.
struct Install {
    /// The directory the installer was given, canonical.
    dir: PathBuf,
}

impl Install {
    fn new(name: &str) -> Self {
        let at =
            std::env::temp_dir().join(format!("crucible-activate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&at);
        fs::create_dir_all(&at).expect("a temporary directory");
        let dir = at.canonicalize().expect("a canonical temporary directory");
        let install = Self { dir };
        directory(&install.prefix());
        directory(&install.releases());
        install.release(ACTIVE, &executable(ACTIVE));
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
        fs::create_dir(install.downloads()).expect("the downloads");
        install.publish(NEXT);
        install
    }

    fn prefix(&self) -> PathBuf {
        self.dir.join(PREFIX)
    }

    fn releases(&self) -> PathBuf {
        self.prefix().join(RELEASES)
    }

    fn unit(&self, version: &str) -> PathBuf {
        self.releases().join(version)
    }

    fn lock(&self) -> PathBuf {
        self.prefix().join(LOCK)
    }

    fn downloads(&self) -> PathBuf {
        self.dir.join("downloads")
    }

    /// Where `version`'s archive and `SHA256SUMS` are.
    fn published(&self, version: &str) -> (PathBuf, PathBuf) {
        let at = self.downloads().join(version);
        (at.join(archive_name(version)), at.join("SHA256SUMS"))
    }

    /// Writes a whole unit for `version` whose `crucible` holds `crucible`.
    fn release(&self, version: &str, crucible: &[u8]) {
        let unit = self.unit(version);
        directory(&unit);
        file(&unit.join(CRUCIBLE), crucible, 0o755);
        file(&unit.join(BROKER), &broker(version), 0o755);
        let receipt = receipt(&self.prefix(), version, crucible);
        file(&unit.join(RECEIPT), receipt.as_bytes(), 0o644);
    }

    /// Publishes `version`'s archive and `SHA256SUMS`.
    fn publish(&self, version: &str) {
        let (archive, checksums) = self.published(version);
        fs::create_dir(archive.parent().expect("its directory")).expect("a download directory");
        let bytes = release_archive(version);
        fs::write(&archive, &bytes).expect("the archive");
        fs::write(
            &checksums,
            format!("{}  {}\n", hex(&bytes), archive_name(version)),
        )
        .expect("the checksums");
    }

    /// Points `current` at `version`, as an install that finished would.
    fn activate(&self, version: &str) {
        let next = self.prefix().join(".current.test");
        symlink(format!("{RELEASES}/{version}"), &next).expect("an activation link");
        fs::rename(&next, self.prefix().join(CURRENT)).expect("the active release switched");
    }

    /// Takes the layout the way the running executable would.
    fn layout(&self) -> ReceiptLayout {
        ReceiptLayout::of_executable(&self.dir.join(CRUCIBLE)).expect("a managed install")
    }

    /// The release `current` names.
    fn active(&self) -> String {
        let target = fs::read_link(self.prefix().join(CURRENT)).expect("the active-release link");
        target
            .strip_prefix(RELEASES)
            .expect("a release")
            .to_str()
            .expect("a version")
            .to_owned()
    }

    /// Every name an interrupted install leaves, as the installer's
    /// `remove_leftovers` finds them, and the lock.
    fn leftovers(&self) -> Vec<String> {
        let mut left = Vec::new();
        for (directory, opening) in [
            (self.releases(), ".incoming."),
            (self.prefix(), ".current."),
        ] {
            for entry in fs::read_dir(&directory).expect("a directory of the layout") {
                let name = entry.expect("an entry").file_name();
                let name = name.to_string_lossy();
                if name.starts_with(opening) {
                    left.push(name.into_owned());
                }
            }
        }
        if fs::symlink_metadata(self.lock()).is_ok() {
            left.push(LOCK.to_owned());
        }
        left
    }

    /// Why the install is not one whole release active, the only state a
    /// person may be left with, or `None` when it is: the layout is taken as
    /// the running executable takes it, `current` names one of `allowed`,
    /// every release in place is whole, and the command runs as that release.
    fn inconsistent(&self, allowed: &[&str]) -> Option<String> {
        if let Err(error) = ReceiptLayout::of_executable(&self.dir.join(CRUCIBLE)) {
            return Some(format!("the layout is not taken: {error:?}"));
        }
        let active = self.active();
        if !allowed.contains(&active.as_str()) {
            return Some(format!("{active} is active, not one of {allowed:?}"));
        }
        for entry in fs::read_dir(self.releases()).expect("the releases") {
            let entry = entry.expect("an entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(INCOMING) {
                continue;
            }
            let version = Version::parse(name.as_bytes()).expect("a release's name");
            if let Err(error) = unit_receipt(&self.prefix(), &entry.path(), &version) {
                return Some(format!(
                    "the release {name} in place is not whole: {error:?}"
                ));
            }
        }
        let said = Command::new(self.dir.join(CRUCIBLE))
            .arg("--version")
            .output()
            .expect("the command run");
        let said = String::from_utf8_lossy(&said.stdout);
        if said.trim_end() != format!("crucible {active}") {
            return Some(format!(
                "the command says {said:?} while {active} is active"
            ));
        }
        None
    }

    /// Recovers the install as a person would after a kill: a lock the killed
    /// update left is refused by the next one, so it is removed, and the next
    /// update removes what the killed one left. `None` when that went as it
    /// should.
    fn recover(&self, locked: bool) -> Option<String> {
        if locked {
            match RecoverableActivation::begin(&self.layout()) {
                Err(ActivationError::Stale) => {}
                other => return Some(format!("the lock left behind was not refused: {other:?}")),
            }
            fs::remove_file(self.lock()).expect("the lock left behind removed");
        }
        match RecoverableActivation::begin(&self.layout()) {
            Ok(held) => drop(held),
            Err(error) => return Some(format!("the next update did not begin: {error:?}")),
        }
        let left = self.leftovers();
        if !left.is_empty() {
            return Some(format!("the next update left {left:?}"));
        }
        None
    }

    /// Starts the test binary as the update, which crosses its boundaries,
    /// writing each to `record`, and is killed at crossing `kill_at`.
    fn update(&self, rolls_back: bool, record: &Path, kill_at: Option<usize>) -> Output {
        let mut update = Command::new(std::env::current_exe().expect("the test binary"));
        update
            .args(["--exact", UPDATE, "--nocapture", "--test-threads=1"])
            .env(UPDATE_DIR, &self.dir)
            .env(CROSSED, record)
            .env_remove(KILL_AT)
            .env_remove(UPDATE_ROLLS_BACK)
            .stdin(Stdio::null());
        if rolls_back {
            update.env(UPDATE_ROLLS_BACK, "1");
        }
        if let Some(at) = kill_at {
            update.env(KILL_AT, at.to_string());
        }
        update.output().expect("the update run")
    }

    /// Runs the installer offline over this install, installing `version`
    /// from its published archive.
    fn installer(&self, version: &str) -> Output {
        let (archive, checksums) = self.published(version);
        let scratch = self.dir.join("installer");
        let _ = fs::create_dir(&scratch);
        Command::new("bash")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/sh/install.sh"))
            .args(["--version", version, "--dir"])
            .arg(&self.dir)
            .arg("--archive")
            .arg(&archive)
            .arg("--checksums")
            .arg(&checksums)
            .env("HOME", &scratch)
            .env("TMPDIR", &scratch)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .expect("the installer run")
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

fn target() -> &'static str {
    Target::running().expect("a supported target").as_str()
}

fn archive_name(version: &str) -> String {
    format!("crucible-{version}-{}.tar.gz", target())
}

/// A `crucible` that says it is `version`, as the installer asks it to.
fn executable(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'crucible {version}'\n").into_bytes()
}

/// A broker of `version`.
fn broker(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\n# crucible-sandbox-broker {version}\n").into_bytes()
}

/// The receipt of a unit of `version` holding `crucible` and its broker.
fn receipt(prefix: &Path, version: &str, crucible: &[u8]) -> String {
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

/// The archive `version` ships, as the release packs it.
fn release_archive(version: &str) -> Vec<u8> {
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
    entry(CRUCIBLE, EntryType::Regular, &executable(version));
    entry(BROKER, EntryType::Regular, &broker(version));
    entry("README.md", EntryType::Regular, b"# crucible\n");
    entry("LICENSE", EntryType::Regular, b"MIT\n");
    entry("install.sh", EntryType::Regular, b"#!/bin/sh\n");
    entry("uninstall.sh", EntryType::Regular, b"#!/bin/sh\n");
    let tar = builder.into_inner().expect("a finished archive");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&tar).expect("compressed");
    encoder.finish().expect("a finished gzip stream")
}

/// Stages and activates the next release, and rolls it back when it is set
/// to, crossing each boundary as the update does.
///
/// A test in its own right only so that the test binary can be started as
/// this process; run any other way, it finds nothing to do.
#[test]
fn the_update_the_kill_point_tests_stop() {
    let Some(dir) = std::env::var_os(UPDATE_DIR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let layout = ReceiptLayout::of_executable(&dir.join(CRUCIBLE)).expect("a managed install");
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let at = dir.join("downloads").join(NEXT);
    let activation = RecoverableActivation::begin(&layout).expect("the lock");
    let staged = activation
        .stage(&next, &at.join(archive_name(NEXT)), &at.join("SHA256SUMS"))
        .expect("the next release staged");
    let activated = activation
        .activate(staged)
        .expect("the next release active");
    if std::env::var_os(UPDATE_ROLLS_BACK).is_some() {
        activated.roll_back().expect("the release before restored");
    }
}

/// What an update finds and does, which decides the boundaries it crosses.
#[derive(Clone, Copy)]
struct Run {
    /// It finds what an interrupted install left.
    leftovers: bool,
    /// It finds the same build of the next release already in place.
    in_place: bool,
    /// It rolls back the release it activated.
    rolls_back: bool,
}

/// A plain update: nothing left behind, nothing in place, no rollback.
const UPDATE_RUN: Run = Run {
    leftovers: false,
    in_place: false,
    rolls_back: false,
};

impl Run {
    /// Makes `install` what this update finds.
    fn prepare(self, install: &Install) {
        if self.leftovers {
            leave_leftovers(install);
        }
        if self.in_place {
            install.release(NEXT, &executable(NEXT));
        }
    }
}

/// The boundaries an update crosses, in order: one for each of the two
/// leftovers it finds, the staged unit removed rather than moved when the
/// same build is in place, and those of a rollback.
fn boundaries(run: Run) -> Vec<&'static str> {
    let mut crossed = vec!["took the lock"];
    if run.leftovers {
        crossed.extend(["removed what an interrupted install left"; 2]);
    }
    crossed.extend([
        "made the staging directory",
        "copied the archive",
        "unpacked the executable",
        "unpacked the broker",
        "removed the archive's copy",
        "wrote the receipt",
        "sealed the staged release",
    ]);
    crossed.push(if run.in_place {
        "removed the staging directory"
    } else {
        "moved the release into place"
    });
    crossed.extend(["made the activation link", "switched the active release"]);
    if run.rolls_back {
        crossed.extend(["made the activation link", "switched the active release"]);
    }
    crossed.push("released the lock");
    crossed
}

/// The boundaries a run wrote to `record`.
fn recorded(record: &Path) -> Vec<String> {
    fs::read_to_string(record)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// What an interrupted install leaves: a staging directory of the
/// installer's, made by its own command, holding part of a release, and an
/// activation link of its.
fn leave_leftovers(install: &Install) {
    let made = Command::new("mktemp")
        .arg("-d")
        .arg(install.releases().join(".incoming.XXXXXX"))
        .output()
        .expect("mktemp run");
    assert!(made.status.success(), "mktemp made no staging directory");
    let staging = PathBuf::from(String::from_utf8(made.stdout).expect("a path").trim_end());
    fs::write(staging.join(CRUCIBLE), b"half a release").expect("part of a release");
    symlink(
        format!("{RELEASES}/{NEXT}"),
        install.prefix().join(".current.4242"),
    )
    .expect("an activation link");
}

/// Kills the update at each boundary it crosses, in a fresh install each
/// time, and holds what it leaves to the rule: one whole release active, the
/// one before or the one after, and an install the next update recovers.
/// Returns each kill point that broke it, the boundary named.
fn kill_matrix(name: &str, run: Run, finish: &[&str]) -> Vec<String> {
    let expected = boundaries(run);
    let mut broken = Vec::new();
    for at in 1..=expected.len() {
        let install = Install::new(&format!("{name}-{at}"));
        run.prepare(&install);
        let record = install.dir.join("crossed");
        let killed = install.update(run.rolls_back, &record, Some(at));
        let crossed = recorded(&record);
        let boundary = crossed.last().cloned().unwrap_or_default();
        let point = format!("killed at crossing {at}, after `{boundary}`");
        if killed.status.signal() != Some(rustix::process::Signal::KILL.as_raw()) {
            broken.push(format!(
                "{point}: the update was not killed there but ended with {:?}: {}",
                killed.status,
                String::from_utf8_lossy(&killed.stderr)
            ));
            continue;
        }
        if let Some(problem) = install.inconsistent(&[ACTIVE, NEXT]) {
            broken.push(format!("{point}: {problem}"));
            continue;
        }
        let locked = fs::symlink_metadata(install.lock()).is_ok();
        if let Some(problem) = install.recover(locked) {
            broken.push(format!("{point}: {problem}"));
            continue;
        }
        if let Some(problem) = install.inconsistent(&[ACTIVE, NEXT]) {
            broken.push(format!("{point}: once recovered, {problem}"));
            continue;
        }
        let again = install.update(run.rolls_back, &install.dir.join("again"), None);
        if !again.status.success() {
            broken.push(format!(
                "{point}: the next update failed: {}",
                String::from_utf8_lossy(&again.stdout)
            ));
            continue;
        }
        if let Some(problem) = install.inconsistent(finish) {
            broken.push(format!("{point}: once the next update ran, {problem}"));
        }
    }
    broken
}

/// What an update that is not killed crosses, and leaves active.
fn uninterrupted(name: &str, run: Run) -> (Vec<String>, String) {
    let install = Install::new(name);
    run.prepare(&install);
    let record = install.dir.join("crossed");
    let ran = install.update(run.rolls_back, &record, None);
    assert!(
        ran.status.success(),
        "the update failed: {}",
        String::from_utf8_lossy(&ran.stdout)
    );
    assert_eq!(
        install.leftovers(),
        Vec::<String>::new(),
        "the update left these"
    );
    (recorded(&record), install.active())
}

#[test]
fn a_kill_at_every_boundary_of_an_update_leaves_one_whole_release_it_recovers_from() {
    let run = Run {
        leftovers: true,
        ..UPDATE_RUN
    };
    let broken = kill_matrix("kill-update", run, &[NEXT]);
    assert!(broken.is_empty(), "{}", broken.join("\n"));
    let (crossed, active) = uninterrupted("kill-update-whole", run);
    assert_eq!(active, NEXT, "the update did not activate the next release");
    assert_eq!(
        crossed,
        boundaries(run),
        "the matrix does not kill at every boundary the update crosses"
    );
}

#[test]
fn a_kill_at_every_boundary_of_an_update_to_a_release_in_place_leaves_one_whole_release() {
    let run = Run {
        in_place: true,
        ..UPDATE_RUN
    };
    let broken = kill_matrix("kill-in-place", run, &[NEXT]);
    assert!(broken.is_empty(), "{}", broken.join("\n"));
    let (crossed, active) = uninterrupted("kill-in-place-whole", run);
    assert_eq!(
        active, NEXT,
        "the update did not activate the release in place"
    );
    assert_eq!(
        crossed,
        boundaries(run),
        "the matrix does not kill at every boundary an update to a release in place crosses"
    );
}

#[test]
fn a_kill_at_every_boundary_of_a_rollback_leaves_one_whole_release_it_recovers_from() {
    let run = Run {
        rolls_back: true,
        ..UPDATE_RUN
    };
    let broken = kill_matrix("kill-rollback", run, &[ACTIVE, NEXT]);
    assert!(broken.is_empty(), "{}", broken.join("\n"));
    let (crossed, active) = uninterrupted("kill-rollback-whole", run);
    assert_eq!(
        active, ACTIVE,
        "the rollback did not restore the release before"
    );
    assert_eq!(
        crossed,
        boundaries(run),
        "the matrix does not kill at every boundary the rollback crosses"
    );
}

/// Stages the next release into `install` under its lock.
fn staged(install: &Install) -> (RecoverableActivation, StagedUnit) {
    let activation = RecoverableActivation::begin(&install.layout()).expect("the lock");
    let (archive, checksums) = install.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let staged = activation
        .stage(&next, &archive, &checksums)
        .expect("staged");
    (activation, staged)
}

#[test]
fn a_switch_that_cannot_be_synced_says_the_next_release_is_active() {
    let install = Install::new("unsynced-switch");
    let (activation, staged) = staged(&install);

    refusing_sync(Some(install.prefix()));
    let refused = activation.activate(staged);
    refusing_sync(None);

    assert!(
        matches!(refused, Err(ActivationError::Unsynced(_))),
        "{refused:?}"
    );
    assert_eq!(install.active(), NEXT);
    assert_eq!(install.inconsistent(&[NEXT]), None);
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn a_rollback_whose_switch_cannot_be_synced_says_the_release_before_is_active() {
    let install = Install::new("unsynced-rollback");
    let (activation, staged) = staged(&install);
    let activated = activation.activate(staged).expect("activated");

    refusing_sync(Some(install.prefix()));
    let refused = activated.roll_back();
    refusing_sync(None);

    assert!(
        matches!(refused, Err(ActivationError::Unsynced(_))),
        "{refused:?}"
    );
    assert_eq!(install.active(), ACTIVE);
    assert_eq!(install.inconsistent(&[ACTIVE]), None);
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn a_release_moved_into_place_that_cannot_be_synced_is_not_activated() {
    let install = Install::new("unsynced-place");
    let (activation, staged) = staged(&install);

    refusing_sync(Some(install.releases()));
    let refused = activation.activate(staged);
    refusing_sync(None);

    assert!(
        matches!(
            refused,
            Err(ActivationError::Io {
                step: ActivationStep::Place,
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(install.active(), ACTIVE);
    assert_eq!(install.inconsistent(&[ACTIVE]), None);
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn rolling_back_restores_the_release_that_was_active_as_it_was() {
    let install = Install::new("roll-back");
    let before = fs::read(install.unit(ACTIVE).join(RECEIPT)).expect("the receipt before");
    let inode = fs::metadata(install.unit(ACTIVE).join(CRUCIBLE))
        .expect("the executable before")
        .ino();
    let activation = RecoverableActivation::begin(&install.layout()).expect("the lock");
    let (archive, checksums) = install.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let staged = activation
        .stage(&next, &archive, &checksums)
        .expect("staged");
    let activated = activation.activate(staged).expect("activated");
    assert_eq!(activated.previous().version().as_str(), ACTIVE);
    assert_eq!(activated.active().version().as_str(), NEXT);
    assert_eq!(install.active(), NEXT);

    activated.roll_back().expect("rolled back");

    assert_eq!(install.active(), ACTIVE, "the release before is not active");
    assert_eq!(install.inconsistent(&[ACTIVE]), None);
    assert_eq!(
        fs::read(install.unit(ACTIVE).join(RECEIPT)).expect("the receipt after"),
        before,
        "the release before was rewritten"
    );
    assert_eq!(
        fs::metadata(install.unit(ACTIVE).join(CRUCIBLE))
            .expect("the executable after")
            .ino(),
        inode,
        "the release before was replaced rather than restored"
    );
    assert!(
        unit_receipt(&install.prefix(), &install.unit(NEXT), &next).is_ok(),
        "the release rolled back from is not kept whole"
    );
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn a_previous_release_changed_since_activation_is_not_rolled_back_to() {
    let install = Install::new("roll-back-changed");
    let activation = RecoverableActivation::begin(&install.layout()).expect("the lock");
    let (archive, checksums) = install.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let staged = activation
        .stage(&next, &archive, &checksums)
        .expect("staged");
    let activated = activation.activate(staged).expect("activated");
    let changed = install.unit(ACTIVE).join(CRUCIBLE);
    fs::remove_file(&changed).expect("the executable removed");
    file(
        &changed,
        b"#!/bin/sh\necho 'crucible 0.46.0'\n# changed\n",
        0o755,
    );

    let refused = activated.roll_back();

    assert!(
        matches!(
            refused,
            Err(ActivationError::Unit {
                unit: UnitRole::Previous,
                source: LayoutError::Digest { .. },
            })
        ),
        "{refused:?}"
    );
    assert_eq!(
        install.active(),
        NEXT,
        "the active release was switched anyway"
    );
}

#[test]
fn a_staged_release_changed_before_activation_is_not_activated() {
    let install = Install::new("staged-changed");
    let activation = RecoverableActivation::begin(&install.layout()).expect("the lock");
    let (archive, checksums) = install.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let staged = activation
        .stage(&next, &archive, &checksums)
        .expect("staged");
    let changed = staged.path().join(CRUCIBLE);
    fs::remove_file(&changed).expect("the staged executable removed");
    file(
        &changed,
        b"#!/bin/sh\necho 'crucible 0.46.1'\n# changed\n",
        0o755,
    );

    let refused = activation.activate(staged);

    assert!(
        matches!(
            refused,
            Err(ActivationError::Unit {
                unit: UnitRole::Staged,
                source: LayoutError::Digest { .. },
            })
        ),
        "{refused:?}"
    );
    assert_eq!(
        install.active(),
        ACTIVE,
        "the changed release was activated"
    );
    assert!(
        !install.unit(NEXT).exists(),
        "the changed release was moved into place"
    );
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

/// What `error` says, with each error it was caused by.
fn said(error: &dyn std::error::Error) -> String {
    let mut said = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        said.push_str(": ");
        said.push_str(&cause.to_string());
        source = cause.source();
    }
    said
}

#[test]
fn a_staged_release_that_is_not_whole_is_not_called_the_active_one() {
    for name in ["writable", "contents"] {
        let install = Install::new(&format!("staged-said-{name}"));
        let (activation, staged) = staged(&install);
        if name == "writable" {
            fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o775))
                .expect("its mode");
        } else {
            file(&staged.path().join("extra"), b"not in the receipt", 0o644);
        }

        let refused = activation.activate(staged);

        assert!(
            matches!(
                refused,
                Err(ActivationError::Unit {
                    unit: UnitRole::Staged,
                    ..
                })
            ),
            "{name}: {refused:?}"
        );
        let said = refused.err().map(|error| said(&error)).unwrap_or_default();
        assert!(
            !said.contains("active"),
            "{name}: a staged release is called the active one: {said}"
        );
        assert_eq!(install.active(), ACTIVE);
    }
}

#[test]
fn a_staged_release_whose_receipt_was_rewritten_to_match_is_not_activated() {
    let install = Install::new("staged-rewritten");
    let activation = RecoverableActivation::begin(&install.layout()).expect("the lock");
    let (archive, checksums) = install.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let staged = activation
        .stage(&next, &archive, &checksums)
        .expect("staged");
    let other = b"#!/bin/sh\necho 'crucible 0.46.1'\n# another build\n";
    fs::remove_file(staged.path().join(CRUCIBLE)).expect("the staged executable removed");
    file(&staged.path().join(CRUCIBLE), other, 0o755);
    fs::remove_file(staged.path().join(RECEIPT)).expect("the staged receipt removed");
    let rewritten = receipt(&install.prefix(), NEXT, other);
    file(&staged.path().join(RECEIPT), rewritten.as_bytes(), 0o644);

    let refused = activation.activate(staged);

    assert!(
        matches!(
            refused,
            Err(ActivationError::Altered {
                unit: UnitRole::Staged
            })
        ),
        "{refused:?}"
    );
    assert_eq!(install.active(), ACTIVE);
}

#[test]
fn staging_an_interrupted_install_left_is_removed_under_the_lock() {
    let install = Install::new("leftovers");
    leave_leftovers(&install);
    let kept = install.unit("0.45.0");
    install.release("0.45.0", &executable("0.45.0"));
    let lookalike = install.releases().join(".incomingx");
    fs::create_dir(&lookalike).expect("a directory that is not staging");

    let held = RecoverableActivation::begin(&install.layout()).expect("the lock");
    assert_eq!(
        install.leftovers(),
        vec![LOCK.to_owned()],
        "what an interrupted install left is still there under the lock"
    );
    drop(held);

    assert_eq!(install.leftovers(), Vec::<String>::new());
    assert!(kept.is_dir(), "a release in place was removed");
    assert!(
        lookalike.is_dir(),
        "a directory that is not staging was removed"
    );
}

#[test]
fn a_lock_left_by_an_install_that_stopped_is_refused_and_kept() {
    let install = Install::new("stale-lock");
    let left = Command::new("sh")
        .arg("-c")
        .arg(r#"ln -sn "$$@$(uname -n)" "$1""#)
        .arg("sh")
        .arg(install.lock())
        .status()
        .expect("the installer's lock command run");
    assert!(left.success(), "the lock was not made");
    let text = fs::read_link(install.lock()).expect("the lock");

    let refused = RecoverableActivation::begin(&install.layout());

    assert!(
        matches!(refused, Err(ActivationError::Stale)),
        "{refused:?}"
    );
    assert_eq!(
        fs::read_link(install.lock()).expect("the lock after"),
        text,
        "the lock left behind was taken"
    );
}

#[test]
fn a_lock_a_running_process_or_another_host_holds_is_refused() {
    let install = Install::new("held-lock");
    for holder in [
        format!(
            "{}@{}",
            std::process::id(),
            String::from_utf8_lossy(&host())
        ),
        "1@another-host".to_owned(),
        "not-a-pid@another-host".to_owned(),
    ] {
        symlink(&holder, install.lock()).expect("a lock");

        let refused = RecoverableActivation::begin(&install.layout());

        assert!(
            matches!(refused, Err(ActivationError::Held)),
            "{holder}: {refused:?}"
        );
        assert_eq!(
            fs::read_link(install.lock()).expect("the lock after"),
            PathBuf::from(&holder),
            "{holder}: the lock was taken"
        );
        fs::remove_file(install.lock()).expect("the lock removed");
    }
}

#[test]
fn a_lock_that_is_not_a_link_is_refused() {
    let install = Install::new("file-lock");
    fs::write(install.lock(), b"held").expect("a file where the lock goes");

    let refused = RecoverableActivation::begin(&install.layout());

    assert!(
        matches!(refused, Err(ActivationError::NotLock)),
        "{refused:?}"
    );
    assert_eq!(fs::read(install.lock()).expect("the file"), b"held");
}

#[test]
fn the_lock_is_released_once_the_activation_ends() {
    let install = Install::new("released");
    let held = RecoverableActivation::begin(&install.layout()).expect("the lock");
    assert!(
        matches!(
            RecoverableActivation::begin(&install.layout()),
            Err(ActivationError::Held)
        ),
        "a second activation took the lock the first holds"
    );
    drop(held);
    assert_eq!(install.leftovers(), Vec::<String>::new());
    drop(RecoverableActivation::begin(&install.layout()).expect("the lock again"));
}

#[test]
fn an_install_activated_since_it_was_read_is_not_updated_over() {
    let install = Install::new("changed");
    let layout = install.layout();
    install.release("0.46.2", &executable("0.46.2"));
    install.activate("0.46.2");

    let refused = RecoverableActivation::begin(&layout);

    assert!(
        matches!(refused, Err(ActivationError::Changed)),
        "{refused:?}"
    );
    assert_eq!(install.active(), "0.46.2");
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn a_release_already_in_place_as_the_same_build_is_used_again() {
    let install = Install::new("reuse");
    install.release(NEXT, &executable(NEXT));
    let inode = fs::metadata(install.unit(NEXT)).expect("the release").ino();
    let activation = RecoverableActivation::begin(&install.layout()).expect("the lock");
    let (archive, checksums) = install.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let staged = activation
        .stage(&next, &archive, &checksums)
        .expect("staged");

    let activated = activation.activate(staged).expect("activated");

    assert_eq!(activated.active().version().as_str(), NEXT);
    assert_eq!(install.active(), NEXT);
    assert_eq!(
        fs::metadata(install.unit(NEXT)).expect("the release").ino(),
        inode,
        "the release in place was replaced"
    );
    drop(activated);
    assert_eq!(install.inconsistent(&[NEXT]), None);
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn another_build_already_in_place_under_the_version_is_refused_and_kept() {
    let install = Install::new("other-build");
    let other = b"#!/bin/sh\necho 'crucible 0.46.1'\n# another build\n";
    install.release(NEXT, other);
    let activation = RecoverableActivation::begin(&install.layout()).expect("the lock");
    let (archive, checksums) = install.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let staged = activation
        .stage(&next, &archive, &checksums)
        .expect("staged");

    let refused = activation.activate(staged);

    assert!(
        matches!(
            refused,
            Err(ActivationError::Altered {
                unit: UnitRole::Existing
            })
        ),
        "{refused:?}"
    );
    assert_eq!(install.active(), ACTIVE);
    assert_eq!(
        fs::read(install.unit(NEXT).join(CRUCIBLE)).expect("the build in place"),
        other,
        "the build in place was replaced"
    );
    assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn a_release_staged_for_another_install_is_not_activated_here() {
    let here = Install::new("foreign-here");
    let there = Install::new("foreign-there");
    let (archive, checksums) = there.published(NEXT);
    let next = Version::parse(NEXT.as_bytes()).expect("a version");
    let elsewhere = RecoverableActivation::begin(&there.layout()).expect("that lock");
    let staged = elsewhere
        .stage(&next, &archive, &checksums)
        .expect("staged there");
    let activation = RecoverableActivation::begin(&here.layout()).expect("this lock");

    let refused = activation.activate(staged);

    assert!(
        matches!(refused, Err(ActivationError::Foreign)),
        "{refused:?}"
    );
    assert_eq!(here.active(), ACTIVE);
    assert_eq!(there.active(), ACTIVE);
    assert!(!here.unit(NEXT).exists() && !there.unit(NEXT).exists());
}

#[test]
fn the_installer_refuses_the_lock_a_killed_update_left_and_finishes_once_it_is_removed() {
    let install = Install::new("installer");
    let record = install.dir.join("crossed");
    let at = boundaries(UPDATE_RUN)
        .iter()
        .position(|boundary| *boundary == "unpacked the executable")
        .expect("a boundary in staging")
        + 1;
    let killed = install.update(false, &record, Some(at));
    assert_eq!(
        killed.status.signal(),
        Some(rustix::process::Signal::KILL.as_raw())
    );
    assert_eq!(install.leftovers().len(), 2, "{:?}", install.leftovers());

    let refused = install.installer(NEXT);

    assert!(!refused.status.success(), "the installer took the lock");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("stopped before it finished"), "{said}");

    fs::remove_file(install.lock()).expect("the lock left behind removed");
    let finished = install.installer(NEXT);
    assert!(
        finished.status.success(),
        "{}",
        String::from_utf8_lossy(&finished.stderr)
    );
    assert_eq!(
        install.leftovers(),
        Vec::<String>::new(),
        "the installer left these"
    );
    assert_eq!(install.inconsistent(&[NEXT]), None);
}

#[test]
fn the_update_refuses_the_lock_the_installer_holds() {
    let install = Install::new("installer-holds");
    // The installer's own command, run by a process that is still running.
    let mut holder = Command::new("sh")
        .arg("-c")
        .arg(r#"ln -sn "$$@$(uname -n)" "$1" && exec sleep 30"#)
        .arg("sh")
        .arg(install.lock())
        .spawn()
        .expect("a process holding the lock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while fs::symlink_metadata(install.lock()).is_err() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let refused = RecoverableActivation::begin(&install.layout());

    holder.kill().expect("the holder stopped");
    holder.wait().expect("the holder reaped");
    assert!(matches!(refused, Err(ActivationError::Held)), "{refused:?}");
}
