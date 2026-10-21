use std::ffi::OsStr;

use crucible_config::Driver;

use super::{FAKE_ROWS, registry, registry_from};
use crate::AppError;
use crate::backends::REGISTRY;

#[test]
fn with_nothing_named_a_test_build_starts_what_ships() {
    let chosen = registry_from(None).expect("the shipped registry");

    assert!(std::ptr::eq(chosen, &raw const REGISTRY));
}

#[test]
fn fake_lists_every_agent_this_build_can_run_as_shipping() {
    let chosen = registry_from(Some(OsStr::new("fake"))).expect("the fake registry");

    let drivers: Vec<Driver> = chosen.shipping().map(|row| row.driver).collect();
    assert_eq!(drivers, [Driver::Claude, Driver::Antigravity]);
}

#[test]
fn fake_lists_every_agent_the_shipped_registry_knows() {
    let fake: Vec<Driver> = FAKE_ROWS.iter().map(|row| row.driver).collect();
    let known: Vec<Driver> = REGISTRY.all().map(|row| row.driver).collect();

    assert_eq!(fake, known);
}

#[test]
fn empty_lists_no_agent() {
    let chosen = registry_from(Some(OsStr::new("empty"))).expect("the empty registry");

    assert_eq!(chosen.all().count(), 0);
}

#[test]
fn any_other_value_is_refused_naming_the_two_it_takes() {
    for value in ["bogus", "", "Fake", "fake "] {
        let Err(problem) = registry_from(Some(OsStr::new(value))) else {
            panic!("{value:?} chose a registry");
        };

        assert!(
            matches!(&problem, AppError::ProofRegistry { value: said } if &**said == value),
            "{problem:?}"
        );
        let said = problem.to_string();
        assert!(said.contains("fake") && said.contains("empty"), "{said}");
    }
}

#[test]
fn a_registry_over_any_rows_lists_those_rows_and_no_others() {
    assert_eq!(registry(&[]).all().count(), 0);
    let one = FAKE_ROWS.get(..1).expect("a fake row");
    let drivers: Vec<Driver> = registry(one).all().map(|row| row.driver).collect();
    assert_eq!(drivers, [Driver::Claude]);
}
