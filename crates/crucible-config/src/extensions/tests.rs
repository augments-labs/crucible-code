//! What a sweep of the extensions directory answers, and what it refuses.

use std::ffi::OsString;

use super::{Extensions, MAX_EXTENSIONS, Refusal};
use crate::home::{HOME, Home};
use crate::sample::Scratch;

/// A manifest naming itself, with everything else the same.
fn manifest(id: &str) -> String {
    format!(
        r#"{{
  "id": "{id}",
  "version": "1.4.0",
  "protocol": "1.3",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "0.35.0",
  "capabilities": ["registerTools"],
  "contributions": ["tools"]
}}"#
    )
}

/// The home directory this scratch tree holds.
fn home(scratch: &Scratch) -> Home {
    let named = scratch.text("home");
    Home::find(&move |wanted| (wanted == HOME).then(|| OsString::from(&named)))
        .expect("an absolute path was given")
}

/// Every refusal, as the user would read them.
fn said(found: &Extensions) -> String {
    found
        .refused()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_machine_with_nothing_installed_answers_empty_rather_than_refusing() {
    // The ordinary case, and the one that must not cost a sentence at startup:
    // almost every machine has no extensions directory at all.
    let scratch = Scratch::new("extensions-none");
    scratch.make("home");

    let found = Extensions::discover(&home(&scratch));

    assert!(found.found().is_empty());
    assert!(found.refused().is_empty(), "{}", said(&found));
    assert!(!found.stopped());
    assert_eq!(found.at(), scratch.at("home/extensions"));
}

#[test]
fn an_installed_manifest_is_read_and_answered_with_what_it_said() {
    let scratch = Scratch::new("extensions-one");
    scratch.write(
        "home/extensions/reviewer/manifest.json",
        &manifest("acme.reviewer"),
    );

    let found = Extensions::discover(&home(&scratch));

    assert!(found.refused().is_empty(), "{}", said(&found));
    let [one] = found.found() else {
        panic!(
            "one extension was installed, {} were read",
            found.found().len()
        );
    };
    assert_eq!(one.manifest().id(), "acme.reviewer");
    assert_eq!(one.manifest().identity().version.as_ref(), "1.4.0");
    // Where it came from, not only what it calls itself: two extensions can
    // claim one name and only the path says which is which.
    assert!(one.file().ends_with("manifest.json"), "{}", one.file());
}

#[test]
fn one_manifest_that_will_not_parse_does_not_hide_the_ones_that_will() {
    // The whole reason refusals are collected rather than returned. An author
    // who ships a trailing comma must not be able to stop somebody else's
    // machine from starting.
    let scratch = Scratch::new("extensions-broken");
    scratch.write("home/extensions/broken/manifest.json", "{ \"id\": ");
    scratch.write(
        "home/extensions/working/manifest.json",
        &manifest("acme.working"),
    );

    let found = Extensions::discover(&home(&scratch));

    let [one] = found.found() else {
        panic!("the working extension was not read");
    };
    assert_eq!(one.manifest().id(), "acme.working");

    let [refused] = found.refused() else {
        panic!("the broken extension was not refused");
    };
    assert!(
        matches!(refused, Refusal::Rejected { .. }),
        "{refused} is not a manifest being refused"
    );
    // Named by the file the author has to open, not by "an extension".
    assert!(refused.to_string().contains("broken"), "{refused}");
}

#[test]
fn a_directory_with_no_manifest_at_all_is_refused_rather_than_passed_over() {
    // A directory an installer half-wrote looks exactly like an extension that
    // is not there. Saying so is the only way the difference reaches anybody.
    let scratch = Scratch::new("extensions-empty");
    scratch.make("home/extensions/half-written");

    let found = Extensions::discover(&home(&scratch));

    assert!(found.found().is_empty());
    let [refused] = found.refused() else {
        panic!("a directory with no manifest was passed over");
    };
    assert!(
        matches!(refused, Refusal::Unreadable { .. }),
        "{refused} is not a missing manifest"
    );
}

#[test]
fn a_file_sitting_beside_the_directories_is_not_a_half_installed_extension() {
    // A README, an archive an installer left behind, a file the desktop wrote.
    // Refusing one would put a sentence in front of the user about a file that
    // is doing no harm.
    let scratch = Scratch::new("extensions-stray");
    scratch.write("home/extensions/README.md", "extensions go in here\n");
    scratch.write(
        "home/extensions/reviewer/manifest.json",
        &manifest("acme.reviewer"),
    );

    let found = Extensions::discover(&home(&scratch));

    assert_eq!(found.found().len(), 1);
    assert!(found.refused().is_empty(), "{}", said(&found));
}

#[test]
fn a_manifest_over_the_boundary_is_refused_by_its_length_rather_than_its_syntax() {
    // Read one byte past the boundary so the refusal is about the size, which
    // is what the author can act on. Cut exactly at it, the same file would be
    // reported as broken JSON and send them looking for a missing brace.
    let scratch = Scratch::new("extensions-large");
    let padding = " ".repeat(crucible_core::EXTENSION_MANIFEST_BYTES);
    scratch.write(
        "home/extensions/enormous/manifest.json",
        &format!("{padding}{}", manifest("acme.enormous")),
    );

    let found = Extensions::discover(&home(&scratch));

    assert!(found.found().is_empty());
    let [refused] = found.refused() else {
        panic!("an oversized manifest was read");
    };
    let said = refused.to_string();
    assert!(said.contains("the manifest is"), "{said}");
    assert!(
        said.contains(&crucible_core::EXTENSION_MANIFEST_BYTES.to_string()),
        "{said}"
    );
}

#[test]
fn two_directories_claiming_one_identifier_keep_the_first_and_refuse_the_second() {
    // An identifier is what configuration and every registration will key on,
    // so two extensions answering to one name is a question with no answer.
    // The one that keeps it is decided by the sorted order rather than by
    // whichever the filesystem happened to hand back first.
    let scratch = Scratch::new("extensions-twice");
    scratch.write(
        "home/extensions/alpha/manifest.json",
        &manifest("acme.reviewer"),
    );
    scratch.write(
        "home/extensions/omega/manifest.json",
        &manifest("acme.reviewer"),
    );

    let found = Extensions::discover(&home(&scratch));

    let [one] = found.found() else {
        panic!("neither claim was kept");
    };
    assert!(one.file().contains("alpha"), "{}", one.file());

    let [refused] = found.refused() else {
        panic!("the second claim was not refused");
    };
    let said = refused.to_string();
    // Both directories, because the person fixing it has to know which two.
    assert!(said.contains("omega") && said.contains("alpha"), "{said}");
    assert!(said.contains("acme.reviewer"), "{said}");
}

#[test]
fn more_directories_than_the_sweep_looks_at_stops_and_says_the_answer_is_short() {
    // A directory somebody filled must cost a bounded amount of startup. What
    // it must not do is report a short list as a complete one: an extension
    // that is installed, absent from the list and impossible to explain is
    // worse than being told the sweep stopped.
    let scratch = Scratch::new("extensions-many");
    for number in 0..=MAX_EXTENSIONS {
        scratch.write(
            &format!("home/extensions/plugin-{number:04}/manifest.json"),
            &manifest(&format!("acme.plugin{number}")),
        );
    }

    let found = Extensions::discover(&home(&scratch));

    assert_eq!(found.found().len(), MAX_EXTENSIONS);
    assert!(
        found.stopped(),
        "a short answer was reported as a whole one"
    );
}

#[test]
fn what_is_installed_is_read_in_a_settled_order_rather_than_the_filesystems() {
    // Two runs on one disk have to answer the same way: the order decides which
    // of two identical claims is kept, and a listing that reshuffles itself
    // between runs is one nobody can compare against yesterday's.
    let scratch = Scratch::new("extensions-order");
    for name in ["charlie", "alpha", "bravo"] {
        scratch.write(
            &format!("home/extensions/{name}/manifest.json"),
            &manifest(&format!("acme.{name}")),
        );
    }

    let found = Extensions::discover(&home(&scratch));
    let read: Vec<&str> = found
        .found()
        .iter()
        .map(|one| one.manifest().id())
        .collect();

    assert_eq!(read, ["acme.alpha", "acme.bravo", "acme.charlie"]);
}
