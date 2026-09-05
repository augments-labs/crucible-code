//! What a session puts on screen before it asks for anything.
//!
//! The one place in the wiring that composes components rather than lines. Its
//! job is to hand them the facts they cannot know — the release, the directory
//! and recent sessions — and to keep the answer, because the facts are read
//! where the wiring has them and the rows are drawn where the renderer is.

use std::time::SystemTime;

use crucible_core::Workspace;
use crucible_runner::Recorded;
use crucible_tui::{
    Notice, Recent, Renderer, Row, Slot, Terminal, TerminalError, Welcome, fold,
};

use crate::cli::release::Newer;
use crate::cli::style::Style;

use super::when;

/// Everything the opening says that it cannot work out for itself.
///
/// A struct rather than six parameters, because four of them are `&str`-shaped
/// and a call with four of those in a row is one nobody can read.
pub(crate) struct Opening<'a> {
    /// The model this session will ask, or `None` where nothing chose one. Used
    /// only to decide whether the setup warning belongs under the card; the
    /// row under the prompt box owns model display.
    pub(crate) model: Option<&'a str>,
    /// What to say where there is none: which of the two halves of setting
    /// crucible up is the one still missing.
    pub(crate) unasked: &'a str,
    /// What reading the logins could not do, where it could not. `None` is the
    /// ordinary case, which includes a machine that has never logged in.
    pub(crate) trouble: Option<&'a str>,
    /// The directory being worked in.
    pub(crate) workspace: &'a Workspace,
    /// What was worked on here before, newest first.
    pub(crate) sessions: &'a [Recorded],
    /// A release newer than this one, where this machine has heard of one.
    pub(crate) update: Option<&'a Newer>,
    /// Whether to write colour, and what to draw with.
    pub(crate) style: Style,
}

/// The opening, read off the wiring and not yet laid out.
///
/// Owned rather than borrowed from whatever composed it, because it outlives
/// that call: what the card says is known where the session's files and logins
/// were opened, and the width it is drawn against is only known one call
/// further on, in the loop that owns the renderer. Between the two it is
/// facts — a clock read once, so four recent sessions are four ages measured
/// from one now. Cloned once, on the way into the record, so that what lays the
/// card out is still there when the window changes size under it.
#[derive(Clone)]
pub(crate) struct Standing {
    /// The directory being worked in, already shortened for drawing.
    root: String,
    /// What was worked on here before: what each session was about, and how
    /// long ago it was. Read once, at the one instant this was built, so four
    /// rows are four ages measured from one now.
    recent: Vec<(String, String)>,
    /// What a newer release is called and where to get it.
    update: Option<(String, String)>,
    /// What reading the logins could not do, where it could not.
    trouble: Option<String>,
    /// Which half of setting crucible up is still missing, where one is.
    unasked: Option<String>,
    /// Whether to write colour, and what to draw with.
    style: Style,
}

impl Standing {
    /// Everything the opening says, read off `opening` once.
    ///
    /// The clock is read here rather than per row: four rows drawn against four
    /// different instants would be four sessions dated from four different
    /// nows, and this is the only place that has one to read.
    pub(crate) fn new(opening: &Opening<'_>, now: SystemTime) -> Self {
        Self {
            root: opening.workspace.root().display().to_string(),
            recent: opening
                .sessions
                .iter()
                .map(|session| {
                    (
                        session.asked().to_owned(),
                        when::ago(session.started(), now),
                    )
                })
                .collect(),
            update: opening.update.map(|newer| {
                (
                    format!(
                        "New version {} is available. Download it from",
                        newer.version
                    ),
                    newer.from.to_string(),
                )
            }),
            trouble: opening.trouble.map(str::to_owned),
            unasked: opening.model.is_none().then(|| opening.unasked.to_owned()),
            style: opening.style,
        }
    }

    /// The whole opening, drawn for a terminal `columns` wide.
    ///
    /// The root is drawn because every tool path is relative to it, and a user
    /// who started crucible in the wrong directory should find out before the
    /// first tool call rather than after it.
    ///
    /// Rows rather than lines committed one at a time: these are rows crucible
    /// composed itself, so their colour is decided by the palette rather than
    /// arriving as escape bytes inside a string, and rows are what can be drawn
    /// again at another width.
    ///
    /// The order is the order of what the reader can act on. A release they do
    /// not have yet is a fact about the program and comes first; a session that
    /// cannot take a turn is a fact about this run and sits nearest the prompt
    /// where the answer gets typed. Every piece is followed by the blank row
    /// that keeps the next one off it.
    pub(crate) fn rows(&self, columns: usize) -> Vec<Row> {
        let glyphs = self.style.glyphs();
        let recent: Vec<Recent<'_>> = self
            .recent
            .iter()
            .map(|(title, when)| Recent { title, when })
            .collect();

        let welcome = Welcome {
            version: concat!("v", env!("CARGO_PKG_VERSION")),
            root: &self.root,
            sessions: &recent,
        };

        let mut rows = welcome.rows(columns, glyphs);
        rows.push(Row::new());

        if let Some((said, from)) = &self.update {
            let notice = Notice {
                heading: "Update Available",
                said,
                named: Some(from),
            };

            rows.extend(notice.rows(columns, glyphs));
            rows.push(Row::new());
        }

        // Above the sentence about there being nothing to ask, because it is
        // one of the reasons there might be nothing: a store that could not be
        // read is a login that did not count, and somebody who ran `/login`
        // yesterday would otherwise be told only that they have no key.
        if let Some(trouble) = &self.trouble {
            let said = format!("! {trouble}");
            rows.extend(
                fold(&said, columns)
                    .into_iter()
                    .map(|row| Row::new().then(Slot::Plain, row)),
            );
            rows.push(Row::new());
        }

        // Bold and in the accent, which is the loudest this program gets, and
        // not a notice between rules like the one above it: the rules are what
        // say a block is about the release rather than about this run, and this
        // one is about exactly this run.
        if let Some(unasked) = &self.unasked {
            rows.push(Row::new());
            rows.extend(
                fold(unasked, columns)
                    .into_iter()
                    .map(|row| Row::new().then(Slot::Strong, row)),
            );
            rows.push(Row::new());
        }

        rows
    }

    /// Writes the opening into the record, and hands over what draws it.
    ///
    /// Called before the session asks for anything, because the opening is the
    /// first thing in the transcript rather than something standing over the
    /// box: it scrolls with whatever is said after it. What goes over is a
    /// clone rather than the rows it makes, which is what lets the card be laid
    /// out for the window there is rather than the one there was — the facts it
    /// draws from were read once and never change again, so the only thing that
    /// can be out of date about it is the width.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub(crate) fn commit<T: Terminal>(
        &self,
        renderer: &mut Renderer<T>,
    ) -> Result<(), TerminalError> {
        let standing = self.clone();
        renderer.opens(Box::new(move |columns| standing.rows(columns)))
    }
}

/// Reads the opening off what only the wiring knows, ready to be drawn.
///
/// The width was read once when the renderer was built, which is before the
/// session's files were opened and the launch was resolved — a stretch on a slow
/// disk is long enough to have resized the window over. Asked again here so the
/// first frame is laid out against the terminal it is about to be drawn on
/// rather than the one that was there at launch.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be written to.
pub(crate) fn opening<T: Terminal>(
    renderer: &mut Renderer<T>,
    opening: &Opening<'_>,
) -> Result<Standing, TerminalError> {
    renderer.resized()?;
    Ok(Standing::new(opening, SystemTime::now()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crucible_core::Message;
    use crucible_runner::Session;

    use crate::cli::NOTHING_TO_ASK;
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
        shown(
            columns,
            terminal,
            &Opening {
                model: Some("claude-sonnet-5"),
                unasked: NOTHING_TO_ASK,
                workspace,
                sessions,
                trouble: None,
                update: None,
                style: Style::plain(),
            },
        )
    }

    /// Whatever the opening was given, as it reaches the terminal.
    fn shown(columns: usize, terminal: bool, opening: &Opening<'_>) -> String {
        let recording = if terminal {
            Recording::new(columns, 24)
        } else {
            Recording::redirected(columns, 24)
        };
        let mut renderer = Renderer::new(recording);

        super::opening(&mut renderer, opening)
            .expect("the opening to be read")
            .commit(&mut renderer)
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
    fn a_session_opens_by_saying_what_it_is_and_where() {
        let scratch = Scratch::new("asking");
        let screen = drawn(80, true, &scratch.workspace(), &[]);

        assert!(
            screen.contains(concat!("crucible v", env!("CARGO_PKG_VERSION"))),
            "{screen}"
        );
        assert!(!screen.contains("claude-sonnet-5"), "{screen}");

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

        let session = Session::start(&scratch.logs(), &workspace, None).expect("a new session");
        session.append(&Message::said("count the columns in the tail"));
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
        // last line of the opening. Asserted on the redirected path, where a row
        // ending is the only thing written between one row and the next.
        let screen = opened(80, false);

        assert!(screen.ends_with("\n\n"), "{screen}");
    }

    #[test]
    fn the_opening_never_reaches_a_row_it_did_not_write() {
        // A presented row is put down at a position the frame names, so a
        // sequence that moves the cursor or erases would write somewhere the
        // frame is not accounting for — a band that is not this one.
        for columns in [40, 46, 80, 200] {
            let screen = opened(columns, true);

            for upward in ["\x1b[2J", "\x1b[1J", "\x1b[3J", "\x1b[H", "\x1b[A"] {
                assert!(!screen.contains(upward), "{columns}: {screen:?}");
            }
        }
    }

    #[test]
    fn a_session_with_nothing_to_ask_says_so_under_the_welcome() {
        // The one screen somebody with a fresh install sees. It has to say both
        // halves — how to authenticate, and how to choose — because neither on
        // its own gets them to a turn.
        let workspace = Workspace::open(std::env::temp_dir()).expect("a temporary directory");
        let screen = shown(
            80,
            false,
            &Opening {
                model: None,
                unasked: NOTHING_TO_ASK,
                workspace: &workspace,
                sessions: &[],
                trouble: None,
                update: None,
                style: Style::plain(),
            },
        );

        assert!(screen.contains("No models available"), "{screen}");
        assert!(screen.contains("/model"), "{screen}");
    }

    #[test]
    fn the_card_says_nothing_about_the_live_turn_selection() {
        // All three facts are on the row under the prompt box. `/model` and
        // `/effort` change them long after this card has joined the transcript.
        let screen = opened(80, false);

        assert!(!screen.contains("claude-sonnet-5"), "{screen}");
        assert!(!screen.contains("effort"), "{screen}");
    }

    #[test]
    fn a_session_that_can_take_a_turn_says_nothing_about_setting_one_up() {
        let screen = opened(80, false);

        assert!(!screen.contains("No models available"), "{screen}");
    }

    #[test]
    fn a_newer_release_is_the_first_thing_under_the_welcome() {
        // Above the warning on purpose: a release is a fact about the program
        // and the warning is a fact about this run, so the one that is acted on
        // at the prompt sits nearest the prompt.
        let workspace = Workspace::open(std::env::temp_dir()).expect("a temporary directory");
        let newer = Newer {
            version: "9.9.9".into(),
            from: "https://example.test/releases".into(),
        };
        let screen = shown(
            80,
            false,
            &Opening {
                model: None,
                unasked: NOTHING_TO_ASK,
                workspace: &workspace,
                sessions: &[],
                trouble: None,
                update: Some(&newer),
                style: Style::plain(),
            },
        );

        let update = screen.find("Update Available").expect("the heading");
        let warning = screen.find("No models available").expect("the warning");

        assert!(screen.contains("New version 9.9.9"), "{screen}");
        assert!(screen.contains("https://example.test/releases"), "{screen}");
        assert!(update < warning, "{screen}");
    }

    #[test]
    fn a_release_nobody_has_heard_of_draws_nothing_at_all() {
        let screen = opened(80, false);

        assert!(!screen.contains("Update Available"), "{screen}");
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
