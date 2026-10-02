//! A home for the cases that show how far `/resume` looks.
//!
//! Each of those cases starts crucible in a home an earlier run would have
//! left: two sessions recorded in this directory on two branches, one in another
//! checkout of this repository, and one in another project. The cases
//! themselves live in `main.rs` beside every other accepted screen; this is what
//! they plant before the window opens.

use std::fs;
use std::path::{Path, PathBuf};

use crucible_session::Session;
use crucible_types::{Message, SessionId};
use crucible_workspace::Workspace;

/// The configuration every such case starts with: nothing to answer, and no
/// confinement, as every other screen fixture.
pub(crate) const DOCUMENT: &str = concat!(
    "{\n",
    "  \"sandbox\": {\"enabled\": false},\n",
    "  \"updates\": {\"check\": \"never\"}\n",
    "}\n"
);

/// What a case's earlier run left, and the one session that is somewhere
/// else entirely.
pub(crate) struct Planted {
    /// The home to launch from.
    pub(crate) earlier: PathBuf,
    /// The session recorded in another project.
    pub(crate) website: SessionId,
}

/// Plants a home holding four sessions from before crucible started, with
/// `main` checked out in the directory `case` will be started in and another
/// checkout of this repository beside it.
///
/// The directories are the ones `Watched` gives `case`: its scratch directory
/// under the system's temporary one, and the working directory below that. A
/// session names the directory it was recorded in, so these have to be the
/// same paths, and a case whose directory moved finds nothing to list.
pub(crate) fn planted(case: &str) -> Planted {
    let scratch = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-{case}",
        std::process::id()
    ));
    let here = scratch
        .join("a-directory-to-be-working-in")
        .join("and-something-below-that-again")
        .join("workspace");
    let checkout = here.with_file_name("checkout");
    let website = scratch.join("projects").join("website");
    for directory in [&here, &checkout, &website] {
        fs::create_dir_all(directory).expect("a directory a session was recorded in");
    }

    // The repository as git leaves it with a second checkout added: this
    // directory on `main`, and a record naming the other one.
    let git = here.join(".git");
    let record = git.join("worktrees").join("checkout");
    fs::create_dir_all(&record).expect("a worktree record");
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").expect("a branch checked out");
    fs::write(
        record.join("gitdir"),
        format!("{}\n", checkout.join(".git").display()),
    )
    .expect("the checkout the record names");

    // Oldest first, so the list, newest first, reads from the bottom up.
    let earlier = scratch.join("earlier");
    let sessions = earlier.join("sessions");
    let website = recorded(&sessions, &website, Some("main"), "tidy the stylesheet");
    recorded(
        &sessions,
        &checkout,
        Some("release"),
        "cut the release notes",
    );
    recorded(
        &sessions,
        &here,
        Some("feature"),
        "draft the parser rewrite",
    );
    recorded(
        &sessions,
        &here,
        Some("main"),
        "fix the failing parser test",
    );
    Planted { earlier, website }
}

/// Plants a home whose one session was recorded in another project, none in
/// the directory `case` will be started in, and returns that home.
///
/// With `said` false the session never got past its header — a run that
/// opened and left — which is no session to offer anywhere.
pub(crate) fn away(case: &str, said: bool) -> PathBuf {
    let scratch = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-{case}",
        std::process::id()
    ));
    let here = scratch
        .join("a-directory-to-be-working-in")
        .join("and-something-below-that-again")
        .join("workspace");
    let website = scratch.join("projects").join("website");
    for directory in [&here, &website] {
        fs::create_dir_all(directory).expect("a directory a session was recorded in");
    }

    let earlier = scratch.join("earlier");
    let sessions = earlier.join("sessions");
    if said {
        recorded(&sessions, &website, Some("main"), "tidy the stylesheet");
    } else {
        let workspace = Workspace::open(&website).expect("the directory exists");
        drop(Session::start(&sessions, &workspace, Some("main")).expect("a session log"));
    }
    earlier
}

/// Records one session in `root` on `branch` that asked `asked`, and closes
/// it.
fn recorded(sessions: &Path, root: &Path, branch: Option<&str>, asked: &str) -> SessionId {
    let workspace = Workspace::open(root).expect("the directory exists");
    let session = Session::start(sessions, &workspace, branch).expect("a session log");
    session.append(&Message::said(asked));
    session.id().expect("a recorded session has a name").clone()
}

/// `picture` with what it shows of `id` written as `<id>`, padded to the
/// width it took, so the baseline is of the screen rather than of one run.
pub(crate) fn unnamed(picture: &str, id: &SessionId) -> String {
    let id = id.as_str();
    let shown = (1..=id.len())
        .rev()
        .filter_map(|end| id.get(..end))
        .find(|start| picture.contains(&format!("--resume {start}")))
        .unwrap_or(id);
    let named = format!("{:<width$}", "<id>", width = shown.len());
    picture.replace(&format!("--resume {shown}"), &format!("--resume {named}"))
}
