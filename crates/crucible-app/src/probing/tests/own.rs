//! The directory a program the user installed lies in is decided once for its
//! probe and its launch, and nothing it may not read or write is given.

use std::path::{Path, PathBuf};

use crate::launching::guarded;
use crate::own_program::{HOME_SEARCH, Platform, Root, decided, root};

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
    parts.iter().fold(home.to_path_buf(), |path, part| path.join(part))
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
    let Root::Refused(why) = decided_on(&under(&home, &[".cargo", "agent"]), Platform::Other) else {
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
        ["bin", ".local/bin", ".cargo/bin", "AppData/Local/Microsoft/WindowsApps"]
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
        assert!(mounts.contains(&"/usr") && mounts.contains(&"/bin"), "{mounts:?}");
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
        assert!(matches!(decided_on(&program, Platform::Other), Root::Grant(_)));
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
