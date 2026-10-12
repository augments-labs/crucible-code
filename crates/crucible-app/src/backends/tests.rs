use std::collections::BTreeSet;
use std::path::Path;

use super::*;
use crate::content_use::{WARNED, agent_route, launches};

/// Each row is one driver the configuration accepts, spelled with that
/// driver's own profile key, held under that driver's agent route, and that
/// route is one the content-use panel warns about; its section exists on the
/// page it names, and no driver has two rows.
#[test]
fn every_row_agrees_with_its_driver_its_route_and_its_page() {
    let mut drivers = BTreeSet::new();
    for row in REGISTRY.all() {
        assert!(drivers.insert(row.driver.name()), "{:?} twice", row.driver);
        assert!(Driver::ALL.contains(&row.driver), "{row:?}");
        assert_eq!(row.profile_key, row.driver.profile_key(), "{row:?}");
        assert_eq!(row.route, agent_route(row.driver.name()), "{row:?}");
        assert!(
            WARNED.iter().any(|warned| warned.route == row.route),
            "{row:?}"
        );
        let page = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/providers/agents.md"),
        )
        .expect("the agents page");
        assert!(
            page.lines()
                .any(|line| line.starts_with('#') && anchor(line) == row.docs_section),
            "{row:?}"
        );
        assert_eq!((row.dialect)().program(), row.driver.name(), "{row:?}");
    }
}

/// Every agent route the panel warns about is a driver's, so no route is
/// asked about that nothing could launch.
#[test]
fn every_agent_route_warned_is_a_drivers() {
    for warned in WARNED.iter().filter(|warned| launches(warned.route)) {
        assert!(
            Driver::ALL
                .iter()
                .any(|driver| agent_route(driver.name()) == warned.route),
            "{}",
            warned.route
        );
    }
}

/// A row that does not ship is not offered, and is found by its driver.
#[test]
fn a_row_that_does_not_ship_is_not_offered() {
    static HELD_BACK: [Row; 1] = [Row {
        driver: Driver::Antigravity,
        dialect: || Box::new(crucible_external_harnesses::fake_peer::Plain),
        profile_key: "profileDir",
        route: "agent:antigravity",
        docs_section: "antigravity",
        reaches: &[],
        shipping: false,
    }];
    let registry = Registry::new(&HELD_BACK);
    assert_eq!(registry.shipping().count(), 0);
    assert!(registry.row(Driver::Antigravity).is_some());
    assert!(registry.row(Driver::Claude).is_none());
}

/// The anchor a Markdown heading gets: lower case, spaces as hyphens, and
/// nothing but letters, digits and hyphens.
fn anchor(heading: &str) -> String {
    heading
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|letter| match letter {
            ' ' => Some('-'),
            letter if letter.is_alphanumeric() || letter == '-' => Some(letter),
            _ => None,
        })
        .collect()
}
