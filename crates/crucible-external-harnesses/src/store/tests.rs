//! What the install store keeps: a version committed whole with its receipt,
//! kept while an instance names it or a process runs it, adopted from an
//! install made before receipts were kept, and verified against what its
//! receipt recorded.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_types::AgentKey;
use sha2::{Digest as _, Sha256};

use super::{
    Adoption, Differs, Fetched, Listing, Mismatch, Origin, PROGRAMS_BYTES, Pause, Provenance,
    RECEIPT_BYTES, Receipt, Store, StoreError, Version,
};
use crate::catalog::{Admits, Artifact, AssetRef, File, FileKind, Files, Format, Launch, Target};
use crate::release::{OwnedPin, Unpacker, proof};
use crate::tests::Owned;

/// Where a process a test starts finds the store it works in.
const CHILD_ROOT: &str = "CRUCIBLE_STORE_TEST_ROOT";

/// Where that process says it has got to the point it is to be killed at.
const CHILD_READY: &str = "CRUCIBLE_STORE_TEST_READY";

/// The two files 0.48 unpacked Antigravity's release to.
const SERVER: &[u8] = b"agy acp server";
const HARNESS: &[u8] = b"local harness";

// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn key(name: &str) -> AgentKey {
    AgentKey::new(name).expect("a key")
}

// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn version(text: &str, lock_revision: Option<u64>) -> Version {
    Version::new(text, lock_revision).expect("a version")
}

/// `bytes`' SHA-256, in lowercase hexadecimal.
fn digest(bytes: &[u8]) -> Box<str> {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
        .into()
}

// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn listed_file(path: &str, bytes: &[u8], exec: bool) -> File {
    File {
        path: path.into(),
        kind: FileKind::File {
            size: u64::try_from(bytes.len()).expect("a size"),
            sha256: digest(bytes),
            exec,
        },
    }
}

fn listed_dir(path: &str) -> File {
    File {
        path: path.into(),
        kind: FileKind::Dir,
    }
}

fn listed_link(path: &str, target: &str) -> File {
    File {
        path: path.into(),
        kind: FileKind::Link {
            target: target.into(),
        },
    }
}

/// What the release `proof::catalog_release` publishes unpacks to.
fn listed() -> Vec<File> {
    vec![
        listed_file("agent", proof::EXECUTABLE, true),
        listed_file("LICENSE", proof::LICENSE, false),
        listed_dir("lib"),
        listed_file("lib/harness", proof::COMPANION, true),
        listed_link("lib/current", "harness"),
    ]
}

/// The release `proof::catalog_release` publishes, and its artifact with the
/// files it unpacks to listed.
// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn release() -> (Vec<u8>, Artifact) {
    let (archive, described) = proof::catalog_release();
    let mut artifact = Artifact::unsigned(&described).expect("an artifact");
    artifact.files = Files::Listed(listed());
    (archive, artifact)
}

/// What the catalog said of the release.
fn listing() -> Listing {
    Listing {
        serial: 7,
        review: "https://example.invalid/review/12".into(),
        terms: Some("https://example.invalid/terms".into()),
    }
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Where `artifact` came from, fetched `seconds` after the epoch.
fn provenance(artifact: &Artifact, seconds: u64) -> Provenance {
    Provenance {
        listing: listing(),
        fetched: at(seconds),
        artifacts: vec![Fetched {
            url: artifact.url.clone(),
            hosts: vec!["example.invalid".into(), "objects.example.invalid".into()],
            size: artifact.size,
            sha256: artifact.sha256.clone(),
        }],
    }
}

fn programs(owned: &Owned) -> PathBuf {
    owned.0.join("programs")
}

/// `version` of `key` installed from the test release, fetched `seconds`
/// after the epoch.
// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn install(store: &Store, key: &AgentKey, version: &Version, seconds: u64) -> Receipt {
    let (archive, artifact) = release();
    let staging = store.stage(key).expect("a staging directory");
    let staged = Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
    store
        .commit(
            staging,
            version,
            staged.files(),
            provenance(&artifact, seconds),
        )
        .expect("committed")
}

/// The names in `directory`, sorted; none where it is not there.
fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// This test binary started again as the one test `name`, working in the
/// store at `root` and saying at `ready` when it has got where it is to be
/// killed.
// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn started(name: &str, root: &Path, ready: &Path) -> Child {
    Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD_ROOT, root)
        .env(CHILD_READY, ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the second process")
}

/// Waits, for at most 30 s, for `child` to say at `ready` that it has got
/// where it is to be killed.
// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used, clippy::panic)]
fn reached(child: &mut Child, ready: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("its status") {
            panic!("the second process ended first, {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "the second process never got there");
}

// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn killed(mut child: Child) {
    child.kill().expect("the kill");
    child.wait().expect("the killed process reaped");
}

/// Commits release 2.0.0 of `goose` into the store it is given, and is killed
/// once the staging directory, its receipt in it, is renamed into place.
///
/// A test in its own right only so that the test binary can be started as this
/// process; run any other way, it finds nothing to do.
#[test]
fn the_process_killed_once_its_rename_is_made() {
    let (Some(root), Some(ready)) = (std::env::var_os(CHILD_ROOT), std::env::var_os(CHILD_READY))
    else {
        return;
    };
    let store = Store::new(PathBuf::from(root)).pausing(PathBuf::from(ready), Pause::Renamed);
    install(&store, &key("goose"), &version("2.0.0", None), 20);
    panic!("the commit went past its rename without pausing");
}

/// Commits release 1.0.0 of `goose` again over the one installed, and is
/// killed once the installed one is set aside and before the new one is
/// renamed into its place.
///
/// A test in its own right only so that the test binary can be started as this
/// process; run any other way, it finds nothing to do.
#[test]
fn the_process_killed_between_its_two_renames() {
    let (Some(root), Some(ready)) = (std::env::var_os(CHILD_ROOT), std::env::var_os(CHILD_READY))
    else {
        return;
    };
    let store = Store::new(PathBuf::from(root)).pausing(PathBuf::from(ready), Pause::SetAside);
    install(&store, &key("goose"), &version("1.0.0", None), 20);
    panic!("the commit went past setting the old version aside without pausing");
}

/// Holds release 1.0.0 of `goose` in the store it is given, as a running
/// agent does, says so, and waits to be killed.
///
/// A test in its own right only so that the test binary can be started as this
/// process; run any other way, it finds nothing to do.
#[test]
fn the_process_holding_a_version() {
    let (Some(root), Some(ready)) = (std::env::var_os(CHILD_ROOT), std::env::var_os(CHILD_READY))
    else {
        return;
    };
    let store = Store::new(PathBuf::from(root));
    let _held = store
        .hold(&key("goose"), &version("1.0.0", None))
        .expect("held");
    fs::write(ready, b"held\n").expect("the ready file");
    loop {
        std::thread::park();
    }
}

#[test]
fn a_version_is_committed_by_rename_from_staging_with_its_receipt() {
    let owned = Owned::new("store-commit");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);

    let receipt = install(&store, &goose, &one, 10);

    let directory = programs(&owned).join("goose").join("1.0.0");
    assert_eq!(store.directory(&goose, &one), directory);
    assert_eq!(
        fs::read(directory.join("lib").join("harness")).ok(),
        Some(proof::COMPANION.to_vec())
    );
    assert_eq!(names(&directory), [".receipt", "LICENSE", "agent", "lib"]);
    assert!(
        names(&programs(&owned).join("goose").join(".staging")).is_empty(),
        "the staging directory became the version, and its lock went with it"
    );
    let (_, artifact) = release();
    let expected = Receipt {
        key: goose.clone(),
        version: "1.0.0".into(),
        lock_revision: None,
        listing: listing(),
        fetched: at(10),
        origin: Origin::Fetched,
        artifacts: provenance(&artifact, 10).artifacts,
        files: listed(),
    };
    assert_eq!(receipt, expected);
    assert_eq!(store.receipt(&goose, &one).ok().flatten(), Some(expected));
    assert_eq!(store.verify(&goose, &one).ok(), Some(Vec::new()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |path: &Path| {
            fs::symlink_metadata(path).map_or(0, |metadata| metadata.permissions().mode() & 0o777)
        };
        assert_eq!(mode(&programs(&owned).join("goose")), 0o700);
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&directory.join(".receipt")), 0o600);
    }
}

#[test]
fn a_process_killed_once_its_rename_is_made_leaves_the_new_version_whole() {
    let owned = Owned::new("store-killed");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let (one, two) = (version("1.0.0", None), version("2.0.0", None));
    let kept = install(&store, &goose, &one, 10);
    let ready = owned.0.join("ready");

    let mut child = started(
        "store::tests::the_process_killed_once_its_rename_is_made",
        &root,
        &ready,
    );
    reached(&mut child, &ready);
    killed(child);

    // The receipt was written before the one rename that put 2.0.0 in place,
    // so it is installed whole.
    let receipt = store.receipt(&goose, &two).ok().flatten();
    assert_eq!(receipt.map(|receipt| receipt.fetched), Some(at(20)));
    assert_eq!(store.verify(&goose, &two).ok(), Some(Vec::new()));
    // The version installed before is as it was.
    assert_eq!(store.receipt(&goose, &one).ok().flatten(), Some(kept));
    assert_eq!(store.verify(&goose, &one).ok(), Some(Vec::new()));
    // The killed process's lock went with it, and what it left in staging is
    // removed when the next change starts.
    let store = Store::new(root.clone()).waiting(Duration::from_secs(2));
    let _staging = store.stage(&goose).expect("the store's lock is free");
    let left = names(&root.join("goose").join(".staging"));
    assert_eq!(left.len(), 2, "only the new staging and its lock: {left:?}");
}

#[test]
fn a_process_killed_between_its_two_renames_leaves_the_old_version_named_and_whole() {
    let owned = Owned::new("store-killed-aside");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let one = version("1.0.0", None);
    let kept = install(&store, &goose, &one, 10);
    let ready = owned.0.join("ready");

    let mut child = started(
        "store::tests::the_process_killed_between_its_two_renames",
        &root,
        &ready,
    );
    reached(&mut child, &ready);
    killed(child);

    // The next process to reach the store puts the version that was set aside
    // back, rather than taking it for what a dead process left.
    let held = store
        .hold(&goose, &one)
        .expect("the version is there again");
    assert_eq!(store.receipt(&goose, &one).ok().flatten(), Some(kept));
    assert_eq!(store.verify(&goose, &one).ok(), Some(Vec::new()));
    drop(held);
    let _staging = store.stage(&goose).expect("a staging directory");
    let left = names(&root.join("goose").join(".staging"));
    assert_eq!(left.len(), 2, "only the new staging and its lock: {left:?}");
}

#[test]
fn a_receipt_that_cannot_be_written_leaves_the_version_it_would_replace_whole() {
    let owned = Owned::new("store-receipt-fails");
    let root = programs(&owned);
    let goose = key("goose");
    let one = version("1.0.0", None);
    let kept = install(&Store::new(root.clone()), &goose, &one, 10);
    let failing = Store::new(root.clone()).failing();

    let (archive, artifact) = release();
    let staging = failing.stage(&goose).expect("a staging directory");
    let staged = Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
    let refused = failing.commit(staging, &one, staged.files(), provenance(&artifact, 20));
    assert!(matches!(refused, Err(StoreError::Io { .. })), "{refused:?}");
    assert_eq!(failing.receipt(&goose, &one).ok().flatten(), Some(kept));
    assert_eq!(failing.verify(&goose, &one).ok(), Some(Vec::new()));
    assert!(names(&root.join("goose").join(".staging")).is_empty());

    // Nor is a first install of another version left half in place.
    let two = version("2.0.0", None);
    let staging = failing.stage(&goose).expect("a staging directory");
    let staged = Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
    let refused = failing.commit(staging, &two, staged.files(), provenance(&artifact, 30));
    assert!(matches!(refused, Err(StoreError::Io { .. })), "{refused:?}");
    assert!(!failing.directory(&goose, &two).exists());
}

#[test]
fn a_version_another_process_holds_is_kept_by_a_prune_until_that_process_ends() {
    let owned = Owned::new("store-held");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let [one, two, three] = ["1.0.0", "2.0.0", "3.0.0"].map(|text| version(text, None));
    install(&store, &goose, &one, 10);
    install(&store, &goose, &two, 20);
    install(&store, &goose, &three, 30);
    let ready = owned.0.join("ready");
    let mut child = started("store::tests::the_process_holding_a_version", &root, &ready);
    reached(&mut child, &ready);
    let named = store.directory(&goose, &three).join("agent");

    let pruned = store.prune(&goose, &[named.as_path()]).expect("pruned");

    // 3.0.0 is named and 2.0.0 is the newest of the rest; 1.0.0 would go, but
    // another process runs it.
    assert!(pruned.removed.is_empty(), "{pruned:?}");
    assert_eq!(pruned.held, std::slice::from_ref(&one));
    assert!(store.directory(&goose, &one).join("agent").is_file());

    killed(child);
    let pruned = store.prune(&goose, &[named.as_path()]).expect("pruned");

    assert_eq!(pruned.removed, std::slice::from_ref(&one));
    assert!(pruned.held.is_empty(), "{pruned:?}");
    assert!(!store.directory(&goose, &one).exists());
    assert_eq!(
        names(&root.join("goose")),
        [".lock", ".staging", "2.0.0", "3.0.0"]
    );
}

#[test]
fn two_revisions_of_one_version_are_installed_side_by_side() {
    let owned = Owned::new("store-revisions");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let first = version("1.0.0", Some(1));
    let second = version("1.0.0", Some(2));

    let one = install(&store, &goose, &first, 10);
    let two = install(&store, &goose, &second, 20);

    assert_eq!(
        names(&programs(&owned).join("goose")),
        [".lock", ".staging", "1.0.0+1", "1.0.0+2"]
    );
    assert_eq!((one.lock_revision, two.lock_revision), (Some(1), Some(2)));
    assert_eq!(store.receipt(&goose, &first).ok().flatten(), Some(one));
    assert_eq!(store.receipt(&goose, &second).ok().flatten(), Some(two));
    assert_eq!(store.verify(&goose, &first).ok(), Some(Vec::new()));
    assert_eq!(store.verify(&goose, &second).ok(), Some(Vec::new()));
    // Rolled back to the first, the second is the newest of the rest.
    let rolled_back = store.directory(&goose, &first).join("lib").join("harness");
    assert_eq!(store.version_of(&goose, &rolled_back), Some(first.clone()));
    let pruned = store
        .prune(&goose, &[rolled_back.as_path()])
        .expect("pruned");
    assert!(pruned.removed.is_empty(), "{pruned:?}");
    assert!(store.directory(&goose, &second).is_dir());
}

/// What 0.48 left in `programs/antigravity/1.3.0/`: its files, each made so
/// that it can be run, and no receipt.
// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn earlier_install(root: &Path, files: &[(&str, &[u8])]) -> PathBuf {
    let directory = root.join("antigravity").join("1.3.0");
    fs::create_dir_all(&directory).expect("the version's directory");
    for (name, bytes) in files {
        let path = directory.join(name);
        fs::write(&path, bytes).expect("a file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("its mode");
        }
    }
    directory
}

/// Antigravity 1.3.0 as a catalog lists it for this platform, with each of
/// its two files' digest, or none where `digests` is false, as the snapshot
/// built from 0.48's manifest lists them.
fn antigravity(digests: bool) -> OwnedPin {
    let file = |path: &str, bytes: &[u8]| {
        let mut listed = listed_file(path, bytes, true);
        if let (false, FileKind::File { sha256, .. }) = (digests, &mut listed.kind) {
            *sha256 = Box::default();
        }
        listed
    };
    OwnedPin {
        key: key("antigravity"),
        version: "1.3.0".into(),
        lock_revision: None,
        admits: Admits {
            name: "antigravity-acp".into(),
            version: "1.3.0".into(),
        },
        target: Target {
            platform: "linux-x86_64".into(),
            cmd: "agy_acp_server.par".into(),
            args: Vec::new(),
            env: Vec::new(),
            hosts: Vec::new(),
            artifacts: vec![Artifact {
                url: "https://dl.google.com/agy-extensions/releases/linux/agy.zip".into(),
                mirror: None,
                sha256: digest(b"the archive"),
                size: 11,
                format: Format::Zip,
                unpacked: 27,
                decompressed: 27,
                files: Files::Listed(vec![
                    file("agy_acp_server.par", SERVER),
                    file("localharness_external", HARNESS),
                ]),
                kept: Vec::new(),
                replaced_by: None,
            }],
            launch: Launch::Binary,
        },
    }
}

#[test]
fn an_earlier_install_that_differs_from_the_list_is_left_to_be_checked() {
    let owned = Owned::new("store-adopt-differs");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let pin = antigravity(true);
    let one = version("1.3.0", None);
    let directory = earlier_install(
        &root,
        &[
            ("agy_acp_server.par", SERVER),
            ("localharness_external", b"LOCAL HARNESS"),
        ],
    );

    let adopted = store.adopt(&pin, listing(), at(40)).expect("adoption ran");

    let differs = |path: &str, what| Mismatch {
        path: path.into(),
        what,
    };
    assert_eq!(
        adopted,
        Adoption::CheckNeeded(vec![differs("localharness_external", Differs::Bytes)])
    );
    assert_eq!(store.receipt(&pin.key, &one).ok(), Some(None));
    // A file the list does not name is a difference too.
    fs::write(directory.join("localharness_external"), HARNESS).expect("the file mended");
    fs::write(directory.join("planted.so"), b"something else").expect("a file planted");
    assert_eq!(
        store.adopt(&pin, listing(), at(40)).ok(),
        Some(Adoption::CheckNeeded(vec![differs(
            "planted.so",
            Differs::Unlisted
        )]))
    );
    // A list with no digests, as the snapshot built from 0.48's manifest has,
    // matches nothing.
    fs::remove_file(directory.join("planted.so")).expect("the planted file gone");
    assert_eq!(
        store.adopt(&antigravity(false), listing(), at(40)).ok(),
        Some(Adoption::CheckNeeded(vec![
            differs("agy_acp_server.par", Differs::Bytes),
            differs("localharness_external", Differs::Bytes),
        ]))
    );
    assert_eq!(store.receipt(&pin.key, &one).ok(), Some(None));
}

#[test]
fn an_earlier_install_that_matches_the_list_is_adopted_with_a_receipt() {
    let owned = Owned::new("store-adopt");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let pin = antigravity(true);
    let one = version("1.3.0", None);
    earlier_install(
        &root,
        &[
            ("agy_acp_server.par", SERVER),
            ("localharness_external", HARNESS),
        ],
    );

    let adopted = store.adopt(&pin, listing(), at(40));

    let Ok(Adoption::Adopted(receipt)) = adopted else {
        panic!("not adopted: {adopted:?}");
    };
    let Some(artifact) = pin.target.artifacts.first() else {
        panic!("the pin has an artifact");
    };
    assert_eq!(
        receipt,
        Receipt {
            key: pin.key.clone(),
            version: "1.3.0".into(),
            lock_revision: None,
            listing: listing(),
            fetched: at(40),
            origin: Origin::Adopted,
            artifacts: vec![Fetched {
                url: artifact.url.clone(),
                hosts: Vec::new(),
                size: artifact.size,
                sha256: artifact.sha256.clone(),
            }],
            files: vec![
                listed_file("agy_acp_server.par", SERVER, true),
                listed_file("localharness_external", HARNESS, true),
            ],
        }
    );
    assert_eq!(
        store.receipt(&pin.key, &one).ok().flatten(),
        Some(receipt.clone())
    );
    assert_eq!(store.verify(&pin.key, &one).ok(), Some(Vec::new()));
    // Adopted once, it keeps the receipt it was given.
    assert_eq!(
        store.adopt(&pin, listing(), at(50)).ok(),
        Some(Adoption::Adopted(receipt))
    );
}

#[test]
fn a_prune_keeps_every_version_an_executable_names_and_the_newest_of_the_rest() {
    let owned = Owned::new("store-prune");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let [one, two, three, four] =
        ["1.0.0", "2.0.0", "3.0.0", "4.0.0"].map(|text| version(text, None));
    install(&store, &goose, &one, 10);
    // Fetched in another order than their names sort in: the newest is the
    // one installed last.
    install(&store, &goose, &four, 15);
    install(&store, &goose, &two, 25);
    install(&store, &goose, &three, 30);
    let goose_root = root.join("goose");
    // A version no receipt was written for, a holder whose process is gone,
    // and what 0.48's install leaves beside its versions.
    fs::create_dir_all(goose_root.join("0.9.0")).expect("a version");
    fs::write(goose_root.join("0.9.0").join("agent"), b"half").expect("a file");
    fs::create_dir_all(goose_root.join("3.0.0").join(".in-use")).expect("holders");
    fs::write(goose_root.join("3.0.0").join(".in-use").join("1-0"), b"").expect("a holder");
    fs::write(goose_root.join(".download-1-0.zip"), b"").expect("a download");
    let first = store.directory(&goose, &one).join("agent");
    let third = store.directory(&goose, &three).join("lib").join("harness");
    let elsewhere = owned.0.join("bin").join("goose");

    let pruned = store
        .prune(
            &goose,
            &[first.as_path(), third.as_path(), elsewhere.as_path()],
        )
        .expect("pruned");

    assert_eq!(pruned.removed, [version("0.9.0", None), four]);
    assert!(pruned.held.is_empty(), "{pruned:?}");
    assert_eq!(
        names(&goose_root),
        [
            ".download-1-0.zip",
            ".lock",
            ".staging",
            "1.0.0",
            "2.0.0",
            "3.0.0"
        ]
    );
    assert!(names(&goose_root.join("3.0.0").join(".in-use")).is_empty());
    assert_eq!(store.verify(&goose, &three).ok(), Some(Vec::new()));
    assert!(names(&goose_root.join(".staging")).is_empty());
}

#[test]
fn staging_a_dead_process_left_is_removed_when_the_next_change_starts() {
    let owned = Owned::new("store-staging");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let live = store.stage(&goose).expect("a staging directory");
    fs::write(live.directory().join("partial"), b"half an agent").expect("a file");
    let area = programs(&owned).join("goose").join(".staging");
    // What a process that died left: a staging directory whose lock nobody
    // holds, one with no lock, and a lock with no directory.
    fs::create_dir(area.join("dead-1")).expect("a directory");
    fs::write(area.join("dead-1").join("partial"), b"half").expect("a file");
    fs::write(area.join("dead-1.lock"), b"").expect("a lock");
    fs::create_dir(area.join("dead-2")).expect("a directory");
    fs::write(area.join("dead-3.lock"), b"").expect("a lock");

    let next = store.stage(&goose).expect("another staging directory");

    let left = names(&area);
    assert!(
        !left.iter().any(|name| name.starts_with("dead")),
        "{left:?}"
    );
    assert_eq!(left.len(), 4, "two live stagings and their locks: {left:?}");
    assert!(
        live.directory().join("partial").is_file(),
        "a live staging directory is not removed"
    );
    assert_ne!(live.directory(), next.directory());
    drop(live);
    drop(next);
    assert!(
        names(&area).is_empty(),
        "a staging directory given up removes itself"
    );
}

/// A symbolic link at `path` to `target`.
// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn symlink(target: &str, path: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, path).expect("a link");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, path).expect("a link");
}

#[test]
fn verify_names_every_way_a_version_differs_from_its_receipt() {
    let owned = Owned::new("store-verify");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    install(&store, &goose, &one, 10);
    let directory = store.directory(&goose, &one);
    fs::write(directory.join("agent"), b"short").expect("a file shortened");
    fs::write(directory.join("LICENSE"), b"BSD License\n").expect("a file changed");
    fs::remove_file(directory.join("lib").join("harness")).expect("a file removed");
    let current = directory.join("lib").join("current");
    fs::remove_file(&current).expect("a link removed");
    symlink("elsewhere", &current);
    fs::write(directory.join("lib").join("sitecustomize.py"), b"import os").expect("planted");

    let found = store.verify(&goose, &one).expect("verified");

    let differs = |path: &str, what| Mismatch {
        path: path.into(),
        what,
    };
    assert_eq!(
        found,
        [
            differs("agent", Differs::Size),
            differs("LICENSE", Differs::Bytes),
            differs("lib/harness", Differs::Missing),
            differs("lib/current", Differs::Target),
            differs("lib/sitecustomize.py", Differs::Unlisted),
        ]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let two = version("2.0.0", None);
        install(&store, &goose, &two, 20);
        let directory = store.directory(&goose, &two);
        fs::set_permissions(directory.join("agent"), fs::Permissions::from_mode(0o644))
            .expect("its mode");
        fs::remove_file(directory.join("LICENSE")).expect("a file removed");
        fs::create_dir(directory.join("LICENSE")).expect("a directory in its place");

        assert_eq!(
            store.verify(&goose, &two).ok(),
            Some(vec![
                differs("agent", Differs::Exec),
                differs("LICENSE", Differs::Kind),
            ])
        );
    }
}

#[test]
fn a_receipt_in_a_format_this_crucible_does_not_know_or_that_is_not_one_is_refused() {
    let owned = Owned::new("store-receipt");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    install(&store, &goose, &one, 10);
    let path = store.directory(&goose, &one).join(".receipt");
    let good = fs::read_to_string(&path).expect("the receipt");
    let replaced = |from: &str, to: &str| {
        assert!(good.contains(from), "{from} is written");
        good.replacen(from, to, 1)
    };

    for (what, text, format) in [
        (
            "another format",
            replaced(r#""format":1"#, r#""format":2"#),
            Some(2),
        ),
        ("not JSON", "a receipt".to_owned(), None),
        ("not a receipt", "{}".to_owned(), None),
        (
            "a path that climbs out",
            replaced(r#""path":"lib/harness""#, r#""path":"../harness""#),
            None,
        ),
        (
            "another release",
            replaced(r#""version":"1.0.0""#, r#""version":"1.0.1""#),
            None,
        ),
        ("an oversize one", " ".repeat(RECEIPT_BYTES + 1), None),
    ] {
        fs::write(&path, &text).expect("the receipt rewritten");

        let read = store.receipt(&goose, &one);

        match format {
            Some(number) => assert!(
                matches!(read, Err(StoreError::Format(found)) if found == number),
                "{what}: {read:?}"
            ),
            None => assert!(
                matches!(read, Err(StoreError::Corrupt | StoreError::Oversize)),
                "{what}: {read:?}"
            ),
        }
        assert!(store.verify(&goose, &one).is_err(), "{what}");
    }
}

/// What the unpacker makes of a program published bare at an address that
/// ends in `name`: it writes the program under that name, or refuses it.
// A test that cannot set up what it checks has nothing to report.
#[allow(clippy::expect_used)]
fn surveyed(owned: &Owned, index: usize, name: &str) -> bool {
    let bytes = b"a program";
    let artifact = Artifact {
        url: format!("https://example.invalid/releases/{name}").into(),
        mirror: None,
        sha256: digest(bytes),
        size: 9,
        format: Format::Bare,
        unpacked: 9,
        decompressed: 9,
        files: Files::Listed(Vec::new()),
        kept: Vec::new(),
        replaced_by: None,
    };
    let staging = owned.0.join(format!("survey-{index}"));
    fs::create_dir(&staging).expect("a staging directory");
    Unpacker::survey(bytes, &artifact, &staging).is_ok()
}

#[test]
fn a_directory_name_windows_cannot_hold_is_refused_as_the_unpacker_refuses_it() {
    let owned = Owned::new("store-names");
    let long = "a".repeat(256);
    let names = [
        "con",
        "CON",
        "Con.txt",
        "con .tar",
        "nul",
        "aux.1",
        "prn",
        "com1",
        "COM9.log",
        "lpt0",
        "conin$",
        "CONOUT$.x",
        "1.4.1.",
        "1.4.1 ",
        "a<b",
        "a>b",
        "a|b",
        "a\"b",
        "a*b",
        "a\\b",
        "a:b",
        ".",
        "..",
        "caf\u{e9}",
        "a\tb",
        "",
        &long,
        "1.3.0",
        "1.4.1-beta.2",
        "v2",
        "com10",
        "con1",
        "lpt",
        "conx",
        "nul-agent",
        " 1.0",
    ];

    for (index, name) in names.iter().enumerate() {
        let unpacker = surveyed(&owned, index, name);
        let store = Version::new(name, None).is_ok();
        assert_eq!(store, unpacker, "{name:?}");
    }
    // The store refuses more: no version begins with a dot, so none takes a
    // name the store keeps for itself, and none carries a `+`, so no two
    // releases share a directory.
    for name in [
        ".lock",
        ".staging",
        ".in-use",
        ".1.0",
        "1.0.0+2",
        "1.0+build",
    ] {
        assert!(
            matches!(Version::new(name, None), Err(StoreError::Name { .. })),
            "{name:?}"
        );
    }
    // An agent's key is held to the same rules before its directory is made.
    let store = Store::new(programs(&owned));
    for name in ["con", "nul", "com1", "lpt9", "aux", "prn"] {
        let refused = store.stage(&key(name));
        assert!(
            matches!(refused, Err(StoreError::Name { .. })),
            "{name}: {refused:?}"
        );
    }
    assert!(!programs(&owned).exists(), "nothing is made for them");
}

#[test]
fn a_release_larger_than_the_store_or_its_room_is_refused_before_a_byte_is_written() {
    let owned = Owned::new("store-cap");
    let root = programs(&owned);
    let (_, artifact) = release();
    let target = |artifacts: Vec<Artifact>| Target {
        platform: "linux-x86_64".into(),
        cmd: "agent".into(),
        args: Vec::new(),
        env: Vec::new(),
        hosts: Vec::new(),
        artifacts,
        launch: Launch::Binary,
    };
    let past = PROGRAMS_BYTES + 1;
    let store = Store::new(root.clone());

    assert!(store.fits(&target(vec![artifact.clone()])).is_ok());
    for (what, oversized) in [
        (
            "its download",
            Artifact {
                size: past,
                ..artifact.clone()
            },
        ),
        (
            "what it unpacks to",
            Artifact {
                unpacked: past,
                ..artifact.clone()
            },
        ),
        (
            "what it decompresses to",
            Artifact {
                decompressed: past,
                ..artifact.clone()
            },
        ),
        (
            "the asset built in its place",
            Artifact {
                replaced_by: Some(AssetRef {
                    sha256: digest(b"built"),
                    size: past,
                }),
                ..artifact.clone()
            },
        ),
    ] {
        let refused = store.fits(&target(vec![oversized]));
        assert!(
            matches!(refused, Err(StoreError::TooLarge { .. })),
            "{what}: {refused:?}"
        );
    }
    let half = Artifact {
        unpacked: PROGRAMS_BYTES / 2 + 1,
        decompressed: PROGRAMS_BYTES / 2 + 1,
        ..artifact.clone()
    };
    let refused = store.fits(&target(vec![half.clone(), half]));
    assert!(
        matches!(refused, Err(StoreError::TooLarge { .. })),
        "{refused:?}"
    );
    assert!(!root.exists(), "nothing is written to find that out");

    // The room is the store's total less what the receipts already kept
    // account for.
    let goose = key("goose");
    let capacity = artifact.unpacked + 60;
    let roomy = Store::new(root.clone()).bounded(capacity);
    install(&roomy, &goose, &version("1.0.0", None), 10);
    assert!(roomy.fits(&target(vec![artifact.clone()])).is_ok());
    install(&roomy, &goose, &version("2.0.0", None), 20);
    let refused = roomy.fits(&target(vec![artifact.clone()]));
    assert!(
        matches!(refused, Err(StoreError::Full { .. })),
        "{refused:?}"
    );

    // A commit past it is refused before the rename, and its staging goes.
    let tight = Store::new(root.clone()).bounded(120);
    let (archive, artifact) = release();
    let staging = tight.stage(&goose).expect("a staging directory");
    let staged = Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
    let refused = tight.commit(
        staging,
        &version("3.0.0", None),
        staged.files(),
        provenance(&artifact, 30),
    );
    assert!(
        matches!(refused, Err(StoreError::Full { .. })),
        "{refused:?}"
    );
    assert!(!root.join("goose").join("3.0.0").exists());
    assert!(names(&root.join("goose").join(".staging")).is_empty());
}

#[test]
fn a_store_change_waits_a_while_for_another_process_and_is_then_refused() {
    let owned = Owned::new("store-lock");
    let root = programs(&owned);
    let goose = key("goose");
    let store = Store::new(root.clone());
    install(&store, &goose, &version("1.0.0", None), 10);
    let lock = crucible_privacy::lock(&root.join("goose").join(".lock")).expect("the lock file");
    assert!(
        crucible_privacy::try_lock_identity(&lock).unwrap_or(false),
        "the lock is taken"
    );
    let wait = Duration::from_millis(200);
    let store = Store::new(root.clone()).waiting(wait);

    let started = Instant::now();
    let refused = store.stage(&goose);

    assert!(matches!(refused, Err(StoreError::Contended)), "{refused:?}");
    assert!(started.elapsed() >= wait, "{:?}", started.elapsed());
    assert!(started.elapsed() < Duration::from_secs(10));
    let one = version("1.0.0", None);
    assert!(matches!(
        store.prune(&goose, &[]),
        Err(StoreError::Contended)
    ));
    assert!(matches!(
        store.hold(&goose, &one),
        Err(StoreError::Contended)
    ));
    let pin = OwnedPin {
        key: goose.clone(),
        ..antigravity(true)
    };
    assert!(matches!(
        store.adopt(&pin, listing(), at(1)),
        Err(StoreError::Contended)
    ));
    drop(lock);
    assert!(store.stage(&goose).is_ok());
}

#[test]
fn a_version_in_use_is_not_replaced_and_one_not_installed_cannot_be_held() {
    let owned = Owned::new("store-in-use");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    let refused = store.hold(&goose, &one);
    assert!(
        matches!(refused, Err(StoreError::NotInstalled)),
        "{refused:?}"
    );
    install(&store, &goose, &one, 10);
    let holders = store.directory(&goose, &one).join(".in-use");

    let held = store.hold(&goose, &one).expect("held");

    assert_eq!(names(&holders).len(), 1);
    let (archive, artifact) = release();
    let staging = store.stage(&goose).expect("a staging directory");
    let staged = Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
    let refused = store.commit(staging, &one, staged.files(), provenance(&artifact, 20));
    assert!(matches!(refused, Err(StoreError::InUse)), "{refused:?}");
    assert!(names(&programs(&owned).join("goose").join(".staging")).is_empty());
    drop(held);
    assert!(
        names(&holders).is_empty(),
        "a hold that ended removes its file"
    );
    let receipt = install(&store, &goose, &one, 20);
    assert_eq!(receipt.fetched, at(20));
}

#[test]
fn a_staged_tree_that_takes_a_store_name_or_is_not_its_list_is_not_committed() {
    let owned = Owned::new("store-reserved");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    for name in [".in-use", ".receipt", ".RECEIPT", ".receipt.writing"] {
        let staging = store.stage(&goose).expect("a staging directory");
        fs::write(staging.directory().join(name), b"x").expect("a file");
        let files = [listed_file(name, b"x", false)];

        let refused = store.commit(staging, &one, &files, provenance(&release().1, 10));

        assert!(
            matches!(refused, Err(StoreError::Reserved { .. })),
            "{name}: {refused:?}"
        );
    }
    let (archive, artifact) = release();
    let mut short = listed();
    short.retain(|file| *file.path != *"LICENSE");
    let mut long = listed();
    long.push(listed_file("extra", b"extra", false));
    for (what, files, expected) in [
        ("one file left out", short, ("LICENSE", Differs::Unlisted)),
        ("one file too many", long, ("extra", Differs::Missing)),
    ] {
        let staging = store.stage(&goose).expect("a staging directory");
        Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");

        let refused = store.commit(staging, &one, &files, provenance(&artifact, 10));

        assert!(
            matches!(&refused, Err(StoreError::Unrecorded(found))
                if *found.path == *expected.0 && found.what == expected.1),
            "{what}: {refused:?}"
        );
    }
    assert!(!store.directory(&goose, &one).exists());
}

/// A link where a version goes is not a version: its receipt is not read,
/// it is not verified or held, and a prune leaves it alone.
#[cfg(unix)]
#[test]
fn a_link_where_a_version_goes_is_not_taken_for_one() {
    let owned = Owned::new("store-linked");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    install(&store, &goose, &one, 10);
    let linked = version("9.9.9", None);
    let elsewhere = owned.0.join("elsewhere");
    fs::rename(store.directory(&goose, &one), &elsewhere).expect("the version moved");
    std::os::unix::fs::symlink(&elsewhere, store.directory(&goose, &linked)).expect("a link");
    fs::write(
        elsewhere.join(".receipt"),
        fs::read_to_string(elsewhere.join(".receipt"))
            .expect("the receipt")
            .replacen(r#""version":"1.0.0""#, r#""version":"9.9.9""#, 1),
    )
    .expect("the receipt rewritten for the link");

    assert_eq!(store.receipt(&goose, &linked).ok(), Some(None));
    assert!(matches!(
        store.verify(&goose, &linked),
        Err(StoreError::NotInstalled)
    ));
    assert!(matches!(
        store.hold(&goose, &linked),
        Err(StoreError::NotInstalled)
    ));
    let pruned = store.prune(&goose, &[]).expect("pruned");
    assert_eq!(pruned, super::Pruned::default());
    assert!(store.directory(&goose, &linked).is_symlink());
}

#[test]
fn what_the_store_cannot_read_whole_is_reported_and_never_counted_as_nothing() {
    let owned = Owned::new("store-crowded");
    let root = programs(&owned);
    let (_, artifact) = release();
    let target = Target {
        platform: "linux-x86_64".into(),
        cmd: "agent".into(),
        args: Vec::new(),
        env: Vec::new(),
        hosts: Vec::new(),
        artifacts: vec![artifact],
        launch: Launch::Binary,
    };
    // A version with no receipt still takes room, and is counted by what it
    // holds.
    let unreceipted = root.join("goose").join("0.9.0");
    fs::create_dir_all(&unreceipted).expect("a version");
    fs::write(unreceipted.join("agent"), vec![b'x'; 4096]).expect("a file");
    let tight = Store::new(root.clone()).bounded((1 << 20) + 100);
    let refused = tight.fits(&target);
    assert!(
        matches!(refused, Err(StoreError::Full { .. })),
        "{refused:?}"
    );
    // More agents than the store reads is said, not taken for none.
    for index in 0..=4096 {
        fs::create_dir(root.join(format!("agent-{index}"))).expect("an agent's directory");
    }
    let refused = Store::new(root.clone()).fits(&target);
    assert!(
        matches!(refused, Err(StoreError::Crowded { .. })),
        "{refused:?}"
    );
    // And staging past one reading's worth is removed whole.
    let area = root.join("goose").join(".staging");
    fs::create_dir_all(&area).expect("the staging directory");
    for index in 0..5000 {
        fs::write(area.join(format!("dead-{index}")), b"").expect("a leftover");
    }
    let staging = Store::new(root.clone())
        .stage(&key("goose"))
        .expect("a staging directory");
    assert_eq!(names(&area).len(), 2);
    drop(staging);
}

#[test]
fn a_prune_that_cannot_tell_which_version_an_executable_names_removes_nothing() {
    let owned = Owned::new("store-unmatched");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let [one, two, three] = ["1.0.0", "2.0.0", "3.0.0"].map(|text| version(text, None));
    install(&store, &goose, &one, 10);
    install(&store, &goose, &two, 20);
    install(&store, &goose, &three, 30);
    let named = store.directory(&goose, &three).join("agent");
    for doubtful in [
        root.join("goose").join("1.0.0+x").join("agent"),
        root.join("goose").join(".staging").join("agent"),
        root.join("goose").join("agent"),
    ] {
        let refused = store.prune(&goose, &[named.as_path(), doubtful.as_path()]);
        assert!(
            matches!(refused, Err(StoreError::Unmatched { .. })),
            "{doubtful:?}: {refused:?}"
        );
        assert!(store.directory(&goose, &one).is_dir());
    }
    // A path to the store spelled through a link, to a file that is not
    // there, still names its version.
    #[cfg(unix)]
    {
        let spelled = owned.0.join("spelled");
        std::os::unix::fs::symlink(&root, &spelled).expect("a link to the store");
        let first = spelled.join("goose").join("1.0.0").join("gone");
        let pruned = store
            .prune(&goose, &[named.as_path(), first.as_path()])
            .expect("pruned");
        assert!(pruned.removed.is_empty(), "{pruned:?}");
        assert!(store.directory(&goose, &one).is_dir());
    }
}

#[test]
fn a_version_whose_receipt_cannot_be_read_is_never_pruned_and_is_to_be_checked() {
    let owned = Owned::new("store-later-format");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let [one, two, three] = ["1.0.0", "2.0.0", "3.0.0"].map(|text| version(text, None));
    install(&store, &goose, &one, 10);
    install(&store, &goose, &two, 20);
    install(&store, &goose, &three, 30);
    let path = store.directory(&goose, &one).join(".receipt");
    let later = fs::read_to_string(&path).expect("the receipt").replacen(
        r#""format":1"#,
        r#""format":2"#,
        1,
    );
    fs::write(&path, later).expect("a receipt a later crucible wrote");
    let named = store.directory(&goose, &three).join("agent");

    let pruned = store.prune(&goose, &[named.as_path()]).expect("pruned");

    assert!(pruned.removed.is_empty(), "{pruned:?}");
    assert_eq!(pruned.unreadable, std::slice::from_ref(&one));
    assert!(store.directory(&goose, &one).is_dir());
    let pin = OwnedPin {
        key: goose.clone(),
        version: "1.0.0".into(),
        ..antigravity(true)
    };
    assert_eq!(
        store.adopt(&pin, listing(), at(40)).ok(),
        Some(Adoption::CheckNeeded(vec![Mismatch {
            path: ".receipt".into(),
            what: Differs::Unreadable,
        }]))
    );
}

#[test]
fn a_file_list_with_a_path_or_link_that_leaves_the_version_is_refused() {
    let owned = Owned::new("store-paths");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    let (archive, artifact) = release();
    for (what, extra) in [
        (
            "a path that climbs out",
            listed_file("../agent", b"x", false),
        ),
        (
            "a path Windows cannot hold",
            listed_file("lib/con", b"x", false),
        ),
        (
            "a link that climbs out",
            listed_link("lib/out", "../../agent"),
        ),
        ("an absolute link", listed_link("lib/out", "/etc/passwd")),
    ] {
        let staging = store.stage(&goose).expect("a staging directory");
        Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
        let mut files = listed();
        files.push(extra);

        let refused = store.commit(staging, &one, &files, provenance(&artifact, 10));

        assert!(
            matches!(refused, Err(StoreError::Listed { .. })),
            "{what}: {refused:?}"
        );
    }
    let mut pin = antigravity(true);
    if let Some(Files::Listed(files)) = pin.target.artifacts.first_mut().map(|a| &mut a.files) {
        files.push(listed_file("../outside", b"x", false));
    }
    earlier_install(&programs(&owned), &[("agy_acp_server.par", SERVER)]);
    let refused = store.adopt(&pin, listing(), at(40));
    assert!(
        matches!(refused, Err(StoreError::Listed { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_set_aside_version_that_cannot_be_put_back_is_kept_and_said() {
    let owned = Owned::new("store-unrestorable");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let one = version("1.0.0", None);
    let kept = install(&store, &goose, &one, 10);
    let ready = owned.0.join("ready");
    let mut child = started(
        "store::tests::the_process_killed_between_its_two_renames",
        &root,
        &ready,
    );
    reached(&mut child, &ready);
    killed(child);
    let area = root.join("goose").join(".staging");

    let refused = Store::new(root.clone()).unrestorable().stage(&goose);

    assert!(matches!(refused, Err(StoreError::Io { .. })), "{refused:?}");
    let asides: Vec<String> = names(&area)
        .into_iter()
        .filter(|name| name.starts_with("aside"))
        .collect();
    assert_eq!(asides.len(), 1, "the only copy is kept: {:?}", names(&area));
    // The next change that can put it back does.
    let _staging = store.stage(&goose).expect("a staging directory");
    assert_eq!(store.receipt(&goose, &one).ok().flatten(), Some(kept));
    assert_eq!(store.verify(&goose, &one).ok(), Some(Vec::new()));
}

#[test]
fn a_commit_whose_rename_back_fails_keeps_the_version_it_set_aside() {
    let owned = Owned::new("store-restore-fails");
    let root = programs(&owned);
    let store = Store::new(root.clone()).unrestorable();
    let goose = key("goose");
    let one = version("1.0.0", None);
    install(&Store::new(root.clone()), &goose, &one, 10);
    let destination = store.directory(&goose, &one);
    let area = root.join("goose").join(".staging");
    let aside = area.join("aside-test");
    fs::create_dir(&aside).expect("an aside");
    fs::rename(&destination, aside.join("1.0.0")).expect("set aside");

    let restored = store.restored(Some(&aside), &destination);

    assert!(restored.is_err(), "{restored:?}");
    assert!(aside.join("1.0.0").join(".receipt").is_file());
}

#[test]
fn a_link_that_leaves_the_version_through_another_link_is_refused() {
    let owned = Owned::new("store-chained");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    let (_, artifact) = release();
    let staging = store.stage(&goose).expect("a staging directory");
    let files = vec![
        listed_dir("e"),
        listed_dir("e/f"),
        listed_file("x", b"x", false),
        // Each is inside by its own components; through the first, the
        // second climbs out.
        listed_link("e/f/a", "../../x"),
        listed_link("l", "e/f/a/../.."),
    ];

    let refused = store.commit(staging, &one, &files, provenance(&artifact, 10));

    assert!(
        matches!(refused, Err(StoreError::Listed { .. })),
        "{refused:?}"
    );
    // A chain that ends on a file inside is not refused for being a chain.
    let staging = store.stage(&goose).expect("a staging directory");
    let files = vec![
        listed_file("x", b"x", false),
        listed_link("b", "x"),
        listed_link("c", "b"),
    ];
    let refused = store.commit(staging, &one, &files, provenance(&artifact, 10));
    assert!(
        !matches!(refused, Err(StoreError::Listed { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_version_whose_receipt_cannot_be_read_is_replaced_only_when_asked() {
    let owned = Owned::new("store-replace-unreadable");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    let one = version("1.0.0", None);
    install(&store, &goose, &one, 10);
    let path = store.directory(&goose, &one).join(".receipt");
    let later = fs::read_to_string(&path).expect("the receipt").replacen(
        r#""format":1"#,
        r#""format":2"#,
        1,
    );
    fs::write(&path, &later).expect("a later crucible's receipt");
    let (archive, artifact) = release();

    let staging = store.stage(&goose).expect("a staging directory");
    let staged = Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
    let refused = store.commit(staging, &one, staged.files(), provenance(&artifact, 20));

    assert!(
        matches!(refused, Err(StoreError::Unreadable)),
        "{refused:?}"
    );
    assert_eq!(fs::read_to_string(&path).ok(), Some(later));
    let mut staging = store.stage(&goose).expect("a staging directory");
    let staged = Unpacker::unpack(&archive, &artifact, staging.directory()).expect("unpacked");
    staging.replacing_unreadable();
    let receipt = store
        .commit(staging, &one, staged.files(), provenance(&artifact, 20))
        .expect("replaced when asked");
    assert_eq!(receipt.fetched, at(20));
}

#[test]
fn removing_an_agent_takes_every_version_unless_an_instance_names_one_or_a_process_runs_one() {
    let owned = Owned::new("store-remove");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let [one, two] = ["1.0.0", "2.0.0"].map(|text| version(text, None));
    install(&store, &goose, &one, 10);
    install(&store, &goose, &two, 20);
    let named = store.directory(&goose, &two).join("agent");

    let refused = store.remove(&goose, &[named.as_path()]);

    assert!(
        matches!(refused, Err(StoreError::Named { ref path }) if *path == named),
        "{refused:?}"
    );
    assert!(store.directory(&goose, &one).join("agent").is_file());
    assert!(store.directory(&goose, &two).join("agent").is_file());

    let ready = owned.0.join("ready");
    let mut child = started("store::tests::the_process_holding_a_version", &root, &ready);
    reached(&mut child, &ready);
    let elsewhere = owned.0.join("bin").join("goose");

    let refused = store.remove(&goose, &[elsewhere.as_path()]);

    assert!(matches!(refused, Err(StoreError::InUse)), "{refused:?}");
    assert!(store.directory(&goose, &one).join("agent").is_file());
    assert!(store.directory(&goose, &two).join("agent").is_file());

    killed(child);
    let removed = store
        .remove(&goose, &[elsewhere.as_path()])
        .expect("removed");

    assert_eq!(removed, [one, two]);
    assert_eq!(names(&root.join("goose")), [".lock", ".staging"]);
    assert!(names(&root.join("goose").join(".staging")).is_empty());
}

#[test]
fn the_versions_installed_are_listed_with_their_receipts() {
    let owned = Owned::new("store-installed");
    let store = Store::new(programs(&owned));
    let goose = key("goose");
    assert!(store.installed(&goose).expect("listed").is_empty());
    let [one, two] = ["1.0.0", "2.0.0"].map(|text| version(text, None));
    let first = install(&store, &goose, &one, 10);
    let second = install(&store, &goose, &two, 20);
    fs::create_dir_all(programs(&owned).join("goose").join("0.9.0")).expect("a version");

    let installed = store.installed(&goose).expect("listed");

    assert_eq!(installed, [first, second]);
}

#[test]
fn a_download_beside_a_live_staging_is_kept_by_another_change_and_goes_with_it() {
    use std::io::{Read as _, Write as _};

    let owned = Owned::new("store-download");
    let root = programs(&owned);
    let store = Store::new(root.clone());
    let goose = key("goose");
    let staging = store.stage(&goose).expect("a staging directory");
    let mut written = staging.download().expect("a download file");
    written.write_all(b"the first artifact").expect("written");
    drop(written);
    let mut again = staging.download().expect("the download file again");
    again.write_all(b"the second").expect("written");
    drop(again);

    // Another change cleans staging, and this one is live.
    let other = store.stage(&goose).expect("another staging directory");
    drop(other);
    let mut read = String::new();
    staging
        .downloaded()
        .expect("the download opened")
        .read_to_string(&mut read)
        .expect("read");

    assert_eq!(read, "the second");
    let area = root.join("goose").join(".staging");
    assert_eq!(names(&area).len(), 3, "{:?}", names(&area));
    drop(staging);
    assert!(names(&area).is_empty(), "{:?}", names(&area));
}
