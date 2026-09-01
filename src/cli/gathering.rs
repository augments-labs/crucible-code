//! A run of calls that only looked around, as one line of the transcript.
//!
//! A turn that read four files, searched for a pattern and listed two
//! directories has done one thing, and the reader who scrolls past it is
//! looking for what came of it rather than for seven rows saying it happened.
//! So the run is gathered here and drawn once: a line while it is going, and a
//! line once it has settled.
//!
//! What is gathered is a count and a list of call identities — never the calls'
//! own words. Those are already held by [`Kept`](super::kept::Kept), which is
//! where the reader who opens the settled line is sent, and a second copy of
//! them here would be a second thing to bound and a second thing to drop from.
//!
//! Nothing is folded away. Every call in the run keeps its result, and the
//! settled line is the row that offers all of them at once; folding is about
//! how many rows a reader scrolls past, and never about what they can still
//! reach.

use crucible_core::{Looking, ToolId, ToolOutput};

/// How the counters are said, in the order they are said in.
///
/// One table rather than a match per tense, because the two tenses of a counter
/// and its two numbers are four spellings of one thing, and four spellings kept
/// apart are four chances for `directory` to come back as `directorys`.
const COUNTERS: [Counter; 4] = [
    Counter {
        doing: "searching for",
        did: "searched for",
        one: "pattern",
        many: "patterns",
    },
    Counter {
        doing: "reading",
        did: "read",
        one: "file",
        many: "files",
    },
    Counter {
        doing: "listing",
        did: "listed",
        one: "directory",
        many: "directories",
    },
    Counter {
        doing: "running",
        did: "ran",
        one: "command",
        many: "commands",
    },
];

/// How many calls a run has to hold before it is worth folding.
///
/// Two, because one call folded into a count of one reads as an evasion: the
/// row it replaced was the same length and said which file it was.
pub(crate) const FOLDS: usize = 2;

/// One counter, in the tenses and numbers it is said in.
struct Counter {
    /// While the run is still going.
    doing: &'static str,
    /// Once it has settled.
    did: &'static str,
    /// The noun, where there was one.
    one: &'static str,
    /// The noun, where there was more than one.
    many: &'static str,
}

/// The one call a run is still holding back, in case it turns out to be alone.
///
/// A run of one is not folded, so its row is the row it always was — and that
/// row can only be written once it is known no second call is coming. So the
/// first call of every run waits here for the length of one event, and is
/// either let go as itself or taken into the count.
///
/// One at most, whatever the run comes to. The second call settles the
/// question, and everything after it goes straight to
/// [`Kept`](super::kept::Kept) where results are already bounded.
#[derive(Debug)]
pub(crate) struct Alone {
    /// Which call it was.
    pub(crate) call: ToolId,
    /// The words its row would say.
    pub(crate) said: String,
    /// What it came back with, where it has come back at all. `None` is a call
    /// the turn ended underneath, which has a row to write and no result to
    /// hang under it.
    pub(crate) output: Option<ToolOutput>,
}

/// The calls in one run, and what they were looking at.
///
/// Counted by kind rather than kept in arrival order, because the line says one
/// order whatever order they came in — a reader comparing two turns is reading
/// the same sentence twice, and a sentence whose clauses move is two sentences.
#[derive(Debug, Default)]
pub(crate) struct Gathering {
    /// How many of each, indexed as [`COUNTERS`] is.
    counted: [usize; COUNTERS.len()],
    /// Which calls, so that opening the settled line opens all of them.
    calls: Vec<ToolId>,
    /// The first call, held back until the run has a second one.
    alone: Option<Alone>,
}

impl Gathering {
    /// Takes one more call into the run.
    ///
    /// Hands back the call the run was holding, where this is the one that
    /// settles it: the run has two calls now, so the first will not be written
    /// as a row of its own and its result belongs where the rest of the run's
    /// results are.
    pub(crate) fn took(&mut self, call: ToolId, looking: Looking, said: String) -> Option<Alone> {
        self.counted(call.clone(), looking);

        if self.calls.len() == 1 {
            self.alone = Some(Alone {
                call,
                said,
                output: None,
            });
            return None;
        }

        self.alone.take()
    }

    /// Counts one more call into the run, holding none of them back.
    ///
    /// For a reader of a session rather than a watcher of one. The walk that
    /// puts a session back on the screen has the whole transcript in front of
    /// it, so it knows how long every run is before it draws a row and never
    /// has to hold a call back to find out. [`Gathering::took`] is for the
    /// turn, which does not.
    pub(crate) fn counted(&mut self, call: ToolId, looking: Looking) {
        if let Some(counted) = self.counted.get_mut(at(looking)) {
            *counted += 1;
        }
        self.calls.push(call);
    }

    /// Keeps what the call the run is holding came back with.
    ///
    /// Hands it back where it was not kept, which is every other call in the
    /// run: its result belongs in [`Kept`](super::kept::Kept) at once, since
    /// there is no longer a row it might be written under on its own.
    pub(crate) fn answered(&mut self, call: &ToolId, output: ToolOutput) -> Option<ToolOutput> {
        match self.alone.as_mut().filter(|alone| alone.call == *call) {
            Some(alone) => {
                alone.output = Some(output);
                None
            }
            None => Some(output),
        }
    }

    /// Whether this call is one the run is counting.
    pub(crate) fn holds(&self, call: &ToolId) -> bool {
        self.calls.iter().any(|one| one == call)
    }

    /// The call the run was holding back, where it never found a second one.
    pub(crate) fn alone(&mut self) -> Option<Alone> {
        self.alone.take()
    }

    /// How many calls are in it.
    pub(crate) fn len(&self) -> usize {
        self.calls.len()
    }

    /// Whether it holds enough to be worth folding.
    pub(crate) fn folds(&self) -> bool {
        self.len() >= FOLDS
    }

    /// The calls it holds, in the order they were made.
    pub(crate) fn calls(&self) -> &[ToolId] {
        &self.calls
    }

    /// Empties it, handing back what it held.
    pub(crate) fn taken(&mut self) -> Self {
        std::mem::take(self)
    }

    /// The line while the run is still going.
    pub(crate) fn doing(&self) -> String {
        self.words(|counter| counter.doing)
    }

    /// The line once the run has settled.
    pub(crate) fn did(&self) -> String {
        self.words(|counter| counter.did)
    }

    /// The line, with the verbs the tense asks for.
    fn words(&self, tense: fn(&Counter) -> &'static str) -> String {
        let mut said = String::new();

        for (counter, &count) in COUNTERS.iter().zip(&self.counted) {
            // A counter nothing was counted against is not said at all. A run
            // that read four files did not search for nothing, and a line
            // saying so would be four words of noise on every row.
            if count == 0 {
                continue;
            }

            if !said.is_empty() {
                said.push_str(", ");
            }

            let noun = if count == 1 {
                counter.one
            } else {
                counter.many
            };
            said.push_str(tense(counter));
            said.push(' ');
            said.push_str(&count.to_string());
            said.push(' ');
            said.push_str(noun);
        }

        opened(said)
    }
}

/// Which counter a kind of looking is counted against.
///
/// A match rather than a discriminant, so that a fifth kind of looking is a
/// compile error here rather than a call counted against `patterns`.
fn at(looking: Looking) -> usize {
    match looking {
        Looking::Pattern => 0,
        Looking::File => 1,
        Looking::Directory => 2,
        Looking::Command => 3,
    }
}

/// The line with its first letter raised.
///
/// Only the first, because the rest are the middle of a sentence: `Searching
/// for 1 pattern, reading 2 files` is one line and not two labels.
fn opened(said: String) -> String {
    let mut characters = said.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => said,
    }
}

#[cfg(test)]
mod tests;
