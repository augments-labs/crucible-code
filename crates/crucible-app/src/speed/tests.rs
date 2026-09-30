//! The fast table in the docs, held to the words each provider answers with.

use crucible_models::FastForm;
use crucible_provider::OpenAi;

use crate::providers::{Served, offered, providers};

/// The page the fast forms are documented on.
const DOCUMENTED: &str = include_str!("../../../../docs/providers/fast.md");

/// The cells of every row of the fast table, the header and rule left out.
fn rows() -> Vec<Vec<String>> {
    DOCUMENTED
        .lines()
        .filter(|line| line.starts_with("| ") && !line.starts_with("| ---"))
        .map(|line| {
            line.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect::<Vec<_>>()
        })
        .filter(|cells| cells.len() == 8 && cells.first().is_some_and(|first| first != "Provider"))
        .collect()
}

/// The names written between backticks in `cell`.
fn named(cell: &str) -> Vec<&str> {
    cell.split('`').skip(1).step_by(2).collect()
}

/// How `model` is asked to answer fast under `credential`, as its provider
/// answers.
fn form(served: Served, credential: &str, model: &str) -> FastForm {
    match (served.name, credential) {
        ("openai", "sign-in") => OpenAi::fast_signed_in(model),
        _ => (served.fast)(model),
    }
}

fn cell(row: &[String], at: usize) -> &str {
    row.get(at).map_or("", String::as_str)
}

/// Every model the table names has the fast form its row says, in the words
/// the panel shows, with its source and the day it was read; and every model
/// this build offers with a fast form has a row.
#[test]
fn every_fast_form_is_its_row_of_the_docs_table() {
    let catalogue = providers().expect("the built-in providers").snapshot();
    let every: Vec<Served> = offered(&catalogue).collect();
    let table = rows();
    assert!(!table.is_empty(), "the page has no fast table");

    let mut documented = Vec::new();
    for row in &table {
        let served = every
            .iter()
            .copied()
            .find(|served| served.shown == cell(row, 0))
            .unwrap_or_else(|| panic!("{row:?} names no provider"));
        let credential = cell(row, 1);
        assert!(
            !cell(row, 6).is_empty() && !cell(row, 7).is_empty(),
            "{row:?}"
        );

        for model in named(cell(row, 2)) {
            let cost = match form(served, credential, model) {
                FastForm::Field(cost) => {
                    assert!(!cell(row, 2).contains("of its own"), "{row:?}");
                    cost
                }
                FastForm::Own(cost) => {
                    assert!(cell(row, 2).contains("of its own"), "{row:?}");
                    cost
                }
                FastForm::None => panic!("{model} has no fast form under {credential}"),
            };
            assert_eq!(cost.price, cell(row, 3), "{model}");
            assert_eq!(cost.caveat.unwrap_or_default(), cell(row, 4), "{model}");
            documented.push((served.name, credential.to_owned(), model.to_owned()));
        }
    }

    for served in &every {
        for model in served.models {
            let credentials: &[&str] = match served.name {
                "openai" => &["key", "sign-in"],
                "moonshot" => &["any"],
                _ => &["key"],
            };
            for credential in credentials {
                if form(*served, credential, model.name) != FastForm::None {
                    let row = (served.name, (*credential).to_owned(), model.name.to_owned());
                    assert!(documented.contains(&row), "{row:?} has no row");
                }
            }
        }
    }
}
