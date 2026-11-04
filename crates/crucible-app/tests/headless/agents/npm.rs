//! The agents door installs a release a pinned Node starts: its lock is
//! fetched from the catalog before the question, which names the package
//! count, the whole size, every host the install reaches, the licence, each
//! install script left unrun and how the agent is started; Node and every
//! tarball are fetched only after the yes, each tarball held to its SHA-512,
//! and the version is committed with its tree, its preload and `.bin`, holding
//! a stub for each program the tree declares, which only their owner may
//! change, whole or not at all. The question counts the tree's packages under
//! each licence. A tarball
//! that is not its pin commits nothing, a tarball the registry answers 410 for
//! says the version can likely no longer be installed, a tree the store has no
//! room for fetches no tarball, and an offer that gives Node arguments of its
//! own is refused before anything is asked.
//!
//! An install script run at install runs in a sandbox of its own, sees only
//! the environment it is given and must leave its package as its review
//! listed it; one that cannot be so run is refused before anything is
//! fetched. One built at review runs nothing, so its tests need no sandbox and
//! run on every platform. A test that needs the sandbox to enforce says
//! so and skips where this machine's does not; one that needs a script run, or
//! the question before one, says so and skips anywhere but Linux.
//!
//! The registry here is the fake host's `registry/`, which a test build alone
//! fetches a registry address from, and only for a catalog on this machine.

use std::fs;

use crucible_app::agent_download::DownloadError;
use crucible_app::client::agents::{AgentsError, How, Outcome};
use crucible_client_api::{Pending, Text};
use crucible_external_harnesses::catalog::fixture::{Member, tar_gz};
use crucible_external_harnesses::fake_peer;
use crucible_external_harnesses::release::{BIN, PRELOAD, Preload, Stub, UnpackError, proof};
use crucible_external_harnesses::store::StoreError;
use serde_json::{Value, json};

use super::shipped_hold;
use super::uvx::{unchanged, version, versioned};
use super::{
    Bench, CATALOG, Command, Failed, Reply, Server, answering, answers, artifact, asset_path,
    digest, facts, install, name, peer, platform, row_of, set, shape, stating,
};

/// Node's path in the version, as the catalog lists it.
pub(super) fn node() -> String {
    format!("node/bin/node{}", std::env::consts::EXE_SUFFIX)
}

/// The script Node is given, in the version.
const SCRIPT: &str = "node_modules/peer/bin/peer.js";

/// Where the fake host keeps the registry's copy of the tree's one tarball.
const TARBALL: &str = "/catalog/registry/peer/-/peer-1.0.0.tgz";

/// The fake peer as Node, and the script beside it.
type Played = (Vec<u8>, Vec<u8>);

/// The lock of `peer` 1.0.0's tree, and its one tarball.
struct Tree {
    lock: Vec<u8>,
    tarball: Vec<u8>,
}

fn tree() -> Tree {
    let (lock, _, tarball) = proof::npm_tree();
    Tree { lock, tarball }
}

/// `goose` 1.0.0 as a pinned Node starts it: the fake peer playing `played`
/// as Node, with its script beside it, and `tree`'s lock, kept on `server`;
/// the tarball is not served, so each test says what the registry answers.
fn noded(server: &Server, played: &Played, tree: &Tree, node_args: &Value) -> Value {
    let (program, script) = played;
    for bytes in [program, script, &tree.lock] {
        server.serve(&asset_path(bytes), Reply::Body(bytes.clone()));
    }
    stating(json!({
        "key": "goose",
        "registry_id": "goose-acp",
        "version": "1.0.0",
        "kind": "npm",
        "license": "MIT",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/12",
        "admits": {"name": "goose", "version": "1.0.0"},
        "targets": [{
            "platform": platform(),
            "cmd": node(),
            "node_args": node_args,
            "bin": SCRIPT,
            "lock": {"sha256": digest(&tree.lock), "size": tree.lock.len(), "unpack_rules": 1},
            "scripts": [
                {"package": "node_modules/peer", "event": "postinstall", "disposition": "inert"},
            ],
            "args": ["acp"],
            "env": {"GOOSE_NO_UPDATE": "1"},
            "hosts": ["downloads.example.com", "registry.npmjs.org"],
            "artifacts": [artifact(&node(), program), artifact("node/bin/script", script)]
        }]
    }))
}

/// `goose` 1.0.0 as a pinned Node starts it, the fake peer playing `played` as
/// Node, with its whole tree on `server`.
pub(super) fn noded_as(server: &Server, played: &Played) -> Value {
    let tree = tree();
    server.serve(TARBALL, Reply::Body(tree.tarball.clone()));
    noded(server, played, &tree, &json!([]))
}

/// A bench named `name` whose catalog lists goose 1.0.0 as a pinned Node
/// starts it, whose one instance would run its Node; and the fake peer and the
/// tree it is released as.
fn bench(name: &str) -> Result<(Bench, Played, Tree), Failed> {
    bench_of(name, tree())
}

/// [`bench`], released as `tree`.
fn bench_of(name: &str, tree: Tree) -> Result<(Bench, Played, Tree), Failed> {
    let bench = Bench::new(name)?;
    let played = peer(&answers("1.0.0"))?;
    let release = noded(&bench.server, &played, &tree, &json!([]));
    bench.server.catalog(1, &[release], &json!([]))?;
    bench.configure(&version(&bench).join(node()), &json!({}))?;
    Ok((bench, played, tree))
}

/// `tree` with `fields` given to each package its lock lists, by its index,
/// and the packages of `bundled` added to it, bundled in its first.
fn amended(tree: &Tree, fields: &[(usize, Value)], bundled: &[Value]) -> Result<Tree, Failed> {
    let mut lock: Value = serde_json::from_slice(&tree.lock)?;
    let packages = lock
        .pointer_mut("/packages")
        .and_then(Value::as_array_mut)
        .ok_or("the lock's packages")?;
    for (index, given) in fields {
        let (Some(package), Some(given)) = (
            packages.get_mut(*index).and_then(Value::as_object_mut),
            given.as_object(),
        ) else {
            return Err(format!("no package {index} to give fields to").into());
        };
        package.extend(given.clone());
    }
    packages.extend(bundled.iter().cloned());
    Ok(Tree {
        lock: serde_json::to_vec(&lock)?,
        tarball: tree.tarball.clone(),
    })
}

/// The catalog, then the lock, as the fake host was asked for them.
fn catalog_and_lock(tree: &Tree) -> Vec<String> {
    let mut asked: Vec<String> = CATALOG.iter().map(|path| (*path).to_owned()).collect();
    asked.push(asset_path(&tree.lock));
    asked
}

#[test]
fn the_facts_of_a_release_a_pinned_node_starts_are_listed_before_any_tarball_is_fetched()
-> Result<(), Failed> {
    let (bench, played, tree) = bench("agents-door-npm-facts")?;
    bench
        .server
        .serve(TARBALL, Reply::Body(tree.tarball.clone()));
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, None);

    let (outcome, told) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    let [(Pending::Install { facts, .. }, asked)] = front.put.as_slice() else {
        return Err("one install question was put".into());
    };
    // The lock before the question, and nothing of the release.
    assert_eq!(asked, &catalog_and_lock(&tree));
    let facts: Vec<&str> = facts.iter().map(Text::as_str).collect();
    let total = played.0.len() + played.1.len() + tree.tarball.len();
    let expected = [
        format!("goose 1.0.0, {total} bytes to download."),
        // Node from the catalog's own host, which built it at review; the
        // tarballs from the registry, through any host the target lists.
        format!(
            "Fetched from 127.0.0.1:{}, downloads.example.com, registry.npmjs.org, the only \
             hosts it may reach.",
            bench.server.port
        ),
        "Its package tree holds 1 package, each unpacked from the tarball its lock pins by \
         SHA-512."
            .to_owned(),
        "Licence MIT.".to_owned(),
        "Install script not run: node_modules/peer postinstall, inert: it does nothing the agent \
         needs."
            .to_owned(),
        format!("Started as: {} --require {PRELOAD} {SCRIPT} acp", node()),
    ];
    for fact in &expected {
        assert!(
            facts.contains(&fact.as_str()),
            "a fact of the {} expected is missing",
            expected.len()
        );
    }
    assert!(
        facts
            .iter()
            .any(|fact| fact.starts_with("Node is given the preload") && fact.contains(".bin")),
        "how Node is started is said"
    );
    assert_eq!(bench.server.asked(), catalog_and_lock(&tree));
    assert!(told.is_empty());
    unchanged(&bench, &before, "declined")
}

#[test]
fn a_tarball_that_is_not_its_pin_commits_nothing() -> Result<(), Failed> {
    let (bench, _, tree) = bench("agents-door-npm-mismatch")?;
    let mut other = tree.tarball.clone();
    if let Some(last) = other.last_mut() {
        *last ^= 0xff;
    }
    bench.server.serve(TARBALL, Reply::Body(other));
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Err(AgentsError::Download(DownloadError::Digest))),
        "{}",
        shape(&outcome)
    );
    assert!(bench.server.asked().iter().any(|path| path == TARBALL));
    unchanged(&bench, &before, "a tarball not its pin")
}

#[test]
fn a_tarball_the_registry_no_longer_has_says_the_version_can_likely_no_longer_be_installed()
-> Result<(), Failed> {
    let (bench, _, _) = bench("agents-door-npm-gone")?;
    bench.server.serve(TARBALL, Reply::Status(410));
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    let Err(AgentsError::Download(gone @ DownloadError::Status(410))) = &outcome else {
        return Err(format!("a 410, not {}", shape(&outcome)).into());
    };
    let said = gone.to_string();
    assert!(
        said.contains("likely permanent") && said.contains("can likely no longer be installed"),
        "a 410 reads as permanent"
    );
    unchanged(&bench, &before, "a tarball gone")
}

#[test]
fn a_release_a_pinned_node_starts_is_installed_after_the_yes_and_started_under_its_preload()
-> Result<(), Failed> {
    let (bench, played, tree) = bench("agents-door-npm")?;
    bench
        .server
        .serve(TARBALL, Reply::Body(tree.tarball.clone()));
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(
            &outcome,
            Ok(Outcome::Moved { how: How::Installed, version, instances, downloaded: true, .. })
                if &**version == "1.0.0" && instances.len() == 1
        ),
        "{}",
        shape(&outcome)
    );
    let mut fetched = catalog_and_lock(&tree);
    fetched.extend([
        asset_path(&played.0),
        asset_path(&played.1),
        TARBALL.to_owned(),
    ]);
    assert_eq!(bench.server.asked(), fetched);
    let root = version(&bench);
    let program = root.join(node());
    assert_eq!(fs::read(&program)?, played.0);
    assert_eq!(fs::read(root.join(PRELOAD))?, Preload::bytes());
    assert_eq!(
        fs::read(root.join("node_modules/peer/index.js"))?,
        proof::NPM_INDEX
    );
    assert_eq!(fs::read(root.join(SCRIPT))?, proof::NPM_BIN);
    assert!(
        fs::read_dir(root.join(BIN))?.next().is_none(),
        "an empty .bin"
    );
    assert!(!root.join("tarballs").exists(), "no tarball is kept");
    let receipt: Value = serde_json::from_slice(&fs::read(root.join(".receipt"))?)?;
    let files = receipt.pointer("/files").and_then(Value::as_array);
    assert!(
        files.is_some_and(|files| files
            .iter()
            .any(|file| file.pointer("/path").and_then(Value::as_str) == Some(PRELOAD))),
        "the receipt lists the preload"
    );
    assert_eq!(bench.executable()?, program.to_string_lossy());

    // Started, Node is given the preload, then the script, then the agent's
    // arguments.
    let release = noded(&bench.server, &played, &tree, &json!([]));
    let read = versioned(&bench, row_of(&release)?, &program)??;

    assert!(
        matches!(read, crucible_external_harnesses::Reading::Version(_)),
        "it did not answer initialize"
    );
    let started = fake_peer::argued(&root.join("node/bin"));
    let expected = [
        "--require".to_owned(),
        fs::canonicalize(root.join(PRELOAD))?
            .to_string_lossy()
            .into_owned(),
        fs::canonicalize(root.join(SCRIPT))?
            .to_string_lossy()
            .into_owned(),
        "acp".to_owned(),
    ];
    assert_eq!(
        started.first().map(Vec::as_slice),
        Some(expected.as_slice())
    );

    // Committed whole, so asked again it is the version in place, and
    // nothing but the catalog and the lock is fetched.
    bench.server.forget();
    let mut again = answering(&bench.server, Some("Install"));
    let (kept, _) = bench.ask(install("1.0.0")?, &mut again, false)?;

    assert!(
        matches!(
            &kept,
            Ok(Outcome::Moved {
                downloaded: false,
                ..
            })
        ),
        "{}",
        shape(&kept)
    );
    assert_eq!(bench.server.asked(), catalog_and_lock(&tree));
    Ok(())
}

/// A tree the install store has no room for, beside what it already keeps,
/// is refused before a single tarball is fetched, though the rest of the
/// release would fit.
#[test]
fn a_tree_the_store_has_no_room_for_is_refused_before_any_tarball_is_fetched() -> Result<(), Failed>
{
    let (bench, played, tree) = bench("agents-door-npm-room")?;
    bench
        .server
        .serve(TARBALL, Reply::Body(tree.tarball.clone()));
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(installed.is_ok(), "{}", shape(&installed));
    // The version kept says it holds nearly all the store may.
    let receipt = version(&bench).join(".receipt");
    let mut kept: Value = serde_json::from_slice(&fs::read(&receipt)?)?;
    let held = kept
        .pointer_mut("/files")
        .and_then(Value::as_array_mut)
        .and_then(|files| {
            files
                .iter_mut()
                .find(|file| file.pointer("/size").is_some())
        })
        .ok_or("a file the receipt sizes")?;
    set(held, "/size", json!(15_u64 << 30));
    fs::write(&receipt, serde_json::to_vec(&kept)?)?;
    // The newer tree unpacks to more than the room left, which Node alone
    // would fit in.
    let mut lock: Value = serde_json::from_slice(&tree.lock)?;
    set(&mut lock, "/packages/0/unpacked", json!(2_u64 << 30));
    let larger = Tree {
        lock: serde_json::to_vec(&lock)?,
        tarball: tree.tarball.clone(),
    };
    let mut newer = noded(&bench.server, &played, &larger, &json!([]));
    set(&mut newer, "/version", json!("2.0.0"));
    set(&mut newer, "/admits/version", json!("2.0.0"));
    let older = noded(&bench.server, &played, &tree, &json!([]));
    bench.server.catalog(2, &[newer, older], &json!([]))?;
    bench.server.forget();
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("2.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Err(AgentsError::Store(StoreError::Full { .. }))),
        "{}",
        shape(&outcome)
    );
    assert!(
        !bench.server.asked().iter().any(|path| path == TARBALL),
        "a tarball was fetched"
    );
    assert!(!bench.program("2.0.0").exists());
    Ok(())
}

/// The preload Node loads before the agent's script is written so no one but
/// its owner may change it, and `.bin` made so only its owner may reach it,
/// whatever the umask would have allowed.
#[cfg(unix)]
#[test]
fn the_preload_and_bin_of_an_installed_tree_are_made_with_their_own_modes() -> Result<(), Failed> {
    use std::os::unix::fs::PermissionsExt as _;

    let (bench, _, tree) = bench("agents-door-npm-modes")?;
    bench.server.serve(TARBALL, Reply::Body(tree.tarball));
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(outcome.is_ok(), "{}", shape(&outcome));
    let root = version(&bench);
    let mode = |path: &str| -> Result<u32, Failed> {
        Ok(fs::symlink_metadata(root.join(path))?.permissions().mode() & 0o777)
    };
    let preload = mode(PRELOAD)?;
    assert_eq!(preload & 0o022, 0, "the preload is {preload:o}");
    assert_eq!(preload & 0o111, 0, "the preload is {preload:o}");
    assert_eq!(mode(BIN)?, 0o700, "the mode of .bin");
    Ok(())
}

#[test]
fn a_release_that_gives_node_arguments_of_its_own_is_refused_before_anything_is_asked()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-npm-args")?;
    let release = noded(
        &bench.server,
        &peer(&answers("1.0.0"))?,
        &tree(),
        &json!(["--inspect"]),
    );
    bench.server.catalog(1, &[release], &json!([]))?;
    bench.configure(&version(&bench).join(node()), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unsupported(why)) if why.contains("Node")),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    unchanged(&bench, &before, "Node arguments of its own")
}

/// A package bundled inside `peer`, named `name` and under `license`.
fn inside(name: &str, license: Option<&str>) -> Value {
    let mut package = json!({
        "location": format!("node_modules/peer/node_modules/{name}"),
        "name": name,
        "version": "1.0.0",
        "in_bundle": true,
    });
    if let (Some(fields), Some(license)) = (package.as_object_mut(), license) {
        fields.insert("license".to_owned(), json!(license));
    }
    package
}

/// The facts that count licences in the question asked for the tree whose
/// top package is under `top` and that bundles `bundled`, run in a bench
/// named `named`.
fn licence_facts(named: &str, top: Option<&str>, bundled: &[Value]) -> Result<Vec<String>, Failed> {
    let given: Vec<(usize, Value)> = top
        .map(|top| (0, json!({ "license": top })))
        .into_iter()
        .collect();
    let tree = amended(&tree(), &given, bundled)?;
    let (bench, _, _) = bench_of(named, tree)?;
    let mut front = answering(&bench.server, None);

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    Ok(facts(&front)?
        .into_iter()
        .filter(|fact| fact.contains(" of its tree under ") || fact.ends_with(" more licences."))
        .collect())
}

/// The question counts the packages of the tree under each licence its lock
/// gives, quoted, the most common first, a package that gives none counted as
/// such, and names no more than eight licences before it says how many more.
#[test]
fn the_question_counts_the_packages_of_the_tree_under_each_licence() -> Result<(), Failed> {
    let mut bundled = vec![
        inside("a", Some("MIT")),
        inside("b", Some("MIT")),
        inside("c", Some("Apache-2.0")),
        inside("d", Some("Apache-2.0")),
        inside("e", None),
        inside("f", None),
    ];
    let single = [
        "Zlib",
        "Unlicense",
        "ISC",
        "CC0-1.0",
        "BlueOak-1.0.0",
        "Python-2.0",
        "0BSD",
        "(MIT OR Apache-2.0)",
    ];
    for (at, license) in single.iter().enumerate() {
        bundled.push(inside(&format!("single{at}"), Some(license)));
    }

    let counted = licence_facts("agents-door-npm-licences", Some("MIT"), &bundled)?;

    let expected = [
        "3 packages of its tree under \"MIT\".",
        "2 packages of its tree under \"Apache-2.0\".",
        "2 packages of its tree under no licence given.",
        "1 package of its tree under \"(MIT OR Apache-2.0)\".",
        "1 package of its tree under \"0BSD\".",
        "1 package of its tree under \"BlueOak-1.0.0\".",
        "1 package of its tree under \"CC0-1.0\".",
        "1 package of its tree under \"ISC\".",
        "And 3 more licences.",
    ];
    assert_eq!(counted, expected);
    Ok(())
}

/// A licence the lock gives is quoted, so one that spells the question's own
/// words for a package that gives none is told apart from that row.
#[test]
fn a_licence_spelling_the_questions_own_words_is_told_apart_from_them() -> Result<(), Failed> {
    let bundled = [inside("a", Some("no licence given")), inside("b", None)];

    let counted = licence_facts("agents-door-npm-licence-words", None, &bundled)?;

    let expected = [
        "2 packages of its tree under no licence given.",
        "1 package of its tree under \"no licence given\".",
    ];
    assert_eq!(counted, expected);
    Ok(())
}

/// A lock in which no package gives a licence carries no counts, so the
/// question counts none.
#[test]
fn a_lock_no_package_of_which_gives_a_licence_is_counted_under_none() -> Result<(), Failed> {
    let bundled = [inside("a", None), inside("b", None)];

    let counted = licence_facts("agents-door-npm-unlicensed", None, &bundled)?;

    assert_eq!(counted, Vec::<String>::new());
    Ok(())
}

/// Each program the tree declares is given a stub in `.bin`, which starts the
/// pinned Node on its script under the preload, and the receipt names it.
#[test]
fn each_program_the_tree_declares_is_given_a_stub_in_bin_that_the_receipt_names()
-> Result<(), Failed> {
    let declared = amended(
        &tree(),
        &[(0, json!({"bin": {"peer": "./bin/peer.js"}}))],
        &[],
    )?;
    let (bench, _, tree) = bench_of("agents-door-npm-stubs", declared)?;
    bench.server.serve(TARBALL, Reply::Body(tree.tarball));
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(outcome.is_ok(), "{}", shape(&outcome));
    let root = version(&bench);
    let script = root
        .join("node_modules")
        .join("peer")
        .join("bin")
        .join("peer.js");
    let started = (root.join(node()), root.join(PRELOAD));
    let stub = if cfg!(windows) {
        Stub::windows("peer", &started.0, &started.1, &script)?
    } else {
        Stub::unix("peer", &started.0, &started.1, &script)?
    };
    assert_eq!(
        fs::read(root.join(stub.path())).ok().as_deref(),
        Some(stub.bytes()),
        "the stub in .bin"
    );
    assert_eq!(fs::read_dir(root.join(BIN))?.count(), 1, "one stub in .bin");
    let receipt: Value = serde_json::from_slice(&fs::read(root.join(".receipt"))?)?;
    let named = receipt
        .pointer("/files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files
                .iter()
                .find(|file| file.pointer("/path").and_then(Value::as_str) == Some(stub.path()))
        });
    assert!(named.is_some(), "the receipt names the stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(root.join(stub.path()))?.permissions().mode() & 0o777;
        assert_eq!(mode & 0o022, 0, "the stub is {mode:o}");
        assert_eq!(mode & 0o100, 0o100, "the stub is {mode:o}");
    }

    // Asked again, the version in place is the one its receipt names, stub
    // and all, so nothing is fetched.
    bench.server.forget();
    let mut again = answering(&bench.server, Some("Install"));
    let (kept, _) = bench.ask(install("1.0.0")?, &mut again, false)?;
    assert!(
        matches!(
            &kept,
            Ok(Outcome::Moved {
                downloaded: false,
                ..
            })
        ),
        "{}",
        shape(&kept)
    );

    // The stub is a file the receipt names like any other, so a changed byte
    // in it is named by verify.
    let verify = || -> Result<Command, Failed> {
        Ok(Command::VerifyAgent {
            agent: name("goose")?,
        })
    };
    let mut quiet = answering(&bench.server, None);
    let (whole, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert!(
        matches!(&whole, Ok(Outcome::Verified { versions, .. })
            if versions.iter().all(|(_, differs)| differs.is_empty())),
        "{}",
        shape(&whole)
    );
    fs::write(root.join(stub.path()), b"changed")?;
    let (differs, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert!(
        matches!(&differs, Ok(Outcome::Verified { versions, .. })
            if versions.iter().any(|(_, differs)| differs
                .iter()
                .any(|one| &*one.path == stub.path()))),
        "{}",
        shape(&differs)
    );
    Ok(())
}

/// `goose` 1.0.0 as [`noded`] releases it, the tarball served, its one install
/// script `script`.
fn scripted(server: &Server, played: &Played, tree: &Tree, script: &Value) -> Value {
    server.serve(TARBALL, Reply::Body(tree.tarball.clone()));
    let mut release = noded(server, played, tree, &json!([]));
    set(&mut release, "/targets/0/scripts", json!([script]));
    release
}

/// What `node_modules/peer` holds once review's run of its script ended: the
/// tree's two files as they unpack, and each of `more`, by its path, its
/// bytes and whether it runs.
fn listing(more: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut files = vec![
        json!({"path": "index.js", "size": proof::NPM_INDEX.len(), "sha256": digest(proof::NPM_INDEX), "exec": false}),
        json!({"path": "bin/peer.js", "size": proof::NPM_BIN.len(), "sha256": digest(proof::NPM_BIN), "exec": true}),
    ];
    files.extend(more.iter().map(|(path, bytes, exec)| {
        json!({"path": path, "size": bytes.len(), "sha256": digest(bytes), "exec": exec})
    }));
    json!({ "files": files }).to_string().into_bytes()
}

/// A bench named `name` whose catalog lists goose 1.0.0 with the one install
/// script `peer`'s `postinstall`, run at install as `command`, after which
/// its package must hold what `result` lists, kept on the catalog's host;
/// and the release.
fn local(name: &str, command: &str, result: &[u8]) -> Result<(Bench, Value), Failed> {
    let bench = Bench::new(name)?;
    bench
        .server
        .serve(&asset_path(result), Reply::Body(result.to_vec()));
    let release = scripted(
        &bench.server,
        &peer(&answers("1.0.0"))?,
        &tree(),
        &json!({
            "package": "node_modules/peer",
            "event": "postinstall",
            "disposition": "local",
            "command": command,
            "hosts": [],
            "result": {"sha256": digest(result), "size": result.len()},
        }),
    );
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    bench.configure(&version(&bench).join(node()), &json!({}))?;
    Ok((bench, release))
}

/// Whether this platform runs an install script at install at all; where it
/// does not, the release is refused before the question, and the test that
/// needs that question says so.
#[allow(clippy::print_stderr)]
fn scripts_run_here(test: &str) -> bool {
    let run = cfg!(target_os = "linux");
    if !run {
        eprintln!("{test}: skipped, as install scripts run on Linux alone");
    }
    run
}

/// Whether this machine's sandbox can hold an install script in; where it
/// cannot, the test that needs it says so and passes over nothing.
#[allow(clippy::print_stderr)]
fn sandboxed(runtime: &tokio::runtime::Runtime, test: &str) -> Result<bool, Failed> {
    if !scripts_run_here(test) {
        return Ok(false);
    }
    let enforced = super::enforcing(runtime)?;
    if !enforced {
        eprintln!("{test}: skipped, as this machine's sandbox does not enforce its rules");
    }
    Ok(enforced)
}

/// `bench` asked to install goose 1.0.0, lent a launcher whose sandbox is on
/// and whose route has its yes.
fn installed_in_sandbox(
    bench: &Bench,
    runtime: &tokio::runtime::Runtime,
) -> Result<Result<Outcome, AgentsError>, Failed> {
    let mut front = answering(&bench.server, Some("Install"));
    bench.ask_smoking((install("1.0.0")?, &mut front), (true, true), runtime)
}

/// What a build left of `node_modules/peer` in these tests: the tree's
/// `index.js`, an addon of its own, and none of the tarball's `bin`.
const ADDON: &[u8] = b"the addon the build made\n";

fn build() -> Vec<Member<'static>> {
    vec![
        Member::Dir("build"),
        Member::File("build/addon.node", ADDON, true),
        Member::File("index.js", proof::NPM_INDEX, false),
    ]
}

/// The list a built script's asset is read against, naming `members`.
fn build_list(members: &[Member<'_>]) -> Vec<u8> {
    let listed: Vec<Value> = members
        .iter()
        .filter_map(|member| match *member {
            Member::File(path, bytes, exec) => Some(json!({
                "path": path, "kind": "file", "size": bytes.len(),
                "sha256": digest(bytes), "exec": exec,
            })),
            Member::Dir(path) => Some(json!({"path": path, "kind": "dir"})),
            Member::Symlink(..) | Member::Hard(..) => None,
        })
        .collect();
    json!(listed).to_string().into_bytes()
}

/// A script of `package` on `event` built at review as `archive`, listed by
/// `list`, each kept on `server`.
fn built_script(server: &Server, package: &str, event: &str, archive: &[u8], list: &[u8]) -> Value {
    server.serve(&asset_path(archive), Reply::Body(archive.to_vec()));
    server.serve(&asset_path(list), Reply::Body(list.to_vec()));
    json!({
        "package": package,
        "event": event,
        "disposition": "built",
        "asset": {"sha256": digest(archive), "size": archive.len()},
        "files": {"sha256": digest(list), "size": list.len()},
        "unpacked": 1 << 20,
        "decompressed": 1 << 20,
    })
}

/// A bench named `name` whose catalog lists goose 1.0.0 with the one install
/// script `peer`'s `postinstall`, built at review as an archive of `archived`
/// and listed as `listed`; and that archive.
fn built_as(
    name: &str,
    archived: &[Member<'_>],
    listed: &[Member<'_>],
) -> Result<(Bench, Vec<u8>), Failed> {
    let bench = Bench::new(name)?;
    let archive = tar_gz(archived);
    let script = built_script(
        &bench.server,
        "node_modules/peer",
        "postinstall",
        &archive,
        &build_list(listed),
    );
    let release = scripted(&bench.server, &peer(&answers("1.0.0"))?, &tree(), &script);
    bench.server.catalog(1, &[release], &json!([]))?;
    bench.configure(&version(&bench).join(node()), &json!({}))?;
    Ok((bench, archive))
}

/// A script built at review runs nothing at install, so it takes no launcher
/// and no sandbox: its asset comes from the catalog's host, the package's
/// directory ends as the review listed it, whatever the tarball left there,
/// the receipt names its files and verify covers them; a second install of the
/// version fetches nothing.
#[test]
fn a_script_built_at_review_is_installed_from_the_catalog_and_listed_in_the_receipt()
-> Result<(), Failed> {
    let (bench, archive) = built_as("agents-door-npm-built", &build(), &build())?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(
            &outcome,
            Ok(Outcome::Moved {
                downloaded: true,
                ..
            })
        ),
        "{}",
        shape(&outcome)
    );
    assert!(
        bench.server.asked().contains(&asset_path(&archive)),
        "the build came from the catalog's own host"
    );
    let root = version(&bench);
    let package = root.join("node_modules").join("peer");
    let addon = package.join("build").join("addon.node");
    assert_eq!(fs::read(&addon)?, ADDON);
    assert_eq!(fs::read(package.join("index.js"))?, proof::NPM_INDEX);
    assert!(
        !package.join("bin").exists(),
        "what the tarball left and the review's listing does not name is gone"
    );
    let receipt: Value = serde_json::from_slice(&fs::read(root.join(".receipt"))?)?;
    let named: Vec<&str> = receipt
        .pointer("/files")
        .and_then(Value::as_array)
        .ok_or("the receipt's files")?
        .iter()
        .filter_map(|file| file.pointer("/path").and_then(Value::as_str))
        .collect();
    assert!(named.contains(&"node_modules/peer/build/addon.node"));
    assert!(named.contains(&"node_modules/peer/index.js"));
    assert!(!named.contains(&"node_modules/peer/bin/peer.js"));

    let verify = || -> Result<Command, Failed> {
        Ok(Command::VerifyAgent {
            agent: name("goose")?,
        })
    };
    let mut quiet = answering(&bench.server, None);
    let (whole, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert!(
        matches!(&whole, Ok(Outcome::Verified { versions, .. })
            if versions.iter().all(|(_, differs)| differs.is_empty())),
        "{}",
        shape(&whole)
    );
    bench.server.forget();
    let mut again = answering(&bench.server, Some("Install"));
    let (kept, _) = bench.ask(install("1.0.0")?, &mut again, false)?;
    assert!(
        matches!(
            &kept,
            Ok(Outcome::Moved {
                downloaded: false,
                ..
            })
        ),
        "{}",
        shape(&kept)
    );
    assert!(!bench.server.asked().contains(&asset_path(&archive)));

    fs::write(&addon, b"changed")?;
    let (differs, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert!(
        matches!(&differs, Ok(Outcome::Verified { versions, .. })
            if versions.iter().any(|(_, differs)| !differs.is_empty())),
        "{}",
        shape(&differs)
    );
    Ok(())
}

/// The question names the package and the size of what replaces it, and its
/// list is read before it is put; a no fetches none of the build.
#[test]
fn the_question_names_a_built_scripts_package_and_its_assets_size() -> Result<(), Failed> {
    let (bench, archive) = built_as("agents-door-npm-built-facts", &build(), &build())?;
    let list = build_list(&build());
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, None);

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    let [(Pending::Install { facts, .. }, asked)] = front.put.as_slice() else {
        return Err("one install question was put".into());
    };
    let said = format!("{} bytes", archive.len());
    assert!(
        facts.iter().map(Text::as_str).any(|fact| {
            fact.starts_with(
                "Install script not run: node_modules/peer postinstall, built at review",
            ) && fact.contains(&said)
        }),
        "the built script is named with its asset's size"
    );
    assert!(asked.contains(&asset_path(&list)));
    assert!(!asked.contains(&asset_path(&archive)));
    assert!(!bench.server.asked().contains(&asset_path(&archive)));
    unchanged(&bench, &before, "declined")
}

/// A build runs no code at install, so `agents.update = "reviewed"` stands
/// for its question, as it does for any artifact.
#[test]
fn under_reviewed_an_update_with_a_built_script_is_taken_without_the_question() -> Result<(), Failed>
{
    let bench = Bench::new("agents-door-npm-built-reviewed")?;
    bench.configure(
        &bench.program("1.0.0"),
        &json!({"agents": {"update": "reviewed"}}),
    )?;
    bench.server.publish(1, &[("1.0.0", super::ONE)], &[]);
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(&installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let script = built_script(
        &bench.server,
        "node_modules/peer",
        "postinstall",
        &tar_gz(&build()),
        &build_list(&build()),
    );
    let mut release = scripted(&bench.server, &peer(&answers("2.0.0"))?, &tree(), &script);
    set(&mut release, "/version", json!("2.0.0"));
    set(&mut release, "/admits/version", json!("2.0.0"));
    bench.server.catalog(2, &[release], &json!([]))?;
    let mut front = answering(&bench.server, Some("Cancel"));

    let (updated, _) = bench.ask(
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &mut front,
        false,
    )?;

    assert!(front.put.is_empty(), "nothing was put to the client");
    assert!(
        matches!(
            &updated,
            Ok(Outcome::Moved {
                how: How::Updated,
                standing: true,
                ..
            })
        ),
        "{}",
        shape(&updated)
    );
    assert_eq!(
        fs::read(
            bench
                .programs()
                .join("2.0.0")
                .join("node_modules")
                .join("peer")
                .join("build")
                .join("addon.node")
        )?,
        ADDON
    );
    Ok(())
}

/// An asset from any host but the catalog's, one that is not its hash and one
/// that is not its size, each leave nothing behind.
#[test]
fn a_built_asset_from_another_host_or_not_its_pin_is_refused_and_leaves_no_staging()
-> Result<(), Failed> {
    let elsewhere = Server::start()?;
    let archive = tar_gz(&build());
    elsewhere.serve("/stolen", Reply::Body(archive.clone()));
    let mut flipped = archive.clone();
    if let Some(last) = flipped.last_mut() {
        *last ^= 0xff;
    }
    let mut short = archive.clone();
    short.pop();
    let cases: [(&str, Reply); 3] = [
        ("redirected", Reply::Redirect(elsewhere.at("/stolen"))),
        ("flipped", Reply::Body(flipped)),
        ("short", Reply::Body(short)),
    ];
    for (case, reply) in cases {
        let (bench, archive) =
            built_as(&format!("agents-door-npm-built-{case}"), &build(), &build())?;
        bench.server.serve(&asset_path(&archive), reply);
        let before = fs::read(bench.user())?;
        let mut front = answering(&bench.server, Some("Install"));

        let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

        assert!(
            matches!(&outcome, Err(AgentsError::Download(_))),
            "{case}: {}",
            shape(&outcome)
        );
        assert!(elsewhere.asked().is_empty(), "{case}");
        unchanged(&bench, &before, case)?;
    }
    Ok(())
}

/// The package ends as the listing says or nothing is installed: an archive
/// holding what the listing does not name, missing what it does, or holding a
/// member that is neither a file nor a directory, is refused, the links
/// whether or not the listing names their paths.
#[test]
fn a_built_asset_that_is_not_its_listing_or_holds_a_link_is_refused_and_leaves_no_staging()
-> Result<(), Failed> {
    let file = Member::File("index.js", proof::NPM_INDEX, false);
    let copy = Member::File("copy.js", proof::NPM_INDEX, false);
    let cases: [(&str, Vec<Member<'_>>, Vec<Member<'_>>, &str); 5] = [
        ("unlisted", vec![file, copy], vec![file], "unlisted"),
        ("missing", vec![file], vec![file, copy], "missing"),
        (
            "symlink",
            vec![file, Member::Symlink("copy.js", "index.js")],
            vec![file, copy],
            "kind",
        ),
        (
            "hard",
            vec![file, Member::Hard("copy.js", "index.js")],
            vec![file, copy],
            "kind",
        ),
        (
            "linked",
            vec![file, Member::Symlink("copy.js", "index.js")],
            vec![file],
            "kind",
        ),
    ];
    for (case, archived, listed, refused) in cases {
        let (bench, _) = built_as(&format!("agents-door-npm-built-{case}"), &archived, &listed)?;
        let before = fs::read(bench.user())?;
        let mut front = answering(&bench.server, Some("Install"));

        let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

        let held = matches!(
            (&outcome, refused),
            (
                Err(AgentsError::Unpacked(UnpackError::Unlisted { .. })),
                "unlisted"
            ) | (
                Err(AgentsError::Unpacked(UnpackError::Missing { .. })),
                "missing"
            ) | (Err(AgentsError::Unpacked(UnpackError::Kind { .. })), "kind")
        );
        assert!(held, "{case}: {}", shape(&outcome));
        unchanged(&bench, &before, case)?;
    }
    Ok(())
}

/// A build's list that leaves out a program its package declares would leave
/// a stub with nothing to start, so it is refused before the question; the
/// same list holding it is taken.
#[test]
fn a_build_that_leaves_out_a_program_its_package_declares_is_refused_before_the_question()
-> Result<(), Failed> {
    let declared = amended(
        &tree(),
        &[(0, json!({"bin": {"peer": "./bin/peer.js"}}))],
        &[],
    )?;
    let without = build();
    let mut with = build();
    with.extend([
        Member::Dir("bin"),
        Member::File("bin/peer.js", proof::NPM_BIN, true),
    ]);
    for (case, members) in [("without", &without), ("with", &with)] {
        let bench = Bench::new(&format!("agents-door-npm-built-program-{case}"))?;
        let archive = tar_gz(members);
        let script = built_script(
            &bench.server,
            "node_modules/peer",
            "postinstall",
            &archive,
            &build_list(members),
        );
        let release = scripted(&bench.server, &peer(&answers("1.0.0"))?, &declared, &script);
        bench.server.catalog(1, &[release], &json!([]))?;
        bench.configure(&version(&bench).join(node()), &json!({}))?;
        let mut front = answering(&bench.server, Some("Install"));

        let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

        if case == "without" {
            assert!(
                matches!(&outcome, Err(AgentsError::Unsupported(said)) if said.contains("a program")),
                "{}",
                shape(&outcome)
            );
            assert!(front.put.is_empty(), "no question");
            assert!(!bench.server.asked().contains(&asset_path(&archive)));
        } else {
            assert!(outcome.is_ok(), "{}", shape(&outcome));
        }
    }
    Ok(())
}

/// A question names every built script, so a release with more than it shows
/// is refused before anything but its catalog is fetched; so is one a package
/// its lock does not install, and one with two builds of a package or of one
/// inside another, whose listings cannot both be what it ends as. The whole rule
/// is the table the client's and the intake's tests share.
#[test]
fn a_release_whose_built_scripts_cannot_be_installed_is_refused_before_the_question()
-> Result<(), Failed> {
    let archive = tar_gz(&build());
    let list = build_list(&build());
    let cases = [
        (
            "many",
            (0..17)
                .map(|index| (format!("node_modules/{index}"), "postinstall"))
                .collect(),
            "more install scripts",
        ),
        (
            "absent",
            vec![("node_modules/absent".to_owned(), "postinstall")],
            "belongs to no package",
        ),
        (
            "twice",
            vec![
                ("node_modules/peer".to_owned(), "preinstall"),
                ("node_modules/peer".to_owned(), "postinstall"),
            ],
            "two builds",
        ),
        (
            "nested",
            vec![
                ("node_modules/peer".to_owned(), "postinstall"),
                (
                    "node_modules/peer/node_modules/inner".to_owned(),
                    "postinstall",
                ),
            ],
            "two builds",
        ),
    ];
    for (case, scripts, why) in cases {
        let bench = Bench::new(&format!("agents-door-npm-built-{case}"))?;
        let scripts: Vec<Value> = scripts
            .iter()
            .map(|(package, event)| built_script(&bench.server, package, event, &archive, &list))
            .collect();
        let mut release = scripted(
            &bench.server,
            &peer(&answers("1.0.0"))?,
            &tree(),
            &json!(null),
        );
        set(&mut release, "/targets/0/scripts", json!(scripts));
        bench.server.catalog(1, &[release], &json!([]))?;
        bench.configure(&version(&bench).join(node()), &json!({}))?;
        let before = fs::read(bench.user())?;
        let mut front = answering(&bench.server, Some("Install"));

        let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

        assert!(
            matches!(&outcome, Err(AgentsError::Unsupported(said)) if said.contains(why)),
            "{case}: {}",
            shape(&outcome)
        );
        assert!(front.put.is_empty(), "{case}");
        assert!(
            !bench.server.asked().contains(&asset_path(&archive)),
            "{case}"
        );
        unchanged(&bench, &before, case)?;
    }
    Ok(())
}

/// The receipt of a version in place is held to the build the catalog lists,
/// so a version whose build the catalog changed is refused rather than kept.
#[test]
fn a_version_in_place_with_another_build_than_the_catalog_lists_is_refused() -> Result<(), Failed> {
    let (bench, _) = built_as("agents-door-npm-built-kept", &build(), &build())?;
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(&installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let other = [Member::File("index.js", proof::NPM_INDEX, false)];
    let script = built_script(
        &bench.server,
        "node_modules/peer",
        "postinstall",
        &tar_gz(&other),
        &build_list(&other),
    );
    let release = scripted(&bench.server, &peer(&answers("1.0.0"))?, &tree(), &script);
    bench.server.catalog(2, &[release], &json!([]))?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Kept { .. })),
        "{}",
        shape(&outcome)
    );
    Ok(())
}

#[test]
fn a_release_whose_install_script_runs_at_install_is_refused_without_a_conversation()
-> Result<(), Failed> {
    if !scripts_run_here(
        "a_release_whose_install_script_runs_at_install_is_refused_without_a_conversation",
    ) {
        return Ok(());
    }
    let (bench, _) = local("agents-door-npm-unlent", "true", &listing(&[]))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unsupported(why)) if why.contains("conversation")),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    unchanged(&bench, &before, "no conversation's sandbox")
}

#[test]
fn a_release_whose_install_script_runs_at_install_is_refused_with_the_sandbox_off()
-> Result<(), Failed> {
    let runtime = super::runtime()?;
    let (bench, _) = local("agents-door-npm-unconfined", "true", &listing(&[]))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let outcome = bench.ask_smoking((install("1.0.0")?, &mut front), (false, true), &runtime)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unsupported(why)) if why.contains("sandbox")),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    unchanged(&bench, &before, "the sandbox off")
}

/// `agents.update = "reviewed"` stands in for the question before an update,
/// which is where a script's command and hosts are shown, so an update with
/// one to run is asked about, and a no runs nothing and moves nothing.
#[test]
fn under_reviewed_an_update_with_an_install_script_to_run_is_still_asked() -> Result<(), Failed> {
    if !scripts_run_here("under_reviewed_an_update_with_an_install_script_to_run_is_still_asked") {
        return Ok(());
    }
    let runtime = super::runtime()?;
    let bench = Bench::new("agents-door-npm-reviewed")?;
    bench.configure(
        &bench.program("1.0.0"),
        &json!({"agents": {"update": "reviewed"}}),
    )?;
    bench.server.publish(1, &[("1.0.0", super::ONE)], &[]);
    let mut front = answering(&bench.server, Some("Install"));
    let (installed, _) = bench.ask(install("1.0.0")?, &mut front, false)?;
    assert!(
        matches!(&installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    let result = listing(&[]);
    bench
        .server
        .serve(&asset_path(&result), Reply::Body(result.clone()));
    let mut release = scripted(
        &bench.server,
        &peer(&answers("2.0.0"))?,
        &tree(),
        &json!({
            "package": "node_modules/peer",
            "event": "postinstall",
            "disposition": "local",
            "command": "touch ran",
            "hosts": ["downloads.example.com"],
            "result": {"sha256": digest(&result), "size": result.len()},
        }),
    );
    set(&mut release, "/version", json!("2.0.0"));
    set(&mut release, "/admits/version", json!("2.0.0"));
    bench
        .server
        .catalog(2, std::slice::from_ref(&release), &json!([]))?;
    let mut front = answering(&bench.server, Some("Cancel"));

    let updated = bench.ask_smoking(
        (
            Command::UpdateAgent {
                agent: name("goose")?,
            },
            &mut front,
        ),
        (true, true),
        &runtime,
    )?;

    assert!(
        matches!(&updated, Ok(Outcome::Undecided)),
        "{}",
        shape(&updated)
    );
    let [(Pending::Install { facts, .. }, _)] = front.put.as_slice() else {
        return Err(format!("asked {}", front.put.len()).into());
    };
    assert!(
        facts.iter().any(|fact| {
            let fact = fact.as_str();
            fact.contains("touch ran") && fact.contains("downloads.example.com")
        }),
        "the question shows the command and its hosts"
    );
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );
    Ok(())
}

/// The release [`local`] gives, with `count` install scripts, each its own
/// command, all of them to run at install.
fn with_scripts(bench: &Bench, mut release: Value, count: usize) -> Result<Value, Failed> {
    let result = listing(&[]);
    let scripts: Vec<Value> = (0..count)
        .map(|at| {
            json!({
                "package": "node_modules/peer",
                "event": "postinstall",
                "disposition": "local",
                "command": format!("command number {at}."),
                "hosts": [format!("host{at}.example.com")],
                "result": {"sha256": digest(&result), "size": result.len()},
            })
        })
        .collect();
    set(&mut release, "/targets/0/scripts", json!(scripts));
    bench
        .server
        .catalog(2, std::slice::from_ref(&release), &json!([]))?;
    Ok(release)
}

/// `name`'s bench, listing goose's scripted release signed for the agent `key`,
/// stated or not, and the door's answer to installing it under `route`'s yes.
fn script_of_key_given(
    name: &str,
    (key, stated, route): (&str, bool, &str),
    runtime: &tokio::runtime::Runtime,
) -> Result<(Bench, Result<Outcome, AgentsError>), Failed> {
    let (bench, release) = local(name, "true", &listing(&[]))?;
    let mut release = shipped_hold::rekeyed(release, key);
    if !stated && let Some(object) = release.as_object_mut() {
        object.remove("content_use");
    }
    bench
        .server
        .catalog(2, std::slice::from_ref(&release), &json!([]))?;
    let consent = shipped_hold::shipped_given(route)?;
    let mut front = answering(&bench.server, Some("Install"));
    let outcome = shipped_hold::installed_over(&bench, key, &consent, &mut front, runtime)?;
    Ok((bench, outcome))
}

/// A signed entry keyed `claude` backs no built-in, so the yes to Claude Code's
/// route does not stand for it: its install script is refused before its tree
/// is fetched or anything is run, stated or not, and says why.
#[test]
fn an_install_script_of_an_entry_keyed_for_a_built_in_it_does_not_back_is_refused_under_that_yes()
-> Result<(), Failed> {
    let runtime = super::runtime()?;
    if !sandboxed(
        &runtime,
        "an_install_script_of_an_entry_keyed_for_a_built_in_it_does_not_back_is_refused_under_that_yes",
    )? {
        return Ok(());
    }
    for (case, stated) in [("stated", true), ("unstated", false)] {
        let (bench, outcome) = script_of_key_given(
            &format!("agents-door-npm-keyed-claude-{case}"),
            ("claude", stated, "agent:claude"),
            &runtime,
        )?;

        assert!(
            matches!(&outcome, Err(AgentsError::Script { why, .. })
                if why.to_string().contains("is not its entry's own")),
            "{case}: {}",
            shape(&outcome)
        );
        assert!(
            !bench.home.path().join("programs/claude").exists(),
            "{case}: a version was put in place"
        );
        // What review reads lies on the catalog's host; nothing to run was fetched.
        let asked = bench.server.asked();
        assert!(
            asked.iter().all(|path| path.starts_with("/catalog/")),
            "{case}: something was fetched: {asked:?}"
        );
    }
    Ok(())
}

/// A yes covers only what was shown, so a release with more scripts to run
/// than a question names is not offered that yes.
#[test]
fn a_release_with_more_install_scripts_than_a_question_shows_is_refused_before_anything_is_fetched()
-> Result<(), Failed> {
    let runtime = super::runtime()?;
    let (bench, release) = local("agents-door-npm-many", "true", &listing(&[]))?;
    with_scripts(&bench, release, 17)?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let outcome = bench.ask_smoking((install("1.0.0")?, &mut front), (true, true), &runtime)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unsupported(why)) if why.contains("more install scripts")),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    unchanged(&bench, &before, "too many scripts")
}

/// The most a question shows are each shown, with its own command and hosts.
#[test]
fn every_install_script_of_the_most_a_question_shows_is_named_with_its_hosts() -> Result<(), Failed>
{
    if !scripts_run_here(
        "every_install_script_of_the_most_a_question_shows_is_named_with_its_hosts",
    ) {
        return Ok(());
    }
    let runtime = super::runtime()?;
    let (bench, release) = local("agents-door-npm-sixteen", "true", &listing(&[]))?;
    with_scripts(&bench, release, 16)?;
    let mut front = answering(&bench.server, Some("Cancel"));

    let outcome = bench.ask_smoking((install("1.0.0")?, &mut front), (true, true), &runtime)?;

    assert!(
        matches!(&outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    let [(Pending::Install { facts, .. }, _)] = front.put.as_slice() else {
        return Err(format!("asked {}", front.put.len()).into());
    };
    for at in 0..16 {
        let (command, host) = (
            format!("command number {at}."),
            format!("host{at}.example.com"),
        );
        assert!(
            facts.iter().any(|fact| {
                let fact = fact.as_str();
                fact.contains(&command) && fact.contains(&host)
            }),
            "script {at} is named"
        );
    }
    assert!(
        !facts
            .iter()
            .any(|fact| fact.as_str().contains("more install scripts")),
        "nothing is summed up"
    );
    Ok(())
}

/// A command is delimited, so nothing at its end can read as the sentence's
/// own clause about what it reaches.
#[test]
fn an_install_scripts_command_cannot_read_as_the_question_s_own_words() -> Result<(), Failed> {
    if !scripts_run_here("an_install_scripts_command_cannot_read_as_the_question_s_own_words") {
        return Ok(());
    }
    let runtime = super::runtime()?;
    let (bench, mut release) = local(
        "agents-door-npm-delimited",
        r#"printf "a\b", reaching no host"#,
        &listing(&[]),
    )?;
    set(
        &mut release,
        "/targets/0/scripts/0/hosts",
        json!(["downloads.example.com"]),
    );
    bench
        .server
        .catalog(2, std::slice::from_ref(&release), &json!([]))?;
    let mut front = answering(&bench.server, Some("Cancel"));

    bench
        .ask_smoking((install("1.0.0")?, &mut front), (true, true), &runtime)?
        .map_err(|why| format!("{why}"))?;

    let [(Pending::Install { facts, .. }, _)] = front.put.as_slice() else {
        return Err(format!("asked {}", front.put.len()).into());
    };
    let told = r#"sh -c "printf \"a\\b\", reaching no host", reaching "#;
    assert!(
        facts.iter().any(|fact| fact.as_str().contains(told)),
        "the command is delimited"
    );
    Ok(())
}

/// A mode a script leaves does not keep what it left from being removed.
#[test]
fn an_install_script_leaving_a_closed_directory_refuses_and_leaves_no_staging() -> Result<(), Failed>
{
    let runtime = super::runtime()?;
    if !sandboxed(
        &runtime,
        "an_install_script_leaving_a_closed_directory_refuses_and_leaves_no_staging",
    )? {
        return Ok(());
    }
    let (bench, _) = local(
        "agents-door-npm-closed",
        "mkdir -p left/inner && chmod 000 left/inner left",
        &listing(&[]),
    )?;
    let before = fs::read(bench.user())?;

    let outcome = installed_in_sandbox(&bench, &runtime)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Script { .. })),
        "{}",
        shape(&outcome)
    );
    unchanged(&bench, &before, "a closed directory")
}

/// On macOS nothing could prove a script's every process stopped, so one is
/// not run there, with the sandbox on and a conversation lending it.
#[cfg(target_os = "macos")]
#[test]
fn a_release_whose_install_script_runs_at_install_is_refused_on_macos() -> Result<(), Failed> {
    let runtime = super::runtime()?;
    let (bench, _) = local("agents-door-npm-macos", "true", &listing(&[]))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let outcome = bench.ask_smoking((install("1.0.0")?, &mut front), (true, true), &runtime)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unsupported(why)) if why.contains("platform")),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    unchanged(&bench, &before, "macOS")
}

/// From a command line too, macOS is told it is the platform, and not to try
/// from a conversation.
#[cfg(target_os = "macos")]
#[test]
fn a_release_whose_install_script_runs_at_install_says_macos_from_a_command_line()
-> Result<(), Failed> {
    let (bench, _) = local("agents-door-npm-macos-unlent", "true", &listing(&[]))?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unsupported(why)) if why.contains("platform")),
        "{}",
        shape(&outcome)
    );
    assert_eq!(bench.server.asked(), CATALOG);
    Ok(())
}

/// What the script below writes into `seen`: the names of its environment
/// but those a shell sets itself, sorted, then what it found of them.
const SEEN: &[u8] = b"HOME\nPATH\nTMPDIR\nnpm_lifecycle_event\nnpm_package_name\n\
npm_package_version\nnode first\npostinstall peer 1.0.0\nhome\ntmp\n";

#[test]
fn an_install_script_sees_only_the_environment_it_is_given() -> Result<(), Failed> {
    let runtime = super::runtime()?;
    if !sandboxed(
        &runtime,
        "an_install_script_sees_only_the_environment_it_is_given",
    )? {
        return Ok(());
    }
    let command = "env | sed 's/=.*//' | grep -v -x -e PWD -e SHLVL -e _ -e OLDPWD \
                   | LC_ALL=C sort > seen; \
                   case \"${PATH%%:*}\" in */node/bin) echo 'node first' >> seen;; esac; \
                   echo \"$npm_lifecycle_event $npm_package_name $npm_package_version\" >> seen; \
                   [ -d \"$HOME\" ] && echo home >> seen; \
                   : > \"$TMPDIR/written\" && echo tmp >> seen";
    let (bench, _) = local(
        "agents-door-npm-environment",
        command,
        &listing(&[("seen", SEEN, false)]),
    )?;

    let outcome = installed_in_sandbox(&bench, &runtime)?;

    assert!(
        matches!(
            &outcome,
            Ok(Outcome::Moved {
                downloaded: true,
                ..
            })
        ),
        "{}",
        shape(&outcome)
    );
    let root = version(&bench);
    assert_eq!(fs::read(root.join("node_modules/peer/seen"))?, SEEN);
    let receipt: Value = serde_json::from_slice(&fs::read(root.join(".receipt"))?)?;
    assert!(
        receipt
            .pointer("/files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.iter().any(|file| {
                file.pointer("/path").and_then(Value::as_str) == Some("node_modules/peer/seen")
            })),
        "the receipt lists what the script made"
    );
    let staging = bench.programs().join(".staging");
    assert!(
        !staging.exists() || fs::read_dir(staging)?.next().is_none(),
        "nothing of the staging is left"
    );
    for private in [".script-home", ".script-tmp"] {
        assert!(!root.join(private).exists(), "{private} is removed");
        assert!(
            receipt
                .pointer("/files")
                .and_then(Value::as_array)
                .is_some_and(|files| files.iter().all(|file| {
                    file.pointer("/path")
                        .and_then(Value::as_str)
                        .is_none_or(|path| !path.starts_with(private))
                })),
            "the receipt does not list {private}"
        );
    }
    Ok(())
}

#[test]
fn an_install_script_writing_outside_its_package_is_held_back_by_the_sandbox() -> Result<(), Failed>
{
    let runtime = super::runtime()?;
    if !sandboxed(
        &runtime,
        "an_install_script_writing_outside_its_package_is_held_back_by_the_sandbox",
    )? {
        return Ok(());
    }
    // Inside the bench made below, which is there when the script runs.
    let outside = fs::canonicalize(std::env::temp_dir())?
        .join(format!(
            "crucible-app-agents-door-npm-outside-{}",
            std::process::id()
        ))
        .join("outside");
    // Linux gives it a `/tmp` of its own, where that first write lands and is
    // gone with the sandbox, and macOS refuses it: only what reaches the real
    // place, tested below, tells the two from one that got out. The second
    // write has nowhere of its own to land.
    let command = format!(
        "(printf x > '{}') 2>/dev/null; \
         (printf x > ../../escaped) 2>/dev/null || printf b >> blocked",
        outside.display()
    );
    let (bench, _) = local(
        "agents-door-npm-outside",
        &command,
        &listing(&[("blocked", b"b", false)]),
    )?;

    let outcome = installed_in_sandbox(&bench, &runtime)?;

    assert!(
        matches!(&outcome, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&outcome)
    );
    assert!(!outside.exists(), "nothing is written outside staging");
    assert!(!version(&bench).join("escaped").exists());
    assert_eq!(
        fs::read(version(&bench).join("node_modules/peer/blocked"))?,
        b"b"
    );
    Ok(())
}

#[test]
fn an_install_script_leaving_a_mode_review_did_not_find_refuses_and_leaves_no_staging()
-> Result<(), Failed> {
    let runtime = super::runtime()?;
    if !sandboxed(
        &runtime,
        "an_install_script_leaving_a_mode_review_did_not_find_refuses_and_leaves_no_staging",
    )? {
        return Ok(());
    }
    let (bench, _) = local("agents-door-npm-mode", "chmod +x index.js", &listing(&[]))?;
    let before = fs::read(bench.user())?;

    let outcome = installed_in_sandbox(&bench, &runtime)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Script { .. })),
        "{}",
        shape(&outcome)
    );
    unchanged(&bench, &before, "a mode review did not find")
}

/// The child is stopped with its leader and is not left to finish, so the
/// file it would have written later is not there to be listed, and never
/// appears in what was checked.
#[test]
fn the_check_of_an_install_script_waits_for_a_child_it_left_running_to_be_stopped()
-> Result<(), Failed> {
    let runtime = super::runtime()?;
    if !sandboxed(
        &runtime,
        "the_check_of_an_install_script_waits_for_a_child_it_left_running_to_be_stopped",
    )? {
        return Ok(());
    }
    let (bench, _) = local(
        "agents-door-npm-lingering",
        "printf early > early; (sleep 1; printf late > late) &",
        &listing(&[("early", b"early", false)]),
    )?;

    let outcome = installed_in_sandbox(&bench, &runtime)?;

    assert!(
        matches!(&outcome, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&outcome)
    );
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(
        !version(&bench).join("node_modules/peer/late").exists(),
        "a child wrote into what had been checked"
    );
    Ok(())
}
