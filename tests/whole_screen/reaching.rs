//! How far `/resume` looks, on a real screen at a real size.
//!
//! Each case starts crucible in a home an earlier run would have left: two
//! sessions recorded in this directory on two branches, one in another
//! checkout of this repository, and one in another project. Each picture is
//! the picker after the keys a case is about, at eighty columns, which is the
//! width the keys row has to give up its long form in.

use std::fs;
use std::path::Path;

use crucible_session::Session;
use crucible_types::{Message, SessionId};
use crucible_workspace::Workspace;

use crate::watched::{self, Launch, Watched};

/// The configuration every case here starts with: nothing to answer, and no
/// confinement, as every other screen fixture.
const DOCUMENT: &str = concat!(
    "{\n",
    "  \"sandbox\": {\"enabled\": false},\n",
    "  \"updates\": {\"check\": \"never\"}\n",
    "}\n"
);

/// What a case's earlier run left, and the one session that is somewhere
/// else entirely.
struct Planted {
    window: Watched,
    website: SessionId,
}

/// Starts crucible in a home holding four sessions from before it started,
/// with `main` checked out here and another checkout of this repository beside
/// this one.
fn planted(case: &str) -> Planted {
    let scratch = watched::scratch(case);
    let here = watched::working(&scratch);
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

    let window = Watched::launched(
        case,
        80,
        24,
        &Launch {
            document: DOCUMENT,
            home: Some(&earlier),
            ..Launch::default()
        },
    );
    Planted { window, website }
}

/// Records one session in `root` on `branch` that asked `asked`, and closes
/// it.
fn recorded(sessions: &Path, root: &Path, branch: Option<&str>, asked: &str) -> SessionId {
    let workspace = Workspace::open(root).expect("the directory exists");
    let session = Session::start(sessions, &workspace, branch).expect("a session log");
    session.append(&Message::said(asked));
    session.id().expect("a recorded session has a name").clone()
}

#[test]
fn the_resume_picker_opens_on_this_directory() {
    let Planted { mut window, .. } = planted("reach-here");

    window.types_until("/resume\r", "a session, or a branch");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_a_shows_every_project_and_says_where_each_session_is() {
    let Planted { mut window, .. } = planted("reach-all");

    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x01", "all projects");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_w_adds_this_repositorys_other_checkouts() {
    let Planted { mut window, .. } = planted("reach-worktrees");

    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x17", "this repository's worktrees");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_b_keeps_the_branch_checked_out_here() {
    let Planted { mut window, .. } = planted("reach-branch");

    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x02", "1 of 1");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn enter_on_another_projects_session_says_how_to_resume_it_there() {
    let Planted {
        mut window,
        website,
    } = planted("reach-elsewhere");

    // Found by the directory its row shows, which nothing in what it asked
    // says.
    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x01", "all projects");
    window.types_until("website", "1 of 4");
    window.types_until("\r", "crucible --resume");

    // The picker is still standing, with the session's own tail beside it:
    // a row the keys put on the list is a row the pane can show.
    let picture = window.picture();
    assert!(picture.contains("Resume a session"), "{picture}");
    assert!(
        picture
            .lines()
            .any(|row| row.contains("│ │ › tidy the stylesheet")),
        "{picture}"
    );

    // The id is this run's own, and at eighty columns only its start fits.
    insta::assert_snapshot!(unnamed(&picture, &website));
}

/// `picture` with what it shows of `id` written as `<id>`, padded to the
/// width it took, so the baseline is of the screen rather than of one run.
fn unnamed(picture: &str, id: &SessionId) -> String {
    let id = id.as_str();
    let shown = (1..=id.len())
        .rev()
        .filter_map(|end| id.get(..end))
        .find(|start| picture.contains(&format!("--resume {start}")))
        .unwrap_or(id);
    let named = format!("{:<width$}", "<id>", width = shown.len());
    picture.replace(&format!("--resume {shown}"), &format!("--resume {named}"))
}
