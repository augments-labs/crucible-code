use std::ffi::OsStr;

use crucible_config::Driver;

use crucible_external_harnesses::catalog::{KEYS, Keys};

use super::{CATALOG, FAKE_ROWS, Served, catalog, catalog_from, registry, registry_from};
use crate::AppError;
use crate::backends::REGISTRY;
use crate::catalog_fetch::{HOSTS, PUBLISHED, Source};

/// A key's shape, as a minisign public key's base64 line has it; nothing
/// verifies under it.
const KEY: &str = "RWQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

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

#[test]
fn with_no_catalog_named_a_test_build_fetches_the_published_one() {
    assert_eq!(catalog_from(None).expect("nothing named"), None);
    assert_eq!(catalog(&|_| None), None);

    let published = Source::published(&|_| None);
    assert_eq!(published.base, PUBLISHED);
    assert_eq!(published.keys, KEYS);
    assert_eq!(published.hosts, HOSTS);
}

#[test]
fn a_catalog_named_is_the_address_it_is_served_at_and_the_key_it_is_signed_with() {
    let value = format!("http://127.0.0.1:9/catalog/ {KEY}");
    let served = Served {
        base: "http://127.0.0.1:9/catalog/".to_owned(),
        key: KEY.to_owned(),
    };

    let named = catalog_from(Some(OsStr::new(&value))).expect("a served catalog");
    assert_eq!(named, Some(served.clone()));
    assert_eq!(served.keys(), Keys::new(&[KEY]));

    let source = Source::published(&|name| (name == CATALOG).then(|| value.clone()));
    assert_eq!(source.base, served.base);
    assert_eq!(source.keys, Keys::new(&[KEY]));
    assert!(source.hosts.is_empty(), "{:?}", source.hosts);
}

#[test]
fn a_catalog_named_any_other_way_is_refused_naming_the_variable() {
    let values = [
        String::new(),
        "http://127.0.0.1:9/catalog/".to_owned(),
        format!("http://127.0.0.1:9/catalog/ {KEY} {KEY}"),
        format!(" {KEY}"),
        format!("http://127.0.0.1:9/catalog/  {KEY}"),
        format!("{KEY} http://127.0.0.1:9/catalog/"),
        format!("file:///catalog/ {KEY}"),
    ];
    for value in &values {
        let Err(problem) = catalog_from(Some(OsStr::new(value))) else {
            panic!("{value:?} named a catalog");
        };

        assert!(
            matches!(&problem, AppError::ProofCatalog { value: said } if &**said == value.as_str()),
            "{problem:?}"
        );
        assert!(problem.to_string().contains(CATALOG), "{problem}");
        let source = Source::published(&|name| (name == CATALOG).then(|| value.clone()));
        assert_eq!(source.base, PUBLISHED, "{value:?}");
    }
}
