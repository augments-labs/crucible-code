//! What a query leaves, against the catalogue this build actually ships.
//!
//! Against the shipped list rather than a fixture on purpose: the thing worth
//! catching is a query that stopped matching because a model was renamed, and a
//! fixture is exactly the thing that goes on agreeing after that happens.

use crate::cli::offered;

use super::*;

/// The built-in providers, as one generation to read a query against.
fn catalogue() -> Providers {
    crate::cli::providers()
        .expect("the built-in providers register")
        .snapshot()
}

/// How many models one provider serves, found by its own name.
fn serves(name: &str) -> usize {
    offered(&catalogue())
        .find(|one| one.name == name)
        .map_or(0, |one| one.models.len())
}

/// The provider name on every row of a narrowing, deduplicated in order.
fn providers(shelf: &[Selected]) -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for one in shelf {
        if !seen.contains(&one.provider.name) {
            seen.push(one.provider.name);
        }
    }
    seen
}

#[test]
fn a_query_naming_a_provider_the_way_it_is_read_shows_everything_it_serves() {
    // "OpenAI" is the spelling on the row, and the one nothing is ever matched
    // against anywhere else in this build. Somebody reading the panel has no
    // way to know that, so it is a spelling the search line has to answer to.
    let shelf = shelved(&every(&catalogue()), "OpenAI", None);

    assert_eq!(providers(&shelf), ["openai"]);
    assert_eq!(shelf.len(), serves("openai"));
}

#[test]
fn a_query_naming_a_provider_the_way_it_is_typed_shows_the_same_shelf() {
    let shelf = shelved(&every(&catalogue()), "moonshot", None);

    assert_eq!(providers(&shelf), ["moonshot"]);
    assert_eq!(shelf.len(), serves("moonshot"));
}

#[test]
fn a_query_naming_a_model_the_way_it_is_read_finds_what_its_own_name_never_says() {
    // The product name and the wire identifier are different strings for these,
    // and the one on the row is the one nobody could have guessed from the
    // other. A shelf that matched only what `--model` carries would answer
    // nothing to the name the reader is looking straight at.
    let shelf = shelved(&every(&catalogue()), "K2.7", None);

    assert!(!shelf.is_empty());
    for one in &shelf {
        assert!(one.model.shown.to_lowercase().contains("k2.7"));
        assert!(!one.model.name.to_lowercase().contains("k2.7"));
    }
}

#[test]
fn a_query_is_folded_on_both_sides_rather_than_on_the_one_that_was_typed() {
    // Folding the query alone matches "anthropic" and not "Anthropic"; folding
    // the catalogue alone does the reverse. Somebody typing at a search line
    // does neither on purpose.
    for typed in ["anthropic", "ANTHROPIC", "Anthropic", "aNtHrOpIc"] {
        let shelf = shelved(&every(&catalogue()), typed, None);

        assert_eq!(providers(&shelf), ["anthropic"], "{typed}");
        assert_eq!(shelf.len(), serves("anthropic"), "{typed}");
    }
}

#[test]
fn an_empty_query_answers_every_model_in_the_order_the_catalogue_holds_them() {
    let all = every(&catalogue());
    let shelf = shelved(&all, "", None);

    let served: usize = offered(&catalogue()).map(|one| one.models.len()).sum();
    assert_eq!(shelf.len(), served);
    assert_eq!(providers(&shelf), providers(&all));
    assert_eq!(
        providers(&shelf),
        offered(&catalogue())
            .map(|one| one.name)
            .collect::<Vec<_>>(),
        "the walk is the catalogue's order, not a sorted one"
    );
}

#[test]
fn a_query_matching_nothing_answers_nothing_rather_than_widening_back_out() {
    // The tempting mercy is to show everything again when a query empties the
    // shelf. It is the panel telling the reader their query did not happen.
    assert!(shelved(&every(&catalogue()), "zzzz-no-such-model", None).is_empty());
}

#[test]
fn every_provider_keeps_its_row_and_what_the_rows_count_is_what_is_shelved() {
    let all = every(&catalogue());

    for query in ["", "openai", "k3", "claude", "zzzz-no-such-model"] {
        let rows = counted(&catalogue(), &all, query);

        assert_eq!(
            rows.iter().map(|(one, _)| one.name).collect::<Vec<_>>(),
            offered(&catalogue())
                .map(|one| one.name)
                .collect::<Vec<_>>(),
            "{query}: a provider a query emptied is still a provider"
        );

        let counted: usize = rows.iter().filter_map(|(_, left)| *left).sum();
        assert_eq!(counted, shelved(&all, query, None).len(), "{query}");
    }
}

#[test]
fn a_provider_a_query_emptied_is_told_apart_from_one_it_left_alone() {
    let rows = counted(&catalogue(), &every(&catalogue()), "openai");

    for (provider, left) in rows {
        if provider.name == "openai" {
            assert_eq!(left, Some(serves("openai")));
        } else {
            assert_eq!(left, None, "{}", provider.name);
        }
    }
}

#[test]
fn narrowing_to_one_provider_answers_that_provider_alone() {
    let all = every(&catalogue());
    let shelf = shelved(&all, "", Some("moonshot"));

    assert_eq!(providers(&shelf), ["moonshot"]);
    assert_eq!(shelf.len(), serves("moonshot"));
}

#[test]
fn a_provider_the_query_emptied_answers_nothing_rather_than_all_it_serves() {
    // The two narrowings are read together, not one after the other with the
    // second forgiving the first. A marked row that the search line has emptied
    // shows an empty shelf, which is what the search line says it should.
    assert!(shelved(&every(&catalogue()), "openai", Some("anthropic")).is_empty());
}
