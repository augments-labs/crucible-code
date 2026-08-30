//! What a reader is told when a file does not reach the model.
//!
//! An attachment can be left out for two different reasons — it did not fit
//! the request, or the model being asked does not read that modality — and
//! each is said at the moment it becomes true: the first as the request goes,
//! the second where the answer arrives. Silence is the third answer, and it
//! belongs to the request that carried everything it was given.

use super::*;

/// Writes a file and returns the attachment naming it.
fn file(under: &Path, name: &str, bytes: &[u8]) -> Attachment {
    let path = under.join(name);
    std::fs::write(&path, bytes).expect("a temporary file");
    Attachment {
        path: path.to_string_lossy().into_owned().into(),
        modality: Modality::Image,
        media_type: "image/png".into(),
        hash: Sha256::digest(bytes).into(),
    }
}

#[test]
fn attachments_a_request_went_out_without_are_named_as_it_goes() {
    // The design working is invisible from the answer: the model is handed a
    // sentence where a picture was and says something a little vaguer. Without
    // this the reader has no way to know which file it did not get to look at.
    let sample = Sample::new("aged-over");
    let under = sample.workspace().root().to_path_buf();
    let third = crucible_core::CEILING / 3 + 1;

    let mut scripted = Scripted::new(
        Script::new(vec![saying("looking")]),
        Tools::new(),
        Verdict::Allow,
    );
    let files: Vec<Attachment> = ["first.png", "second.png", "third.png", "fourth.png"]
        .iter()
        .map(|name| file(&under, name, &vec![7; third]))
        .collect();
    let named: Vec<Box<str>> = files.iter().map(|one| one.path.clone()).collect();

    scripted
        .turning("what is in these", files.into())
        .expect("the turn to have run");

    let posted = scripted.aged();
    let [aged] = posted.as_slice() else {
        panic!("one request went out, and it went out short");
    };
    let [first, second, ..] = named.as_slice() else {
        panic!("four files were attached");
    };
    assert_eq!(aged, &[first.clone(), second.clone()]);
}

#[test]
fn attachments_that_all_fit_leave_nothing_to_say() {
    // Nothing happened to them, so nothing is said. A row per turn reporting
    // that everything was carried is a row that is always there and never read.
    let sample = Sample::new("aged-under");
    let under = sample.workspace().root().to_path_buf();

    let mut scripted = Scripted::new(
        Script::new(vec![saying("looking")]),
        Tools::new(),
        Verdict::Allow,
    );
    scripted
        .turning(
            "what is in this",
            vec![file(&under, "one.png", &[1; 16])].into(),
        )
        .expect("the turn to have run");

    assert!(scripted.aged().is_empty());
}

#[test]
fn a_retry_says_again_which_attachments_it_went_out_without() {
    // Once per request rather than once per turn: a retry is a second answer
    // built from a second short request, and a reader watching it arrive is
    // owed the same sentence about it.
    let sample = Sample::new("aged-retry");
    let under = sample.workspace().root().to_path_buf();
    let third = crucible_core::CEILING / 3 + 1;

    let mut scripted = Scripted::new(
        Script::dropping(1, vec![saying("done")]),
        Tools::new(),
        Verdict::Allow,
    );
    let files: Vec<Attachment> = ["first.png", "second.png", "third.png", "fourth.png"]
        .iter()
        .map(|name| file(&under, name, &vec![7; third]))
        .collect();

    scripted
        .turning("what is in these", files.into())
        .expect("the turn to have run");

    // One reading of the channel, because each of these drains it.
    let posted = scripted.aged();
    let [first, second] = posted.as_slice() else {
        panic!("the request that was dropped, and the one that replaced it");
    };
    assert_eq!(first, second);
}

#[test]
fn a_picture_a_model_does_not_read_is_named_where_its_answer_arrives() {
    // Not the ceiling: this file would not have gone out at any size. The
    // reader is owed a different sentence for it, because asking again is the
    // one move that cannot help.
    let sample = Sample::new("unread-kind");
    let under = sample.workspace().root().to_path_buf();

    let mut scripted = Scripted::new(
        Script::new(vec![saying("looking")]),
        Tools::new(),
        Verdict::Allow,
    );
    scripted.runner.spec.model.accepts = Some(Modalities::empty().insert(Modality::Text));

    let one = file(&under, "chart.png", &[3; 64]);
    let named = one.path.clone();

    scripted
        .turning("what is in this", vec![one].into())
        .expect("the turn to have run");

    // Only the one drain: reading the channel twice would leave the second
    // reader nothing, and which of the two rows this file belongs on is
    // settled a layer down, where the request is resolved.
    let posted = scripted.unread();
    let [unread] = posted.as_slice() else {
        panic!("one request went out, and it went out without the picture");
    };
    assert_eq!(unread, &[named]);
}
