//! Discovery of the packaged pre-Seatbelt launcher.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crucible_sandbox::SandboxError;

const MAX_BROKER_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct Broker {
    path: PathBuf,
}

impl Broker {
    pub(super) fn find(excluded: &[&Path]) -> Result<Self, SandboxError> {
        let executable = std::env::current_exe()
            .map_err(|_| unavailable("could not locate the Crucible executable"))?;
        let mut candidates = Vec::new();
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("crucible-sandbox-broker"));
            if let Some(build_root) = parent.parent() {
                candidates.push(build_root.join("crucible-sandbox-broker"));
            }
        }
        candidates.sort();
        candidates.dedup();

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
                || !trusted_owner(metadata.uid(), metadata.permissions().mode())
                || !trusted_parent_chain(&path)
            {
                continue;
            }
            return Ok(Self { path });
        }
        Err(unavailable(
            "the packaged crucible-sandbox-broker executable is unavailable or writable by another user",
        ))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

fn trusted_owner(uid: u32, mode: u32) -> bool {
    (uid == 0 || uid == rustix::process::getuid().as_raw()) && mode & 0o022 == 0
}

fn trusted_parent_chain(path: &Path) -> bool {
    path.parent().is_some_and(|parent| {
        parent.ancestors().all(|directory| {
            directory.symlink_metadata().is_ok_and(|metadata| {
                metadata.is_dir() && trusted_owner(metadata.uid(), metadata.permissions().mode())
            })
        })
    })
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn only_root_or_the_current_user_may_own_a_non_shared_launcher() {
        let me = rustix::process::getuid().as_raw();
        assert!(trusted_owner(0, 0o755));
        assert!(trusted_owner(me, 0o700));
        assert!(!trusted_owner(me, 0o775));
        assert!(!trusted_owner(me.wrapping_add(1), 0o755));
    }

    /// The file a copy of this test binary, started by [`found_by_process`],
    /// writes the launcher it found to.
    const LOOKUP_REPORT: &str = "CRUCIBLE_TEST_BROKER_LOOKUP_REPORT";

    /// The test a started copy of this binary runs.
    const LOOKUP_HELPER: &str = "macos::broker::tests::broker_lookup_helper_process";

    /// An install or a build laid out on disk the way its installer or cargo
    /// leaves one, with this test binary standing in for `crucible`.
    ///
    /// It is made below the directory this test binary is in, not under the
    /// temporary directory: a launcher is trusted only below directories
    /// nobody else can write to, and a hard link to the running binary needs
    /// the same file system.
    struct Layout {
        root: PathBuf,
    }

    impl Layout {
        fn new(name: &str) -> Self {
            static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let binary = std::env::current_exe()
                .and_then(|binary| binary.canonicalize())
                .expect("this test binary's path");
            let root = binary.parent().expect("a directory").join(format!(
                "crucible-broker-lookup-{name}-{}-{made}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            directory(&root);
            Self { root }
        }

        fn at(&self, path: &str) -> PathBuf {
            self.root.join(path)
        }

        /// This test binary, at `path`, as the program the layout runs.
        fn program(&self, path: &str) -> PathBuf {
            let program = self.at(path);
            directory(program.parent().expect("a directory"));
            let binary = std::env::current_exe().expect("this test binary");
            if std::fs::hard_link(&binary, &program).is_err() {
                std::fs::copy(&binary, &program).expect("a copy of this test binary");
            }
            program
        }

        /// A launcher at `path`, which a lookup that finds it names this way.
        fn broker(&self, path: &str) -> PathBuf {
            let broker = self.at(path);
            directory(broker.parent().expect("a directory"));
            std::fs::write(&broker, b"#!/bin/sh\nexit 0\n").expect("a launcher fixture");
            std::fs::set_permissions(&broker, std::fs::Permissions::from_mode(0o755))
                .expect("the launcher fixture's mode");
            broker.canonicalize().expect("the launcher fixture's path")
        }

        /// A symbolic link at `path` to `target`.
        fn link(&self, path: &str, target: &str) {
            let link = self.at(path);
            directory(link.parent().expect("a directory"));
            crate::sample::symlink(target, link);
        }

        /// What `path` is replaced, in one rename, by a link to `target`, the
        /// way an installer activates a release.
        fn relink(&self, path: &str, target: &str) {
            let next = self.at(&format!("{path}.next"));
            crate::sample::symlink(target, &next);
            std::fs::rename(&next, self.at(path)).expect("the link replaced");
        }
    }

    impl Drop for Layout {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A directory, and those above it, that only their owner can write to,
    /// whatever the umask of the run.
    fn directory(path: &Path) {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(path)
            .expect("a directory for the layout");
    }

    /// The launcher a process started as `launch` finds, once it has looked
    /// for one as it started and `meanwhile` has happened since.
    ///
    /// A real process, because what is being asked is what the operating
    /// system says the running executable is, and macOS answers a process
    /// started through a link with the link's path.
    fn found_by_process(layout: &Layout, launch: &Path, meanwhile: impl FnOnce()) -> PathBuf {
        let report = layout.at("lookup-report");
        let ready = report.with_extension("ready");
        let go = report.with_extension("go");
        let mut child = std::process::Command::new(launch)
            .args(["--exact", LOOKUP_HELPER, "--test-threads=1"])
            .env(LOOKUP_REPORT, &report)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("a copy of this test binary started");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready.exists() {
            if child.try_wait().expect("the helper's status").is_some() || Instant::now() > deadline
            {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the started copy never looked for its launcher");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        meanwhile();
        std::fs::write(&go, b"").expect("the go-ahead");
        assert!(
            child.wait().expect("the helper's status").success(),
            "the started copy failed"
        );
        let said = std::fs::read_to_string(&report).expect("the helper's report");
        let Some(found) = said.strip_prefix("found ") else {
            panic!("the started copy found no launcher: {said}");
        };
        PathBuf::from(found)
    }

    /// What a copy of this binary started by [`found_by_process`] runs: it
    /// looks for its launcher as it starts, waits to be told to go on, and
    /// writes down the launcher it finds then.
    #[test]
    fn broker_lookup_helper_process() {
        let Some(report) = std::env::var_os(LOOKUP_REPORT).map(PathBuf::from) else {
            return;
        };
        let started = Broker::find(&[]);
        std::fs::write(report.with_extension("ready"), b"").expect("the readiness mark");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !report.with_extension("go").exists() {
            assert!(Instant::now() < deadline, "never told to go on");
            std::thread::sleep(Duration::from_millis(10));
        }
        let said = match Broker::find(&[]) {
            Ok(broker) => format!("found {}", broker.path().display()),
            Err(refused) => format!("refused {refused}"),
        };
        drop(started);
        std::fs::write(&report, said).expect("the report");
    }

    #[test]
    fn an_old_process_finds_its_own_broker_after_a_new_unit_activates() {
        // Started through the link to the active release, and through the
        // command's link in the directory the installer was given, which is
        // how a user starts it.
        for launch in [".crucible-install/current/crucible", "crucible"] {
            let layout = Layout::new("unit");
            layout.program(".crucible-install/releases/0.46.0/crucible");
            let own = layout.broker(".crucible-install/releases/0.46.0/crucible-sandbox-broker");
            layout.program(".crucible-install/releases/0.46.1/crucible");
            layout.broker(".crucible-install/releases/0.46.1/crucible-sandbox-broker");
            layout.link(".crucible-install/current", "releases/0.46.0");
            layout.link("crucible", ".crucible-install/current/crucible");

            let found = found_by_process(&layout, &layout.at(launch), || {
                layout.relink(".crucible-install/current", "releases/0.46.1");
            });
            assert_eq!(found, own, "started as {launch}");
        }
    }

    #[test]
    fn a_flat_install_finds_the_broker_beside_the_executable() {
        // How `install.sh` before release units left an install, started by
        // its name and by its alias, and how `cargo install` leaves one.
        for (program, launch) in [
            ("bin/crucible", "bin/crucible"),
            ("bin/crucible", "bin/cru"),
            (".cargo/bin/crucible", ".cargo/bin/crucible"),
        ] {
            let layout = Layout::new("flat");
            layout.program(program);
            layout.link("bin/cru", "crucible");
            let beside = layout.broker(&format!("{program}-sandbox-broker"));
            let found = found_by_process(&layout, &layout.at(launch), || {});
            assert_eq!(found, beside, "started as {launch}");
        }
    }

    #[test]
    fn a_build_finds_the_broker_it_built() {
        // `crucible` beside its launcher in each profile's directory, and a
        // test binary one directory below it, which is what the second place
        // is for.
        for (program, broker) in [
            (
                "target/debug/crucible",
                "target/debug/crucible-sandbox-broker",
            ),
            (
                "target/release/crucible",
                "target/release/crucible-sandbox-broker",
            ),
            (
                "target/debug/deps/tests-0123",
                "target/debug/crucible-sandbox-broker",
            ),
        ] {
            let layout = Layout::new("build");
            let program = layout.program(program);
            let built = layout.broker(broker);
            let found = found_by_process(&layout, &program, || {});
            assert_eq!(found, built, "started as {}", program.display());
        }
    }

    #[test]
    fn a_process_from_a_flat_install_keeps_its_broker_once_the_install_moves_to_release_units() {
        let layout = Layout::new("migrated");
        let program = layout.program("bin/crucible");
        let flat = layout.broker("bin/crucible-sandbox-broker");
        layout.program("bin/.crucible-install/releases/0.46.0/crucible");
        layout.broker("bin/.crucible-install/releases/0.46.0/crucible-sandbox-broker");
        layout.link("bin/.crucible-install/current", "releases/0.46.0");

        let found = found_by_process(&layout, &program, || {
            layout.relink("bin/crucible", ".crucible-install/current/crucible");
        });
        assert_eq!(found, flat);
    }
}
