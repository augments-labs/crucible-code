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
//! The registry here is the fake host's `registry/`, which a test build alone
//! fetches a registry address from, and only for a catalog on this machine.

use std::fs;

use crucible_app::agent_download::DownloadError;
use crucible_app::client::agents::{AgentsError, How, Outcome};
use crucible_client_api::{Pending, Text};
use crucible_external_harnesses::fake_peer;
use crucible_external_harnesses::release::{BIN, PRELOAD, Preload, Stub, proof};
use crucible_external_harnesses::store::StoreError;
use serde_json::{Value, json};

use super::uvx::{unchanged, version, versioned};
use super::{
    Bench, CATALOG, Failed, Reply, Server, answering, answers, artifact, asset_path, digest, facts,
    install, peer, platform, row_of, set, shape, stating,
};

/// Node's path in the version, as the catalog lists it.
fn node() -> String {
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
    Ok(())
}
