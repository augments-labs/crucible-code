use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crucible_config::Serves;
use crucible_external_harnesses::catalog::{Trust, fixture};
use crucible_external_harnesses::fake_peer::{self, scripts};
use crucible_external_harnesses::{Control, Shape};
use crucible_sandbox::SandboxPolicy;
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::Mode;
use serde_json::{Value, json};
use tokio::runtime::Handle;

use super::*;
use crate::access::Route;
use crate::content_use::{Consent, Routes, WARNED, agent_route, launches};
use crate::launching::{Agent, Launcher, Unlaunched};
use crate::sample::Sample;

/// The catalog's trust state in `sample`'s home, as it is now.
pub(crate) fn trust_of(sample: &Sample) -> Trust {
    Trust::load(&sample.state(), SystemTime::now())
}

/// The catalog's trust state in `sample`'s home once a catalog listing
/// `entries` has been signed for it and taken in at `serial`.
pub(crate) fn trusting(sample: &Sample, serial: u64, entries: &[Value]) -> Trust {
    let catalog = json!({
        "schema": 1,
        "serial": serial,
        "revoked": [],
        "entries": entries,
    });
    let signed = fixture::signed(catalog.to_string().as_bytes());
    let now = SystemTime::now();
    Trust::load(&sample.state(), now)
        .take_in(&signed.index, &signed.signature, &signed.keys, now)
        .expect("a catalog the test signed is taken in");

    trust_of(sample)
}

/// How the catalog spells the platform these tests run on.
fn platform_here() -> String {
    let os = if std::env::consts::OS == "macos" {
        "darwin"
    } else {
        std::env::consts::OS
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// The catalog entry for `version` of the agent `key`, published for the
/// platform these tests run on as one zip at `url` of `sha256` and `size`
/// bytes, holding the two files `files` names with their sizes, the first of
/// which is started with `args`.
pub(crate) fn release_of(
    key: &str,
    version: &str,
    (url, sha256, size): (&str, &str, u64),
    files: [(&str, u64); 2],
    args: &[&str],
) -> Value {
    let listed: Vec<Value> = files
        .iter()
        .map(|(path, size)| {
            json!({"path": path, "size": size, "sha256": "b".repeat(64), "exec": true})
        })
        .collect();
    json!({
        "key": key,
        "registry_id": format!("{key}-acp"),
        "version": version,
        "kind": "binary",
        "license": "Apache-2.0",
        "review": "https://example.com/review/1",
        "admits": {"name": "antigravity-acp", "version": version},
        "targets": [{
            "platform": platform_here(),
            "cmd": files[0].0,
            "args": args,
            "artifacts": [{
                "url": url,
                "sha256": sha256,
                "size": size,
                "format": "zip",
                "unpacked": files[0].1 + files[1].1,
                "decompressed": files[0].1 + files[1].1,
                "files": listed,
            }],
        }],
    })
}

/// The version the snapshot built into the binary lists for the platform
/// these tests run on, where it lists one.
pub(crate) fn snapshot_version() -> Option<&'static str> {
    let published = matches!(std::env::consts::OS, "macos" | "linux" | "windows")
        && matches!(std::env::consts::ARCH, "x86_64" | "aarch64");
    published.then_some("1.3.0")
}

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
        assert_eq!(Some(row.profile_key), row.driver.profile_key(), "{row:?}");
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
            if let Ok(dialect) = (row.dialect)(mode, None) {
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
        dialect: |_, _| Ok(Box::new(crucible_external_harnesses::fake_peer::Plain)),
        profile_key: "profileDir",
        route: "agent:antigravity",
        docs_section: "antigravity",
        reaches: &[],
        sign_in: |_| None,
        pinned: |_| None,
        catalogued: false,
        shipping: false,
    }];
    let registry = Registry::new(&HELD_BACK);
    assert_eq!(registry.shipping().count(), 0);
    assert!(registry.row(Driver::Antigravity, None).is_some());
    assert!(registry.row(Driver::Claude, None).is_none());
}

/// Claude Code's own row, held back from a release, is refused at launch by
/// the section that documents it and nothing internal, and starts no
/// process.
#[tokio::test(flavor = "multi_thread")]
async fn the_claude_code_row_held_back_is_refused_by_its_section_and_starts_nothing() {
    let shipped = REGISTRY.built(Driver::Claude).expect("Claude Code's row");
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
    let state = sample.state();
    let launcher = Launcher::new(sandbox, Handle::current(), &policy, &consent, &state);
    let route = Route::External {
        serves: Serves::Provider("anthropic"),
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
            (shipped.dialect)(Mode::Ask, None).expect("asking").as_mut(),
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
        .row(Driver::Antigravity, None)
        .expect("Antigravity's row");
    let sample = Sample::new("backends-antigravity-modes");
    let trust = trust_of(&sample);
    for mode in every_mode() {
        let mut dialect = row.dialect(mode, Some(&trust)).expect("every mode");
        assert_eq!(
            dialect.request(Shape::default()).arguments,
            row.dialect(Mode::Ask, Some(&trust))
                .expect("asking")
                .request(Shape::default())
                .arguments,
            "{mode:?}"
        );
    }
}

/// The catalog's entry for `version` of Antigravity, as one zip of the two
/// files 0.48 installed, started with `args`.
fn antigravity(version: &str, args: &[&str]) -> Value {
    release_of(
        "antigravity",
        version,
        (
            &format!("https://dl.google.com/agy-extensions/releases/linux/agy-{version}.zip"),
            &"a".repeat(64),
            1000,
        ),
        [("agy_acp_server.par", 700), ("localharness_external", 300)],
        args,
    )
}

/// The release the row installs is the one the catalog lists, where one is
/// taken in, and the one the snapshot built into the binary lists where none
/// is: a reviewed version reaches the built-in agent without a release of
/// crucible.
#[test]
fn the_antigravity_row_offers_the_version_the_catalog_lists_and_the_snapshots_without_one() {
    let sample = Sample::new("backends-antigravity-offered");
    let offered = |trust: &Trust| (ANTIGRAVITY.pinned)(trust).map(|pin| pin.version.to_string());

    let snapshot_only = offered(&trust_of(&sample));
    let catalog = offered(&trusting(&sample, 2, &[antigravity("1.4.1", &["--uid="])]));

    assert_eq!(snapshot_only.as_deref(), snapshot_version());
    assert_eq!(catalog.as_deref(), Some("1.4.1"));
    assert_eq!((CLAUDE.pinned)(&trust_of(&sample)), None);
}

/// A release the installer cannot unpack, being a tarball, is not the row's
/// to install, whatever the catalog says of it.
#[test]
fn the_antigravity_row_installs_only_what_the_installer_unpacks() {
    let sample = Sample::new("backends-antigravity-unpackable");
    let mut tarball = antigravity("1.4.1", &[]);
    if let Some(format) = tarball.pointer_mut("/targets/0/artifacts/0/format") {
        *format = serde_json::json!("tar.gz");
    }

    let trust = trusting(&sample, 2, &[tarball]);

    assert_eq!(
        (ANTIGRAVITY.pinned)(&trust)
            .map(|pin| pin.version.to_string())
            .as_deref(),
        snapshot_version(),
        "the newest release it can install is the snapshot's"
    );
}

/// Antigravity is started with the arguments of the target the catalog lists
/// for the release it offers, which for the snapshot's is the empty user id on
/// Linux and nothing elsewhere, and with none while nothing is offered.
#[test]
fn antigravity_is_started_with_the_arguments_of_the_release_offered() {
    let sample = Sample::new("backends-antigravity-arguments");
    let started = |trust: Option<&Trust>| {
        (ANTIGRAVITY.dialect)(Mode::Ask, trust)
            .expect("asking")
            .request(Shape::default())
            .arguments
    };
    let listed = trusting(&sample, 2, &[antigravity("1.4.1", &["--uid=", "--quiet"])]);

    assert_eq!(started(Some(&listed)), ["--uid=", "--quiet"]);
    let snapshot_args: &[&str] = if std::env::consts::OS == "linux" {
        &["--uid="]
    } else {
        &[]
    };
    let sample = Sample::new("backends-antigravity-snapshot-arguments");
    if snapshot_version().is_some() {
        assert_eq!(started(Some(&trust_of(&sample))), snapshot_args);
    }
    assert!(started(None).is_empty());
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
    let row = REGISTRY
        .row(Driver::Claude, None)
        .expect("Claude Code's row");
    for (mode, named) in [
        (Mode::Ask, "default"),
        (Mode::AllowEdits, "acceptEdits"),
        (Mode::FullAccess, "bypassPermissions"),
    ] {
        let mut dialect = row.dialect(mode, None).expect("every mode is launched");
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
    assert_eq!(allowing(Mode::Ask), Allowing::Nothing);
    assert_eq!(allowing(Mode::AllowEdits), Allowing::Edits);
    assert_eq!(allowing(Mode::FullAccess), Allowing::Everything);
}

/// Every driver a row is built in for: all but a catalog agent's, which is
/// whichever agent its key names.
fn built_in() -> impl Iterator<Item = Driver> {
    Driver::ALL.into_iter().filter(|driver| match driver {
        Driver::Claude | Driver::Antigravity => true,
        Driver::Catalog => false,
    })
}

/// A catalog agent is the agent its key names, so no row built in is its, and
/// it serves that key and no provider; every other driver has its row.
#[test]
fn a_catalog_agent_has_no_built_in_row_and_serves_its_key() {
    let goose = crucible_types::AgentKey::new("goose").expect("an agent key");
    assert!(REGISTRY.row(Driver::Catalog, None).is_none());
    assert!(REGISTRY.row(Driver::Catalog, Some(&goose)).is_none());
    assert_eq!(
        Driver::Catalog.serves(Some(&goose)),
        Some(Serves::Agent(goose))
    );
    for driver in built_in() {
        assert!(
            matches!(REGISTRY.row(driver, None), Some(AgentRow::Built(row)) if row.driver == driver),
            "{driver:?}"
        );
    }
}

/// The configuration takes a model for exactly the drivers whose launch names
/// it, so a model written for an agent is never one its launch drops, and no
/// launch names one the configuration would refuse.
#[test]
fn a_model_is_taken_for_exactly_the_drivers_that_launch_with_it() {
    for driver in built_in() {
        let row = REGISTRY
            .row(driver, None)
            .expect("every built-in driver has a row");
        let mut dialect = row.dialect(Mode::Ask, None).expect("asking");
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

/// Every built-in driver reaches a provider this build offers, so connecting
/// an agent never puts a conversation on a provider nothing here can name.
#[test]
fn every_driver_s_provider_is_one_this_build_offers() {
    let providers = crate::providers::providers()
        .expect("the built-in providers register")
        .snapshot();
    let offered: BTreeSet<&str> = crate::providers::offered(&providers)
        .map(|one| one.name)
        .collect();
    for driver in built_in() {
        let provider = driver.serves(None).and_then(|serves| serves.provider());
        assert!(
            provider.is_some_and(|provider| offered.contains(provider)),
            "{driver:?}: {offered:?}"
        );
    }
}

/// A catalog agent's row is the one its registry lists for its key, made from
/// the entry that reviewed it and held under the agent route of that key; no
/// other key has it, a catalog instance naming no key has none, and a
/// built-in driver's row is its own, whatever key is asked with.
#[test]
fn a_catalog_agent_s_row_is_the_one_listed_for_its_key() {
    let peer = crucible_types::AgentKey::new("peer").expect("an agent key");
    let goose = crucible_types::AgentKey::new("goose").expect("an agent key");
    let registry = Registry::new(&[CLAUDE])
        .cataloging(Box::leak(Box::new([crate::launching::tests::cataloged()])));

    let Some(row @ AgentRow::Catalog(_)) = registry.row(Driver::Catalog, Some(&peer)) else {
        panic!("the catalog row of {peer}");
    };
    assert_eq!(row.named(), "peer");
    assert_eq!(row.command(), "peer");
    assert_eq!(row.route(), agent_route("peer"));
    assert!(registry.row(Driver::Catalog, Some(&goose)).is_none());
    assert!(registry.row(Driver::Catalog, None).is_none());
    assert!(matches!(
        registry.row(Driver::Claude, None),
        Some(AgentRow::Built(row)) if row.driver == Driver::Claude
    ));
    assert!(registry.row(Driver::Antigravity, None).is_none());
}
