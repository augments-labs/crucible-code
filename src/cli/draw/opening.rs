//! What a session puts on screen before it asks for anything.
//!
//! The one place in the wiring that draws a component rather than a line. Its
//! job is to hand the component the facts it cannot know — the release, the
//! model, the directory — and to hand the renderer the terminal's answers about
//! colour and glyphs, both settled once at startup.

use std::time::SystemTime;

use crucible_core::Workspace;
use crucible_runner::Recorded;
use crucible_tui::{Recent, Renderer, Terminal, TerminalError, Welcome};

use crate::cli::style::Style;

use super::when;

/// Draws the welcome and leaves a row under it.
///
/// The root is drawn because every tool path is relative to it, and a user who
/// started crucible in the wrong directory should find out before the first
/// tool call rather than after it.
///
/// Through [`Renderer::present`] rather than `commit`: these are rows crucible
/// composed itself, so their colour is decided here by the palette rather than
/// arriving as escape bytes inside a string. The blank row afterwards is the
/// one thing the component does not draw — it says how wide it is, not what
/// follows it.
pub(crate) fn opening<T: Terminal>(
    renderer: &mut Renderer<T>,
    model: &str,
    workspace: &Workspace,
    sessions: &[Recorded],
    style: Style,
) -> Result<(), TerminalError> {
    let root = workspace.root().display().to_string();

    // The clock is read once, here, rather than per row: four rows drawn
    // against four different instants would be four sessions dated from four
    // different nows, and this is the only place that has one to read.
    let now = SystemTime::now();
    let when: Vec<String> = sessions
        .iter()
        .map(|session| when::ago(session.started(), now))
        .collect();

    let recent: Vec<Recent<'_>> = sessions
        .iter()
        .zip(&when)
        .map(|(session, when)| Recent {
            title: session.asked(),
            when,
        })
        .collect();

    let welcome = Welcome {
        version: concat!("v", env!("CARGO_PKG_VERSION")),
        model,
        // Nothing crucible asks for yet. A line saying how hard the model is
        // being asked to think, drawn where no such request is being made,
        // would be the one thing on this screen that is not true.
        effort: None,
        root: &root,
        sessions: &recent,
    };

    renderer.present(
        &welcome.rows(renderer.columns(), style.glyphs()),
        style.palette(),
    )?;
    renderer.commit("")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crucible_core::Message;
    use crucible_runner::Session;
    use crucible_tui::Recording;

    use super::*;

    /// What a session that has just started writes, to a terminal this wide or
    /// to a pipe standing in for one.
    fn opened(columns: usize, terminal: bool) -> String {
        let workspace = Workspace::open(std::env::temp_dir()).expect("a temporary directory");

        drawn(columns, terminal, &workspace, &[])
    }

    /// The same, for a directory that has been worked in.
    fn drawn(
        columns: usize,
        terminal: bool,
        workspace: &Workspace,
        sessions: &[Recorded],
    ) -> String {
        let recording = if terminal {
            Recording::new(columns, 24)
        } else {
            Recording::redirected(columns, 24)
        };
        let mut renderer = Renderer::new(recording);

        opening(
            &mut renderer,
            "claude-sonnet-5",
            workspace,
            sessions,
            Style::plain(),
        )
        .expect("the opening to draw");

        renderer.terminal().written().to_string()
    }

    /// A workspace and a sessions directory of their own, deleted with it.
    struct Scratch(PathBuf);

    /// What the workspace directory is called, on every machine and under every
    /// temporary directory. A test that looks for where it is working on screen
    /// looks for this: it is the last component of the path, which is the part
    /// shortening keeps.
    const WORKED_IN: &str = "worked-in";

    impl Scratch {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("crucible-opening-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join(WORKED_IN)).expect("a temporary directory");

            Self(base)
        }

        fn workspace(&self) -> Workspace {
            Workspace::open(self.0.join(WORKED_IN)).expect("the directory exists")
        }

        fn logs(&self) -> PathBuf {
            self.0.join("logs")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_session_opens_by_saying_what_it_is_asking_and_where() {
        let scratch = Scratch::new("asking");
        let screen = drawn(80, true, &scratch.workspace(), &[]);

        assert!(
            screen.contains(concat!("crucible v", env!("CARGO_PKG_VERSION"))),
            "{screen}"
        );
        assert!(screen.contains("claude-sonnet-5"), "{screen}");

        // The name of the directory rather than the whole path to it. A path
        // too wide for its column keeps its two ends and gives up the route
        // between them, and where a machine puts its temporary directory
        // decides whether this one is that wide — one column on Linux, five
        // nested directories on macOS. The name is the end that answers
        // "where", and it is the end shortening never drops.
        assert!(screen.contains(WORKED_IN), "{screen}");

        // A directory nobody has worked in. The heading stays either way: its
        // absence would be a different thing than its emptiness.
        assert!(screen.contains("No recent sessions"), "{screen}");
    }

    #[test]
    fn a_directory_that_has_been_worked_in_says_what_was_worked_on() {
        // Through a real session log, written by the thing that writes them:
        // what is being checked is that the wiring hands the component the
        // sessions rather than the empty list it was given for a release.
        let scratch = Scratch::new("worked");
        let workspace = scratch.workspace();

        let session = Session::start(&scratch.logs(), &workspace).expect("a new session");
        session.append(&Message::User("count the columns in the tail".into()));
        // Dropping is what waits for the queue, so the file is whole after it.
        drop(session);

        let sessions = crucible_runner::recent(&scratch.logs(), &workspace, Welcome::WANTED);
        assert_eq!(sessions.len(), 1, "the session that was just recorded");

        let screen = drawn(80, true, &workspace, &sessions);

        assert!(screen.contains("count the columns in the tail"), "{screen}");
        assert!(screen.contains("just now"), "{screen}");
        assert!(!screen.contains("No recent sessions"), "{screen}");
    }

    #[test]
    fn the_opening_leaves_a_row_between_itself_and_the_first_turn() {
        // Without it the first thing the model says starts on the row under the
        // frame. Asserted on the redirected path, where a row ending is the
        // only thing written between one row and the next.
        let screen = opened(80, false);

        assert!(screen.ends_with("╯\n\n"), "{screen}");
    }

    #[test]
    fn the_opening_never_reaches_a_row_it_did_not_write() {
        // `present` writes above whatever comes after it and is never redrawn
        // over, so a sequence that moves the cursor up or erases upward would
        // take a row belonging to the terminal's own scrollback.
        for columns in [40, 46, 80, 200] {
            let screen = opened(columns, true);

            for upward in ["\x1b[2J", "\x1b[1J", "\x1b[3J", "\x1b[H", "\x1b[A"] {
                assert!(!screen.contains(upward), "{columns}: {screen:?}");
            }
        }
    }

    #[test]
    fn a_redirected_run_is_drawn_the_same_component_without_the_frame_sequences() {
        // What `scripts/bench.sh` reads, and what somebody piping a session into
        // a file keeps. The rows are the same rows; what a pipe does not get is
        // the carriage returns and the erase that only mean anything on a
        // screen.
        let screen = opened(80, false);

        assert!(screen.contains("crucible v"), "{screen}");
        assert!(!screen.contains('\r'), "{screen:?}");
        assert!(!screen.contains('\x1b'), "{screen:?}");
    }
}
