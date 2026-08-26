//! Reaching back through what was asked here before.
//!
//! The arrows already mean two things at the box: a line of many rows moves
//! within itself, and a slash list standing over the box is walked. This is the
//! third and last of them, and it is the one that answers when neither of the
//! others has anything to move — a one-row line with no list open, which is
//! what the box is nearly all of the time.
//!
//! A walk is a place in a list and the line it interrupted. The line matters as
//! much as the place: somebody reaching back to check how they phrased
//! something last week has not finished writing what they were writing, and an
//! arrow that could only go backwards would have thrown it away.
//!
//! The walk ends at the first edit, and the top border stops saying where the
//! line came from at the same moment. That is the whole of the rule, and it is
//! what makes the number honest: while it is up, the line in the box is a
//! retained prompt exactly; the moment it is not, the number is gone.
//!
//! The window is the store's, and it is the same number in three places: what
//! the store keeps for this directory, what is held here between two reads of
//! it, and what the border counts the walk against. One constant rather than
//! three, so they cannot drift into disagreeing about how far back is back.

use std::path::PathBuf;

use crucible_core::Workspace;
use crucible_runner::{PROMPTS, prompts, remember};
use crucible_tui::{Editor, Recalled, Typed};

/// Where the walk stands, what it is walking, and where it started.
#[derive(Debug, Default)]
pub(super) struct Recalling {
    /// The prompts this directory holds, oldest last-reachable first and the
    /// newest at the end — the order the walk goes back through.
    held: Vec<String>,
    /// How far back the walk has gone, as an index into [`Recalling::held`].
    /// Nothing while nobody is walking, which is the resting state.
    at: Option<usize>,
    /// The line that was in the box when the walk started, given back when it
    /// walks past the newest prompt again.
    kept: Option<String>,
    /// The line the walk last put in the box, as the box holds it.
    ///
    /// What [`Recalling::standing`] reads. Kept rather than compared against
    /// the retained prompt it came from, because the box sanitizes what it is
    /// given and a prompt it changed on the way in would read as an edit
    /// nobody made — which would end the walk on the next key with the line
    /// untouched.
    stood: String,
    /// Where a finished line is written down, or nothing for a walk that only
    /// lives as long as the process.
    store: Option<Store>,
}

/// The two facts a prompt has to be written down against.
#[derive(Debug)]
struct Store {
    /// Where this machine keeps its session logs, which is where the history
    /// lives too — one file for the machine, filtered by the directory below.
    sessions: PathBuf,
    /// The directory this conversation is about, which decides which of the
    /// retained prompts are this one's.
    workspace: Workspace,
}

impl Recalling {
    /// What this directory has been asked before, and somewhere to add to it.
    ///
    /// A history that would not read is an empty one. It is a key nobody has
    /// to press: refusing to start a session over it would trade the thing
    /// somebody asked for against the convenience of asking it again.
    pub(super) fn new(sessions: PathBuf, workspace: Workspace) -> Self {
        let held = prompts(&sessions, &workspace).unwrap_or_default();

        Self {
            held,
            at: None,
            kept: None,
            stood: String::new(),
            store: Some(Store {
                sessions,
                workspace,
            }),
        }
    }

    /// One step further back, or `false` where there is none.
    ///
    /// The line standing in the box is kept on the first step of a walk and
    /// not on the ones after: what a walk interrupted is the line somebody was
    /// writing, and the prompts it steps over are not that.
    pub(super) fn back(&mut self, editor: &mut Editor) -> bool {
        let next = match self.at {
            None => self.held.len().checked_sub(1),
            Some(at) => at.checked_sub(1),
        };
        let Some(next) = next else {
            return false;
        };

        if self.at.is_none() {
            self.kept = Some(editor.text().to_owned());
        }

        self.stands(editor, next)
    }

    /// One step forward, back to the interrupted line past the newest prompt.
    ///
    /// `false` where no walk is open, so the key goes on meaning whatever else
    /// it means at a box nobody has reached back from.
    pub(super) fn on(&mut self, editor: &mut Editor) -> bool {
        let Some(at) = self.at else {
            return false;
        };

        if at + 1 < self.held.len() {
            return self.stands(editor, at + 1);
        }

        // Past the newest, which is where the walk gives the line back and
        // ends. Empty where the box was empty, which it usually was.
        let kept = self.kept.take().unwrap_or_default();
        editor.put(&kept);
        self.left();
        true
    }

    /// Where the border says the line came from, or nothing at all.
    ///
    /// How many presses back the walk has come, so the first one is `1` and the
    /// number rises with the key. What a reader is keeping track of while they
    /// hold the arrow down is how far back they have gone, not which slot of a
    /// file they have landed in.
    ///
    /// Counted against the window rather than against how much of it is filled,
    /// so the second number is the one fact about the history that never moves.
    /// A count that grew with every prompt sent would make `1/3` and `1/4` mean
    /// the same press, and a reader watching the border would be told the
    /// history had changed size when what changed was that they had typed.
    pub(super) fn place(&self) -> Recalled {
        self.at.map_or_else(Recalled::default, |at| {
            Recalled::new(self.held.len() - at, PROMPTS)
        })
    }

    /// Ends the walk, leaving the line where it is.
    ///
    /// Called by every key that changes the line. What it takes away is the
    /// claim the border was making — that what stands in the box is a retained
    /// prompt — which stopped being true on that key.
    pub(super) fn left(&mut self) {
        self.at = None;
        self.kept = None;
        self.stood.clear();
    }

    /// Ends the walk where the line is no longer the one it put there.
    ///
    /// Asked once after every key the box has read, rather than named at each
    /// of the arms that edit: the arms are a list kept in step by prose, and
    /// the one that gets forgotten is a border still claiming the line came
    /// out of the history after somebody has rewritten it.
    ///
    /// A key that only moved the cursor leaves the walk standing, because the
    /// claim the border makes is about the line and not about where in it
    /// somebody is looking.
    pub(super) fn standing(&mut self, editor: &Editor) {
        if self.at.is_some() && editor.text() != self.stood {
            self.left();
        }
    }

    /// Puts a finished line last, in memory and on disk, and ends the walk.
    ///
    /// A line of nothing but spaces is not one somebody would reach back for,
    /// and it is what an empty box submits on the platforms that let it.
    pub(super) fn keep(&mut self, said: &str) {
        self.left();
        if said.trim().is_empty() {
            return;
        }

        self.held.push(said.to_owned());
        // The store's own window, because what is held here between two
        // reads of the file has to be what the next read would give back:
        // a session that asks a thousand things keeps the last hundred.
        self.held.drain(..self.held.len().saturating_sub(PROMPTS));

        // Total, for the reason [`Recalling::new`] gives about reading: the
        // prompt has been asked and the turn is about to run, and a file that
        // would not take it is not a reason to stop.
        if let Some(store) = &self.store {
            drop(remember(&store.sessions, &store.workspace, said));
        }
    }

    /// Puts `held[at]` in the box and stands the walk there.
    ///
    /// Always a frame, even where the line was already what it is about to be:
    /// the border is what moved, and two prompts in a row that read the same
    /// are two different places in the history.
    fn stands(&mut self, editor: &mut Editor, at: usize) -> bool {
        let Some(said) = self.held.get(at) else {
            return false;
        };

        // Refused cannot happen — the store's per-prompt bound is far under
        // the editor's — and a line the box would not take is one the walk has
        // no way to show, so it stands where it was.
        if editor.put(said) == Typed::Refused {
            return false;
        }

        self.stood = editor.text().to_owned();
        self.at = Some(at);
        true
    }
}

#[cfg(test)]
impl Recalling {
    /// A walk over prompts that came from nowhere and go nowhere.
    pub(super) fn holding(held: Vec<String>) -> Self {
        Self {
            held,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests;
