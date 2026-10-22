//! A program is started only as it was when it was found.

use crate::{Discovery, Held, Launch, Refusal, Request};
use crucible_sandbox::{
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits,
};

/// A launch of this test's own executable, which is never started.
fn launch() -> Launch {
    let path = std::env::current_exe().expect("this test");
    let Discovery::Found(program) = Discovery::of(Some(&path)) else {
        panic!("this test is a program");
    };
    let here = path.parent().expect("a directory");
    let root = SandboxFilesystemRule::new(
        here,
        SandboxFilesystemAccess::ReadOnly,
        SandboxFilesystemProvenance::Workspace,
    )
    .expect("an absolute root");
    let policy = SandboxPolicy::new(
        true,
        [root],
        here,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::confining(),
    )
    .expect("a valid policy");
    Launch::new(program, policy, Held::granted("agent:plain"))
}

/// The next launch puts a resume reference on the program's command line, so
/// one that reads as an option, or carries a control character, is refused.
#[test]
fn a_resume_reference_that_could_be_read_as_more_than_a_name_is_refused() {
    for reference in [
        "-x",
        "--dangerously-skip-permissions",
        "one\ntwo",
        "one\u{1b}[2J",
    ] {
        assert!(
            matches!(launch().resuming(reference), Err(Refusal::Resume)),
            "{reference:?}"
        );
    }
    assert!(launch().resuming("a-session-1").is_ok());
}

/// An environment value may be a credential, so what a launch prints names
/// each variable and shows none of their values.
#[test]
fn a_launch_printed_for_debugging_shows_no_environment_value() {
    let request = Request {
        arguments: vec!["--print".to_owned()],
        environment: vec![(
            "PLAIN_TOKEN".to_owned(),
            "sentinel-c0ffee-5ec7e7".to_owned(),
        )],
        shown: Vec::new(),
    };

    let printed = format!("{request:?} {:?}", launch().asking(request.clone()));

    assert!(printed.contains("PLAIN_TOKEN"), "{printed}");
    assert!(!printed.contains("sentinel-c0ffee-5ec7e7"), "{printed}");
}

/// A name the program gave its session is handed back on its command line,
/// so only the letters a session name is made of are kept: a space, a shell
/// word or a character that reorders what a terminal shows is refused.
#[test]
fn a_session_name_is_kept_only_as_a_plain_word() {
    for name in [
        "one two",
        "a/b",
        "$(id)",
        "a;b",
        "one\u{202e}owt",
        "one\u{200b}two",
        "é",
    ] {
        assert!(!crate::launch::nameable(name), "{name:?}");
    }
    for name in [
        "a-session-1",
        "0193a5c2-7e1f-7c3b-9d1e-2f4a5b6c7d8e",
        "run_2.b:c",
    ] {
        assert!(crate::launch::nameable(name), "{name:?}");
    }
}

/// A program put in the selected one's place that is the same size, with its
/// modification time set back to what it was, is still not the one selected.
#[cfg(unix)]
#[test]
fn a_same_size_program_with_its_time_restored_is_not_the_one_selected() {
    use std::fs::{self, File, FileTimes};
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let at = super::Owned::new("restored-time");
    let path = at.0.join("agent");
    let bytes = |fill: u8| vec![fill; 4096];
    let selected = |path: &std::path::Path| {
        let Discovery::Found(program) = Discovery::of(Some(path)) else {
            panic!("a program");
        };
        program
    };
    fs::write(&path, bytes(1)).expect("written");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
    let modified = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .expect("a modification time");
    let restored = |path: &std::path::Path| {
        File::options()
            .write(true)
            .open(path)
            .and_then(|file| file.set_times(FileTimes::new().set_modified(modified)))
            .expect("time restored");
    };

    // Replaced by another file.
    let program = selected(&path);
    let beside = at.0.join("beside");
    fs::write(&beside, bytes(2)).expect("written");
    fs::set_permissions(&beside, fs::Permissions::from_mode(0o755)).expect("executable");
    restored(&beside);
    fs::rename(&beside, &path).expect("replaced");
    assert!(
        matches!(program.unchanged(), Err(Refusal::Changed(_))),
        "a replacement"
    );

    // Rewritten where it lies.
    fs::write(&path, bytes(1)).expect("written");
    restored(&path);
    let program = selected(&path);
    let mut file = File::options().write(true).open(&path).expect("opened");
    file.write_all(&bytes(3)).expect("rewritten");
    drop(file);
    restored(&path);
    assert!(
        matches!(program.unchanged(), Err(Refusal::Changed(_))),
        "a rewrite"
    );

    // Left alone, it is the one selected.
    let program = selected(&path);
    assert!(program.unchanged().is_ok());
}
