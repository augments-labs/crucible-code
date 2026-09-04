//! What a confined command cannot reach, asked of the running command.
//!
//! The mount plan and the argument list say what was requested. These start a
//! real command and ask the kernel what it ended up with: which mounts would
//! honour a setuid bit, what the command can see of the host's processes, and
//! what a writable root lets it make.

use crucible_core::{SandboxManifest, SandboxService};

use super::tests::{command, finish, request};
use crate::LocalSandbox;
use crate::sample::{Sample, skipped_without_enforcement};

/// One line of `/proc/self/mountinfo`: where it is mounted and how.
///
/// The fields before the options are fixed, so the two this needs are read by
/// position rather than by matching the whole line, and a line too short to
/// hold them is not a mount this can judge.
fn mount_points(mountinfo: &str) -> Vec<(&str, &str)> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().skip(4);
            Some((fields.next()?, fields.next()?))
        })
        .collect()
}

/// The device mounts Bubblewrap makes one by one inside the sandbox's `/dev`.
///
/// They are the only mounts allowed to carry device authority, because they are
/// the devices: a ceiling that forbade `dev` here would forbid `/dev/null`.
/// `/dev/pts` is the terminals themselves, and is mounted `noexec` besides.
const BOUND_DEVICES: [&str; 7] = [
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/pts",
];

#[test]
fn nothing_the_command_can_see_would_honour_a_setuid_bit() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-mount-authority");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    let (status, output, errors) = finish(
        session
            .start(command("cat /proc/self/mountinfo"))
            .expect("started command"),
    );

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
    let mountinfo = String::from_utf8(output).expect("utf8");
    let mounts = mount_points(&mountinfo);
    assert!(!mounts.is_empty(), "{mountinfo}");
    // The writable projection is Bubblewrap's overlay, and Bubblewrap takes no
    // mount flags for it. It is named here rather than passed over, because the
    // exception is the finding: what stands in for the flag is that this user
    // namespace maps no privileged identity, so a file owned by a uid it cannot
    // see cannot become that uid. The two tests below probe exactly that.
    let projection = sample.workspace().root().display().to_string();
    for (point, options) in mounts {
        if point.starts_with(&projection) {
            continue;
        }
        assert!(
            options.split(',').any(|option| option == "nosuid"),
            "{point} would honour a setuid bit: {options}"
        );
        assert!(
            BOUND_DEVICES.contains(&point) || options.split(',').any(|option| option == "nodev"),
            "{point} carries device authority: {options}"
        );
    }
}

#[test]
fn a_setuid_bit_a_confined_command_sets_itself_grants_it_nothing() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-setuid");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    // A shell it owns, marked setuid, run: the identity on the other side is
    // the identity it started with. The mode is set before the identities are
    // read so a kernel that honoured the bit would be read here rather than
    // argued about, and taken off before the command ends because a setuid file
    // left in a writable root is metadata this sandbox refuses to publish —
    // which is a different rule, proved next door, and not the one under test.
    let (status, output, errors) = finish(
        session
            .start(command(
                "cp /bin/sh ./climb && chmod 4755 ./climb && \
                 ./climb -c 'awk \"/^Uid:/ { print \\$2, \\$3, \\$4 }\" /proc/self/status'; \
                 outcome=$?; chmod 0755 ./climb; exit \"$outcome\"",
            ))
            .expect("started command"),
    );

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
    let line = String::from_utf8(output).expect("utf8");
    let mut identities = line.split_whitespace();
    // Real, effective and saved, in that order: the three the bit would have
    // moved apart.
    let real = identities.next().expect("a real identity").to_owned();
    let rest: Vec<&str> = identities.collect();
    assert_eq!(rest.len(), 2, "{line}");
    assert!(
        rest.iter().all(|identity| *identity == real),
        "a setuid bit moved the command's identity: {real} {rest:?}"
    );
}

#[test]
fn a_writable_root_does_not_let_a_confined_command_make_a_device() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-mknod");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");

    // The disc the host boots from, asked for by number. Making the node is
    // what the kernel refuses; nothing here has to know whether reading it
    // would have worked.
    let (status, _, errors) = finish(
        session
            .start(command("mknod ./disc b 259 0"))
            .expect("started command"),
    );

    assert!(!status.success(), "a confined command made a device node");
    assert!(!errors.is_empty(), "the refusal was silent");
}

#[test]
fn a_confined_command_can_neither_see_nor_signal_a_host_process() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-host-processes");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("prepared sandbox");
    session.materialize().expect("materialized workspace");
    let host = std::process::id();

    // Signalling is the half a missing `/proc` entry does not prove: a process
    // outside the namespace has no number in here to name, so the signal has
    // nowhere to land and tracing has nothing to attach to.
    let script = format!(
        "test ! -e /proc/{host} && ! kill -0 {host} 2>/dev/null && \
         test \"$(awk '/^TracerPid:/ {{ print $2 }}' /proc/self/status)\" = 0"
    );
    let (status, _, errors) = finish(session.start(command(&script)).expect("started command"));

    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&errors)
    );
}
