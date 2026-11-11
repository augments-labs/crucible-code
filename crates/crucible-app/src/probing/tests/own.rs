//! The directory a program the user installed lies in is decided once for its
//! probe and its launch, and nothing it may not read or write is given.

use std::fs;
use std::path::{Path, PathBuf};

use crucible_config::Driver;
use crucible_external_harnesses::catalog::Variable;
use crucible_external_harnesses::{Dialect as _, Shape};
use crucible_sandbox::{SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxPolicy};
use serde_json::{Map, Value, json};

use crate::backends::REGISTRY;
use crate::launching::{Sandbox, guarded};
use crate::own_program::{
    DID_NOT_SAY, EMPTY_PROVENANCE, Empty, HOME_SEARCH, Instance, Places, Platform, RECORDS, Root,
    Unowned, admit, decided, dialect, forget, launch_policy, probe_policy, read, root, told,
};
use crate::sample::Sample;

/// A home, crucible's home within it and its session logs, none of which is
/// read from disk.
fn places() -> (PathBuf, PathBuf, PathBuf) {
    let home = Path::new("/").join("home").join("someone");
    let crucible = home.join(".crucible");
    let sessions = crucible.join("sessions");
    (home, crucible, sessions)
}

/// The directory of `program` as it is decided on `platform`, with `/usr` and
/// `/bin` read by the sandbox already.
fn decided_on(program: &Path, platform: Platform) -> Root {
    let (home, crucible, sessions) = places();
    decided(
        program,
        &home,
        &guarded(&home, &crucible, &sessions),
        platform,
        &["/usr", "/bin"],
    )
}

/// `home` with each of `parts` joined, a component at a time.
fn under(home: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part))
}

#[test]
fn the_own_probe_refuses_a_root_that_is_home_or_never_writable() {
    let (home, _, _) = places();
    let refused = [
        home.join("agent"),
        Path::new("/").join("agent"),
        under(&home, &[".ssh", "agent"]),
        under(&home, &[".rustup", "toolchains", "stable", "bin", "agent"]),
        under(&home, &[".cargo", "agent"]),
        Path::new("/").join("home").join("agent"),
        under(&home, &["bin", "nested", "agent"]),
    ];
    for program in &refused {
        assert!(
            matches!(decided_on(program, Platform::Other), Root::Refused(_)),
            "{} was given",
            program.display()
        );
    }
    let Root::Refused(why) = decided_on(&under(&home, &[".cargo", "agent"]), Platform::Other)
    else {
        panic!("~/.cargo was given");
    };
    assert!(why.contains("contains ~/.cargo/bin"), "{why}");

    for search in [&["bin"][..], &[".local", "bin"], &[".cargo", "bin"]] {
        let dir = under(&home, search);
        assert_eq!(
            decided_on(&dir.join("agent"), Platform::Other),
            Root::Grant(dir),
            "{search:?}"
        );
    }
    assert_eq!(
        HOME_SEARCH,
        [
            "bin",
            ".local/bin",
            ".cargo/bin",
            "AppData/Local/Microsoft/WindowsApps"
        ]
    );

    // A crucible home kept outside the user's is guarded where it lies.
    let crucible = Path::new("/").join("srv").join("crucible").join("home");
    let sessions = crucible.join("sessions");
    let program = Path::new("/").join("srv").join("crucible").join("agent");

    let decided = decided(
        &program,
        &home,
        &guarded(&home, &crucible, &sessions),
        Platform::Other,
        &[],
    );

    let Root::Refused(why) = decided else {
        panic!("{decided:?}");
    };
    assert!(why.contains("crucible's own home"), "{why}");
}

#[test]
fn a_root_inside_the_runtime_mounts_gets_no_rule() {
    let (home, crucible, sessions) = places();
    let mounts = crucible_sandbox_local::runtime_directories();

    for (program, mounted) in [
        (Path::new("/").join("usr").join("bin").join("agent"), "/usr"),
        (
            under(Path::new("/"), &["usr", "local", "bin", "agent"]),
            "/usr",
        ),
    ] {
        let expected = if mounts.contains(&mounted) {
            Root::NoRule
        } else {
            Root::Grant(program.parent().map(Path::to_path_buf).unwrap_or_default())
        };
        assert_eq!(root(&program, &home, &crucible, &sessions), expected);
    }
    if cfg!(target_os = "linux") {
        assert!(
            mounts.contains(&"/usr") && mounts.contains(&"/bin"),
            "{mounts:?}"
        );
    }
    let tool = under(Path::new("/"), &["opt", "tool", "bin"]);
    assert_eq!(
        root(&tool.join("agent"), &home, &crucible, &sessions),
        Root::Grant(tool)
    );
}

#[test]
fn a_root_the_platform_has_not_been_recorded_for_is_refused() {
    let (home, _, _) = places();
    let homebrew = under(Path::new("/"), &["opt", "homebrew", "bin"]);
    let windows_apps = under(&home, &["AppData", "Local", "Microsoft", "WindowsApps"]);

    for program in [homebrew.join("agent"), homebrew.join("sub").join("agent")] {
        let Root::Refused(why) = decided_on(&program, Platform::MacOs) else {
            panic!("{} was given on macOS", program.display());
        };
        assert!(why.contains("macOS"), "{why}");
        assert!(matches!(
            decided_on(&program, Platform::Other),
            Root::Grant(_)
        ));
    }
    let program = windows_apps.join("agy.exe");
    let Root::Refused(why) = decided_on(&program, Platform::Windows) else {
        panic!("WindowsApps was given on Windows");
    };
    assert!(why.contains("Windows"), "{why}");
    assert_eq!(
        decided_on(&program, Platform::Other),
        Root::Grant(windows_apps)
    );
}

/// What a program answers to `initialize` with `result`.
fn said(result: &Value) -> Map<String, Value> {
    match json!({"jsonrpc": "2.0", "id": 0, "result": result}) {
        Value::Object(said) => said,
        _ => Map::new(),
    }
}

/// An answer that admits: protocol 1, capabilities, a name and a version.
fn speaking() -> Map<String, Value> {
    said(&json!({
        "protocolVersion": 1,
        "agentCapabilities": {"loadSession": true},
        "agentInfo": {"name": "gemini-cli", "version": "0.9.0"},
        "authMethods": [{"id": "login", "name": "Log in", "description": "Sign in \u{1b}[31mhere"}],
    }))
}

/// A catalog instance named `name` of the entry `agent`, declared to start
/// with `args`.
fn instance<'a>(name: &'a str, agent: &'a str, args: &'a [Box<str>]) -> Instance<'a> {
    Instance {
        name,
        driver: "catalog",
        agent: Some(agent),
        args,
        env: &[],
    }
}

/// A program file in `sample`'s tree, outside its home, and its canonical
/// path.
#[allow(clippy::panic)]
fn program_in(sample: &Sample) -> PathBuf {
    let dir = sample.root().join("tools").join("bin");
    fs::create_dir_all(&dir).unwrap_or_else(|why| panic!("a directory: {why}"));
    let program = dir.join("agent");
    fs::write(&program, "#!/bin/sh\n").unwrap_or_else(|why| panic!("a program: {why}"));
    fs::canonicalize(&program).unwrap_or_else(|why| panic!("a canonical path: {why}"))
}

/// The rule `policy` has at `path`, as its access and provenance.
fn rule_at(
    policy: &SandboxPolicy,
    path: &Path,
) -> Option<(SandboxFilesystemAccess, SandboxFilesystemProvenance)> {
    policy
        .filesystem()
        .iter()
        .find(|rule| rule.path() == path)
        .map(|rule| (rule.access(), rule.provenance()))
}

#[test]
fn the_own_root_rules_are_the_same_for_the_probe_and_the_launch() {
    let sample = Sample::new("own-root-rules");
    let program = program_in(&sample);
    let home = sample.home();
    let crucible = home.join(".crucible");
    let sessions = crucible.join("sessions");
    let places = Places {
        home: &home,
        crucible: &crucible,
        sessions: &sessions,
    };
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");
    let empty = Empty::made().expect("an empty directory");

    // A granted root is read-only on both sides, each with its provenance.
    let granted = program.parent().expect("a directory").to_path_buf();
    let root = places.root(&program);
    assert_eq!(root, Root::Grant(granted.clone()));
    let probing = probe_policy(&root, empty.path(), true).expect("a probe policy");
    let launching = launch_policy(&conversation, &root).expect("a launch policy");
    assert_eq!(
        rule_at(&probing, &granted),
        Some((
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Workspace
        ))
    );
    assert_eq!(probing.working_directory(), granted);
    assert_eq!(probing.filesystem().len(), 1);
    assert_eq!(
        rule_at(&launching, &granted),
        Some((
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime
        ))
    );
    assert!(probing.enabled());

    // No rule is the empty directory alone on the probe, and nothing added
    // on the launch.
    let probing = probe_policy(&Root::NoRule, empty.path(), true).expect("a probe policy");
    assert_eq!(
        probing
            .filesystem()
            .iter()
            .map(|rule| (rule.path(), rule.access(), rule.provenance()))
            .collect::<Vec<_>>(),
        [(
            empty.path(),
            SandboxFilesystemAccess::ReadOnly,
            EMPTY_PROVENANCE
        )]
    );
    assert_eq!(EMPTY_PROVENANCE, SandboxFilesystemProvenance::Workspace);
    assert_eq!(probing.working_directory(), empty.path());
    assert_eq!(
        launch_policy(&conversation, &Root::NoRule).expect("a launch policy"),
        conversation
    );
    assert_eq!(fs::read_dir(empty.path()).expect("a directory").count(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(empty.path())
            .expect("a directory")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    let made = empty.path().to_path_buf();
    drop(empty);
    assert!(!made.exists(), "{} was left behind", made.display());

    // A refused root is refused by both sides alike.
    let refused = places.root(&home.join("agent"));
    let Root::Refused(why) = &refused else {
        panic!("{refused:?}");
    };
    let empty = Empty::made().expect("an empty directory");
    for refusal in [
        probe_policy(&refused, empty.path(), true).err(),
        launch_policy(&conversation, &refused).err(),
    ] {
        assert!(
            matches!(&refusal, Some(Unowned::Root(said)) if said == why),
            "{refusal:?}"
        );
    }
}

#[test]
fn the_own_agy_probe_carries_the_drivers_environment() {
    let row = REGISTRY
        .row(Driver::Antigravity, None)
        .expect("Antigravity's row");
    let program = Path::new("/").join("opt").join("bin").join("agy");
    let workspace = Path::new("/").join("tmp");

    let mut probing = dialect(&row, &program).expect("a wire");
    let request = probing.request(Shape {
        workspace: Some(&workspace),
        ..Shape::default()
    });

    let variable = |name: &str| {
        request
            .environment
            .iter()
            .find(|(set, _)| set == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(variable("AGY_ACP_FORCE_FILE_STORAGE"), Some("1"));
    assert_eq!(variable("GEMINI_HOME"), None);
    assert_eq!(request.arguments, ["acp"]);
}

#[test]
fn an_own_probe_record_whose_sandbox_was_not_closed_does_not_admit() {
    let sample = Sample::new("own-unclosed");
    let program = program_in(&sample);
    let crucible = sample.home();
    let gemini = instance("gemini", "gemini", &[]);

    let unconfined = admit(
        &crucible,
        &gemini,
        &["gemini"],
        &program,
        &Sandbox::Unavailable("no user namespaces".into()),
        &speaking(),
    );
    assert!(
        matches!(unconfined, Err(Unowned::Unconfined)),
        "{unconfined:?}"
    );
    assert!(read(&crucible, &gemini).is_none());

    // A record written whole that says its sandbox was not closed is read as
    // none.
    admit(
        &crucible,
        &gemini,
        &["gemini"],
        &program,
        &Sandbox::Closed,
        &speaking(),
    )
    .expect("admitted");
    assert!(read(&crucible, &gemini).is_some());
    let kept = fs::read_dir(crucible.join("own-programs"))
        .expect("the records")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("a record");
    let text = fs::read_to_string(&kept).expect("the record");
    fs::write(
        &kept,
        text.replace("\"closed\"", "\"unavailable: no user namespaces\""),
    )
    .expect("rewritten");
    assert!(read(&crucible, &gemini).is_none());
}

#[test]
fn an_answer_with_no_agent_info_admits_and_records_did_not_say() {
    let sample = Sample::new("own-no-info");
    let program = program_in(&sample);
    let crucible = sample.home();
    let gemini = instance("gemini", "gemini", &[]);

    let record = admit(
        &crucible,
        &gemini,
        &["gemini"],
        &program,
        &Sandbox::Closed,
        &said(&json!({"protocolVersion": 1, "agentCapabilities": {}})),
    )
    .expect("admitted by its capabilities");

    assert_eq!(record.name(), DID_NOT_SAY);
    assert_eq!(record.version(), DID_NOT_SAY);
    assert_eq!(read(&crucible, &gemini).as_ref(), Some(&record));
    assert!(record.current(&program));

    // What it said of itself is kept, and its sign-in's description clipped,
    // never compared with anything.
    let record = admit(
        &crucible,
        &gemini,
        &["gemini"],
        &program,
        &Sandbox::Closed,
        &speaking(),
    )
    .expect("admitted");
    assert_eq!((record.name(), record.version()), ("gemini-cli", "0.9.0"));
    assert_eq!(record.methods().len(), 1);

    // Protocol 1 and a capabilities object are what admits it.
    for refused in [
        json!({"protocolVersion": 2, "agentCapabilities": {}}),
        json!({"protocolVersion": 1}),
        json!({"protocolVersion": 1, "agentCapabilities": []}),
    ] {
        let fresh = Sample::new("own-refused");
        let answer = admit(
            &fresh.home(),
            &gemini,
            &["gemini"],
            &program,
            &Sandbox::Closed,
            &said(&refused),
        );
        assert!(
            matches!(answer, Err(Unowned::Unadmitted(_))),
            "{refused}: {answer:?}"
        );
        assert!(read(&fresh.home(), &gemini).is_none(), "{refused}");
    }
}

#[test]
fn a_changed_agent_key_or_launch_shape_reads_as_no_record() {
    let sample = Sample::new("own-shape");
    let program = program_in(&sample);
    let crucible = sample.home();
    let args: [Box<str>; 1] = ["--acp".into()];
    let env = [Variable {
        name: "MODE".into(),
        value: "acp".into(),
    }];
    let probed = Instance {
        env: &env,
        ..instance("Gemini", "gemini", &args)
    };
    admit(
        &crucible,
        &probed,
        &["Gemini", "gemini"],
        &program,
        &Sandbox::Closed,
        &speaking(),
    )
    .expect("admitted");
    assert!(read(&crucible, &probed).is_some());

    let other_args: [Box<str>; 1] = ["--experimental-acp".into()];
    let other_env = [Variable {
        name: "MODE".into(),
        value: "other".into(),
    }];
    let changed = [
        Instance {
            agent: Some("qwen"),
            ..probed
        },
        Instance {
            driver: "antigravity",
            agent: None,
            ..probed
        },
        Instance {
            args: &other_args,
            ..probed
        },
        Instance {
            env: &other_env,
            ..probed
        },
        Instance { env: &[], ..probed },
        Instance {
            name: "gemini",
            ..probed
        },
    ];
    for instance in &changed {
        assert!(read(&crucible, instance).is_none(), "{instance:?}");
    }

    // A program that changed is still the instance's record, which a launch
    // tells is stale by its path, size and modification time.
    fs::write(&program, "#!/bin/sh\necho updated\n").expect("updated");
    let record = read(&crucible, &probed).expect("the record");
    assert!(!record.current(&program));

    // Two names differing only in case keep two records.
    let lower = Instance {
        name: "gemini",
        ..probed
    };
    admit(
        &crucible,
        &lower,
        &["Gemini", "gemini"],
        &program,
        &Sandbox::Closed,
        &speaking(),
    )
    .expect("admitted");
    assert!(read(&crucible, &probed).is_some());
    assert!(read(&crucible, &lower).is_some());
    assert_eq!(
        fs::read_dir(crucible.join("own-programs"))
            .expect("the records")
            .count(),
        2
    );
}

#[test]
fn the_own_program_records_are_kept_to_64_and_go_with_their_instance() {
    let sample = Sample::new("own-bound");
    let program = program_in(&sample);
    let crucible = sample.home();
    let names: Vec<String> = (0..=RECORDS).map(|n| format!("agent-{n}")).collect();
    let all: Vec<&str> = names.iter().map(String::as_str).collect();
    let (last, kept) = all.split_last().expect("names");

    assert_eq!(RECORDS, 64);
    for name in kept {
        admit(
            &crucible,
            &instance(name, "gemini", &[]),
            &all,
            &program,
            &Sandbox::Closed,
            &speaking(),
        )
        .expect("admitted");
    }
    let full = admit(
        &crucible,
        &instance(last, "gemini", &[]),
        &all,
        &program,
        &Sandbox::Closed,
        &speaking(),
    );
    assert!(matches!(full, Err(Unowned::Full)), "{full:?}");
    assert!(read(&crucible, &instance(last, "gemini", &[])).is_none());

    // A record whose instance is gone is dropped to make room.
    let (gone, live) = kept.split_first().expect("names");
    let mut remaining: Vec<&str> = live.to_vec();
    remaining.push(last);
    admit(
        &crucible,
        &instance(last, "gemini", &[]),
        &remaining,
        &program,
        &Sandbox::Closed,
        &speaking(),
    )
    .expect("admitted once one is gone");
    assert!(read(&crucible, &instance(gone, "gemini", &[])).is_none());
    assert_eq!(
        fs::read_dir(crucible.join("own-programs"))
            .expect("the records")
            .count(),
        RECORDS
    );

    // Removing an instance removes its record.
    forget(&crucible, last).expect("forgotten");
    assert!(read(&crucible, &instance(last, "gemini", &[])).is_none());
    forget(&crucible, last).expect("forgetting twice is nothing");
}

/// The user's own Antigravity started under the sandbox with no profile is
/// told that it cannot reach the sign-in it keeps in `~/.gemini` on every
/// platform, and that a sign-in made under the sandbox is gone when that
/// start ends only on Linux, the one platform whose sandbox is known to give
/// it a private home of its own; the same start with a profile, or with the
/// sandbox off, is told neither.
#[test]
fn the_lost_sign_in_sentence_is_shown_on_linux_only() {
    use crate::backends::{AgentRow, Sandboxing, Started};
    use crate::launching::Origin;

    let Some(built) = REGISTRY.built(Driver::Antigravity) else {
        panic!("Antigravity has no row");
    };
    let row = AgentRow::Built(built);
    let unreachable = Started {
        origin: Origin::Own,
        sandbox: Sandboxing {
            on: true,
            required: false,
        },
        profiled: false,
    };
    let lost = "is gone when that start ends";
    for platform in [
        Platform::Linux,
        Platform::MacOs,
        Platform::Windows,
        Platform::Other,
    ] {
        let told = told(&row, unreachable, platform, None);
        let how = told.how.unwrap_or_default();
        assert!(how.contains("~/.gemini"), "{platform:?}: {how}");
        let said = told.more.iter().any(|line| line.contains(lost));
        assert_eq!(
            said,
            platform == Platform::Linux,
            "{platform:?}: {:?}",
            told.more
        );
    }
    for started in [
        Started {
            profiled: true,
            ..unreachable
        },
        Started {
            sandbox: Sandboxing::default(),
            ..unreachable
        },
        Started {
            origin: Origin::Stored,
            ..unreachable
        },
    ] {
        let told = told(&row, started, Platform::Linux, None);
        assert_eq!(told.how, None, "{started:?}");
        assert!(
            !told.more.iter().any(|line| line.contains(lost)),
            "{started:?}: {:?}",
            told.more
        );
    }
}
