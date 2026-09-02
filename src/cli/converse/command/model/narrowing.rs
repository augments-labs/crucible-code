//! Which of the models this build serves answer to what somebody has typed.
//!
//! The one place the domain decides what a query matches. It lives here rather
//! than beside the panel because a row is `provider/model` — two names of this
//! build's own — and the crate that draws is never told which provider it is
//! drawing.
//!
//! A query is read against four names per model, not one: the provider as it is
//! spelled for reading and as it is spelled for typing, and the same two for the
//! model. Somebody typing `openai`, `OpenAI`, `K2.7` or `kimi-for-coding` is
//! reaching for the same shelf, and nothing on screen says which of the four
//! they are looking at.

use crate::cli::{Providers, Served, offered};

use super::Selected;

/// Every model the catalogue serves, in the order the panel walks them.
pub(super) fn every(providers: &Providers) -> Vec<Selected> {
    offered(providers)
        .flat_map(|provider| {
            provider.models.iter().map(move |model| Selected {
                provider,
                model: *model,
            })
        })
        .collect()
}

/// Whether `selected` answers to `query`.
///
/// Case-folded substring, against the provider's shown name, the provider's own
/// name, the model's shown name and the model's own name. An empty query
/// answers everything.
pub(super) fn matches(selected: &Selected, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    // Folded on both sides. Folding the query alone answers the catalogue's
    // spelling and not the reader's; folding the catalogue alone answers the
    // reader's and not the catalogue's, and there is no third spelling either
    // of them agreed to.
    let query = query.to_lowercase();
    let Selected { provider, model } = selected;

    [provider.shown, provider.name, model.shown, model.name]
        .into_iter()
        .any(|name| name.to_lowercase().contains(&query))
}

/// The models `query` left, narrowed further to one provider where `only` names
/// it. `only` is `None` for the shelf's first pane — every provider at once.
///
/// A query that leaves nothing answers nothing. It does not widen back out: the
/// reader typed the thing that emptied it, and a panel disagreeing with the
/// search line above it is a panel they cannot read.
pub(super) fn shelved(all: &[Selected], query: &str, only: Option<&str>) -> Vec<Selected> {
    all.iter()
        .filter(|one| kept(one, query, only))
        .copied()
        .collect()
}

/// Every provider in the catalogue, in order, with how many models `query` left
/// each — `None` where it left none.
///
/// Every provider, always. The count is what says a provider is empty, and a
/// row that is not there says nothing at all.
pub(super) fn counted(
    providers: &Providers,
    all: &[Selected],
    query: &str,
) -> Vec<(Served, Option<usize>)> {
    offered(providers)
        .map(|provider| {
            let left = all
                .iter()
                .filter(|one| kept(one, query, Some(provider.name)))
                .count();

            (provider, (left > 0).then_some(left))
        })
        .collect()
}

/// Whether one row survives both narrowings, read together.
///
/// One predicate rather than two passes, so what a shelf holds and what a
/// provider's row counts cannot come apart: the second narrowing does not
/// forgive the first, and a provider the query emptied stays empty when it is
/// the marked one.
fn kept(selected: &Selected, query: &str, only: Option<&str>) -> bool {
    only.is_none_or(|name| selected.provider.name == name) && matches(selected, query)
}

#[cfg(test)]
mod tests;
