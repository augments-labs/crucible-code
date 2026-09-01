//! Host-level conformance probes for the Linux backend.

use std::ffi::OsString;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Ancestry, SandboxCommand, SandboxEnvironment, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId, SandboxManifest,
    SandboxManifestEntry, SandboxMode, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy,
    SandboxProcess, SandboxRead, SandboxRequest, SandboxService, ToolId,
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
        SandboxRead::Pending => {}
        SandboxRead::End => *stream = None,
    }
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
    std::fs::rename(
        sample.root(),
        sample.root().with_file_name("validated-inside"),
    )
    .expect("rename validated workspace");
    std::fs::create_dir(sample.root()).expect("replacement workspace");
    std::fs::write(
        sample.root().join("identity.txt"),
        "replacement workspace\n",
    )
    .expect("replacement file");

    let (status, output, _) = finish(
        session
            .start(command("cat identity.txt"))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        String::from_utf8(output).expect("utf8"),
        "validated workspace\n"
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
fn nested_repository_metadata_stays_read_only_beneath_a_writable_root() {
    let service = LocalSandbox::new();
    if service.probe().is_err() {
        return;
    }
    let sample = Sample::new("sandbox-nested-protected-metadata");
    sample.write("nested/.git/config", "protected\n");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let (status, _, _) = finish(
        session
            .start(command(
                "printf 'ordinary\\n' > nested/file.txt; \
                 printf 'changed\\n' > nested/.git/config",
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
        Default::default(),
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
