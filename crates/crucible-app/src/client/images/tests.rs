//! Every way a path can reach a file, held against the one route that shows
//! it.

use std::fs;
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crucible_client_api::bounds::IMAGE_BYTES;
use crucible_client_api::{ImagePath, ImageRef, ImageRefused, Outcome, Response, Shown};
use crucible_tools::{Disposition, Mode, Permission, Rules, Settled, Tool as _};
use crucible_types::{ToolArgs, ToolCall, ToolId};
use crucible_workspace::Workspace;

use super::{CHUNK, DEADLINE, Unasked, read, show};
use crate::sample::Sample;

/// A picture three pixels wide and two high, as a file a model could name.
fn picture() -> Vec<u8> {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="2"><rect width="3" height="2" fill="#c33"/></svg>"##;
    let pixels = crucible_raster::rasterise(svg, 3, &[]).expect("a three pixel picture");
    crucible_raster::png(&pixels).expect("its PNG")
}

fn path(text: &str) -> ImagePath {
    ImagePath::new(text).expect("a path a test wrote")
}

fn answer(text: &str) -> ImageRef {
    ImageRef::Answer { path: path(text) }
}

fn engine(written: &[(Disposition, &str)]) -> Permission {
    let mut rules = Rules::new();
    for (kind, text) in written {
        rules.add(*kind, text).expect("a rule the test wrote");
    }
    Permission::with(Mode::Ask, rules)
}

/// The decode slot is the process's, and one test holds it on purpose: the
/// others share it among themselves and wait while that one has it.
static SLOT: RwLock<()> = RwLock::new(());

/// What `image` comes to with `rules`, settled the moment it is asked: no
/// question is put to anybody, so nothing is left to wait on.
fn shown(
    workspace: &Workspace,
    rules: &[(Disposition, &str)],
    image: &ImageRef,
) -> Result<Shown, ImageRefused> {
    let _shared = SLOT.read().unwrap_or_else(PoisonError::into_inner);
    unlocked(workspace, rules, image)
}

/// [`shown`], for a thread whose test already shares the slot.
fn unlocked(
    workspace: &Workspace,
    rules: &[(Disposition, &str)],
    image: &ImageRef,
) -> Result<Shown, ImageRefused> {
    crucible_runtime::answered!(show(image, workspace, &mut engine(rules)))
}

/// A workspace holding the picture as `a.png`, a secret one under `secret/`,
/// and a picture beside the workspace, outside it.
fn tree(name: &str) -> (Sample, Workspace) {
    let sample = Sample::new(name);
    let root = sample.root();
    fs::write(root.join("a.png"), picture()).expect("the picture");
    fs::create_dir_all(root.join("secret")).expect("a directory");
    fs::write(root.join("secret/b.png"), picture()).expect("the secret picture");
    let outside = root
        .parent()
        .expect("the sample's base")
        .join("outside.png");
    fs::write(&outside, picture()).expect("a picture outside");
    let workspace = sample.workspace();
    (sample, workspace)
}

fn outside(root: &Path) -> std::path::PathBuf {
    root.parent()
        .expect("the sample's base")
        .join("outside.png")
}

fn encoded(refused: ImageRefused) -> Vec<u8> {
    Response {
        correlation: None,
        outcome: Outcome::Image(Err(refused)),
    }
    .encode()
    .expect("a refusal fits a frame")
}

#[test]
fn the_largest_png_the_raster_makes_is_one_the_contract_carries() {
    const { assert!(IMAGE_BYTES >= crucible_raster::MOST_PNG) };
}

#[test]
fn a_workspace_picture_is_shown_by_every_spelling_that_reaches_it() {
    let (sample, workspace) = tree("images-spellings");
    let root = sample.root();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("a.png"), root.join("linked.png")).expect("a link");
    let absolute = root.join("a.png").to_string_lossy().into_owned();
    let mut spellings = vec!["a.png", "./a.png", absolute.as_str()];
    if cfg!(unix) {
        spellings.push("linked.png");
    }
    for spelled in spellings {
        let shown = shown(&workspace, &[], &answer(spelled));
        assert!(
            matches!(&shown, Ok(shown) if shown.pixels == (3, 2)),
            "{spelled}: {shown:?}"
        );
    }
}

#[test]
fn outside_the_workspace_under_a_read_rule_and_missing_are_one_refusal_byte_for_byte() {
    let (sample, workspace) = tree("images-hidden");
    let root = sample.root();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside(&root), root.join("out.png")).expect("a link out");
    let absolute = outside(&root).to_string_lossy().into_owned();
    let rules = [(Disposition::Deny, "read(secret/**)")];
    let mut hidden = vec![
        "../outside.png",
        absolute.as_str(),
        "secret/b.png",
        "missing.png",
    ];
    if cfg!(unix) {
        hidden.push("out.png");
    }
    let refusals: Vec<_> = hidden
        .iter()
        .map(|spelled| shown(&workspace, &rules, &answer(spelled)).err())
        .collect();
    for (spelled, refused) in hidden.iter().zip(&refusals) {
        assert_eq!(refused, &Some(ImageRefused::NotShown), "{spelled}");
    }
    let first = encoded(ImageRefused::NotShown);
    for refused in refusals.into_iter().flatten() {
        assert_eq!(encoded(refused), first);
    }
}

#[test]
fn an_ask_rule_is_answered_no_and_puts_no_question() {
    let (_sample, workspace) = tree("images-ask");
    let rules = [(Disposition::Ask, "read(secret/**)")];
    assert_eq!(
        shown(&workspace, &rules, &answer("secret/b.png")),
        Err(ImageRefused::NotShown)
    );
    // The same rule leaves every other file as it was.
    assert!(shown(&workspace, &rules, &answer("a.png")).is_ok());
}

#[test]
fn a_file_a_rule_hides_is_not_shown_whatever_else_is_wrong_with_it() {
    let (sample, workspace) = tree("images-hidden-large");
    let root = sample.root();
    fs::write(
        root.join("secret/large.png"),
        vec![0_u8; crucible_raster::MOST_INPUT + 1],
    )
    .expect("a large file");
    fs::write(root.join("secret/text.png"), b"not a picture").expect("a text file");
    let rules = [(Disposition::Deny, "read(secret/**)")];
    for spelled in ["secret/large.png", "secret/text.png"] {
        assert_eq!(
            shown(&workspace, &rules, &answer(spelled)),
            Err(ImageRefused::NotShown),
            "{spelled}"
        );
    }
}

#[test]
fn a_file_of_the_ceiling_is_read_whole_and_a_byte_more_is_too_large() {
    let (sample, workspace) = tree("images-ceiling");
    let root = sample.root();
    fs::write(
        root.join("whole.png"),
        vec![0_u8; crucible_raster::MOST_INPUT],
    )
    .expect("a file");
    fs::write(
        root.join("over.png"),
        vec![0_u8; crucible_raster::MOST_INPUT + 1],
    )
    .expect("a file");
    fs::write(
        root.join("far.png"),
        vec![0_u8; 3 * crucible_raster::MOST_INPUT],
    )
    .expect("a file");
    // Read whole, and only then found to be no picture: the step a read stops
    // at is the attachments' own, or a file of the ceiling would be cut short.
    assert_eq!(
        shown(&workspace, &[], &answer("whole.png")),
        Err(ImageRefused::NotAnImage)
    );
    assert_eq!(
        shown(&workspace, &[], &answer("over.png")),
        Err(ImageRefused::TooLarge)
    );
    assert_eq!(
        shown(&workspace, &[], &answer("far.png")),
        Err(ImageRefused::TooLarge)
    );
    assert_eq!(crucible_raster::MOST_INPUT % CHUNK, 0);
}

#[test]
fn bytes_no_decoder_reads_are_not_an_image() {
    let (sample, workspace) = tree("images-text");
    fs::write(
        sample.root().join("text.png"),
        b"# a heading, not a picture\n",
    )
    .expect("a file");
    assert_eq!(
        shown(&workspace, &[], &answer("text.png")),
        Err(ImageRefused::NotAnImage)
    );
}

#[test]
fn a_tool_image_is_shown_only_while_its_bytes_are_the_ones_the_tool_read() {
    let (sample, workspace) = tree("images-changed");
    let kept = <[u8; 32]>::from(<sha2::Sha256 as sha2::Digest>::digest(picture()));
    let tool = ImageRef::Tool {
        path: path("a.png"),
        hash: kept,
    };
    assert!(shown(&workspace, &[], &tool).is_ok());

    let mut changed = picture();
    changed.extend_from_slice(b"one more byte");
    fs::write(sample.root().join("a.png"), changed).expect("the picture, changed");
    assert_eq!(shown(&workspace, &[], &tool), Err(ImageRefused::Changed));
}

#[test]
fn an_image_is_shown_exactly_where_the_read_tool_would_be_allowed_to_read_it() {
    let (sample, workspace) = tree("images-as-read");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside(&sample.root()), sample.root().join("out.png"))
        .expect("a link out");
    let reader = crucible_builtins::Read::new(workspace.clone(), crucible_builtins::Ledger::new());
    let ruled: [&[(Disposition, &str)]; 4] = [
        &[],
        &[(Disposition::Deny, "read(secret/**)")],
        &[(Disposition::Ask, "read(secret/**)")],
        &[(Disposition::Deny, "read")],
    ];
    for rules in ruled {
        for spelled in [
            "a.png",
            "secret/b.png",
            "../outside.png",
            "out.png",
            "missing.png",
        ] {
            let args = ToolArgs::new(serde_json::json!({ "path": spelled }).to_string());
            let call = ToolCall {
                id: ToolId::new("a read"),
                name: "read".into(),
                args: args.clone(),
            };
            let reads = matches!(
                crucible_runtime::answered!(engine(rules).decide(
                    &call,
                    &reader.sensitivity(&args),
                    &mut Unasked
                )),
                Settled::Approved(_)
            ) && workspace.existing(spelled).is_ok();
            let showing = shown(&workspace, rules, &answer(spelled));
            assert_eq!(
                showing.is_ok(),
                reads,
                "{spelled} under {rules:?}: {showing:?}"
            );
        }
    }
}

#[test]
fn a_proof_settled_about_another_file_reads_nothing() {
    let (_sample, workspace) = tree("images-other-proof");
    let other = workspace
        .existing("secret/b.png")
        .expect("the secret picture");
    let asked = workspace.existing("a.png").expect("the picture");
    let call = ToolCall {
        id: ToolId::new("a read"),
        name: "read".into(),
        args: ToolArgs::new(r#"{"path":"a.png"}"#),
    };
    let sensitivity = crucible_tools::Sensitivity::ReadOnly {
        target: crucible_tools::Target::resolved(&workspace, &asked),
    };
    let Settled::Approved(approved) =
        crucible_runtime::answered!(engine(&[]).decide(&call, &sensitivity, &mut Unasked))
    else {
        panic!("a read inside the workspace is allowed");
    };
    assert!(read(&approved, &workspace, &asked, Instant::now()).is_ok());
    assert_eq!(
        read(&approved, &workspace, &other, Instant::now()).err(),
        Some(ImageRefused::NotShown)
    );
}

#[test]
fn sixteen_asked_at_once_are_decoded_one_at_a_time_and_each_is_shown() {
    let (_sample, workspace) = tree("images-sixteen");
    let _shared = SLOT.read().unwrap_or_else(PoisonError::into_inner);
    let workspace = Arc::new(workspace);
    let asked: Vec<_> = (0..16)
        .map(|_| {
            let workspace = Arc::clone(&workspace);
            thread::spawn(move || unlocked(&workspace, &[], &answer("a.png")))
        })
        .collect();
    let first = unlocked(&workspace, &[], &answer("a.png")).expect("the picture");
    for one in asked {
        assert_eq!(
            one.join().expect("a thread that showed it"),
            Ok(first.clone())
        );
    }
}

#[test]
fn an_image_waits_for_the_decode_slot_no_longer_than_its_own_deadline() {
    let (_sample, workspace) = tree("images-slot");
    let _alone = SLOT.write().unwrap_or_else(PoisonError::into_inner);
    let (release, held) = std::sync::mpsc::channel();
    let (taken, waiting) = std::sync::mpsc::channel();
    let holder = thread::spawn(move || {
        let _slot = crucible_raster::Slot::take(Duration::from_secs(30)).expect("the slot");
        taken.send(()).expect("the test waits");
        let _ = held.recv_timeout(Duration::from_secs(30));
    });
    waiting.recv().expect("the slot is held");
    let started = Instant::now();
    let refused = unlocked(&workspace, &[], &answer("a.png"));
    let waited = started.elapsed();
    release.send(()).expect("the holder waits");
    holder.join().expect("the holder");
    assert_eq!(refused, Err(ImageRefused::NotInTime));
    assert!(
        waited >= DEADLINE.saturating_sub(Duration::from_millis(100)),
        "{waited:?}"
    );
    assert!(
        waited < DEADLINE.saturating_add(Duration::from_secs(1)),
        "{waited:?}"
    );
}
