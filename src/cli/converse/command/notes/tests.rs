use std::fmt::Write as _;

use crucible_client_api::bounds::{ITEMS, TEXT_BYTES};
use crucible_client_api::{Name, Outcome, Response};
use crucible_tui::{Glyphs, RECORDED, Renderer, Row};

use super::*;

/// A changelog made of `sections`, in the order given, each section being a
/// heading's version and date and what is under it.
fn changelog(sections: &[(&str, &str, &str)]) -> String {
    let mut text = String::from(
        "# Changelog\n\nNotable changes.\n\n## [Unreleased]\n\n### Added\n\n- not yet\n",
    );
    for (version, date, body) in sections {
        let _ = write!(text, "\n## [{version}] - {date}\n\n{body}\n");
    }
    text.push_str(
        "\n[Unreleased]: https://example.invalid/compare\n[0.0.1]: https://example.invalid/tag\n",
    );
    text
}

/// A release's groups as an older release's row says them:
/// `8 added · 2 changed · 18 fixed`.
fn said(release: &Release<'_>) -> String {
    release
        .groups()
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The one release `body` makes, as counted.
fn counted(body: &str) -> String {
    let text = changelog(&[("0.1.0", "2026-01-02", body)]);
    releases(&text).first().map(said).expect("one release")
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
            .map_or_else(|| panic!("no release {version}"), said)
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

/// What the rows say, one after another, with the spaces taken out: what a
/// row folded to one column still says once its pieces are put back.
fn squeezed(rows: &[Row]) -> String {
    rows.iter()
        .map(Row::text)
        .collect::<String>()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

#[test]
fn the_whole_list_keeps_its_first_row_and_counts_what_it_left_out_at_every_width() {
    let read = releases(CHANGELOG);
    let older = read.len() - FULL;

    for columns in [1, 8, 40, 80] {
        let rows = whole(&read, columns, Glyphs::Unicode);
        let at = format!("{columns} columns");
        assert!(rows.len() < RECORDED, "{at}: {} rows", rows.len());

        // Printed, the first row is still in the record after the last.
        let mut renderer = Renderer::new(crucible_tui::Recording::new(columns, 24));
        renderer.present(&rows).expect("a recording cannot fail");
        renderer.commit("").expect("a recording cannot fail");
        let kept = renderer.tail(RECORDED * 2);
        let first = rows.first().map(Row::text).expect("a first row");
        assert!(
            kept.iter().any(|row| row.text() == first),
            "{at}: the first row went: {first:?}"
        );

        // What the closing row says went is what went.
        let hollow = rows
            .iter()
            .filter(|row| row.text().starts_with('◇'))
            .count();
        let filled = rows
            .iter()
            .filter(|row| row.text().starts_with('◆'))
            .count();
        // A release told in full that had to be cut to a row is hollow too.
        let told = FULL - filled;
        let left = older - (hollow - told);
        let closing = squeezed(
            rows.get(
                rows.iter()
                    .rposition(|row| row.text().contains('⎿'))
                    .expect("a closing row")..,
            )
            .unwrap_or_default(),
        );
        if columns >= 40 {
            assert_eq!((hollow, filled), (older, FULL), "{at}");
        }
        if columns == 1 {
            assert!(closing.contains("incomplete"), "{at}: {closing}");
        }
        if left + told == 0 {
            assert!(!closing.contains("incomplete"), "{at}: {closing}");
        } else {
            assert!(closing.contains("incomplete"), "{at}: {closing}");
            if left > 0 {
                assert!(
                    closing.contains(&format!("{left}oftheolderrowsleftout")),
                    "{at}: {closing}"
                );
            }
            if told > 0 {
                assert!(
                    closing.contains(&format!("{told}toldinaroweach")),
                    "{at}: {closing}"
                );
            }
        }
    }
}

#[test]
fn no_key_tag_or_bold_marker_reaches_the_screen_and_a_number_points_at_crucible() {
    let read = releases(CHANGELOG);
    let rows = whole(&read, 80, Glyphs::Unicode);
    for row in &rows {
        for (slot, text) in row.spans() {
            assert!(
                !text.contains("<kbd") && !text.contains("</kbd"),
                "{:?}",
                row.text()
            );
            if slot != crucible_tui::Slot::Code {
                assert!(!text.contains("**"), "{:?}", row.text());
            }
        }
    }

    // A number in a release is counted against crucible's own repository.
    let text = changelog(&[(
        "0.2.0",
        "2026-02-03",
        "### Fixed\n\n- **Fixed.** As #45 asked.",
    )]);
    let read = releases(&text);
    let rows = whole(&read, 80, Glyphs::Unicode);
    let palette = crucible_tui::Palette::resolve(true, crucible_tui::Theme::Dark, None, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
    .addressing(true);
    let number = rows
        .iter()
        .find(|row| row.text().contains("#45"))
        .expect("the number");
    assert!(
        number
            .paint(&palette)
            .contains("https://github.com/augments-labs/crucible-code/issues/45"),
        "{:?}",
        number.text()
    );
}

#[test]
fn one_release_is_told_in_full_with_no_rail() {
    let read = releases(CHANGELOG);
    let release = read
        .iter()
        .find(|release| release.version == "0.41.1")
        .expect("0.41.1");
    let rows = alone(release, 40, Glyphs::Unicode);
    let lines: Vec<String> = rows.iter().map(Row::text).collect();

    assert!(
        lines
            .first()
            .is_some_and(|line| line.starts_with("◆ 0.41.1 · 2026-09-14")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .all(|line| !line.starts_with('│') && !line.contains('⎿')),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line.trim() == "Security"),
        "{lines:#?}"
    );
}

#[test]
fn lines_the_changelog_wrapped_are_joined_and_every_other_break_is_kept() {
    let body = "**The lead.** A summary\nover two lines.\n\n### Fixed\n\n\
                - **One.** A bullet that\n  goes on here\n  and here.\n  - a nested one\n\
                - **Two.** Short.\n\n| a | b |\n| --- | --- |\n| c | d |\n\n\
                ```\nkept\nas it is\n```";
    let text = changelog(&[("0.1.0", "2026-01-02", body)]);
    let read = releases(&text);

    assert_eq!(
        read.first().map(Release::text).as_deref(),
        Some(
            "**The lead.** A summary over two lines.\n\n### Fixed\n\n\
             - **One.** A bullet that goes on here and here.\n  - a nested one\n\
             - **Two.** Short.\n\n| a | b |\n| --- | --- |\n| c | d |\n\n\
             ```\nkept\nas it is\n```"
        )
    );
}

/// `word` as the contract names it.
fn name(word: &str) -> Name {
    Name::new(word).expect("a name")
}

/// The releases of a list answer, or a failure saying what came instead.
fn listed(answer: NotesOutcome) -> (Vec<crucible_client_api::Release>, Name, bool) {
    match answer {
        NotesOutcome::Listed {
            releases,
            running,
            truncated,
        } => (releases, running, truncated),
        other => panic!("not a list: {other:?}"),
    }
}

#[test]
fn a_client_is_answered_every_release_oldest_first_and_the_newest_ten_in_words() {
    let read = releases(CHANGELOG);
    let (told, running, truncated) = listed(answered(None).expect("an answer"));

    assert_eq!(running, name(RUNNING));
    assert!(!truncated, "{} releases", read.len());
    assert_eq!(
        told.iter()
            .map(|release| (release.version.as_str(), release.date.as_str()))
            .collect::<Vec<_>>(),
        read.iter()
            .map(|release| (release.version, release.date))
            .collect::<Vec<_>>()
    );
    for (release, from) in told.iter().zip(&read) {
        assert_eq!(
            release
                .groups
                .iter()
                .map(|group| (group.kind.as_str().to_owned(), group.count))
                .collect::<Vec<_>>(),
            from.groups()
                .into_iter()
                .map(|(kind, count)| (kind, count as u64))
                .collect::<Vec<_>>(),
            "{}",
            from.version
        );
    }
    let worded = told.len() - FULL;
    assert!(
        told.iter()
            .take(worded)
            .all(|release| release.text.is_none())
    );
    for (release, from) in told.iter().zip(&read).skip(worded) {
        let text = release.text.as_ref().expect("the words of a newer release");
        // Some releases say more than one value may carry, and are cut.
        let words = from.text();
        assert!(words.starts_with(text.as_str()), "{}", from.version);
        assert_eq!(
            text.truncated(),
            words.len() > TEXT_BYTES,
            "{}",
            from.version
        );
    }

    // And the answer travels: the whole of it fits in one frame.
    let response = Response {
        correlation: None,
        outcome: Outcome::Notes(answered(None).expect("an answer")),
    };
    assert!(response.encode().is_ok());
}

#[test]
fn a_client_names_a_release_with_or_without_its_v_and_is_told_it_in_words() {
    for word in ["0.41.1", "v0.41.1"] {
        match answered(Some(word)).expect("an answer") {
            NotesOutcome::One(release) => {
                assert_eq!(release.version, name("0.41.1"));
                assert_eq!(release.date, name("2026-09-14"));
                assert!(
                    release
                        .text
                        .is_some_and(|text| text.as_str().contains("### "))
                );
            }
            other => panic!("{word}: {other:?}"),
        }
    }
}

#[test]
fn a_client_is_told_the_newest_release_when_it_names_none_there_is() {
    let newest = releases(CHANGELOG)
        .last()
        .map(|release| name(release.version))
        .expect("a release");

    assert_eq!(
        answered(Some("0.0.0")).expect("an answer"),
        NotesOutcome::Unknown {
            newest: newest.clone()
        }
    );
    for word in ["latest", "0.41", "0.41.1 0.41.0"] {
        assert_eq!(
            answered(Some(word)).expect("an answer"),
            NotesOutcome::NotAVersion {
                newest: newest.clone()
            },
            "{word}"
        );
    }
}

#[test]
fn past_the_contract_s_ceilings_the_oldest_are_left_out_and_long_words_cut_and_both_said() {
    let long = format!("### Fixed\n\n- {}", "word ".repeat(TEXT_BYTES / 4));
    let mut sections: Vec<(String, String, String)> = (0..ITEMS + 2)
        .map(|at| {
            (
                format!("0.{at}.0"),
                "2026-01-02".to_owned(),
                "### Added\n\n- one".to_owned(),
            )
        })
        .collect();
    if let Some(newest) = sections.last_mut() {
        newest.2.clone_from(&long);
    }
    sections.reverse();
    let sections: Vec<(&str, &str, &str)> = sections
        .iter()
        .map(|(version, date, body)| (version.as_str(), date.as_str(), body.as_str()))
        .collect();
    let text = changelog(&sections);

    let (told, _, truncated) = listed(answer(&text, None).expect("an answer"));
    assert!(truncated);
    assert_eq!(told.len(), ITEMS);
    assert_eq!(
        told.first().map(|release| release.version.as_str()),
        Some("0.2.0")
    );
    let newest = told.last().expect("the newest");
    assert_eq!(newest.version.as_str(), format!("0.{}.0", ITEMS + 1));
    let words = newest.text.as_ref().expect("its words");
    assert!(words.truncated());
    assert!(words.as_str().len() <= TEXT_BYTES);
}
