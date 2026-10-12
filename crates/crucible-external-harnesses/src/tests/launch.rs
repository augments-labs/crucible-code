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
    };

    let printed = format!("{request:?} {:?}", launch().asking(request.clone()));

    assert!(printed.contains("PLAIN_TOKEN"), "{printed}");
    assert!(!printed.contains("sentinel-c0ffee-5ec7e7"), "{printed}");
}
