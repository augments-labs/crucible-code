//! What `/agents` draws of what the door listed, which entries a query keeps,
//! and what an add and a check against the receipts say.

use crucible_external_harnesses::store::{Differs, Mismatch};
use crucible_types::AgentKey;

use super::*;

/// What the door lists of `key`, offered at `offered`, under `license`,
/// holding `installed`.
fn listed(key: &str, offered: &[&str], license: &str, installed: &[&str]) -> door::Listed {
    door::Listed {
        agent: AgentKey::new(key).expect("an agent key"),
        offered: offered.iter().map(|&version| version.into()).collect(),
        license: license.into(),
        terms: None,
        caution: None,
        installed: Ok(installed.iter().map(|&version| version.into()).collect()),
        keeps_no_sign_in: false,
    }
}

/// Three agents, two of them under the same licence.
fn three() -> Listing {
    Listing::new(
        vec![
            listed("goose", &["1.0.0"], "Apache-2.0", &[]),
            listed("opencode", &["0.9.1", "0.9.0"], "MIT", &["0.9.0"]),
            listed("aider", &["2.1.0"], "MIT", &[]),
        ],
        false,
        None,
    )
}

/// The rows `listing` lays out at `columns` in `room`, as text.
fn drawn(listing: &mut Listing, columns: usize, room: usize) -> Vec<String> {
    listing
        .laid(columns, room, Glyphs::Unicode)
        .0
        .iter()
        .map(Row::text)
        .collect()
}

/// `text` typed into the listing's search line, a key at a time.
fn typed(listing: &mut Listing, text: &str) {
    for c in text.chars() {
        assert_eq!(listing.pressed(Pressed::Key(Key::Char(c))), Moved::Redraw);
    }
}

/// The keys of the agents `listing` keeps, as its last frame found them.
fn kept(listing: &Listing) -> Vec<&str> {
    listing
        .found
        .iter()
        .filter_map(|&at| listing.agents.get(at))
        .map(|entry| entry.listed.agent.as_str())
        .collect()
}

#[test]
fn an_entry_is_its_key_and_newest_version_over_its_licence_and_what_is_held() {
    let mut listing = three();
    let rows = drawn(&mut listing, 100, 30).join("\n");

    assert!(rows.contains("Agents · 3 of 3 offered"), "{rows}");
    assert!(rows.contains("goose 1.0.0"), "{rows}");
    assert!(rows.contains("Apache-2.0 · not installed"), "{rows}");
    assert!(rows.contains("opencode 0.9.1"), "{rows}");
    assert!(rows.contains("MIT · installed 0.9.0"), "{rows}");
    assert_eq!(kept(&listing), ["goose", "opencode", "aider"]);
}

#[test]
fn what_the_catalog_says_of_an_agent_cannot_draw_a_line_or_reach_the_terminal() {
    let mut hostile = listed("goose", &["1.0\u{1b}[2J"], "MIT\nfake: line", &["0.1\u{7}"]);
    hostile.terms = Some("https://example.com/\u{1b}]8;;x\u{7}".into());
    hostile.caution = Some("read\u{202e}this\nfirst".into());
    let mut listing = Listing::new(vec![hostile], false, None);

    let rows = drawn(&mut listing, 120, 30);
    let told = provenance(&listing.agents.first().expect("an entry").listed, None);

    for line in rows.iter().chain(&told) {
        assert!(
            !line.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "{line:?}"
        );
    }
    let rows = rows.join("\n");
    assert!(rows.contains("goose 1.0\\u{1b}[2J"), "{rows}");
    assert!(rows.contains("MIT\\nfake: line"), "{rows}");
    let told = told.join("\n");
    assert!(
        told.contains("Terms: https://example.com/\\u{1b}]8;;x\\u{7}"),
        "{told}"
    );
    assert!(
        told.contains("Caution: read\\u{202e}this\\nfirst"),
        "{told}"
    );
    assert!(
        told.contains("Installed, each with a receipt: 0.1\\u{7}."),
        "{told}"
    );
}

#[test]
fn what_is_typed_keeps_the_agents_whose_key_or_licence_holds_it() {
    let mut listing = three();
    drawn(&mut listing, 100, 30);

    typed(&mut listing, "mit");
    let rows = drawn(&mut listing, 100, 30).join("\n");
    assert_eq!(kept(&listing), ["opencode", "aider"]);
    assert!(rows.contains("Agents · 2 of 3 offered"), "{rows}");
    assert!(!rows.contains("goose"), "{rows}");

    listing.query.clear();
    typed(&mut listing, "GOO");
    drawn(&mut listing, 100, 30);
    assert_eq!(kept(&listing), ["goose"]);
}

#[test]
fn a_query_that_keeps_nothing_says_so_and_enter_takes_nothing() {
    let mut listing = three();
    let _ = listing.query.paste("zzz\u{1b}[2J");
    let rows = drawn(&mut listing, 100, 30);

    assert!(kept(&listing).is_empty());
    for row in &rows {
        assert!(!row.chars().any(char::is_control), "{row:?}");
    }
    let rows = rows.join("\n");
    assert!(rows.contains("no agent matches \"zzz"), "{rows}");
    assert_eq!(listing.pressed(Pressed::Key(Key::Enter)), Moved::Still);
    assert!(listing.marked().is_none());
}

#[test]
fn the_mark_walks_the_kept_agents_and_enter_takes_the_one_it_is_on() {
    let mut listing = three();
    drawn(&mut listing, 100, 30);

    assert_eq!(listing.pressed(Pressed::Up), Moved::Still);
    assert_eq!(listing.pressed(Pressed::Down), Moved::Redraw);
    assert_eq!(listing.pressed(Pressed::Down), Moved::Redraw);
    assert_eq!(listing.pressed(Pressed::Down), Moved::Still);
    assert_eq!(listing.pressed(Pressed::Key(Key::Enter)), Moved::Took);
    let marked = listing.marked().expect("an agent marked");
    assert_eq!(marked.listed.agent.as_str(), "aider");

    // A query that narrows the list walks the mark back to its top.
    typed(&mut listing, "o");
    drawn(&mut listing, 100, 30);
    let marked = listing.marked().expect("an agent marked");
    assert_eq!(marked.listed.agent.as_str(), "goose");
}

#[test]
fn escape_clears_what_was_typed_before_it_leaves() {
    let mut listing = three();
    typed(&mut listing, "mit");

    assert_eq!(listing.pressed(Pressed::Escape), Moved::Redraw);
    assert!(listing.query.is_empty());
    assert_eq!(listing.pressed(Pressed::Escape), Moved::Left);
    assert_eq!(listing.pressed(Pressed::Key(Key::Interrupt)), Moved::Left);
}

#[test]
fn an_agents_screen_tells_where_it_comes_from() {
    let mut agent = listed("goose", &["1.1.0", "1.0.0"], "Apache-2.0", &["1.0.0"]);
    agent.terms = Some("https://example.com/goose/terms".into());
    agent.caution = Some("Read the goose terms first.".into());
    agent.keeps_no_sign_in = true;

    assert_eq!(
        provenance(&agent, None),
        [
            "Offered: 1.1.0, 1.0.0.",
            "Licence: Apache-2.0.",
            "Terms: https://example.com/goose/terms",
            "Caution: Read the goose terms first.",
            "Installed, each with a receipt: 1.0.0.",
            UNKEPT_SIGN_IN,
        ]
    );
}

#[test]
fn a_long_list_of_versions_names_the_first_few_and_counts_the_rest() {
    let offered: Vec<String> = (0..12).map(|minor| format!("1.{minor}.0")).collect();
    let offered: Vec<&str> = offered.iter().map(String::as_str).collect();
    let agent = listed("goose", &offered, "MIT", &[]);

    let told = provenance(&agent, None);
    let first = told.first().expect("the versions offered");
    assert!(first.starts_with("Offered: 1.0.0, 1.1.0, "), "{first}");
    assert!(first.ends_with("1.7.0, and 4 more."), "{first}");
}

#[test]
fn a_store_that_could_not_be_read_says_so_on_the_entry_and_why_on_the_screen() {
    let mut agent = listed("goose", &["1.0.0"], "MIT", &[]);
    agent.installed = Err("permission\u{1b} denied".into());
    let mut listing = Listing::new(vec![agent], false, None);

    let rows = drawn(&mut listing, 100, 30).join("\n");
    assert!(rows.contains("MIT · the store could not be read"), "{rows}");
    let told = provenance(&listing.agents.first().expect("an entry").listed, None);
    assert!(
        told.contains(&"The install store could not be read: permission\\u{1b} denied".to_owned()),
        "{told:?}"
    );
}

#[test]
fn the_closing_lines_say_more_are_offered_and_that_the_catalog_was_not_refreshed() {
    let mut listing = Listing::new(
        vec![listed("goose", &["1.0.0"], "MIT", &[])],
        true,
        Some(&FetchError::Status(503)),
    );

    let rows = drawn(&mut listing, 200, 30).join("\n");
    assert!(rows.contains(MORE), "{rows}");
    assert!(
        rows.contains(
            "the catalog could not be refreshed, so this is what was believed before: \
             the catalog host answered 503"
        ),
        "{rows}"
    );
}

#[test]
fn an_add_names_the_instance_written_which_is_the_last_the_door_gives() {
    let said = added(&door::Outcome::Moved {
        agent: AgentKey::new("goose").expect("an agent key"),
        how: How::Added,
        version: "1.0.0".into(),
        instances: vec!["goose".into(), "goose-2".into()],
        left: vec!["mine\u{1b}".into()],
        downloaded: false,
        standing: false,
        unsmoked: None,
        unpruned: None,
    })
    .expect("an add says what came of it");

    assert_eq!(
        said,
        "Added goose 1.0.0 as the instance \"goose-2\". \"goose\" moved onto 1.0.0. \
         \"mine\\u{1b}\" changed meanwhile and were left as they are."
    );
}

#[test]
fn an_add_that_was_not_smoke_tested_or_pruned_says_why() {
    let said = added(&door::Outcome::Moved {
        agent: AgentKey::new("goose").expect("an agent key"),
        how: How::Added,
        version: "1.0.0".into(),
        instances: vec!["goose".into()],
        left: Vec::new(),
        downloaded: true,
        standing: false,
        unsmoked: Some("the sandbox is off".into()),
        unpruned: Some(Box::new(AgentsError::TooLarge)),
    })
    .expect("an add says what came of it");

    assert!(
        said.starts_with("Added goose 1.0.0 as the instance \"goose\". It was not smoke-tested: the sandbox is off. Older versions were kept: "),
        "{said}"
    );
    assert!(!said.contains("! "), "{said}");
}

#[test]
fn a_move_that_is_not_an_add_is_not_said_as_one() {
    let moved = door::Outcome::Moved {
        agent: AgentKey::new("goose").expect("an agent key"),
        how: How::Installed,
        version: "1.0.0".into(),
        instances: vec!["goose".into()],
        left: Vec::new(),
        downloaded: true,
        standing: false,
        unsmoked: None,
        unpruned: None,
    };

    assert!(added(&moved).is_none());
    assert!(added(&door::Outcome::Undecided).is_none());
}

#[test]
fn a_check_names_the_files_that_differ_and_counts_the_rest() {
    let mismatch = |path: String| Mismatch {
        path: path.into(),
        what: Differs::Bytes,
    };
    let found = checked(&door::Outcome::Verified {
        agent: AgentKey::new("goose").expect("an agent key"),
        versions: vec![
            ("1.0.0".into(), Vec::new()),
            ("0.9.0".into(), vec![mismatch("bin/goose\u{1b}".to_owned())]),
            (
                "0.8.0".into(),
                (0..10).map(|at| mismatch(format!("lib/{at}"))).collect(),
            ),
        ],
    })
    .expect("a check says what it found");

    let [whole, one, many] = found.as_slice() else {
        panic!("a line a version: {found:?}");
    };
    assert_eq!(whole, "1.0.0: every file is as recorded at install.");
    assert_eq!(
        one,
        "0.9.0: 1 file differs from what was recorded at install: \"bin/goose\\u{1b}\"."
    );
    assert!(
        many.starts_with("0.8.0: 10 files differ from what was recorded at install: \"lib/0\", "),
        "{many}"
    );
    assert!(many.ends_with("\"lib/7\" and 2 more."), "{many}");
    assert!(checked(&door::Outcome::Undecided).is_none());
}

#[test]
fn a_check_is_told_on_the_screen_after_what_the_store_holds() {
    let agent = listed("goose", &["1.0.0"], "MIT", &["1.0.0"]);
    let found = vec!["1.0.0: every file is as recorded at install.".to_owned()];

    let told = provenance(&agent, Some(&found));
    assert_eq!(
        told.last().map(String::as_str),
        Some("1.0.0: every file is as recorded at install.")
    );
}

/// A list of `agents` agents, each offered at many versions under a long
/// licence, its closing lines given or not, `typed` in its search line.
fn listing(agents: usize, closing: bool, typed: &str) -> Listing {
    let offered: Vec<String> = (0..20).map(|minor| format!("1.{minor}.0")).collect();
    let offered: Vec<&str> = offered.iter().map(String::as_str).collect();
    let licence = "Apache-2.0 WITH LLVM-exception OR MIT OR Zlib OR Unlicense ".repeat(4);
    let listed: Vec<door::Listed> = (0..agents)
        .map(|at| {
            let mut agent = listed(&format!("agent-{at}"), &offered, &licence, &["1.0.0"]);
            agent.terms = Some(format!("https://example.com/{}", "terms/".repeat(40)).into());
            agent.caution = Some("Read the terms first. ".repeat(20).into());
            agent
        })
        .collect();
    let mut listing = Listing::new(listed, closing, closing.then_some(&FetchError::Late));
    let _ = listing.query.paste(typed);
    listing
}

#[test]
fn every_state_of_the_list_fits_every_window_it_is_given() {
    // The panel's own fit sweep, since it is laid out here and not among the
    // components the crate's sweep walks: one agent and many, with the closing
    // lines and without, a query that keeps some and one that keeps none, each
    // at every width and room the picker decides anything at, in either glyph
    // set.
    let states = [
        ("one", listing(1, false, "")),
        ("many", listing(40, false, "")),
        ("closing", listing(40, true, "")),
        ("narrowed", listing(40, true, "agent-1")),
        ("nothing kept", listing(40, true, "zzz")),
    ];
    for (state, mut panel) in states {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for columns in 1..=200 {
                for room in 0..=24 {
                    let (laid, _) = panel.laid(columns, room, glyphs);
                    let at = format!("{state} {glyphs:?} {columns}x{room}");
                    assert!(laid.len() <= room, "{at}: {} rows", laid.len());
                    for row in &laid {
                        assert!(row.columns() <= columns, "{at}: {:?}", row.text());
                    }
                }
            }
        }
    }
}
