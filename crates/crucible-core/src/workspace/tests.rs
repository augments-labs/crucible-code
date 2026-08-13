use std::fs;
use std::path::PathBuf;

use super::*;

/// A workspace with a file in it, a sibling directory outside it that the
/// escape attempts aim at, and a second sibling that a widened workspace is
/// pointed at.
struct Fixture {
    outside: PathBuf,
    beside: PathBuf,
    workspace: Workspace,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("crucible-ws-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("inside");
        let outside = base.join("outside");
        let beside = base.join("beside");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&beside).unwrap();
        fs::write(root.join("kept.txt"), "in").unwrap();
        fs::write(outside.join("secret.txt"), "out").unwrap();
        fs::write(beside.join("shared.txt"), "reached").unwrap();
        Self {
            outside: outside.canonicalize().unwrap(),
            beside: beside.canonicalize().unwrap(),
            workspace: Workspace::open(&root).unwrap(),
        }
    }

    /// Widens this workspace to reach the second sibling.
    fn reaching_beside(mut self) -> Self {
        let beside = self.beside.display().to_string();
        self.workspace = self.workspace.clone().reaching([&beside]).unwrap();
        self
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(base) = self.workspace.root().parent() {
            let _ = fs::remove_dir_all(base);
        }
    }
}

/// A symbolic link at `link` pointing at `target`, which need not exist.
///
/// Every link here stands in for one a cloned repository shipped, so the far end
/// is always a file. Windows has a call per kind and no way to make one for a
/// target that is not there yet, which is why the kind is in the name rather
/// than read off the target.
///
/// Making one on Windows is a privilege: developer mode, or an elevated shell.
/// Failing loudly is right — a link that was not made turns a containment test
/// into one that passes because there was nothing to escape through.
fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(target, link);

    made.expect("a symbolic link: on Windows this needs developer mode");
}

#[test]
fn a_path_inside_the_workspace_resolves() {
    let f = Fixture::new("inside");
    let path = f.workspace.existing("kept.txt").unwrap();
    assert!(path.as_path().starts_with(f.workspace.root()));
    assert!(path.as_path().ends_with("kept.txt"));
}

#[test]
fn dot_dot_that_climbs_out_is_rejected() {
    let f = Fixture::new("dotdot");
    let err = f.workspace.existing("../outside/secret.txt").unwrap_err();
    assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
}

#[test]
fn an_absolute_path_outside_is_rejected() {
    let f = Fixture::new("absolute");
    let target = f.outside.join("secret.txt");
    let err = f
        .workspace
        .existing(&target.display().to_string())
        .unwrap_err();
    assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
}

#[test]
fn a_symlink_pointing_out_is_rejected() {
    let f = Fixture::new("symlink");
    let link = f.workspace.root().join("escape.txt");
    symlink(f.outside.join("secret.txt"), &link);

    // The text "escape.txt" says nothing about where it goes. Only the
    // resolved path does, which is why containment runs after
    // canonicalisation and not on what the caller typed.
    let err = f.workspace.existing("escape.txt").unwrap_err();
    assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
}

#[test]
fn dot_dot_that_stays_inside_is_allowed() {
    let f = Fixture::new("staysin");
    let path = f.workspace.existing("sub/../kept.txt").unwrap();
    assert!(path.as_path().ends_with("kept.txt"));
}

#[test]
fn a_new_file_inside_is_creatable() {
    let f = Fixture::new("create");
    let path = f.workspace.creatable("sub/new.txt").unwrap();
    assert!(path.as_path().starts_with(f.workspace.root()));
    assert!(!path.as_path().exists());
}

#[test]
fn a_new_file_outside_is_not_creatable() {
    let f = Fixture::new("createout");
    let err = f
        .workspace
        .creatable("sub/../../outside/new.txt")
        .unwrap_err();
    assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
}

#[test]
fn a_symlinked_leaf_pointing_out_is_not_creatable() {
    let f = Fixture::new("createlink");
    let link = f.workspace.root().join("notes.txt");
    symlink(f.outside.join("secret.txt"), &link);

    // The parent is inside and the name is inside, so the lexical path is
    // inside — and writing to it still lands outside. Only resolving the
    // last component says so.
    let err = f.workspace.creatable("notes.txt").unwrap_err();
    assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
}

#[test]
fn a_dangling_symlinked_leaf_pointing_out_is_not_creatable() {
    let f = Fixture::new("createdangling");
    let link = f.workspace.root().join("fresh.txt");
    symlink(f.outside.join("absent.txt"), &link);

    // Nothing exists at the far end, so there is no resolved path to decide
    // containment on and the write is refused. Which is also what stops a
    // file being *created* outside the tree through a link.
    let err = f.workspace.creatable("fresh.txt").unwrap_err();
    assert!(matches!(err, PathError::Dangling { .. }), "got {err:?}");
}

#[test]
fn a_dangling_leaf_is_refused_for_the_reason_it_is_actually_refused_for() {
    let f = Fixture::new("createdanglingin");
    let link = f.workspace.root().join("fresh.txt");
    symlink(f.workspace.root().join("absent.txt"), &link);

    // The target is inside the workspace, so "resolves outside" is a
    // sentence about this path that is not true — and the model is handed
    // this text to decide what to try next. It is still refused, because
    // nothing here can prove where a link with no target ends up; the reason
    // given has to be the reason.
    let err = f.workspace.creatable("fresh.txt").unwrap_err();
    let said = err.to_string();
    assert!(!said.contains("outside"), "got {said}");
    assert!(matches!(err, PathError::Dangling { .. }), "got {err:?}");
}

#[test]
fn a_symlinked_leaf_pointing_back_inside_is_creatable() {
    let f = Fixture::new("createinside");
    let link = f.workspace.root().join("alias.txt");
    symlink(f.workspace.root().join("kept.txt"), &link);

    let path = f.workspace.creatable("alias.txt").unwrap();
    assert!(path.as_path().starts_with(f.workspace.root()));
}

#[test]
fn creating_under_a_directory_that_does_not_exist_is_rejected() {
    let f = Fixture::new("nodir");
    let err = f.workspace.creatable("absent/new.txt").unwrap_err();
    assert!(matches!(err, PathError::Missing { .. }), "got {err:?}");
}

#[test]
fn a_missing_file_is_missing_rather_than_escaping() {
    let f = Fixture::new("missing");
    let err = f.workspace.existing("absent.txt").unwrap_err();
    assert!(matches!(err, PathError::Missing { .. }), "got {err:?}");
}

#[test]
fn a_reached_directory_is_readable_and_writable() {
    let f = Fixture::new("reached").reaching_beside();

    let read = f
        .workspace
        .existing(&f.beside.join("shared.txt").display().to_string())
        .unwrap();
    assert!(read.as_path().starts_with(&f.beside));

    let written = f
        .workspace
        .creatable(&f.beside.join("new.txt").display().to_string())
        .unwrap();
    assert!(written.as_path().starts_with(&f.beside));
}

#[test]
fn reaching_one_directory_does_not_reach_its_siblings() {
    let f = Fixture::new("reachedonly").reaching_beside();

    // `beside` and `outside` share a parent, and adding the first must not
    // hand over the second.
    let err = f
        .workspace
        .existing(&f.outside.join("secret.txt").display().to_string())
        .unwrap_err();
    assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");

    // Nor the one whose name merely starts the same way. Containment compares
    // whole components, so `beside` covers `beside/x` and not `besidemore/x`;
    // a prefix match on the text would hand over both.
    let mut adjacent = f.beside.clone().into_os_string();
    adjacent.push("more");
    let adjacent = PathBuf::from(adjacent);
    fs::create_dir_all(&adjacent).unwrap();
    fs::write(adjacent.join("secret.txt"), "out").unwrap();

    let err = f
        .workspace
        .existing(&adjacent.join("secret.txt").display().to_string())
        .unwrap_err();
    assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
}

#[test]
fn the_root_still_names_the_directory_crucible_started_in() {
    let f = Fixture::new("stillroot");
    let started = f.workspace.root().to_path_buf();
    let f = f.reaching_beside();

    // A relative path is joined to this and `bash` runs here, so widening
    // reach must not move it. If it did, `read("kept.txt")` would start
    // meaning a different file.
    assert_eq!(f.workspace.root(), started);
    assert!(f.workspace.existing("kept.txt").is_ok());
}

#[cfg(unix)]
#[test]
fn a_root_whose_name_is_not_utf8_is_refused_at_the_door() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // A directory unpacked from an archive written in some other encoding. The
    // name is legal to the operating system and illegal as text, and the root
    // is one of the few paths that has to be both: a session log writes it down
    // and reads it back to find the session belonging to this directory. Left
    // to resolve, every `--continue` here would say there is no earlier session
    // while the log sits beside it — a failure reported as an absence, which is
    // the kind nobody searches for.
    let base = std::env::temp_dir().join(format!("crucible-ws-bytes-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("a writable temporary directory");

    let mut name = base.as_os_str().as_bytes().to_vec();
    name.extend_from_slice(b"/caf\xe9");
    let root = PathBuf::from(OsStr::from_bytes(&name));

    // Not every filesystem will hold a name like that. macOS refuses one
    // outright, and there the case is answered a layer below rather than left
    // to `Workspace::open` — a root that is not text cannot be made, so it
    // cannot be opened either. The directory above it was made a moment ago
    // under a name that is text, so a refusal here is about the name and not
    // about the place.
    if fs::create_dir(&root).is_err() {
        let _ = fs::remove_dir_all(&base);
        return;
    }

    let err = Workspace::open(&root).unwrap_err();
    assert!(matches!(err, PathError::NotText { .. }), "got {err:?}");

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn a_leaf_made_into_a_link_after_the_check_is_not_opened_through() {
    let f = Fixture::new("swapleaf");
    let path = f.workspace.existing("kept.txt").unwrap();

    // Containment established where this name led at the instant it ran. A
    // second writer with the workspace open — a build script, a watcher, a
    // shell in the next pane — can move it afterwards, and opening by name
    // would follow it straight out of the tree.
    fs::remove_file(path.as_path()).unwrap();
    symlink(f.outside.join("secret.txt"), path.as_path());

    let err = path.open_to_change().unwrap_err();
    assert!(matches!(err, PathError::Swapped { .. }), "got {err:?}");
    assert_eq!(
        fs::read_to_string(f.outside.join("secret.txt")).unwrap(),
        "out"
    );
}

#[test]
fn a_link_planted_where_a_new_file_goes_is_not_created_through() {
    let f = Fixture::new("swapcreate");
    let path = f.workspace.creatable("fresh.txt").unwrap();

    symlink(f.outside.join("absent.txt"), path.as_path());

    // `O_CREAT | O_EXCL`, so the operating system refuses to satisfy the
    // create through a link at the last component, dangling or not. There is
    // no second lookup here for the swap to win.
    let err = path.create().unwrap_err();
    assert!(matches!(err, PathError::Swapped { .. }), "got {err:?}");
    assert!(!f.outside.join("absent.txt").exists());
}

#[cfg(unix)]
#[test]
#[ignore = "names the hole that is open: nothing here resolves a step against a directory it holds"]
fn a_directory_swapped_above_a_proven_path_cannot_reach_outside() {
    // The half still reachable, written down as the property that does not hold
    // rather than as prose, so running the ignored tests reports it. Every call
    // that acts on a resolved path resolves the directories above the last
    // component afresh and at the same moment, so a directory replaced with a
    // link between the check and the call is followed by all of them and
    // disagreed with by none. What would see it is resolving each step relative
    // to a directory already held open; until safe Rust can ask for that,
    // neither property below holds and running this reports the first.
    let f = Fixture::new("swapabove");
    fs::write(f.workspace.root().join("sub/one.txt"), "in").unwrap();
    fs::write(f.outside.join("one.txt"), "out").unwrap();
    let existing = f.workspace.existing("sub/one.txt").unwrap();
    let fresh = f.workspace.creatable("sub/fresh.txt").unwrap();

    fs::remove_dir_all(f.workspace.root().join("sub")).unwrap();
    std::os::unix::fs::symlink(&f.outside, f.workspace.root().join("sub")).unwrap();

    assert!(
        existing.open().is_err(),
        "a file outside the workspace was opened for reading"
    );
    let _ = fresh.create();
    assert!(
        !f.outside.join("fresh.txt").exists(),
        "a file was created outside the workspace"
    );
}

#[test]
fn a_relative_directory_cannot_be_reached() {
    let f = Fixture::new("relative");

    // Configuration is read from a file that has no working directory of its
    // own, so "../shared" would name a different place depending on where
    // crucible was started. There is no answer to give, so none is guessed.
    let err = f.workspace.clone().reaching(["../beside"]).unwrap_err();
    assert!(matches!(err, PathError::Relative { .. }), "got {err:?}");
}

#[test]
fn a_directory_that_is_not_there_cannot_be_reached() {
    let f = Fixture::new("reachmissing");
    let absent = f.beside.join("absent");

    // Dropping it silently would leave somebody believing they had reach they
    // do not have, and finding out from a refusal mid-turn instead of at
    // startup.
    let err = f
        .workspace
        .clone()
        .reaching([absent.display().to_string()])
        .unwrap_err();
    assert!(matches!(err, PathError::Missing { .. }), "got {err:?}");
}
