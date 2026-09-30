use super::*;

/// A changelog made of `sections`, in the order given, each section being a
/// heading's version and date and what is under it.
fn changelog(sections: &[(&str, &str, &str)]) -> String {
    let mut text = String::from(
        "# Changelog\n\nNotable changes.\n\n## [Unreleased]\n\n### Added\n\n- not yet\n",
    );
    for (version, date, body) in sections {
        text.push_str(&format!("\n## [{version}] - {date}\n\n{body}\n"));
    }
    text.push_str(
        "\n[Unreleased]: https://example.invalid/compare\n[0.0.1]: https://example.invalid/tag\n",
    );
    text
}

/// The one release `body` makes, as counted.
fn counted(body: &str) -> String {
    let text = changelog(&[("0.1.0", "2026-01-02", body)]);
    releases(&text)
        .first()
        .map(Release::counted)
        .expect("one release")
}

#[test]
fn the_releases_are_the_changelogs_numbered_headings_oldest_first() {
    // Counted from the file itself, so a later release changes nothing here.
    let headings: Vec<&str> = CHANGELOG
        .lines()
        .filter_map(|line| line.strip_prefix("## ["))
        .filter_map(|rest| rest.split_once(']').map(|(version, _)| version))
        .filter(|version| *version != "Unreleased")
        .collect();
    let read: Vec<&str> = releases(CHANGELOG)
        .iter()
        .map(|release| release.version)
        .collect();

    assert!(headings.len() > 90, "{} headings", headings.len());
    let oldest_first: Vec<&str> = headings.iter().rev().copied().collect();
    assert_eq!(read, oldest_first);
    assert_eq!(read.first(), Some(&"0.0.1"));
}

#[test]
fn unreleased_is_never_a_release_and_each_heading_gives_its_date() {
    // Newest first, as a changelog is written.
    let text = changelog(&[
        ("0.2.0", "2026-02-03", "### Added\n\n- two"),
        ("0.1.0", "2026-01-02", "### Fixed\n\n- one"),
    ]);
    let read = releases(&text);

    assert_eq!(
        read.iter()
            .map(|release| (release.version, release.date))
            .collect::<Vec<_>>(),
        [("0.1.0", "2026-01-02"), ("0.2.0", "2026-02-03")]
    );
}

#[test]
fn an_older_release_counts_its_top_level_bullets_by_group_in_a_fixed_order() {
    let body = "### Security\n\n- s\n\n### Fixed\n\n- f1\n- f2\n  - nested, not counted\n\n\
                ### Documented\n\n- d\n\n### Added\n\n- a1\n  continues the bullet\n- a2\n- a3\n\n\
                ### Known limits\n\n- k\n\n### Removed\n\n- r";

    assert_eq!(
        counted(body),
        "3 added · 2 fixed · 1 removed · 1 security · 1 documented · 1 known limits"
    );
}

#[test]
fn bullets_above_any_heading_count_as_changed_and_groups_of_one_name_add_up() {
    let body =
        "- one\n- two\n\n### Changed\n\n- three\n\n### Fixed\n\n- four\n\n### Changed\n\n- five";

    assert_eq!(counted(body), "4 changed · 1 fixed");
}

#[test]
fn a_heading_that_holds_paragraphs_and_no_bullets_counts_its_paragraphs() {
    let body = "### Changed\n\n**First.** One paragraph\nover two lines.\n\nA second one.";

    assert_eq!(counted(body), "2 changed");
}

#[test]
fn a_release_with_no_heading_and_no_bullet_counts_nothing() {
    let body = "**Nothing in crucible changed.** A tidy-up.\n\nAnd a second paragraph.";

    assert_eq!(counted(body), "");
}

#[test]
fn the_four_real_sections_that_break_the_usual_shape_are_counted_by_the_rule() {
    let read = releases(CHANGELOG);
    let of = |version: &str| {
        read.iter()
            .find(|release| release.version == version)
            .map(Release::counted)
            .unwrap_or_else(|| panic!("no release {version}"))
    };

    assert_eq!(of("0.28.2"), "");
    assert_eq!(of("0.23.4"), "1 changed");
    assert_eq!(of("0.9.1"), "");
    assert_eq!(of("0.8.1"), "");
}

#[test]
fn a_releases_words_carry_no_key_tags_and_stop_at_the_next_release() {
    let text = changelog(&[
        ("0.2.0", "2026-02-03", "### Added\n\n- the newer one"),
        (
            "0.1.0",
            "2026-01-02",
            "### Fixed\n\n- Press <kbd>Ctrl+O</kbd> to open it.",
        ),
    ]);
    let read = releases(&text);
    let older = read.first().expect("the older release");

    assert_eq!(older.text(), "### Fixed\n\n- Press Ctrl+O to open it.");
    assert!(
        releases(CHANGELOG)
            .iter()
            .all(|release| !release.text().contains("<kbd") && !release.text().contains("</kbd")),
        "a key tag reached the words"
    );
}

#[test]
fn the_link_lines_at_the_foot_are_no_part_of_the_oldest_release() {
    let read = releases(CHANGELOG);
    let oldest = read.first().expect("a release");

    assert!(!oldest.text().contains("]: https://"), "{}", oldest.text());
}

#[test]
fn a_version_is_three_numbers_with_or_without_a_leading_v() {
    for (word, version) in [
        ("0.41.1", Some("0.41.1")),
        ("v0.41.1", Some("0.41.1")),
        ("12.0.300", Some("12.0.300")),
    ] {
        assert_eq!(asked(word), version, "{word}");
    }
    for word in [
        "latest",
        "0.41",
        "1.2.3.4",
        "v",
        "",
        "0.41.x",
        "V0.41.1",
        "0.41.1-rc1",
        "vv0.41.1",
    ] {
        assert_eq!(asked(word), None, "{word}");
    }
}
