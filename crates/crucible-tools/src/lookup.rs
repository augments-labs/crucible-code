//! Looking up a tool that was not advertised.
//!
//! A schema the model can see is a schema it pays for on every request of every
//! turn, and most sessions never search the web or write a plan. So some tools
//! are registered without being shown, and this is how the model reaches them:
//! it describes what it wants, and what matches is offered from that moment on.
//!
//! What comes back is a name and a sentence, never a schema. The schema arrives
//! the way every other one does — in the next request's tool list — and printing
//! it here would spend the tokens twice over, which is the whole thing this
//! exists to avoid.
//!
//! Matching is deliberately dull: the words of the query against the name and
//! the description, a name given exactly always winning. A model that knows what
//! it wants should be able to say so and be right, and one that is guessing
//! should not be silently given the wrong tool because a cleverer ranking
//! preferred it.
//!
//! Two bounds on what one call can offer, and both are about the same hole. A
//! query matching everything, or an empty one matching by vacuum, would reveal
//! the whole registry in a single call and leave deferral meaning nothing — so
//! a query with no words in it finds nothing at all, and a search offers at
//! most [`MOST`] tools, best first. The model can search again; what it cannot
//! do is ask once and be handed everything.

use std::fmt::Write as _;
use std::sync::LazyLock;

use crucible_core::{
    Approved, DescribeTool, Revealed, Sensitivity, Summary, Target, Tool, ToolArgs, ToolContext,
    ToolEffect, ToolError, ToolOutput,
};

use crate::args::Args;
use crate::schema::{Field, Schema, Shape};
use crate::summary;

#[cfg(test)]
mod tests;

const NAME: &str = "tool_search";

/// What the model wants to do.
const QUERY: &str = "query";

/// The most one search may offer.
///
/// A bound rather than a ranking preference. Without it a broad enough query is
/// a way to ask for the whole registry at once, which is deferral undone by the
/// one tool that is always advertised.
const MOST: usize = 3;

/// The root `description` is the tool's own; the one argument is the query.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    Schema {
        about: "Finds tools that are not in your current tool list and makes them available. \
                Some tools are held back until asked for, so this list is not everything that \
                exists. Search when a task needs something you cannot see — reaching the web, \
                for instance. What you find is callable from your next message onward."
            .into(),
        fields: vec![Field {
            name: QUERY,
            about: "What you want to do, in a word or two, for example web search or plan. A \
                    tool's exact name always matches itself. The closest few are offered, so ask \
                    for one job at a time rather than everything at once."
                .into(),
            needed: true,
            shape: Shape::Text,
        }],
    }
    .text()
});

/// One tool that is held back, as this one describes it.
#[derive(Debug, Clone)]
pub struct Held {
    /// What the model would call.
    pub name: Box<str>,
    /// The first sentence of what its schema says it does.
    pub about: Box<str>,
}

/// Finds tools that were not advertised.
#[derive(Debug)]
pub struct ToolSearch {
    held: Vec<Held>,
    revealed: Revealed,
}

impl ToolSearch {
    /// A search over `held`, revealing into `revealed`.
    #[must_use]
    pub fn new(held: Vec<Held>, revealed: Revealed) -> Self {
        Self { held, revealed }
    }

    /// Whether there is anything to look up.
    ///
    /// A session that defers nothing should not advertise this: a tool that can
    /// only ever answer "nothing found" is a schema spent to say so.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

impl DescribeTool for ToolSearch {
    fn name(&self) -> &str {
        NAME
    }

    fn schema(&self) -> &str {
        SCHEMA.as_str()
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::ReadOnly
    }
}

impl Tool for ToolSearch {
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
        Args::parse(NAME, args)?.text(QUERY).map(drop)
    }

    /// Reads nothing and reaches nothing. It changes what this session offers
    /// and touches no file, no process and no network, so there is no target to
    /// name and nothing a rule could usefully be written about.
    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, QUERY)
    }

    fn run(&self, approved: Approved, _context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let query = args.text(QUERY)?;

        let mut ranked: Vec<(u8, &Held)> = self
            .held
            .iter()
            .filter_map(|held| score(query, held).map(|score| (score, held)))
            .collect();

        // Best first, and ties in the order they were registered — which is the
        // order the wiring thought sensible, and is at least an answer that does
        // not move between two identical searches.
        ranked.sort_by(|(one, _), (two, _)| two.cmp(one));
        let found: Vec<&Held> = ranked
            .into_iter()
            .take(MOST)
            .map(|(_, held)| held)
            .collect();

        if found.is_empty() {
            return Ok(ToolOutput::ok(format!(
                "Nothing held back matches {query}. Everything else you can \
                 call is already in your tool list."
            )));
        }

        let mut said = String::new();
        for held in &found {
            self.revealed.reveal(&held.name);
            let _ = writeln!(said, "{}: {}", held.name, held.about);
        }

        said.push_str(if found.len() == 1 {
            "\nIt is in your tool list from your next message onward."
        } else {
            "\nThey are in your tool list from your next message onward."
        });

        Ok(ToolOutput::ok(said))
    }
}

/// How well `query` asks for this tool, or nothing where it does not.
///
/// The name outranks the description, because a word in a name is a model
/// naming the thing and a word in a description is a model describing a job.
/// Both are looked for; neither is clever.
///
/// **A query with no usable words scores nothing at all.** That is the case
/// worth stating: an empty query matching everything by vacuum would make the
/// one tool that is always advertised into a way of asking for the whole
/// registry, which is exactly what deferring them was for.
fn score(query: &str, held: &Held) -> Option<u8> {
    let name = held.name.to_lowercase();
    let asked = query.to_lowercase();

    if asked.trim() == name {
        return Some(u8::MAX);
    }

    let words: Vec<&str> = asked
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 2)
        .collect();

    if words.is_empty() {
        return None;
    }

    let about = held.about.to_lowercase();
    let mut score = 0u8;
    for word in words {
        if name.contains(word) {
            score = score.saturating_add(2);
        } else if about.contains(word) {
            score = score.saturating_add(1);
        }
    }

    (score > 0).then_some(score)
}
