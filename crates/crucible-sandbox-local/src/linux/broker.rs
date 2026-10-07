//! Descriptor-pinned broker discovery and authenticated wait-status channel.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crucible_sandbox::SandboxError;
use crucible_sandbox_broker::{
    CANCEL_FRAME, GO_FRAME, READY_FRAME, REFUSED_DESCRIPTOR_CLOSURE, REFUSED_FRAME, REFUSED_SCAN,
};

use super::super::process::Canceller;

const MAX_BROKER_BYTES: u64 = 16 * 1024 * 1024;

/// Whether a broker image, or a directory above it, can be rewritten only by
/// root or by the user running Crucible.
///
/// Unlike a system package the image may belong to the user, since a path
/// under their home is the normal case. Group write is refused even for the
/// user's own group: nothing here can prove that group is private to them, and
/// a shared group would let any member rewrite namespace PID 1 in place.
fn trusted_owner(uid: u32, mode: u32) -> bool {
    (uid == 0 || uid == rustix::process::getuid().as_raw()) && mode & 0o022 == 0
}

/// Where a broker image is looked for.
#[derive(Debug, Clone, Copy)]
enum Place {
    /// The directory the running executable is in.
    BesideExecutable,
    /// The directory above that one, where a build puts its helpers.
    AboveExecutable,
}

impl Place {
    /// Which rule put a candidate where it was looked for.
    const fn said(self) -> &'static str {
        match self {
            Self::BesideExecutable => "the broker beside the crucible executable",
            Self::AboveExecutable => "the broker in the directory above the crucible executable",
        }
    }
}

/// One opened broker image whose descriptor is mounted into the namespace.
pub(super) struct Broker {
    path: PathBuf,
    image: File,
}

impl Broker {
    pub(super) fn find(excluded: &[&Path]) -> Result<Self, SandboxError> {
        let executable = std::env::current_exe()
            .map_err(|_| unavailable("could not locate the Crucible executable"))?;
        let mut candidates = Vec::new();
        if let Some(parent) = executable.parent() {
            candidates.push((
                Place::BesideExecutable,
                parent.join("crucible-sandbox-broker"),
            ));
            if let Some(build_root) = parent.parent() {
                candidates.push((
                    Place::AboveExecutable,
                    build_root.join("crucible-sandbox-broker"),
                ));
            }
        }
        candidates.sort_by(|(_, one), (_, other)| one.cmp(other));
        candidates.dedup_by(|(_, one), (_, other)| one == other);
        Self::first_trusted(candidates, excluded)
    }

    fn first_trusted(
        candidates: Vec<(Place, PathBuf)>,
        excluded: &[&Path],
    ) -> Result<Self, SandboxError> {
        let mut refused = Vec::with_capacity(candidates.len());
        for (place, candidate) in candidates {
            match Self::pin(&candidate, excluded) {
                Ok(broker) => return Ok(broker),
                Err(reason) => refused.push((place, reason)),
            }
        }
        Err(none_trusted(&refused))
    }

    /// Bind one candidate by descriptor, or say what disqualified it.
    ///
    /// The broker is namespace PID 1: it applies the resource limits, ends the
    /// process tree and drives the scan that decides what is published back.
    /// Like Bubblewrap it may only come from a path no other unprivileged user
    /// can rewrite. Each refusal carries its own sentence because the two
    /// candidates fail for different reasons, and a caller who is told only
    /// that no broker was found cannot tell a missing build from a checkout
    /// whose directory mode lets a group member replace the image.
    fn pin(candidate: &Path, excluded: &[&Path]) -> Result<Self, &'static str> {
        let Ok(path) = candidate.canonicalize() else {
            return Err("no file stands at that path");
        };
        if excluded.iter().any(|root| path.starts_with(root)) {
            return Err("it lies inside the tree this sandbox is confining");
        }
        let Ok(metadata) = path.metadata() else {
            return Err("its metadata could not be read");
        };
        if !metadata.is_file() {
            return Err("it is not a regular file");
        }
        if metadata.len() == 0 {
            return Err("it is empty");
        }
        if metadata.len() > MAX_BROKER_BYTES {
            return Err("it is larger than a broker image may be");
        }
        if !trusted_owner(metadata.uid(), metadata.permissions().mode()) {
            return Err("another user can rewrite the image itself");
        }
        if !super::probe::trusted_parent_chain_owned_by(&path, |uid, _, mode| {
            trusted_owner(uid, mode)
        }) {
            return Err("a directory above it is writable by a group or by everyone");
        }
        let Ok(image) = File::open(&path) else {
            return Err("it could not be opened");
        };
        let Ok(opened) = image.metadata() else {
            return Err("the opened image's metadata could not be read");
        };
        if opened.len() != metadata.len()
            || opened.ino() != metadata.ino()
            || opened.dev() != metadata.dev()
            || !trusted_owner(opened.uid(), opened.permissions().mode())
        {
            return Err("it changed between being checked and being opened");
        }
        Ok(Self { path, image })
    }

    /// A broker that is never executed, for tests that read the plan.
    ///
    /// `build` binds the image by descriptor and never names it, so any open
    /// file stands in — which lets a plan be asserted on a machine that has no
    /// installed broker to pin.
    #[cfg(test)]
    pub(super) const fn unexecuted(path: PathBuf, image: File) -> Self {
        Self { path, image }
    }

    pub(super) fn descriptor(&self) -> RawFd {
        self.image.as_raw_fd()
    }
}

impl std::fmt::Debug for Broker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Broker")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// One private socket pair; only the broker side crosses the namespace.
pub(super) struct StatusChannel {
    reader: UnixStream,
    writer: Option<UnixStream>,
}

impl StatusChannel {
    pub(super) fn pair() -> io::Result<Self> {
        let (reader, writer) = UnixStream::pair()?;
        Ok(Self {
            reader,
            writer: Some(writer),
        })
    }

    pub(super) fn descriptor(&self) -> io::Result<RawFd> {
        self.writer
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .ok_or_else(|| io::Error::other("broker status writer is already closed"))
    }

    pub(super) fn close_writer(&mut self) {
        self.writer.take();
    }

    pub(super) fn attest_ready(&mut self) -> io::Result<()> {
        let mut ready = [0_u8; READY_FRAME.len()];
        self.reader.read_exact(&mut ready)?;
        if ready == REFUSED_FRAME {
            let mut reason = [0_u8; 1];
            self.reader.read_exact(&mut reason)?;
            return Err(io::Error::other(match reason.first().copied() {
                Some(REFUSED_SCAN) => {
                    "sandbox broker refused the bounded pre-release semantic scan"
                }
                Some(REFUSED_DESCRIPTOR_CLOSURE) => {
                    "sandbox broker could not close undeclared descriptors before release"
                }
                _ => "sandbox broker returned an unknown pre-release refusal",
            }));
        }
        if ready != READY_FRAME {
            return Err(io::Error::other(
                "sandbox broker did not attest readiness before release",
            ));
        }
        Ok(())
    }

    /// A stop a command's status task hands the broker, on a thread of its
    /// own, before it kills the launcher.
    ///
    /// Killing Bubblewrap alone does not end the PID namespace: the broker,
    /// started in its own session, outlives it together with the workload. The
    /// cancellation frame makes the broker kill the workload and write its wait
    /// status, and the wait keeps the launcher alive until that report has
    /// left, or until the budget ends and the kill proceeds regardless.
    ///
    /// An explicit stop writes the same frame on its own clone of this socket.
    /// The two writers share no lock: a stream socket delivers each eight-byte
    /// frame whole, and the broker acts on the first frame it reads, so a
    /// second one changes nothing.
    pub(super) fn canceller(&self) -> io::Result<Canceller> {
        let channel = self.reader.try_clone()?;
        Ok(Box::new(move |leader| {
            (&channel).write_all(&CANCEL_FRAME)?;
            (&channel).flush()?;
            await_launcher_exit(leader)
        }))
    }

    pub(super) fn send_go(&mut self) -> io::Result<()> {
        self.reader.write_all(&GO_FRAME)?;
        self.reader.flush()
    }

    pub(super) fn into_stream(self) -> UnixStream {
        self.reader
    }
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}

/// Name every candidate and why it was refused.
///
/// Both places are searched before this is reached, so reporting only the
/// last one would hide the reason that applies to the reader's own layout.
/// Each is named by the rule that looked there rather than by its path: the
/// reason reaches `crucible sandbox inspect`, whose report is pasted into
/// issues, and a path to the executable is a path into somebody's home.
fn none_trusted(refused: &[(Place, &'static str)]) -> SandboxError {
    let mut reason =
        String::from("no trusted crucible-sandbox-broker executable was found; refused:");
    if refused.is_empty() {
        reason.push_str(" no path was searched");
    }
    for (place, why) in refused {
        reason.push_str("\n  ");
        reason.push_str(place.said());
        reason.push_str(" — ");
        reason.push_str(why);
    }
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}

/// How long a cancelled broker may take to end its workload and exit.
///
/// The cancel waits on a thread of its own and holds no lock the command's
/// status is read under, so a status asked for meanwhile answers at once. A
/// stop meanwhile kills and reaps the launcher itself, which ends this wait at
/// its next look. Reading the broker's report, which includes its scan of the
/// projection, is bounded separately by the protocol's own ceilings.
const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// How often the cancel looks for the launcher's exit.
const CANCEL_POLL: Duration = Duration::from_millis(5);

/// Waits, within the cancellation budget, for the launcher to exit.
///
/// The exit is observed without reaping so the process owner still collects
/// the status. A launcher that outlives the budget is reported as timed out
/// and left to the kill.
fn await_launcher_exit(leader: u32) -> io::Result<()> {
    use rustix::process::{WaitId, WaitIdOptions};

    let raw = i32::try_from(leader)
        .map_err(|_| io::Error::other("launcher process id does not fit this platform"))?;
    let pid = rustix::process::Pid::from_raw(raw)
        .ok_or_else(|| io::Error::other("launcher process id cannot be observed"))?;
    let options = WaitIdOptions::NOHANG | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT;
    let expired = Instant::now() + CANCEL_GRACE;
    while rustix::process::waitid(WaitId::Pid(pid), options)?.is_none() {
        if Instant::now() >= expired {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox broker did not report within the cancellation budget",
            ));
        }
        std::thread::sleep(CANCEL_POLL);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broker_image_is_trusted_only_where_no_other_user_can_rewrite_it() {
        let me = rustix::process::getuid().as_raw();
        assert!(trusted_owner(0, 0o755));
        assert!(trusted_owner(me, 0o755));
        assert!(trusted_owner(me, 0o700));
        assert!(
            !trusted_owner(me, 0o775),
            "the user's own group cannot be proven private"
        );
        assert!(
            !trusted_owner(0, 0o775),
            "root's file, but any group member"
        );
        assert!(!trusted_owner(me, 0o777));
        assert!(
            !trusted_owner(0, 0o1777),
            "a sticky world-writable directory"
        );
        assert!(!trusted_owner(me.wrapping_add(1), 0o755));
    }

    #[test]
    fn a_broker_image_under_a_world_writable_directory_is_refused() {
        let sample = crate::sample::Sample::new("sandbox-broker-untrusted-parent");
        let image = sample.root().join("crucible-sandbox-broker");
        std::fs::write(&image, b"#!/bin/sh\nexit 0\n").expect("broker fixture");
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755))
            .expect("fixture mode");
        std::fs::set_permissions(sample.root(), std::fs::Permissions::from_mode(0o777))
            .expect("world-writable parent");
        let refused = Broker::first_trusted(vec![(Place::BesideExecutable, image.clone())], &[]);
        let Err(SandboxError::BackendUnavailable { reason }) = refused else {
            panic!("a broker anyone can replace was accepted");
        };
        assert!(
            reason.contains("a directory above it is writable by a group or by everyone"),
            "the refusal did not say which check turned the image down: {reason}"
        );
        assert!(
            reason.contains("the broker beside the crucible executable"),
            "the refusal did not say which candidate it turned down: {reason}"
        );
        assert!(
            !reason.contains(&sample.root().display().to_string()),
            "the refusal named the path it turned down: {reason}"
        );
    }

    #[test]
    fn a_broker_under_a_group_writable_directory_says_so_rather_than_only_that_none_was_found() {
        // A checkout created under `umask 002` has group-writable directories,
        // and every candidate then sits under one. Before the reason was
        // carried out, the caller was told only that no broker was available,
        // which reads as a missing build rather than a directory mode.
        let sample = crate::sample::Sample::new("sandbox-broker-group-writable-parent");
        let image = sample.root().join("crucible-sandbox-broker");
        std::fs::write(&image, b"#!/bin/sh\nexit 0\n").expect("broker fixture");
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755))
            .expect("fixture mode");
        std::fs::set_permissions(sample.root(), std::fs::Permissions::from_mode(0o775))
            .expect("group-writable parent");
        let Err(SandboxError::BackendUnavailable { reason }) =
            Broker::first_trusted(vec![(Place::AboveExecutable, image)], &[])
        else {
            panic!("a broker any group member can replace was accepted");
        };
        assert!(
            reason.contains("a directory above it is writable by a group or by everyone"),
            "a group-writable parent was not named as the reason: {reason}"
        );
    }

    #[test]
    fn a_broker_that_was_never_built_says_the_path_is_empty_rather_than_untrusted() {
        let sample = crate::sample::Sample::new("sandbox-broker-absent");
        let absent = sample.root().join("crucible-sandbox-broker");
        let Err(SandboxError::BackendUnavailable { reason }) =
            Broker::first_trusted(vec![(Place::BesideExecutable, absent.clone())], &[])
        else {
            panic!("a broker that does not exist was accepted");
        };
        assert!(
            reason.contains("no file stands at that path"),
            "an absent broker was not told apart from an untrusted one: {reason}"
        );
        assert!(
            reason.contains("the broker beside the crucible executable"),
            "the refusal did not say which candidate it looked for: {reason}"
        );
        assert!(
            !reason.contains(&sample.root().display().to_string()),
            "the refusal named the path it looked at: {reason}"
        );
    }

    /// The file a copy of this test binary, started by [`found_by_process`],
    /// writes the broker it found to.
    const LOOKUP_REPORT: &str = "CRUCIBLE_TEST_BROKER_LOOKUP_REPORT";

    /// The test a started copy of this binary runs.
    const LOOKUP_HELPER: &str = "linux::broker::tests::broker_lookup_helper_process";

    /// An install or a build laid out on disk the way its installer or cargo
    /// leaves one, with this test binary standing in for `crucible`.
    ///
    /// It is made below the directory this test binary is in, not under the
    /// temporary directory: a broker is trusted only below directories nobody
    /// else can write to, which `/tmp` is not, and a hard link to the running
    /// binary needs the same file system.
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

        /// A broker image at `path`, which a lookup that finds it names this way.
        fn broker(&self, path: &str) -> PathBuf {
            let broker = self.at(path);
            directory(broker.parent().expect("a directory"));
            std::fs::write(&broker, b"#!/bin/sh\nexit 0\n").expect("a broker fixture");
            std::fs::set_permissions(&broker, std::fs::Permissions::from_mode(0o755))
                .expect("the broker fixture's mode");
            broker.canonicalize().expect("the broker fixture's path")
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

    /// The broker a process started as `launch` finds, once it has found one
    /// as it started and `meanwhile` has happened since.
    ///
    /// A real process, because what is being asked is what the operating
    /// system says the running executable is, and the process started through
    /// a link answers that differently on each platform.
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
                panic!("the started copy never looked for its broker");
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
            panic!("the started copy found no broker: {said}");
        };
        PathBuf::from(found)
    }

    /// What a copy of this binary started by [`found_by_process`] runs: it
    /// looks for its broker as it starts, waits to be told to go on, and
    /// writes down the broker it finds then.
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
            Ok(broker) => format!("found {}", broker.path.display()),
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
        // `crucible` beside its broker in each profile's directory, and a test
        // binary one directory below it, which is what the second place is
        // for.
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
