//! The directory a program the user installed lies in is decided once for
//! its launch under the sandbox, and nothing it may not read or write is
//! given.

use std::fs;
use std::path::{Path, PathBuf};

use crucible_config::Driver;
use crucible_external_harnesses::{Dialect as _, Shape};
use crucible_sandbox::{SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxPolicy};

use crate::backends::REGISTRY;
use crate::launching::guarded;
use crate::own_program::{
    HOME_SEARCH, Places, Platform, Root, Unowned, decided, dialect, launch_policy, root, told,
};
use crate::sample::Sample;

/// The top of the file system, where a path that starts there is a full path
/// on this platform.
fn top() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\")
    } else {
        PathBuf::from("/")
    }
}

/// A home, crucible's home within it and its session logs, none of which is
/// read from disk.
fn places() -> (PathBuf, PathBuf, PathBuf) {
    let home = top().join("home").join("someone");
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
        top().join("agent"),
        under(&home, &[".ssh", "agent"]),
        under(&home, &[".rustup", "toolchains", "stable", "bin", "agent"]),
        under(&home, &[".cargo", "agent"]),
        top().join("home").join("agent"),
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

/// A home search directory is excepted only from its own listing: another
/// guarded place that lies exactly there, as crucible's home kept in
/// `~/.local/bin` with its session logs elsewhere, still refuses it.
#[test]
fn a_guarded_place_at_a_home_search_directory_is_refused() {
    let (home, _, _) = places();
    let crucible = under(&home, &[".local", "bin"]);
    let sessions = Path::new("/").join("srv").join("sessions");

    let decided = decided(
        &crucible.join("agent"),
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

/// A home reached through a link, made in `sample`: the home as written, the
/// directory it resolves to, and a directory outside both.
#[cfg(unix)]
fn linked_home(sample: &Sample) -> (PathBuf, PathBuf, PathBuf) {
    let base = fs::canonicalize(sample.root()).expect("the sample's directory");
    let real = base.join("real").join("someone");
    fs::create_dir_all(&real).expect("the home it resolves to");
    std::os::unix::fs::symlink(base.join("real"), base.join("linked")).expect("a linked home");
    let outside = base.join("outside");
    (base.join("linked").join("someone"), real, outside)
}

/// The directory of `program`, a canonical path, decided against `home` with
/// crucible's home `crucible` and its session logs at `sessions`.
#[cfg(unix)]
fn decided_against(program: &Path, home: &Path, crucible: &Path, sessions: &Path) -> Root {
    decided(
        program,
        home,
        &guarded(home, crucible, sessions),
        Platform::Other,
        &[],
    )
}

/// A program's directory is judged by where it resolves to, so a home reached
/// through a link is judged where it resolves too: a root that is or contains
/// it is refused as the user's home, not for a place that lies within it.
#[cfg(unix)]
#[test]
fn a_root_at_a_linked_home_is_refused_as_the_home() {
    let sample = Sample::new("own-linked-home");
    let (home, real, outside) = linked_home(&sample);
    let (crucible, sessions) = (outside.join("crucible"), outside.join("sessions"));

    for (root, says) in [
        (real.clone(), "is your home"),
        (
            real.parent().map(Path::to_path_buf).unwrap_or_default(),
            "contains your home",
        ),
    ] {
        let decided = decided_against(&root.join("agent"), &home, &crucible, &sessions);

        let Root::Refused(why) = decided else {
            panic!("{} was given: {decided:?}", root.display());
        };
        assert!(why.contains(says), "{why}");
    }
}

/// A home search directory reached through a linked home is excepted as it is
/// where the home is written, and still refused where crucible's home is kept
/// there, by either spelling.
#[cfg(unix)]
#[test]
fn a_home_search_directory_under_a_linked_home_is_excepted_from_its_listing_alone() {
    let sample = Sample::new("own-linked-search");
    let (home, real, outside) = linked_home(&sample);
    let search = under(&real, &[".local", "bin"]);
    let sessions = outside.join("sessions");

    assert_eq!(
        decided_against(
            &search.join("agent"),
            &home,
            &outside.join("crucible"),
            &sessions
        ),
        Root::Grant(search.clone())
    );
    for crucible in [under(&home, &[".local", "bin"]), search.clone()] {
        let decided = decided_against(&search.join("agent"), &home, &crucible, &sessions);

        let Root::Refused(why) = decided else {
            panic!("{decided:?} with crucible's home at {}", crucible.display());
        };
        assert!(why.contains("crucible's own home"), "{why}");
    }
}

/// A home whose links cannot be followed has nowhere it resolves to that a
/// directory could be judged against, so every root is refused, naming it.
#[cfg(unix)]
#[test]
fn a_root_judged_against_a_home_that_cannot_be_resolved_is_refused() {
    let sample = Sample::new("own-unresolved-home");
    let base = fs::canonicalize(sample.root()).expect("the sample's directory");
    let home = base.join("dangling");
    std::os::unix::fs::symlink(base.join("nowhere"), &home).expect("a link leading nowhere");
    let tool = base.join("tool").join("bin");

    let decided = decided_against(
        &tool.join("agent"),
        &home,
        &base.join("crucible"),
        &base.join("sessions"),
    );

    let Root::Refused(why) = decided else {
        panic!("{decided:?}");
    };
    assert!(why.contains(&format!("`{}`", home.display())), "{why}");
    assert!(why.contains("cannot be followed"), "{why}");
}

/// A home that is no full path, which no directory compares against, refuses
/// every root, naming the home, rather than judging it against nothing.
#[test]
fn a_root_judged_against_a_relative_home_is_refused() {
    let tool = under(&top(), &["opt", "tool", "bin"]);
    for home in [Path::new("someone").to_path_buf(), PathBuf::new()] {
        let crucible = home.join(".crucible");
        let sessions = crucible.join("sessions");

        let decided = decided(
            &tool.join("agent"),
            &home,
            &guarded(&home, &crucible, &sessions),
            Platform::Other,
            &[],
        );

        let Root::Refused(why) = decided else {
            panic!("{decided:?} against {}", home.display());
        };
        assert!(why.contains("not a full path"), "{why}");
        assert!(why.contains(&format!("`{}`", home.display())), "{why}");
    }
}

#[test]
fn a_root_inside_the_runtime_mounts_gets_no_rule() {
    let (home, crucible, sessions) = places();
    let mounts = crucible_sandbox_local::runtime_directories();

    // Linux mounts all of `/usr`; macOS mounts `/usr/bin` but not
    // `/usr/local`; no other platform's sandbox mounts either.
    let usr_bin = under(Path::new("/"), &["usr", "bin"]);
    let usr_local_bin = under(Path::new("/"), &["usr", "local", "bin"]);
    let (bin_mounted, local_mounted) = if cfg!(target_os = "linux") {
        (true, true)
    } else if cfg!(target_os = "macos") {
        (true, false)
    } else {
        (false, false)
    };
    for (dir, mounted) in [(usr_bin, bin_mounted), (usr_local_bin, local_mounted)] {
        let expected = if mounted {
            Root::NoRule
        } else {
            Root::Grant(dir.clone())
        };
        assert_eq!(
            root(&dir.join("agent"), &home, &crucible, &sessions),
            expected,
            "{}",
            dir.display()
        );
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
fn the_own_root_rules_of_a_launch_under_the_sandbox() {
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

    // A granted root is read-only, with the provenance of a release's own.
    let granted = program.parent().expect("a directory").to_path_buf();
    let root = places.root(&program);
    assert_eq!(root, Root::Grant(granted.clone()));
    let launching = launch_policy(&conversation, &root).expect("a launch policy");
    assert_eq!(
        rule_at(&launching, &granted),
        Some((
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime
        ))
    );

    // No rule adds nothing.
    assert_eq!(
        launch_policy(&conversation, &Root::NoRule).expect("a launch policy"),
        conversation
    );

    // A refused root is refused with its reason.
    let refused = places.root(&home.join("agent"));
    let Root::Refused(why) = &refused else {
        panic!("{refused:?}");
    };
    let refusal = launch_policy(&conversation, &refused).err();
    assert!(
        matches!(&refusal, Some(Unowned::Root(said)) if said == why),
        "{refusal:?}"
    );
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

/// An executable file `name` in `directory` that says `says`.
#[cfg(unix)]
fn executable_in(directory: &Path, name: &str, says: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    fs::create_dir_all(directory).expect("a directory");
    let program = directory.join(name);
    fs::write(&program, format!("#!/bin/sh\necho {says}\n")).expect("a program");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("executable");
    program
}

/// A program found through a link that is pointed at another program before
/// its launch starts it is refused as changed: the program started is the one
/// that was found, never one the link leads to afterwards.
#[cfg(unix)]
#[test]
fn a_link_retargeted_after_it_was_found_is_refused_and_not_started() {
    use std::os::unix::fs::symlink;

    use crucible_external_harnesses::{Discovery, Refusal};

    use crate::own_program::started_at;

    let sample = Sample::new("own-link-retargeted");
    let found = executable_in(&sample.root().join("found"), "agent", "found");
    let other = executable_in(&sample.root().join("other"), "agent", "another program");
    let on_path = sample.root().join("path");
    fs::create_dir_all(&on_path).expect("a directory on PATH");
    let link = on_path.join("agent");
    symlink(&found, &link).expect("a link");
    let Discovery::Found(program) = Discovery::of(Some(&link)) else {
        panic!("the program through the link was not found");
    };

    fs::remove_file(&link).expect("the link removed");
    symlink(&other, &link).expect("the link pointed elsewhere");

    let row = REGISTRY
        .row(Driver::Antigravity, None)
        .expect("Antigravity's row");
    let started = started_at(&row, &program);

    assert!(
        matches!(started, Err(Unowned::Refused(Refusal::Changed(_)))),
        "{started:?}"
    );
}
