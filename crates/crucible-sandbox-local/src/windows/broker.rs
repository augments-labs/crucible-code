//! Discovery and provenance of the packaged native-Windows broker.

use std::fs::File;
use std::io::{self, Read as _};
use std::os::windows::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxError,
};
use sha2::{Digest as _, Sha256};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

const MAX_BROKER_BYTES: u64 = 32 * 1024 * 1024;

/// The version crucible gives its account, filtering and token scheme: its own
/// name, so a preparation and an inspection state it without asking anything.
pub(super) const VERSION: &str = "account-wfp-token-v1";

/// The name of the broker in a release, beside `crucible.exe`.
const BROKER: &str = "crucible-sandbox-broker.exe";

/// The directory this process's broker is looked for in.
static HELD: Held = Held::new();

/// Settles, once, the directory this process's broker is looked for in.
pub(crate) fn hold_broker_directory() {
    let _ = HELD.directory(std::env::current_exe);
}

/// The directory a process's broker is looked for in, settled the first time
/// it is asked for and never again.
struct Held(OnceLock<Result<PathBuf, &'static str>>);

impl Held {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    fn directory(
        &self,
        executable: impl FnOnce() -> io::Result<PathBuf>,
    ) -> Result<PathBuf, &'static str> {
        self.0.get_or_init(|| resolved(executable())).clone()
    }
}

/// The directory of the running executable, once every link in its path is
/// resolved.
fn resolved(executable: io::Result<PathBuf>) -> Result<PathBuf, &'static str> {
    let executable = executable.map_err(|_| "could not locate the Crucible executable")?;
    let executable = executable
        .canonicalize()
        .map_err(|_| "could not resolve where the Crucible executable is")?;
    executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or("the Crucible executable is in no directory")
}

/// Where a broker is looked for, in the order it is looked for there: beside
/// the executable, then in the directory above, where a build puts its helpers.
fn candidates(directory: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![directory.join(BROKER)];
    if let Some(above) = directory.parent() {
        candidates.push(above.join(BROKER));
    }
    candidates
}

#[derive(Debug, Clone)]
pub(super) struct Broker {
    path: PathBuf,
    identity: SandboxBackendIdentity,
}

impl Broker {
    pub(super) fn find(excluded: &[&Path]) -> Result<Self, SandboxError> {
        let directory = HELD.directory(std::env::current_exe).map_err(unavailable)?;
        Self::first_trusted(candidates(&directory), excluded)
    }

    fn first_trusted(candidates: Vec<PathBuf>, excluded: &[&Path]) -> Result<Self, SandboxError> {
        for candidate in candidates {
            let Ok(path) = candidate.canonicalize() else {
                continue;
            };
            if excluded.iter().any(|root| path.starts_with(root)) {
                continue;
            }
            let Ok(metadata) = path.metadata() else {
                continue;
            };
            if !metadata.is_file()
                || metadata.len() == 0
                || metadata.len() > MAX_BROKER_BYTES
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                continue;
            }
            let digest = digest(&path, metadata.len())?;
            let id = SandboxBackendId::new("windows-native")
                .map_err(|_| unavailable("invalid built-in Windows backend identity"))?;
            let identity = SandboxBackendIdentity::new(
                id,
                VERSION,
                SandboxBackendProvenance::Bundled,
                Some(digest),
            )
            .map_err(|_| unavailable("invalid built-in Windows backend version"))?;
            return Ok(Self { path, identity });
        }
        Err(unavailable(
            "the packaged Windows sandbox broker is unavailable outside writable roots",
        ))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) const fn identity(&self) -> &SandboxBackendIdentity {
        &self.identity
    }
}

fn digest(path: &Path, expected: u64) -> Result<[u8; 32], SandboxError> {
    let mut file = File::open(path)
        .map_err(|_| unavailable("the Windows sandbox broker could not be opened"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| unavailable("the Windows sandbox broker could not be hashed"))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > expected || total > MAX_BROKER_BYTES {
            return Err(unavailable(
                "the Windows sandbox broker changed while it was inspected",
            ));
        }
        let bytes = buffer.get(..read).ok_or_else(|| {
            unavailable("the Windows sandbox broker returned an invalid read length")
        })?;
        digest.update(bytes);
    }
    if total != expected {
        return Err(unavailable(
            "the Windows sandbox broker changed while it was inspected",
        ));
    }
    Ok(digest.finalize().into())
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file a copy of this test binary, started by [`found_by_process`],
    /// writes the broker it found to.
    const LOOKUP_REPORT: &str = "CRUCIBLE_TEST_BROKER_LOOKUP_REPORT";

    /// The test a started copy of this binary runs.
    const LOOKUP_HELPER: &str = "windows::broker::tests::broker_lookup_helper_process";

    /// An install or a build laid out on disk the way its installer or cargo
    /// leaves one, with this test binary standing in for `crucible.exe`.
    ///
    /// It is made below the directory this test binary is in, where a hard
    /// link to the running binary is on the same volume.
    struct Layout {
        root: PathBuf,
    }

    impl Layout {
        fn new(name: &str) -> Self {
            static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let binary = std::env::current_exe().expect("this test binary's path");
            let root = binary.parent().expect("a directory").join(format!(
                "crucible-broker-lookup-{name}-{}-{made}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("a directory for the layout");
            Self { root }
        }

        fn at(&self, path: &str) -> PathBuf {
            self.root.join(path)
        }

        /// This test binary, at `path`, as the program the layout runs.
        fn program(&self, path: &str) -> PathBuf {
            let program = self.at(path);
            std::fs::create_dir_all(program.parent().expect("a directory"))
                .expect("a directory for the program");
            let binary = std::env::current_exe().expect("this test binary");
            if std::fs::hard_link(&binary, &program).is_err() {
                std::fs::copy(&binary, &program).expect("a copy of this test binary");
            }
            program
        }

        /// A broker image at `path`, which a lookup that finds it names this way.
        fn broker(&self, path: &str) -> PathBuf {
            let broker = self.at(path);
            std::fs::create_dir_all(broker.parent().expect("a directory"))
                .expect("a directory for the broker");
            std::fs::write(&broker, b"a broker fixture").expect("a broker fixture");
            broker.canonicalize().expect("the broker fixture's path")
        }
    }

    impl Drop for Layout {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// The broker a process started as `launch` finds.
    ///
    /// A real process, because what is being asked is what the operating
    /// system says the running executable is.
    fn found_by_process(layout: &Layout, launch: &Path) -> PathBuf {
        let report = layout.at("lookup-report");
        let status = std::process::Command::new(launch)
            .args(["--exact", LOOKUP_HELPER, "--test-threads=1"])
            .env(LOOKUP_REPORT, &report)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .expect("a copy of this test binary ran");
        assert!(status.success(), "the started copy failed");
        let said = std::fs::read_to_string(&report).expect("the helper's report");
        let Some(found) = said.strip_prefix("found ") else {
            panic!("the started copy found no broker: {said}");
        };
        PathBuf::from(found)
    }

    /// What a copy of this binary started by [`found_by_process`] runs: it
    /// writes down the broker it finds.
    #[test]
    fn broker_lookup_helper_process() {
        let Some(report) = std::env::var_os(LOOKUP_REPORT).map(PathBuf::from) else {
            return;
        };
        let said = match Broker::find(&[]) {
            Ok(broker) => format!("found {}", broker.path().display()),
            Err(refused) => format!("refused {refused}"),
        };
        std::fs::write(&report, said).expect("the report");
    }

    #[test]
    fn a_flat_install_finds_the_broker_beside_the_executable() {
        // How `install.ps1` leaves an install, its alias a copy of the
        // command, and how `cargo install` leaves one.
        for (directory, launch) in [
            ("bin", "bin/crucible.exe"),
            ("bin", "bin/cru.exe"),
            (".cargo/bin", ".cargo/bin/crucible.exe"),
        ] {
            let layout = Layout::new("flat");
            layout.program(&format!("{directory}/crucible.exe"));
            layout.program(&format!("{directory}/cru.exe"));
            let beside = layout.broker(&format!("{directory}/crucible-sandbox-broker.exe"));
            let found = found_by_process(&layout, &layout.at(launch));
            assert_eq!(found, beside, "started as {launch}");
        }
    }

    #[test]
    fn a_build_finds_the_broker_it_built() {
        // `crucible.exe` beside its broker in each profile's directory, and a
        // test binary one directory below it, which is what the second place
        // is for.
        for (program, broker) in [
            (
                "target/debug/crucible.exe",
                "target/debug/crucible-sandbox-broker.exe",
            ),
            (
                "target/release/crucible.exe",
                "target/release/crucible-sandbox-broker.exe",
            ),
            (
                "target/debug/deps/tests-0123.exe",
                "target/debug/crucible-sandbox-broker.exe",
            ),
        ] {
            let layout = Layout::new("build");
            let program = layout.program(program);
            let built = layout.broker(broker);
            let found = found_by_process(&layout, &program);
            assert_eq!(found, built, "started as {}", program.display());
        }
    }

    #[test]
    fn the_held_directory_is_not_resolved_again() {
        let layout = Layout::new("held");
        let started = layout.program("bin/crucible.exe");
        let directory = layout.at("bin").canonicalize().expect("the install");
        let asked = std::cell::Cell::new(0);
        let executable = || {
            asked.set(asked.get() + 1);
            Ok(started.clone())
        };

        let held = Held::new();
        assert_eq!(held.directory(executable), Ok(directory.clone()));
        assert_eq!(held.directory(executable), Ok(directory));
        assert_eq!(asked.get(), 1, "the executable was asked for again");
    }

    #[test]
    fn a_directory_that_could_not_be_resolved_is_each_lookup_s_refusal() {
        let layout = Layout::new("unresolved");
        let program = layout.program("bin/crucible.exe");
        let held = Held::new();
        assert_eq!(
            held.directory(|| Ok(layout.at("bin/gone.exe"))),
            Err("could not resolve where the Crucible executable is")
        );
        assert_eq!(
            held.directory(|| Ok(program)),
            Err("could not resolve where the Crucible executable is"),
            "a later lookup looked somewhere else"
        );
        let unlocated = Held::new();
        assert_eq!(
            unlocated.directory(|| Err(io::Error::other("no executable"))),
            Err("could not locate the Crucible executable")
        );
    }

    #[test]
    fn the_unit_s_own_broker_is_tried_first() {
        // A directory whose name sorts after the broker's, so a lookup that
        // ordered its candidates by path would try the one above first.
        let layout = Layout::new("own-first");
        let own = layout.broker("target/release/crucible-sandbox-broker.exe");
        layout.broker("target/crucible-sandbox-broker.exe");
        let directory = layout.at("target/release");
        assert_eq!(
            candidates(&directory),
            [directory.join(BROKER), layout.at("target").join(BROKER)]
        );
        let found = Broker::first_trusted(candidates(&directory), &[]).expect("a trusted broker");
        assert_eq!(found.path, own);
    }
}
