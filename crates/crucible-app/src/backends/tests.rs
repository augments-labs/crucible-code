use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crucible_external_harnesses::fake_peer::{self, scripts};
use crucible_external_harnesses::{Control, Shape};
use crucible_sandbox::SandboxPolicy;
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::Mode;
use tokio::runtime::Handle;

use super::*;
use crate::access::Route;
use crate::content_use::{Consent, Routes, WARNED, agent_route, launches};
use crate::launching::{Agent, Launcher, Unlaunched};
use crate::sample::Sample;

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
        for mode in every_mode() {
            if let Ok(dialect) = (row.dialect)(mode) {
                assert_eq!(dialect.program(), row.driver.name(), "{row:?}");
            }
        }
    }
}

/// Each host a row's agent may reach is named whole, so its sandbox opens no
/// host the agent was not reviewed reaching.
#[test]
fn every_host_a_row_reaches_is_named_whole() {
    for row in REGISTRY.all() {
        for host in row.reaches {
            assert!(!host.contains('*'), "{row:?}: {host}");
        }
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
        dialect: |_| Ok(Box::new(crucible_external_harnesses::fake_peer::Plain)),
        profile_key: "profileDir",
        route: "agent:antigravity",
        docs_section: "antigravity",
        reaches: &[],
        sign_in: |_| None,
        shipping: false,
    }];
    let registry = Registry::new(&HELD_BACK);
    assert_eq!(registry.shipping().count(), 0);
    assert!(registry.row(Driver::Antigravity).is_some());
    assert!(registry.row(Driver::Claude).is_none());
}

/// Claude Code's own row, held back from a release, is refused at launch by
/// the section that documents it and nothing internal, and starts no
/// process.
#[tokio::test(flavor = "multi_thread")]
async fn the_claude_code_row_held_back_is_refused_by_its_section_and_starts_nothing() {
    let shipped = REGISTRY.row(Driver::Claude).expect("Claude Code's row");
    let held_back: &'static [Row] = Box::leak(Box::new([Row {
        shipping: false,
        ..*shipped
    }]));
    let registry = Registry::new(held_back);
    let sample = Sample::new("backends-claude-held-back");
    let here = sample.root().join("peer");
    let peer = fake_peer::install(&here, "claude", &scripts::claude::stopped_first())
        .expect("the fake peer is built");
    let consent = Consent::new(Routes::production());
    consent.give(shipped.route);
    let policy = SandboxPolicy::standard(&sample.workspace())
        .expect("a policy")
        .with_enabled(false);
    let sandbox = Arc::new(LocalSandbox::new().watching_on(Handle::current()));
    let launcher = Launcher::new(sandbox, Handle::current(), &policy, &consent);
    let route = Route::External {
        provider: "anthropic",
        instance: "work".into(),
        driver: Driver::Claude,
        executable: peer.to_string_lossy().into(),
        profile: None,
        model: None,
        effort: None,
        enabled: true,
    };
    let agent = Agent::routed(&route, &registry, |_| None).expect("a row");
    let control = Control::new().expect("a control");

    let refused = launcher
        .launch(
            &agent,
            &control,
            (shipped.dialect)(Mode::Ask).expect("asking").as_mut(),
            None,
        )
        .await
        .expect_err("held back");

    let said = refused.to_string();
    assert!(matches!(refused, Unlaunched::Withheld { .. }), "{said}");
    assert!(
        said.starts_with("Claude Code ") && said.contains("docs/providers/agents.md#claude-code"),
        "{said}"
    );
    // Names no directory of this repository's own bookkeeping, which a user's
    // copy of the program does not have.
    assert!(
        !said
            .split(|letter: char| letter.is_whitespace() || letter == '/')
            .any(|part| part.starts_with('.')),
        "{said}"
    );
    assert!(
        !said
            .split(|letter: char| !letter.is_ascii_alphanumeric())
            .any(|word| word.len() == 3
                && word.starts_with('X')
                && word[1..].bytes().all(|byte| byte.is_ascii_digit())),
        "{said}"
    );
    assert_eq!(fake_peer::played(&here), 0);
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

/// Claude Code's table has one mode for each a conversation runs in, so a
/// mode added to the conversation must be given its row there too.
#[test]
fn claude_code_s_modes_are_the_conversation_s() {
    use crucible_external_harnesses::claude::Mode as Agents;
    let rows: BTreeSet<_> = every_mode()
        .into_iter()
        .map(|mode| format!("{:?}", claude_mode(mode)))
        .collect();
    let table: BTreeSet<_> = Agents::ALL.iter().map(|mode| format!("{mode:?}")).collect();

    assert_eq!(rows, table);
}

/// Antigravity is started the same way whatever the conversation's mode,
/// since what the mode allows is answered on its wire.
#[test]
fn antigravity_is_started_the_same_way_in_every_mode() {
    let row = REGISTRY
        .row(Driver::Antigravity)
        .expect("Antigravity's row");
    for mode in every_mode() {
        let mut dialect = (row.dialect)(mode).expect("every mode");
        assert_eq!(
            dialect.request(Shape::default()).arguments,
            (row.dialect)(Mode::Ask)
                .expect("asking")
                .request(Shape::default())
                .arguments,
            "{mode:?}"
        );
    }
}

/// Every mode a conversation runs in, in the order a cycle visits them.
fn every_mode() -> Vec<Mode> {
    let mut modes = vec![Mode::default()];
    while let Some(next) = modes
        .last()
        .map(|mode| mode.next())
        .filter(|next| *next != Mode::default())
    {
        modes.push(next);
    }
    modes
}

/// Each of the conversation's modes launches Claude Code in the one
/// permission mode mapped to it, full access in the one that skips its
/// prompts.
#[test]
fn claude_code_is_launched_in_the_permission_mode_mapped_to_each_mode() {
    let row = REGISTRY.row(Driver::Claude).expect("Claude Code's row");
    for (mode, named) in [
        (Mode::Ask, "default"),
        (Mode::AllowEdits, "acceptEdits"),
        (Mode::FullAccess, "bypassPermissions"),
    ] {
        let mut dialect = (row.dialect)(mode).expect("every mode is launched");
        let arguments = dialect.request(Shape::default()).arguments;
        assert!(
            arguments.windows(2).any(|pair| pair
                .iter()
                .map(String::as_str)
                .eq(["--permission-mode", named])),
            "{mode:?}: {arguments:?}"
        );
    }
}

/// Antigravity answers yes by itself to nothing when asking, to edits when
/// allowing them, and to everything in full access.
#[test]
fn antigravity_answers_yes_by_itself_to_what_each_mode_allows() {
    assert_eq!(antigravity_allowing(Mode::Ask), Allowing::Nothing);
    assert_eq!(antigravity_allowing(Mode::AllowEdits), Allowing::Edits);
    assert_eq!(antigravity_allowing(Mode::FullAccess), Allowing::Everything);
}

/// The configuration takes a model for exactly the drivers whose launch names
/// it, so a model written for an agent is never one its launch drops, and no
/// launch names one the configuration would refuse.
#[test]
fn a_model_is_taken_for_exactly_the_drivers_that_launch_with_it() {
    for driver in Driver::ALL {
        let row = REGISTRY.row(driver).expect("every driver has a row");
        let mut dialect = (row.dialect)(Mode::Ask).expect("asking");
        let arguments = dialect
            .request(Shape {
                model: Some("agreed-model"),
                ..Shape::default()
            })
            .arguments;
        assert_eq!(
            arguments
                .iter()
                .any(|argument| argument.contains("agreed-model")),
            driver.takes_model(),
            "{driver:?}: {arguments:?}"
        );
    }
}

/// Every driver reaches a provider this build offers, so connecting an agent
/// never puts a conversation on a provider nothing here can name.
#[test]
fn every_driver_s_provider_is_one_this_build_offers() {
    let providers = crate::providers::providers()
        .expect("the built-in providers register")
        .snapshot();
    let offered: BTreeSet<&str> = crate::providers::offered(&providers)
        .map(|one| one.name)
        .collect();
    for driver in Driver::ALL {
        assert!(
            offered.contains(driver.provider()),
            "{driver:?}: {offered:?}"
        );
    }
}
