//! What a session puts on screen before it asks for anything.
//!
//! The one place in the wiring that draws a component rather than a line. Its
//! job is to hand the component the facts it cannot know — the release, the
//! model, the directory — and to hand the renderer the terminal's answers about
//! colour and glyphs, both settled once at startup.

use crucible_core::Workspace;
use crucible_tui::{Recent, Renderer, Terminal, TerminalError, Welcome};

use crate::cli::style::Style;

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
    sessions: &[Recent<'_>],
    style: Style,
) -> Result<(), TerminalError> {
    let root = workspace.root().display().to_string();

    let welcome = Welcome {
        version: concat!("v", env!("CARGO_PKG_VERSION")),
        model,
        // Nothing crucible asks for yet. A line saying how hard the model is
        // being asked to think, drawn where no such request is being made,
        // would be the one thing on this screen that is not true.
        effort: None,
        root: &root,
        sessions,
    };

    renderer.present(
        &welcome.rows(renderer.columns(), style.glyphs()),
        style.palette(),
    )?;
    renderer.commit("")
}

#[cfg(test)]
mod tests {
    use crucible_tui::Recording;

    use super::*;

    /// What a session that has just started writes, to a terminal this wide or
    /// to a pipe standing in for one.
    fn opened(columns: usize, terminal: bool) -> String {
        let workspace = Workspace::open(std::env::temp_dir()).expect("a temporary directory");
        let recording = if terminal {
            Recording::new(columns, 24)
        } else {
            Recording::redirected(columns, 24)
        };
        let mut renderer = Renderer::new(recording);

        opening(
            &mut renderer,
            "claude-sonnet-5",
            &workspace,
            &[],
            Style::plain(),
        )
        .expect("the opening to draw");

        renderer.terminal().written().to_string()
    }

    #[test]
    fn a_session_opens_by_saying_what_it_is_asking_and_where() {
        let screen = opened(80, true);

        assert!(
            screen.contains(concat!("crucible v", env!("CARGO_PKG_VERSION"))),
            "{screen}"
        );
        assert!(screen.contains("claude-sonnet-5"), "{screen}");
        assert!(
            screen.contains(&std::env::temp_dir().display().to_string()),
            "{screen}"
        );

        // Nothing has happened in this directory, and this release cannot yet
        // find out otherwise.
        assert!(screen.contains("No recent sessions"), "{screen}");
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
