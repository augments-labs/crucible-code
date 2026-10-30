//! The agents door installs a release whose file list the catalog keeps as an
//! asset of its own: the list is fetched from the catalog's host before the
//! question and read as an inline one, and the release's own host is reached
//! only after the yes. A list that does not arrive as the catalog lists it, or
//! does not read, refuses the release before the question and changes nothing.

use std::fs;

use crucible_app::client::agents::{How, Outcome};
use crucible_client_api::Pending;
use serde_json::{Value, json};

use super::{
    Bench, CATALOG, Failed, ONE, Reply, answering, asset_path, digest, goose, install, set, shape,
};

/// The one file of a release whose program is `program`, as a file list
/// names it.
fn member(program: &[u8], exec: bool) -> Value {
    json!({
        "exec": exec,
        "path": "goose",
        "sha256": digest(program),
        "size": program.len(),
    })
}

/// `members` as the catalog keeps a file list as an asset: one JSON list,
/// with no space between its values.
fn list_of(members: &[Value]) -> Result<Vec<u8>, Failed> {
    Ok(serde_json::to_vec(members)?)
}

/// `goose` at `version`, built at review as `program`, whose file list the
/// catalog keeps as the asset `list` instead of listing it inline.
fn goose_listed_apart(version: &str, program: &[u8], list: &[u8]) -> Value {
    let mut release = goose(version, program);
    set(
        &mut release,
        "/targets/0/artifacts/0/files",
        json!({"sha256": digest(list), "size": list.len()}),
    );
    release
}

/// What the catalog's host is asked for before a question about a release
/// whose file list is the asset `list`: the catalog, then the list.
fn catalog_and(list: &[u8]) -> Vec<String> {
    CATALOG
        .iter()
        .map(|path| (*path).to_owned())
        .chain([asset_path(list)])
        .collect()
}

#[test]
fn a_file_list_the_catalog_keeps_as_an_asset_is_fetched_before_the_question_and_installed()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-list-asset")?;
    let list = list_of(&[member(ONE, true)])?;
    bench
        .server
        .publish(1, &[], &[goose_listed_apart("1.0.0", ONE, &list)]);
    bench
        .server
        .serve(&asset_path(&list), Reply::Body(list.clone()));
    bench
        .server
        .serve(&asset_path(ONE), Reply::Body(ONE.to_vec()));
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
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
    // The list came from the catalog's host before the question; the
    // release itself only after the yes.
    let [(Pending::Install { .. }, asked)] = front.put.as_slice() else {
        return Err("one install question was put".into());
    };
    assert_eq!(asked, &catalog_and(&list));
    let mut fetched = catalog_and(&list);
    fetched.push(asset_path(ONE));
    assert_eq!(bench.server.asked(), fetched);
    assert_eq!(fs::read(bench.program("1.0.0"))?, ONE);
    assert_eq!(
        bench.executable()?,
        bench.program("1.0.0").to_string_lossy()
    );

    // Committed with the list it was read to, so asked again it is the
    // version in place, and nothing of the release is fetched.
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
    assert_eq!(bench.server.asked(), catalog_and(&list));
    Ok(())
}

#[test]
fn a_file_list_asset_not_as_the_catalog_lists_it_is_refused_before_the_question()
-> Result<(), Failed> {
    let list = list_of(&[member(ONE, true)])?;
    let mut longer = list.clone();
    longer.push(b'\n');
    let twice = list_of(&[member(ONE, true), member(ONE, true)])?;
    let unrun = list_of(&[member(ONE, false)])?;
    let cases = [
        (
            "other bytes of the size it is listed at",
            list.clone(),
            vec![b' '; list.len()],
            "Err(FileListUnfetched)",
        ),
        (
            "the list and a byte more",
            list,
            longer,
            "Err(FileListUnfetched)",
        ),
        (
            "a list naming one path twice",
            twice.clone(),
            twice,
            "Err(FileListUnread)",
        ),
        (
            "a list whose program does not run",
            unrun.clone(),
            unrun,
            "Err(Program)",
        ),
    ];
    for (index, (what, listed, served, refused)) in cases.into_iter().enumerate() {
        let bench = Bench::new(&format!("agents-door-list-asset-refused-{index}"))?;
        bench
            .server
            .publish(1, &[], &[goose_listed_apart("1.0.0", ONE, &listed)]);
        bench
            .server
            .serve(&asset_path(&listed), Reply::Body(served));
        bench
            .server
            .serve(&asset_path(ONE), Reply::Body(ONE.to_vec()));
        bench.configure(&bench.program("1.0.0"), &json!({}))?;
        let before = fs::read(bench.user())?;
        let mut front = answering(&bench.server, Some("Install"));

        let (outcome, told) = bench.ask(install("1.0.0")?, &mut front, false)?;

        assert_eq!(shape(&outcome), refused, "{what}");
        assert!(front.put.is_empty(), "{what}");
        assert_eq!(bench.server.asked(), catalog_and(&listed), "{what}");
        assert!(told.is_empty(), "{what}");
        assert!(!bench.program("1.0.0").exists(), "{what}");
        assert!(bench.stored().is_empty(), "{what}");
        assert_eq!(fs::read(bench.user())?, before, "{what}");
    }
    Ok(())
}
