//! `/release-notes` is answered here, from the changelog built in: no turn is
//! taken, nothing is written to the session's log, and a version that is no
//! release or a word that is no version is said back with what to type.

use crate::cli::sample::Sample;

use super::*;

/// The changelog this build was made from, read the way a person reads it:
/// the newest release is the first numbered heading.
fn newest() -> &'static str {
    include_str!("../../../../CHANGELOG.md")
        .lines()
        .filter_map(|line| line.strip_prefix("## ["))
        .filter_map(|rest| rest.split_once(']').map(|(version, _)| version))
        .find(|version| *version != "Unreleased")
        .expect("a release")
}

/// What typing `typed` printed, with the terminal's escape sequences taken
/// out, and how many requests it cost.
fn noted(typed: &str) -> (String, usize) {
    let (written, asked) = over(Script::new(vec![saying("answered")]), Tools::new(), typed);
    let mut said = String::with_capacity(written.len());
    let mut escaped = false;
    for character in written.chars() {
        if character == '\u{1b}' {
            escaped = true;
        } else if escaped {
            escaped = !character.is_ascii_alphabetic();
        } else {
            said.push(character);
        }
    }
    (said, asked)
}

#[test]
fn every_release_is_printed_here_and_no_turn_is_taken() {
    let (written, asked) = noted("/release-notes\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("ten newest in full"), "{written}");
    assert!(
        written.contains("/release-notes <version> prints one"),
        "{written}"
    );
}

#[test]
fn one_release_is_printed_in_full_by_its_number_with_or_without_a_v() {
    for typed in ["/release-notes 0.41.1\n", "/release-notes v0.41.1\n"] {
        let (written, asked) = noted(typed);

        assert_eq!(asked, 0, "{typed:?}");
        assert!(written.contains("0.41.1"), "{typed:?}: {written}");
        assert!(written.contains("2026-09-14"), "{typed:?}: {written}");
        assert!(written.contains("Security"), "{typed:?}: {written}");
        assert!(
            !written.contains("ten newest in full"),
            "{typed:?}: {written}"
        );
        assert!(!written.contains("0.41.0"), "{typed:?}: {written}");
    }
}

#[test]
fn a_version_that_is_no_release_is_refused_naming_the_newest() {
    let (written, asked) = noted("/release-notes 0.99.0\n");

    assert_eq!(asked, 0, "{written}");
    let refusal = format!("! no release 0.99.0 · newest is {}", newest());
    assert!(written.contains(&refusal), "{written}");
}

#[test]
fn a_word_that_is_no_version_is_refused_saying_how_to_write_one() {
    for (typed, word) in [
        ("/release-notes latest\n", "latest"),
        ("/release-notes 0.41.1 and more\n", "0.41.1 and more"),
    ] {
        let (written, asked) = noted(typed);

        assert_eq!(asked, 0, "{typed:?}");
        let refusal = format!("! not a version: {word} · write it as {}", newest());
        assert!(written.contains(&refusal), "{typed:?}: {written}");
    }
}

#[test]
fn the_notes_are_no_part_of_what_the_model_is_told_or_of_the_log() {
    // A command's answer costs the provider nothing and is not part of the
    // session: the log is the same after it, byte for byte, so a session
    // picked up again does not print them a second time.
    let sample = Sample::new("release-notes-unlogged");
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a session"));
    session.append(&crucible_types::Message::said("earlier words"));
    let path = session.path().to_path_buf();
    let script = Script::new(vec![saying("answered")]);
    let asked = script.asked();
    let conversation = paired(Arc::clone(&session), |session| {
        scripted(script, Tools::new(), session)
    });
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/release-notes\n/release-notes 0.41.1\n".to_vec());
    // Behind the writer, so the line appended above is in the file first.
    let _ = session.take_placed();
    let before = std::fs::read(&path).expect("the log");

    converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");
    drop(session);

    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(std::fs::read(&path).expect("the log"), before);
}
