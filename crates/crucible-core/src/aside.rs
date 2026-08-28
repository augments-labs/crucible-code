//! Asides: a fact handed to a turn already running.
//!
//! Some things the session learns are not the reader speaking and not a tool
//! answering, and they arrive while the agent is mid-work: a command left
//! running has exited, and whatever the agent does next should be done knowing
//! it. The turn cannot ask — it was told not to poll — so the fact has to be
//! pushed at it.
//!
//! That is the difference from [`crate::Steer`]. Both are queues drained at the
//! same boundary, and the shape is deliberately the same one; what goes in them
//! is not. A steer is the reader's own words and joins the transcript as the
//! reader's turn. An aside is the harness's, and the text says so, because a
//! machine note recorded as somebody's typing is a transcript that misquotes
//! them — and one the reader is later shown, or resumes into, is a sentence
//! they never wrote.
//!
//! It is take-once, and that is the whole of its contract. A fact is owed to
//! the model exactly once: whoever drains it has delivered it, and whatever is
//! still in it when the turn ends was never delivered and is still owed. The
//! caller on the other side reads that back and puts it under the next turn.
//!
//! It lives in core beside [`crate::Steer`] and [`crate::Cancel`] because the
//! runner's exchange loop takes one, and core owns every type its own loop
//! names.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A shared "while you were working, this happened" queue.
///
/// Cloning shares the queue rather than copying it, so a clone handed to the
/// turn's thread sees what the drawing thread pushed.
#[derive(Clone, Default)]
pub struct Aside(Arc<Mutex<VecDeque<String>>>);

/// By hand, for the reason [`crate::Steer`]'s is written by hand: a note names
/// the command it is about, and a command line is where a token gets typed by
/// accident. How many are waiting is the whole of what a reader of a `{:?}`
/// needs.
impl std::fmt::Debug for Aside {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let notes = self.0.lock().map(|notes| notes.len()).unwrap_or_default();

        f.debug_struct("Aside")
            .field("notes", &format_args!("{notes} redacted"))
            .finish()
    }
}

impl Aside {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes a note the turn should be working with from its next pass on.
    ///
    /// Called on the thread that draws. The note is taken whole: what it says
    /// is the caller's, which is the one place that knows both the fact and the
    /// words the model is told it in.
    ///
    /// A poisoned lock is not a reason to lose the note, but it is a reason not
    /// to claim it was delivered — see [`Aside::say`]'s caller, which keeps
    /// what it could not hand over.
    pub fn say(&self, note: String) {
        if let Ok(mut notes) = self.0.lock() {
            notes.push_back(note);
        }
    }

    /// Whether anything is waiting to be worked in.
    ///
    /// Cheap and lock-light, so the exchange loop can ask it every pass without
    /// taking the lock when the answer is no — which is almost every pass.
    #[must_use]
    pub fn any(&self) -> bool {
        self.0.lock().is_ok_and(|notes| !notes.is_empty())
    }

    /// Takes every note waiting, oldest first.
    ///
    /// Take-once: what this returns has been delivered, and what it leaves
    /// behind is nothing. Called at the boundary between one pass and the next
    /// by the turn, and once more by the caller when the turn is over — which
    /// is how a note pushed after the last pass is still owed rather than lost.
    ///
    /// A poisoned lock yields nothing, and the note stays unsaid rather than
    /// being reported as said.
    pub fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut notes| notes.drain(..).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_never_shows_what_a_note_is_about() {
        // A note names the command it is about, and a command line is where a
        // token gets typed by accident.
        let aside = Aside::new();
        aside.say("#1 `curl -H aside-debug-canary` finished".to_owned());

        let shown = format!("{aside:?}");
        assert!(!shown.contains("aside-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }

    #[test]
    fn a_new_queue_has_nothing_to_say() {
        assert!(!Aside::new().any());
        assert!(Aside::new().take().is_empty());
    }

    #[test]
    fn a_clone_shares_the_notes_rather_than_copying_them() {
        let aside = Aside::new();
        let turn = aside.clone();

        aside.say("#1 finished".to_owned());
        aside.say("#2 failed with exit status 1".to_owned());

        assert!(turn.any());
        assert_eq!(
            turn.take(),
            vec![
                "#1 finished".to_owned(),
                "#2 failed with exit status 1".to_owned()
            ]
        );
        assert!(!aside.any(), "the drain emptied the shared queue");
    }

    #[test]
    fn a_note_is_given_up_once_and_not_twice() {
        // The whole of the contract: a fact is owed to the model exactly once,
        // and a turn that was told is a turn the next one does not re-tell.
        let aside = Aside::new();
        aside.say("#1 finished".to_owned());

        assert_eq!(aside.take(), vec!["#1 finished".to_owned()]);
        assert!(aside.take().is_empty(), "the note was already delivered");
        assert!(!aside.any());
    }
}
