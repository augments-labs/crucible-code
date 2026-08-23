use std::fs;
use std::path::Path;

use crucible_core::{Attachment, Content, Message, Modality, Transcript};
use sha2::{Digest as _, Sha256};

use super::{CEILING, resolve};
use crate::sample::Sample;

/// Writes a file and returns the attachment naming it.
fn file(under: &Path, name: &str, bytes: &[u8]) -> Attachment {
    let path = under.join(name);
    fs::write(&path, bytes).expect("a temporary file");
    Attachment {
        path: path.to_string_lossy().into_owned().into(),
        modality: Modality::Image,
        media_type: "image/png".into(),
        hash: Sha256::digest(bytes).into(),
    }
}

/// A prompt and the files named with it.
fn asked(text: &str, attachments: Vec<Attachment>) -> Message {
    Message::User {
        text: text.into(),
        attachments: attachments.into(),
    }
}

/// What the model said back, so two prompts are two turns.
fn answered(text: &str) -> Message {
    Message::Agent {
        text: text.into(),
        calls: Vec::new(),
        stop: None,
    }
}

#[test]
fn everything_under_the_ceiling_is_carried_whole() {
    let sample = Sample::new("attach-under");
    let under = sample.workspace().root().to_path_buf();

    let mut transcript = Transcript::new();
    transcript.push(asked(
        "what is in these",
        vec![
            file(&under, "one.png", &[1; 16]),
            file(&under, "two.png", &[2; 32]),
        ],
    ));

    let resolved = resolve(&transcript);
    let attached = resolved.attached();
    let [one, two] = attached.as_slice() else {
        panic!("both files, in transcript order");
    };

    assert!(matches!(one.content, Content::Bytes(bytes) if bytes == [1; 16]));
    assert!(matches!(two.content, Content::Bytes(bytes) if bytes == [2; 32]));
    assert_eq!((one.message, one.index), (0, 0));
    assert_eq!((two.message, two.index), (0, 1));
    assert_eq!(one.media_type, "image/png");
    assert_eq!(one.modality, Modality::Image);
}

#[test]
fn over_the_ceiling_the_newest_are_carried_and_the_rest_stand_in() {
    let sample = Sample::new("attach-over");
    let under = sample.workspace().root().to_path_buf();
    let third = CEILING / 3 + 1;

    let mut transcript = Transcript::new();
    for name in ["first.png", "second.png", "third.png", "fourth.png"] {
        transcript.push(asked(name, vec![file(&under, name, &vec![7; third])]));
        transcript.push(answered("looking"));
    }

    let resolved = resolve(&transcript);
    let attached = resolved.attached();

    assert_eq!(attached.len(), 4, "every attachment keeps its place");
    let carried: Vec<bool> = attached
        .iter()
        .map(|one| matches!(one.content, Content::Bytes(_)))
        .collect();
    assert_eq!(
        carried,
        [false, false, true, true],
        "the two newest fit, the two oldest stand in"
    );

    let oldest = attached.first().expect("the oldest attachment");
    let Content::Instead(line) = oldest.content else {
        panic!("the oldest stands in");
    };
    assert!(
        line.contains("first.png"),
        "the line names the file: {line}"
    );
    assert!(
        line.contains("read it again if you need it"),
        "the line offers the next move: {line}"
    );
    assert!(
        line.contains("size limit"),
        "the line says which half said no: {line}"
    );
}

#[test]
fn the_ceiling_falls_on_bytes_and_not_on_how_many_files() {
    let sample = Sample::new("attach-bytes");
    let under = sample.workspace().root().to_path_buf();

    let mut transcript = Transcript::new();
    transcript.push(asked(
        "twenty small ones",
        (0..20)
            .map(|nth| file(&under, &format!("icon{nth}.png"), &vec![3; 1024]))
            .collect(),
    ));
    transcript.push(answered("seen"));
    transcript.push(asked(
        "and one big one",
        vec![file(&under, "big.png", &vec![9; CEILING - 4096])],
    ));

    let resolved = resolve(&transcript);
    let attached = resolved.attached();

    assert_eq!(attached.len(), 21);
    let stood_in = attached
        .iter()
        .filter(|one| matches!(one.content, Content::Instead(_)))
        .count();
    assert_eq!(
        stood_in, 16,
        "the newest 4096 bytes of icons fit beside the big one; the other sixteen do not"
    );
    let big = attached.last().expect("the newest attachment");
    assert!(
        matches!(big.content, Content::Bytes(bytes) if bytes.len() == CEILING - 4096),
        "the big one is the newest and is carried whole"
    );
}

#[test]
fn a_file_that_changed_after_it_was_attached_stands_in() {
    let sample = Sample::new("attach-changed");
    let under = sample.workspace().root().to_path_buf();

    let attachment = file(&under, "shot.png", &[1; 64]);
    fs::write(under.join("shot.png"), [2; 64]).expect("the file changes underneath");

    let mut transcript = Transcript::new();
    transcript.push(asked("what is in this", vec![attachment]));

    let resolved = resolve(&transcript);
    let attached = resolved.attached();
    let one = attached.first().expect("the attachment");
    let Content::Instead(line) = one.content else {
        panic!("the changed file stands in rather than being sent");
    };

    assert!(line.contains("shot.png"), "the line names the file: {line}");
    assert!(
        line.contains("changed after it was attached"),
        "the line says what happened: {line}"
    );
}

#[test]
fn a_file_that_is_gone_stands_in() {
    let sample = Sample::new("attach-gone");
    let under = sample.workspace().root().to_path_buf();

    let attachment = file(&under, "shot.png", &[1; 64]);
    fs::remove_file(under.join("shot.png")).expect("the file goes");

    let mut transcript = Transcript::new();
    transcript.push(asked("what is in this", vec![attachment]));

    let resolved = resolve(&transcript);
    let attached = resolved.attached();
    let one = attached.first().expect("the attachment");
    let Content::Instead(line) = one.content else {
        panic!("the missing file stands in");
    };

    assert!(line.contains("shot.png"), "the line names the file: {line}");
    assert!(
        line.contains("could not be read"),
        "the line says what happened: {line}"
    );
    assert!(
        line.contains("read it again if you need it"),
        "the next move is offered even here, because this look is already stale: {line}"
    );
}

#[test]
fn all_three_lines_name_the_file_and_offer_the_read() {
    let sample = Sample::new("attach-three");
    let under = sample.workspace().root().to_path_buf();

    let aged = file(&under, "aged.png", &[1; 64]);
    let gone = file(&under, "gone.png", &[2; 64]);
    let changed = file(&under, "changed.png", &[3; 64]);
    fs::remove_file(under.join("gone.png")).expect("the file goes");
    fs::write(under.join("changed.png"), [4; 64]).expect("the file changes");

    let mut transcript = Transcript::new();
    for one in [aged, gone, changed] {
        transcript.push(asked("what is in this", vec![one]));
        transcript.push(answered("looking"));
    }
    transcript.push(asked(
        "and this",
        vec![file(&under, "big.png", &vec![9; CEILING])],
    ));

    let resolved = resolve(&transcript);
    let attached = resolved.attached();
    let [aged, gone, changed, _] = attached.as_slice() else {
        panic!("four attachments, each in its own place");
    };

    for (one, name) in [
        (aged, "aged.png"),
        (gone, "gone.png"),
        (changed, "changed.png"),
    ] {
        let Content::Instead(line) = one.content else {
            panic!("{name} carries a line rather than bytes");
        };
        assert!(line.contains(name), "the line names the file: {line}");
        assert!(
            line.contains("is not attached to this request"),
            "the line says the content is not here: {line}"
        );
        assert!(
            line.contains("read it again if you need it"),
            "the line says it may be read again: {line}"
        );
    }
}

#[test]
fn a_file_that_stood_in_once_is_carried_where_there_is_room() {
    let sample = Sample::new("attach-again");
    let under = sample.workspace().root().to_path_buf();

    let old = file(&under, "old.png", &[1; 64]);
    let mut full = Transcript::new();
    full.push(asked("first", vec![old.clone()]));
    full.push(answered("looking"));
    full.push(asked(
        "and this",
        vec![file(&under, "big.png", &vec![9; CEILING])],
    ));

    let crowded = resolve(&full);
    let crowded = crowded.attached();
    assert!(
        matches!(
            crowded.first().expect("the older attachment").content,
            Content::Instead(_)
        ),
        "crowded out of the full request"
    );

    let mut alone = Transcript::new();
    alone.push(asked("first", vec![old]));
    let roomy = resolve(&alone);
    let roomy = roomy.attached();
    assert!(
        matches!(
            roomy.first().expect("the same attachment").content,
            Content::Bytes(bytes) if bytes == [1; 64]
        ),
        "ageing belongs to the request, not to the attachment"
    );
}

#[test]
fn nothing_resolved_survives_the_request() {
    let sample = Sample::new("attach-survives");
    let under = sample.workspace().root().to_path_buf();

    let mut transcript = Transcript::new();
    transcript.push(asked(
        "what is in this",
        vec![file(&under, "shot.png", &[1; 64])],
    ));

    let first = resolve(&transcript);
    assert!(matches!(
        first.attached().first().expect("the attachment").content,
        Content::Bytes(_)
    ));
    drop(first);

    fs::remove_file(under.join("shot.png")).expect("the file goes");

    let second = resolve(&transcript);
    assert!(
        matches!(
            second.attached().first().expect("the attachment").content,
            Content::Instead(_)
        ),
        "the bytes were the last request's; the transcript kept only the reference"
    );
}
