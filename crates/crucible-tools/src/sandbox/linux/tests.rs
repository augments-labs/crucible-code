//! Host-level conformance probes for the Linux backend.

use std::ffi::{OsStr, OsString};
use std::net::TcpListener;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Ancestry, CallResultKey, CallResultReceipt, SandboxCleanup, SandboxCommand,
    SandboxCredentialHandle, SandboxCredentialProjection, SandboxCredentialProvenance,
    SandboxEnvironment, SandboxFactKind, SandboxFeature, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId, SandboxInvocationMode,
    SandboxLifecycle, SandboxManifest, SandboxManifestEntry, SandboxMode, SandboxNetworkEndpoint,
    SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRead, SandboxRequest, SandboxResourceLimits, SandboxService, SandboxUnreadablePattern,
    ToolId,
};

use crate::LocalSandbox;
use crate::sample::{Sample, symlink};

fn request(sample: &Sample, manifest: SandboxManifest) -> SandboxRequest {
    SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("manifest"),
        crucible_core::SandboxPolicy::standard(&sample.workspace()).expect("policy"),
        manifest,
    )
}

fn command(script: &str) -> SandboxCommand {
    SandboxCommand::new(
        "/bin/sh",
        [OsString::from("-c"), OsString::from(script)],
        SandboxEnvironment::empty(),
    )
    .expect("command")
}

fn direct(program: &str, arguments: impl IntoIterator<Item = OsString>) -> SandboxCommand {
    SandboxCommand::new(program, arguments, SandboxEnvironment::empty()).expect("command")
}

fn finish(mut process: Box<dyn SandboxProcess>) -> (ExitStatus, Vec<u8>, Vec<u8>) {
    let mut stdout = process.take_stdout();
    let mut stderr = process.take_stderr();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut status = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while stdout.is_some() || stderr.is_some() || status.is_none() {
        read_ready(&mut stdout, &mut output);
        read_ready(&mut stderr, &mut errors);
        if status.is_none() {
            status = process.try_wait().expect("wait");
        }
        assert!(Instant::now() < deadline, "sandbox command did not finish");
        thread::sleep(Duration::from_millis(10));
    }
    process.stop().expect("cleanup");
    (status.expect("status"), output, errors)
}

fn read_ready(stream: &mut Option<Box<dyn SandboxOutput>>, bytes: &mut Vec<u8>) {
    let Some(output) = stream else {
        return;
    };
    let mut buffer = [0_u8; 512];
    match output.read_ready(&mut buffer).expect("read output") {
        SandboxRead::Bytes(read) => {
            bytes.extend_from_slice(buffer.get(..read).expect("reported bytes"));
        }
        SandboxRead::Limited {
            retained,
            discarded: _,
        } => {
            bytes.extend_from_slice(buffer.get(..retained).expect("reported bytes"));
        }
        SandboxRead::Pending => {}
        SandboxRead::End => *stream = None,
    }
}

fn wait_for_marker(
    process: &mut dyn SandboxProcess,
    output: &mut Option<Box<dyn SandboxOutput>>,
    marker: &[u8],
) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    while !bytes.windows(marker.len()).any(|window| window == marker) {
        read_ready(output, &mut bytes);
        assert!(process.try_wait().expect("wait").is_none());
        assert!(Instant::now() < deadline, "marker was not emitted in time");
        thread::sleep(Duration::from_millis(10));
    }
    bytes
}

#[test]
fn inline_manifest_files_are_committed_before_the_command_starts() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-manifest-file");
    let manifest = SandboxManifest::new([SandboxManifestEntry::file(
        "inputs/message.txt",
        Box::<[u8]>::from(&b"materialized\n"[..]),
        SandboxFilesystemProvenance::Manifest,
    )
    .expect("entry")])
    .expect("manifest");
    let mut session = service
        .prepare(request(&sample, manifest))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, output, _) = finish(
        session
            .start(command("cat /crucible/manifest/inputs/message.txt"))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(String::from_utf8(output).expect("utf8"), "materialized\n");
}

#[test]
fn explicit_read_only_mounts_are_descriptor_backed() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-manifest-read-only-mount");
    sample.write("source.txt", "mounted contents\n");
    let manifest = SandboxManifest::new([SandboxManifestEntry::mount(
        sample.root().join("source.txt"),
        "mounted/source.txt",
        SandboxFilesystemAccess::ReadOnly,
        SandboxFilesystemProvenance::Manifest,
    )
    .expect("entry")])
    .expect("manifest");
    let mut session = service
        .prepare(request(&sample, manifest))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, output, _) = finish(
        session
            .start(command("cat /crucible/manifest/mounted/source.txt"))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        String::from_utf8(output).expect("utf8"),
        "mounted contents\n"
    );
}

#[test]
fn explicit_writable_directory_mounts_preserve_parent_authority() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-manifest-writable-mount");
    sample.write("shared/existing.txt", "before\n");
    let manifest = SandboxManifest::new([SandboxManifestEntry::mount(
        sample.root().join("shared"),
        "mounted/shared",
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Manifest,
    )
    .expect("entry")])
    .expect("manifest");
    let mut session = service
        .prepare(request(&sample, manifest))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        session
            .start(command(
                "printf 'after\\n' > /crucible/manifest/mounted/shared/generated.txt",
            ))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("shared/generated.txt"))
            .expect("generated host file"),
        "after\n"
    );
}

#[test]
fn writable_effects_stay_private_until_terminal_publication() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-private-writes");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command(
            "printf 'private\n' > delayed.txt; printf 'ready\n'; sleep 0.2",
        ))
        .expect("started command");
    let mut stdout = process.take_stdout();
    let _ = wait_for_marker(process.as_mut(), &mut stdout, b"ready\n");

    assert!(
        !sample.root().join("delayed.txt").exists(),
        "writable effect reached the host before terminal publication"
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = process.try_wait().expect("wait") {
            break status;
        }
        assert!(Instant::now() < deadline, "command did not terminate");
        thread::sleep(Duration::from_millis(10));
    };
    process.stop().expect("cleanup");
    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("delayed.txt")).expect("published file"),
        "private\n"
    );
}

#[test]
fn staging_a_writable_directory_does_not_copy_its_file_contents() {
    let service = LocalSandbox::new();
    service.probe().expect("qualified Linux confinement host");
    let sample = Sample::new("sandbox-overlay-no-full-copy");
    let sparse =
        std::fs::File::create(sample.root().join("large-sparse.bin")).expect("sparse fixture");
    sparse.set_len(64 * 1024 * 1024).expect("sparse length");
    let request = request(&sample, SandboxManifest::empty());
    let sandbox = request.id();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let launch = session.stage(command("exit 0")).expect("staged launch");
    for base in ["/tmp", "/var/tmp"] {
        assert!(
            !PathBuf::from(base)
                .join(format!("crucible-projection-{sandbox}"))
                .join("roots/0/large-sparse.bin")
                .exists(),
            "directory projection copied file contents before GO"
        );
    }
    drop(launch);
}

#[test]
fn cancellation_discards_private_workspace_effects() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-cancelled-writes");
    let request = request(&sample, SandboxManifest::empty());
    let audit = request.audit().clone();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command(
            "printf 'discarded\n' > cancelled.txt; printf 'ready\n'; sleep 30",
        ))
        .expect("started command");
    let mut stdout = process.take_stdout();
    let _ = wait_for_marker(process.as_mut(), &mut stdout, b"ready\n");

    assert!(!sample.root().join("cancelled.txt").exists());
    let stopping = Instant::now();
    process.stop().expect("cancel and clean scope");
    assert!(
        stopping.elapsed() < Duration::from_secs(2),
        "cancellation did not promptly stop the complete sandbox scope"
    );
    assert!(
        !sample.root().join("cancelled.txt").exists(),
        "cancelled command published a private write"
    );
    assert!(
        audit
            .records()
            .expect("audit records")
            .iter()
            .any(|record| {
                matches!(
                    record.fact().kind(),
                    SandboxFactKind::Lifecycle(SandboxLifecycle::RolledBack)
                )
            })
    );
}

#[test]
fn signal_terminated_leader_discards_private_workspace_effects() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-signalled-writes");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        session
            .start(command(
                "printf 'discarded\n' > signalled.txt; kill -TERM $$",
            ))
            .expect("started command"),
    );

    assert!(!status.success(), "leader unexpectedly exited successfully");
    assert!(
        !sample.root().join("signalled.txt").exists(),
        "signal-terminated leader published a private write"
    );
}

#[test]
fn ordinary_nonzero_exit_publishes_valid_workspace_effects() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-nonzero-writes");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        session
            .start(command("printf 'published\n' > nonzero.txt; exit 17"))
            .expect("started command"),
    );

    assert_eq!(status.code(), Some(17));
    assert_eq!(
        std::fs::read_to_string(sample.root().join("nonzero.txt")).expect("published file"),
        "published\n"
    );
}

#[test]
fn ordinary_high_nonzero_exit_is_not_confused_with_signal_termination() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-high-nonzero-writes");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        session
            .start(command("printf 'published\n' > high-nonzero.txt; exit 143"))
            .expect("started command"),
    );

    assert_eq!(status.code(), Some(143));
    assert_eq!(
        std::fs::read_to_string(sample.root().join("high-nonzero.txt")).expect("published file"),
        "published\n"
    );
}

#[test]
fn create_update_delete_rename_and_mode_publish_as_one_terminal_delta() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-terminal-delta");
    sample.write("updated.txt", "before\n");
    sample.write("deleted.txt", "delete me\n");
    sample.write("renamed-before.txt", "rename me\n");
    sample.write("mode.txt", "mode\n");
    let request = request(&sample, SandboxManifest::empty());
    let audit = request.audit().clone();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, errors) = finish(
        session
            .start(command(
                "printf 'after\\n' > updated.txt; \
                 printf 'created\\n' > created.txt; \
                 rm deleted.txt; \
                 mv renamed-before.txt renamed-after.txt; \
                 chmod 640 mode.txt",
            ))
            .expect("started command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(
        std::fs::read_to_string(sample.root().join("updated.txt")).expect("updated file"),
        "after\n"
    );
    assert_eq!(
        std::fs::read_to_string(sample.root().join("created.txt")).expect("created file"),
        "created\n"
    );
    assert!(!sample.root().join("deleted.txt").exists());
    assert!(!sample.root().join("renamed-before.txt").exists());
    assert_eq!(
        std::fs::read_to_string(sample.root().join("renamed-after.txt")).expect("renamed file"),
        "rename me\n"
    );
    assert_eq!(
        std::fs::metadata(sample.root().join("mode.txt"))
            .expect("mode metadata")
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    let lifecycles = audit
        .records()
        .expect("audit records")
        .iter()
        .filter_map(|record| match record.fact().kind() {
            SandboxFactKind::Lifecycle(lifecycle) => Some(*lifecycle),
            _ => None,
        })
        .collect::<Vec<_>>();
    let publication_started = lifecycles
        .iter()
        .position(|state| *state == SandboxLifecycle::PublicationStarted)
        .expect("publication started");
    let published = lifecycles
        .iter()
        .position(|state| *state == SandboxLifecycle::Published)
        .expect("published");
    assert!(publication_started < published);
}

#[test]
fn unsupported_terminal_metadata_refuses_the_complete_private_delta() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-terminal-metadata-refusal");
    let request = request(&sample, SandboxManifest::empty());
    let audit = request.audit().clone();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command(
            "printf 'ordinary\n' > ordinary.txt; \
             printf 'special\n' > special.txt; chmod 4755 special.txt",
        ))
        .expect("started command");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match process.try_wait() {
            Err(_) => break,
            Ok(None) => {}
            Ok(Some(status)) => panic!("unsupported metadata was published after {status}"),
        }
        assert!(
            Instant::now() < deadline,
            "metadata-refusing command did not terminate"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let _ = process.stop();

    assert!(!sample.root().join("ordinary.txt").exists());
    assert!(!sample.root().join("special.txt").exists());
    assert!(
        audit
            .records()
            .expect("audit records")
            .iter()
            .any(|record| {
                matches!(
                    record.fact().kind(),
                    SandboxFactKind::Lifecycle(SandboxLifecycle::RolledBack)
                )
            })
    );
}

#[test]
fn an_external_baseline_conflict_publishes_none_of_the_private_delta() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-publication-conflict");
    sample.write("shared.txt", "baseline\n");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command(
            "printf 'private\\n' > shared.txt; \
             printf 'private new\\n' > private-new.txt; \
             printf 'ready\\n'; sleep 0.2",
        ))
        .expect("started command");
    let mut stdout = process.take_stdout();
    let _ = wait_for_marker(process.as_mut(), &mut stdout, b"ready\n");
    std::fs::write(sample.root().join("shared.txt"), "external\n")
        .expect("external writer fixture");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match process.try_wait() {
            Err(_) => break,
            Ok(None) => {}
            Ok(Some(status)) => panic!("conflicted publication returned {status}"),
        }
        assert!(
            Instant::now() < deadline,
            "conflicted command did not terminate"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let _ = process.stop();
    assert_eq!(
        std::fs::read_to_string(sample.root().join("shared.txt")).expect("external content"),
        "external\n"
    );
    assert!(!sample.root().join("private-new.txt").exists());
}

#[test]
fn complete_workspace_hardlink_groups_keep_one_projected_inode() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-hardlink-group");
    sample.write("first.txt", "before\n");
    std::fs::hard_link(
        sample.root().join("first.txt"),
        sample.root().join("second.txt"),
    )
    .expect("hardlink fixture");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("complete hardlink group is admissible");
    session.materialize().expect("materialized workspace");
    let (status, output, _) = finish(
        session
            .start(command("printf 'after\n' > first.txt; cat second.txt"))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(String::from_utf8(output).expect("utf8"), "after\n");
    let first = std::fs::metadata(sample.root().join("first.txt")).expect("first metadata");
    let second = std::fs::metadata(sample.root().join("second.txt")).expect("second metadata");
    assert_eq!(first.ino(), second.ino());
    assert_eq!(first.nlink(), 2);
}

#[test]
fn a_new_sparse_file_keeps_its_holes_after_terminal_publication() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-sparse-publication");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, errors) = finish(
        session
            .start(command(
                "truncate -s 8388608 sparse.bin; \
                 printf x | dd of=sparse.bin bs=1 seek=8388607 conv=notrunc status=none",
            ))
            .expect("started command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    let metadata = std::fs::metadata(sample.root().join("sparse.bin")).expect("sparse metadata");
    assert_eq!(metadata.len(), 8 * 1024 * 1024);
    assert!(
        metadata.blocks().saturating_mul(512) < 128 * 1024,
        "terminal publication expanded sparse holes into {} allocated bytes",
        metadata.blocks().saturating_mul(512)
    );
}

#[test]
fn dropping_a_staged_launch_refuses_it_before_go_and_completes_cleanup() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-pre-release-refusal");
    let request = request(&sample, SandboxManifest::empty());
    let audit = request.audit().clone();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let launch = session
        .stage(command("printf 'must-not-run\\n' > refused.txt"))
        .expect("staged launch");
    drop(launch);

    assert!(!sample.root().join("refused.txt").exists());
    let facts = audit.records().expect("audit records");
    assert!(facts.iter().any(|record| {
        matches!(
            record.fact().kind(),
            SandboxFactKind::Lifecycle(SandboxLifecycle::Refused)
        )
    }));
    assert!(facts.iter().any(|record| {
        matches!(
            record.fact().kind(),
            SandboxFactKind::Cleanup(SandboxCleanup::Complete)
        )
    }));
    assert!(!facts.iter().any(|record| {
        matches!(
            record.fact().kind(),
            SandboxFactKind::Lifecycle(
                SandboxLifecycle::CommandReleased | SandboxLifecycle::CommandStarted
            )
        )
    }));
}

#[test]
fn background_ownership_precedes_release_and_command_start() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-background-owner-order");
    let context = crate::sample::context_for("manifest");
    let request = request(&sample, SandboxManifest::empty())
        .with_invocation_mode(SandboxInvocationMode::Background)
        .with_call_result_key(context.call_result_key().expect("durable result key"));
    let audit = request.audit().clone();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let mut launch = session.stage(command("exit 0")).expect("staged launch");
    launch.transfer_owner().expect("application owner transfer");
    let mut process = launch.release().expect("released launch");
    let key = context.call_result_key().expect("durable result key");
    process
        .begin_background_acceptance(key)
        .expect("acceptance intent");
    process
        .complete_background_acceptance(CallResultReceipt::from_digest([0x31; 32]))
        .expect("acceptance completion");
    let (status, _, _) = finish(process);
    assert!(status.success(), "{status}");

    let lifecycles = audit
        .records()
        .expect("audit records")
        .iter()
        .filter_map(|record| match record.fact().kind() {
            SandboxFactKind::Lifecycle(lifecycle) => Some(*lifecycle),
            _ => None,
        })
        .collect::<Vec<_>>();
    let position = |lifecycle| {
        lifecycles
            .iter()
            .position(|candidate| *candidate == lifecycle)
            .expect("lifecycle fact")
    };
    assert!(
        position(SandboxLifecycle::OwnerTransferred) < position(SandboxLifecycle::ReleaseIntent)
    );
    assert!(
        position(SandboxLifecycle::ReleaseIntent) < position(SandboxLifecycle::CommandReleased)
    );
    assert!(
        position(SandboxLifecycle::CommandReleased) < position(SandboxLifecycle::CommandStarted)
    );
}

#[test]
fn read_only_background_commands_have_a_durable_lifecycle() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-read-only-background-lifecycle");
    sample.write("input.txt", "read-only input\n");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        [SandboxFilesystemRule::new(
            sample.root().clone(),
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("read-only workspace rule")],
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("read-only policy");
    let context = crate::sample::context_for("read-only-background");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("read-only-background"),
        policy,
        SandboxManifest::empty(),
    )
    .with_invocation_mode(SandboxInvocationMode::Background)
    .with_call_result_key(context.call_result_key().expect("durable result key"));
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let mut launch = session
        .stage(command("cat input.txt"))
        .expect("staged read-only launch");
    launch.transfer_owner().expect("application owner transfer");
    let mut process = launch.release().expect("released read-only launch");
    let key = context.call_result_key().expect("durable result key");
    process
        .begin_background_acceptance(key)
        .expect("read-only acceptance intent");
    process
        .complete_background_acceptance(CallResultReceipt::from_digest([0x32; 32]))
        .expect("read-only acceptance completion");

    let (status, output, errors) = finish(process);

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(output, b"read-only input\n");
}

#[test]
fn background_release_without_an_application_owner_is_refused_before_go() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-background-owner-required");
    let context = crate::sample::context_for("manifest");
    let request = request(&sample, SandboxManifest::empty())
        .with_invocation_mode(SandboxInvocationMode::Background)
        .with_call_result_key(context.call_result_key().expect("durable result key"));
    let audit = request.audit().clone();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let launch = session
        .stage(command("printf 'escaped\n' > ownerless.txt"))
        .expect("staged launch");

    assert!(launch.release().is_err());
    assert!(!sample.root().join("ownerless.txt").exists());
    assert!(
        audit
            .records()
            .expect("audit records")
            .iter()
            .any(|record| {
                matches!(
                    record.fact().kind(),
                    SandboxFactKind::Lifecycle(
                        SandboxLifecycle::Refused | SandboxLifecycle::Quarantined
                    )
                )
            })
    );
}

#[test]
fn writable_transactions_are_globally_serialized_across_disjoint_roots() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let first = Sample::new("sandbox-global-writer-first");
    let second = Sample::new("sandbox-global-writer-second");
    let held = service
        .prepare(request(&first, SandboxManifest::empty()))
        .expect("first writable transaction");

    assert!(matches!(
        service.prepare(request(&second, SandboxManifest::empty())),
        Err(crucible_core::SandboxError::Concurrency)
    ));
    drop(held);
    service
        .prepare(request(&second, SandboxManifest::empty()))
        .expect("writer admitted after lease release");
}

#[test]
fn read_only_mounts_cannot_be_mutated() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-manifest-read-only-refusal");
    sample.write("source.txt", "unchanged\n");
    let manifest = SandboxManifest::new([SandboxManifestEntry::mount(
        sample.root().join("source.txt"),
        "mounted/source.txt",
        SandboxFilesystemAccess::ReadOnly,
        SandboxFilesystemProvenance::Manifest,
    )
    .expect("entry")])
    .expect("manifest");
    let mut session = service
        .prepare(request(&sample, manifest))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        session
            .start(command(
                "printf 'changed\\n' > /crucible/manifest/mounted/source.txt",
            ))
            .expect("started command"),
    );

    assert!(!status.success(), "read-only write unexpectedly succeeded");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("source.txt")).expect("source"),
        "unchanged\n"
    );
}

#[test]
fn replacing_a_writable_file_after_stage_cannot_retarget_publication() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-writable-file-authority");
    let external = PathBuf::from(sample.beside("writable-file"));
    let source = external.join("source.txt");
    std::fs::write(&source, "validated inode\n").expect("source fixture");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem()
            .iter()
            .cloned()
            .chain([SandboxFilesystemRule::new(
                source.clone(),
                SandboxFilesystemAccess::ReadWrite,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("writable file rule")]),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("policy");
    let manifest = SandboxManifest::new([SandboxManifestEntry::mount(
        source.clone(),
        "mounted/source.txt",
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Manifest,
    )
    .expect("entry")])
    .expect("manifest");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("writable-file-authority"),
        policy,
        manifest,
    );
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let launch = session
        .stage(command(
            "printf 'published through file authority\n' > /crucible/manifest/mounted/source.txt",
        ))
        .expect("staged command");
    let validated = external.join("validated.txt");
    std::fs::rename(&source, &validated).expect("rename validated inode");
    std::fs::write(&source, "replacement inode\n").expect("replacement fixture");

    let (status, _, errors) = finish(launch.release().expect("released command"));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(
        std::fs::read_to_string(validated).expect("pinned publication"),
        "published through file authority\n"
    );
    assert_eq!(
        std::fs::read_to_string(source).expect("replacement file"),
        "replacement inode\n"
    );
}

#[test]
fn a_replaced_mount_source_cannot_retarget_the_prepared_descriptor() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-manifest-source-replacement");
    let source = sample.root().join("source.txt");
    sample.write("source.txt", "validated inode\n");
    let manifest = SandboxManifest::new([SandboxManifestEntry::mount(
        source.clone(),
        "mounted/source.txt",
        SandboxFilesystemAccess::ReadOnly,
        SandboxFilesystemProvenance::Manifest,
    )
    .expect("entry")])
    .expect("manifest");
    let mut session = service
        .prepare(request(&sample, manifest))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    std::fs::rename(&source, sample.root().join("validated.txt")).expect("replace source");
    std::fs::write(&source, "replacement inode\n").expect("replacement source");

    let (status, output, _) = finish(
        session
            .start(command("cat /crucible/manifest/mounted/source.txt"))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        String::from_utf8(output).expect("utf8"),
        "validated inode\n"
    );
}

#[test]
fn mount_source_descriptors_do_not_reach_the_untrusted_command() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-manifest-descriptor-closure");
    sample.write("source.txt", "source\n");
    let manifest = SandboxManifest::new([SandboxManifestEntry::mount(
        sample.root().join("source.txt"),
        "mounted/source.txt",
        SandboxFilesystemAccess::ReadOnly,
        SandboxFilesystemProvenance::Manifest,
    )
    .expect("entry")])
    .expect("manifest");
    let mut session = service
        .prepare(request(&sample, manifest))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, output, _) = finish(
        session
            .start(command(
                "for fd in /proc/self/fd/[3-9]*; do readlink \"$fd\" 2>/dev/null || true; done",
            ))
            .expect("started command"),
    );
    let output = String::from_utf8(output).expect("utf8");

    assert!(status.success(), "{status}");
    assert!(!output.contains(&sample.root().to_string_lossy().into_owned()));
    assert!(!output.contains("source.txt"));
}

#[test]
fn replacing_a_workspace_root_after_prepare_cannot_retarget_it() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-workspace-source-replacement");
    sample.write("identity.txt", "validated workspace\n");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let validated = sample.root().with_file_name("validated-inside");
    std::fs::rename(sample.root(), &validated).expect("rename validated workspace");
    std::fs::create_dir(sample.root()).expect("replacement workspace");
    std::fs::write(
        sample.root().join("identity.txt"),
        "replacement workspace\n",
    )
    .expect("replacement file");

    let (status, output, _) = finish(
        session
            .start(command(
                "cat identity.txt; printf 'published through authority\n' > published.txt",
            ))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        String::from_utf8(output).expect("utf8"),
        "validated workspace\n"
    );
    assert_eq!(
        std::fs::read_to_string(validated.join("published.txt")).expect("pinned publication"),
        "published through authority\n"
    );
    assert!(
        !sample.root().join("published.txt").exists(),
        "publication was redirected into the replacement workspace"
    );
}

#[test]
fn workspace_symlinks_cannot_escape_the_mounted_view() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-workspace-symlink-escape");
    let outside = sample.outside("secret.txt", "outside secret\n");
    symlink(&outside, sample.root().join("escape.txt"));
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let (status, output, _) = finish(
        session
            .start(command("cat escape.txt"))
            .expect("started command"),
    );

    assert!(!status.success(), "symlink escape unexpectedly succeeded");
    assert!(!String::from_utf8_lossy(&output).contains("outside secret"));
}

#[test]
fn nested_repository_and_crucible_metadata_stay_read_only_beneath_a_writable_root() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-nested-protected-metadata");
    sample.write("nested/.git/config", "protected\n");
    sample.write("nested/.crucible/auth.json", "credential\n");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let (status, _, _) = finish(
        session
            .start(command(
                "printf 'ordinary\\n' > nested/file.txt; \
                 printf 'changed\\n' > nested/.git/config; \
                 printf 'changed\\n' > nested/.crucible/auth.json",
            ))
            .expect("started command"),
    );

    assert!(
        !status.success(),
        "protected metadata write unexpectedly succeeded"
    );
    assert_eq!(
        std::fs::read_to_string(sample.root().join("nested/file.txt")).expect("ordinary write"),
        "ordinary\n"
    );
    assert_eq!(
        std::fs::read_to_string(sample.root().join("nested/.git/config")).expect("metadata"),
        "protected\n"
    );
    assert_eq!(
        std::fs::read_to_string(sample.root().join("nested/.crucible/auth.json"))
            .expect("credential metadata"),
        "credential\n"
    );
}

#[test]
fn unreadable_rules_mask_only_the_selected_path() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-unreadable-path");
    sample.write("visible.txt", "visible\n");
    sample.write("secret.txt", "secret\n");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        [
            SandboxFilesystemRule::new(
                sample.root().clone(),
                SandboxFilesystemAccess::ReadWrite,
                SandboxFilesystemProvenance::Workspace,
            )
            .expect("workspace rule"),
            SandboxFilesystemRule::new(
                sample.root().join("secret.txt"),
                SandboxFilesystemAccess::Unreadable,
                SandboxFilesystemProvenance::Descendant,
            )
            .expect("unreadable rule"),
        ],
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("unreadable"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let (status, output, errors) = finish(
        session
            .start(command(
                "cat visible.txt; if cat secret.txt 2>/dev/null; then exit 71; fi",
            ))
            .expect("started command"),
    );

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
    assert_eq!(String::from_utf8(output).expect("utf8"), "visible\n");
}

#[test]
fn unreadable_patterns_expand_deterministically_without_hiding_siblings() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-unreadable-patterns");
    sample.write(".env", "root secret\n");
    sample.write("nested/secret.pem", "nested secret\n");
    sample.write("nested/visible.txt", "visible\n");
    let policy = SandboxPolicy::standard(&sample.workspace())
        .expect("base policy")
        .with_unreadable_patterns([
            SandboxUnreadablePattern::new(
                sample.root().join("**/*.pem"),
                SandboxFilesystemProvenance::Descendant,
            )
            .expect("pem pattern"),
            SandboxUnreadablePattern::new(
                sample.root().join("*.env"),
                SandboxFilesystemProvenance::Descendant,
            )
            .expect("env pattern"),
        ])
        .expect("pattern policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("unreadable-patterns"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, output, errors) = finish(
        session
            .start(command(
                "if cat .env 2>/dev/null; then exit 71; fi; \
                 if cat nested/secret.pem 2>/dev/null; then exit 72; fi; \
                 cat nested/visible.txt",
            ))
            .expect("started command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8(output).expect("utf8"), "visible\n");
}

#[test]
fn exact_network_requests_fail_before_materialization_or_spawn() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-network-allowlist-refusal");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let network = SandboxNetworkPolicy::exact(
        [
            SandboxNetworkEndpoint::new("example.com", 443, SandboxNetworkProvenance::User)
                .expect("endpoint"),
        ],
        true,
        false,
    )
    .expect("network policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned(),
        base.working_directory().to_path_buf(),
        network,
        SandboxResourceLimits::default(),
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("exact-network"),
        policy,
        SandboxManifest::empty(),
    );

    assert!(matches!(
        service.prepare(request),
        Err(crucible_core::SandboxError::Unsupported {
            feature: SandboxFeature::NetworkAllowlist
        })
    ));
}

#[test]
fn closed_network_cannot_reach_host_loopback_unix_sockets_dns_or_metadata() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-network-closed");
    let tcp = TcpListener::bind(("127.0.0.1", 0)).expect("host TCP listener");
    let port = tcp.local_addr().expect("listener address").port();
    let socket_path = sample
        .root()
        .parent()
        .expect("sample parent")
        .join("host.sock");
    let _unix = UnixListener::bind(&socket_path).expect("host Unix listener");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let script = r#"
import signal
import socket
import sys

def tcp_refused(address):
    stream = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    stream.settimeout(0.25)
    try:
        return stream.connect_ex(address) != 0
    except OSError:
        return True
    finally:
        stream.close()

if not tcp_refused(("127.0.0.1", int(sys.argv[1]))):
    raise SystemExit("host loopback was reachable")
if not tcp_refused(("169.254.169.254", 80)):
    raise SystemExit("cloud metadata address was reachable")

stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    if stream.connect_ex(sys.argv[2]) == 0:
        raise SystemExit("host Unix socket was reachable")
finally:
    stream.close()

def timed_out(_signal, _frame):
    raise TimeoutError()

signal.signal(signal.SIGALRM, timed_out)
signal.alarm(1)
try:
    socket.getaddrinfo("example.com", 443)
except (OSError, TimeoutError):
    pass
else:
    raise SystemExit("host DNS was usable")
finally:
    signal.alarm(0)
"#;
    let (status, _, errors) = finish(
        session
            .start(direct(
                "/usr/bin/python3",
                [
                    OsString::from("-c"),
                    OsString::from(script),
                    OsString::from(port.to_string()),
                    socket_path.into_os_string(),
                ],
            ))
            .expect("started command"),
    );

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
}

#[test]
fn arbitrary_inheritable_host_descriptors_do_not_reach_the_command() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-inherited-descriptor");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
    let descriptor = listener.as_raw_fd();
    let target = std::fs::read_link(format!("/proc/self/fd/{descriptor}"))
        .expect("listener descriptor target");
    let flags = rustix::io::fcntl_getfd(&listener).expect("descriptor flags");
    rustix::io::fcntl_setfd(&listener, rustix::io::FdFlags::empty())
        .expect("make descriptor inheritable");

    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let started = session.start(command(
        "for fd in /proc/self/fd/[3-9]*; do readlink \"$fd\" 2>/dev/null || true; done",
    ));
    rustix::io::fcntl_setfd(&listener, flags).expect("restore descriptor flags");
    let (status, output, errors) = finish(started.expect("started command"));

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
    assert!(
        !String::from_utf8_lossy(&output).contains(&target.to_string_lossy().to_string()),
        "host listener descriptor reached the sandbox: {}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn explicit_credential_projection_reaches_only_its_named_environment_slot() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-credential-projection");
    let credential = SandboxCredentialProjection::new(
        SandboxCredentialHandle::new(
            "provider/openai/test-account",
            SandboxCredentialProvenance::Account,
        )
        .expect("credential handle"),
        "SANDBOX_TOKEN",
        "credential-value-canary",
    )
    .expect("credential projection");
    let environment =
        SandboxEnvironment::with_credentials([("LANG", OsStr::new("C"))], [credential])
            .expect("environment");
    let command = SandboxCommand::new(
        "/bin/sh",
        [
            OsString::from("-c"),
            OsString::from(
                "test \"$SANDBOX_TOKEN\" = credential-value-canary && \
                 test -z \"${SSH_AUTH_SOCK+x}\" && test \"$HOME\" = /crucible-home",
            ),
        ],
        environment,
    )
    .expect("command");
    let shown = format!("{command:?}");
    assert!(!shown.contains("credential-value-canary"), "{shown}");
    assert!(!shown.contains("provider/openai/test-account"), "{shown}");

    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, errors) = finish(session.start(command).expect("started command"));

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
}

#[test]
fn proc_devices_capabilities_and_nested_user_namespaces_are_minimal() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-kernel-surface");
    let host_pid = std::process::id();
    let script = format!(
        "test ! -e /proc/{host_pid}/status && \
         test \"$(awk '/^NoNewPrivs:/ {{print $2}}' /proc/self/status)\" = 1 && \
         test \"$(awk '/^CapEff:/ {{print $2}}' /proc/self/status)\" = 0000000000000000 && \
         test ! -e /dev/mem && test ! -e /dev/sda && \
         set -- /proc/[0-9]*; test \"$#\" -le 4 && \
         if command -v unshare >/dev/null 2>&1; then ! unshare -U /bin/true 2>/dev/null; fi"
    );
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let (status, _, errors) = finish(session.start(command(&script)).expect("started command"));

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
}

#[test]
fn command_deadline_kills_the_complete_bubblewrap_process_tree() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-deadline-process-tree");
    let marker = format!(
        "crucible-deadline-descendant-{}-{}",
        std::process::id(),
        sample
            .root()
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("sample")
    );
    assert!(live_processes_with_marker(&marker).is_empty());
    let limits = SandboxResourceLimits {
        command_time: Some(Duration::from_secs(1)),
        ..SandboxResourceLimits::default()
    };
    let policy = SandboxPolicy::standard(&sample.workspace())
        .expect("policy")
        .with_limits(limits)
        .expect("limits");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("deadline-tree"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let script = format!(
        "if command -v setsid >/dev/null 2>&1; then \
             setsid /bin/sh -c 'sleep 30; :' {marker} & \
         else \
             /bin/sh -c 'sleep 30; :' {marker} & \
         fi; \
         printf 'ready\n'; wait"
    );
    let mut process = session.start(command(&script)).expect("started command");
    let mut stdout = process.take_stdout();
    let _ = wait_for_marker(process.as_mut(), &mut stdout, b"ready\n");
    assert!(
        !live_processes_with_marker(&marker).is_empty(),
        "marker did not identify the live host descendant"
    );
    thread::sleep(Duration::from_millis(1_100));
    let status = process.try_wait().expect("wait");
    let violation = process.violation();
    process.stop().expect("cleanup");

    assert!(
        status.is_some(),
        "sandbox process tree survived its deadline"
    );
    assert_eq!(
        violation,
        Some(crucible_core::SandboxViolation::CommandTime)
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while !live_processes_with_marker(&marker).is_empty() {
        assert!(
            Instant::now() < deadline,
            "marked descendant remained live after cleanup"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn requested_open_file_limit_is_hard_before_workload_exec() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-open-file-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned(),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            open_files: Some(32),
            ..SandboxResourceLimits::default()
        },
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("open-file-limit"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("supported hard limit");
    session.materialize().expect("materialized workspace");

    let (status, output, errors) = finish(
        session
            .start(command("ulimit -n"))
            .expect("started limited command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8(output).expect("utf8").trim(), "32");
}

#[test]
fn requested_address_space_limit_is_hard_before_workload_exec() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-address-space-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned(),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            memory_bytes: Some(64 * 1024 * 1024),
            ..SandboxResourceLimits::default()
        },
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("address-space-limit"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("supported hard limit");
    session.materialize().expect("materialized workspace");

    let (status, output, errors) = finish(
        session
            .start(command("ulimit -v"))
            .expect("started limited command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8(output).expect("utf8").trim(), "65536");
}

#[test]
fn requested_cpu_limit_terminates_the_workload_scope() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-cpu-limit");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned(),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            cpu_seconds: Some(1),
            ..SandboxResourceLimits::default()
        },
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("cpu-limit"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = service.prepare(request).expect("supported hard limit");
    session.materialize().expect("materialized workspace");
    let started = Instant::now();

    let (status, _, _) = finish(
        session
            .start(command("while :; do :; done"))
            .expect("started limited command"),
    );

    assert!(!status.success(), "CPU-bound workload escaped its ceiling");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "CPU ceiling did not terminate the workload promptly"
    );
}

const CRASH_HELPER_ROOT: &str = "CRUCIBLE_TEST_CRASH_HELPER_ROOT";
const CRASH_HELPER_SANDBOX: &str = "CRUCIBLE_TEST_CRASH_HELPER_SANDBOX";
const CRASH_HELPER_MARKER: &str = "CRUCIBLE_TEST_CRASH_HELPER_MARKER";
const CRASH_HELPER_READY: &str = "CRUCIBLE_TEST_CRASH_HELPER_READY";

#[test]
fn sandbox_crash_helper_process() {
    let (Ok(root), Ok(sandbox), Ok(marker), Ok(ready)) = (
        std::env::var(CRASH_HELPER_ROOT),
        std::env::var(CRASH_HELPER_SANDBOX),
        std::env::var(CRASH_HELPER_MARKER),
        std::env::var(CRASH_HELPER_READY),
    ) else {
        return;
    };
    let workspace = crucible_core::Workspace::open(root).expect("helper workspace");
    let sandbox = SandboxId::parse(&sandbox).expect("helper sandbox identity");
    let policy = SandboxPolicy::standard(&workspace).expect("helper policy");
    let key = CallResultKey::from_digest([0x7c; 32]);
    let request = SandboxRequest::new(
        sandbox,
        Ancestry::new(),
        ToolId::new("crash-helper"),
        policy,
        SandboxManifest::empty(),
    )
    .with_invocation_mode(SandboxInvocationMode::Background)
    .with_call_result_key(key);
    let service = LocalSandbox::new();
    let mut session = service.prepare(request).expect("helper prepare");
    session.materialize().expect("helper materialize");
    let mut launch = session
        .stage(command(&format!(": {marker}; exec sleep 300")))
        .expect("helper stage");
    launch.transfer_owner().expect("helper ownership");
    let mut process = launch.release().expect("helper release");
    process
        .begin_background_acceptance(key)
        .expect("helper acceptance intent");
    process
        .complete_background_acceptance(CallResultReceipt::from_digest([0x7d; 32]))
        .expect("helper acceptance");
    std::fs::write(ready, b"accepted\n").expect("announce durable acceptance");

    loop {
        assert!(
            process.try_wait().expect("helper wait").is_none(),
            "helper workload ended before its host"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn abrupt_host_loss_kills_the_scope_and_the_next_prepare_reconciles_its_wal() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let _serial = super::transaction::TestSerialLease::acquire()
        .expect("serialize the cross-process writer lifecycle");
    let sample = Sample::new("sandbox-host-loss");
    let sandbox = SandboxId::new();
    let marker = format!("crucible-host-loss-{sandbox}");
    let ready = sample.root().join("host-loss-accepted");
    let stage = PathBuf::from(format!("/var/tmp/crucible-projection-{sandbox}"));
    let executable = std::env::current_exe().expect("test executable");
    let mut helper = Command::new(executable)
        .args([
            "--exact",
            "sandbox::linux::tests::sandbox_crash_helper_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_HELPER_ROOT, sample.root())
        .env(CRASH_HELPER_SANDBOX, sandbox.to_string())
        .env(CRASH_HELPER_MARKER, &marker)
        .env(CRASH_HELPER_READY, &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn crash helper");

    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while (!ready.exists() || !stage.exists() || live_processes_with_marker(&marker).is_empty())
        && Instant::now() < ready_deadline
    {
        if helper.try_wait().expect("helper status").is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !ready.exists() || !stage.exists() || live_processes_with_marker(&marker).is_empty() {
        let _ = helper.kill();
        let _ = helper.wait();
        panic!("crash helper did not reach its released lifecycle");
    }

    helper.kill().expect("abrupt helper loss");
    helper.wait().expect("reap crash helper");
    let reap_deadline = Instant::now() + Duration::from_secs(3);
    while !live_processes_with_marker(&marker).is_empty() && Instant::now() < reap_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        live_processes_with_marker(&marker).is_empty(),
        "a confined workload survived abrupt host loss"
    );

    let recovered = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("next prepare reconciles the abandoned WAL");
    assert!(!stage.exists(), "the abandoned lifecycle was not settled");
    drop(recovered);
}

fn live_processes_with_marker(marker: &str) -> Vec<u32> {
    let marker = marker.as_bytes();
    let Ok(processes) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    processes
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let command = std::fs::read(entry.path().join("cmdline")).ok()?;
            command
                .windows(marker.len())
                .any(|window| window == marker)
                .then_some(pid)
        })
        .collect()
}
