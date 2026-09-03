//! What the listing says about what was found, and about what was not.

use super::listing;
use crate::cli::sample::Sample;

/// A manifest naming itself and asking for one capability.
fn manifest(id: &str) -> String {
    format!(
        r#"{{
  "id": "{id}",
  "version": "1.4.0",
  "protocol": "1.3",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "0.35.0",
  "capabilities": ["registerTools", "readRunContext"],
  "contributions": ["tools"]
}}"#
    )
}

#[test]
fn nothing_installed_names_the_directory_that_was_looked_in() {
    // The usual mistake is having put the extension somewhere else, so an
    // empty answer that does not say where crucible looked is one nobody can
    // act on.
    let sample = Sample::new("extensions-listing-none");

    let said = listing(&sample.discovered(), &sample.decided());

    assert!(said.starts_with("no extensions in "), "{said}");
    assert!(said.contains("extensions"), "{said}");
}

#[test]
fn one_extension_is_listed_with_what_it_asked_for_and_where_it_came_from() {
    let sample = Sample::new("extensions-listing-one");
    sample.installed("reviewer", &manifest("acme.reviewer"));

    let said = listing(&sample.discovered(), &sample.decided());

    assert!(said.starts_with("1 extension in "), "{said}");
    assert!(said.contains("acme.reviewer 1.4.0"), "{said}");
    assert!(
        said.contains("protocol  1.3, needs crucible 0.35.0"),
        "{said}"
    );
    // What it wants is the whole reason to read a listing before trusting one.
    assert!(
        said.contains("asks for  registerTools, readRunContext"),
        "{said}"
    );
    assert!(said.contains("gives     tools"), "{said}");
    // Installed is not permitted. Somebody reading this list is deciding
    // whether to trust the thing, so the answer to whether it already runs
    // belongs beside what it asked for.
    assert!(said.contains("enabled   no"), "{said}");
    // The digest the parser took over the file's own bytes, so that two
    // listings a week apart say whether the file changed.
    assert!(said.contains("digest    sha256:"), "{said}");
    assert!(said.contains("manifest.json"), "{said}");
}

#[test]
fn an_extension_that_asks_for_nothing_says_so_rather_than_leaving_a_blank() {
    // Asking for nothing is a real and unremarkable manifest. A blank beside
    // the label reads like the listing failed to work the answer out.
    let sample = Sample::new("extensions-listing-quiet");
    sample.installed(
        "quiet",
        r#"{
  "id": "acme.quiet",
  "version": "0.1.0",
  "protocol": "1.0",
  "entrypoint": "bin/quiet",
  "minimumCrucible": "0.35.0",
  "capabilities": [],
  "contributions": []
}"#,
    );

    let said = listing(&sample.discovered(), &sample.decided());

    assert!(said.contains("asks for  nothing"), "{said}");
    assert!(said.contains("gives     nothing"), "{said}");
}

#[test]
fn a_directory_that_could_not_be_read_is_listed_rather_than_left_out() {
    // An extension the user installed and cannot see in the listing is one
    // they will re-install rather than repair.
    let sample = Sample::new("extensions-listing-broken");
    sample.installed("broken", "{ \"id\": ");
    sample.installed("working", &manifest("acme.working"));

    let said = listing(&sample.discovered(), &sample.decided());

    assert!(said.starts_with("1 extension in "), "{said}");
    assert!(said.contains("acme.working"), "{said}");
    assert!(said.contains("1 directory could not be read:"), "{said}");
    assert!(said.contains("broken"), "{said}");
}

#[test]
fn a_short_answer_says_so_before_the_list_rather_than_after_it() {
    // At the end of sixty-four entries the sentence has scrolled away, and an
    // incomplete listing that reads like a complete one is how an extension
    // ends up installed, absent, and impossible to explain.
    let sample = Sample::new("extensions-listing-many");
    for number in 0..=crucible_config::MAX_EXTENSIONS {
        sample.installed(
            &format!("plugin-{number:04}"),
            &manifest(&format!("acme.plugin{number}")),
        );
    }

    let said = listing(&sample.discovered(), &sample.decided());
    let short = said
        .find("this list is short")
        .expect("a truncated sweep says so");

    assert!(short < said.find("acme.plugin0").expect("the first entry"));
}

#[test]
fn an_extension_the_home_file_turned_on_is_listed_as_enabled() {
    let sample = Sample::new("extensions-listing-enabled");
    sample.installed("reviewer", &manifest("acme.reviewer"));
    sample.configured(r#"{"extensions": {"acme.reviewer": {"enabled": true}}}"#);

    let said = listing(&sample.discovered(), &sample.decided());

    assert!(said.contains("enabled   yes"), "{said}");
    // The remedy is for the ones that are off. Said over a list where nothing
    // is, it sends a reader to change a file that is already right.
    assert!(!said.contains("nothing runs until"), "{said}");
}

#[test]
fn a_list_holding_one_that_is_off_says_once_what_would_turn_it_on() {
    // Every entry carrying the same sentence is noise on the usual machine,
    // where nothing has been turned on yet and every entry says no.
    let sample = Sample::new("extensions-listing-remedy");
    sample.installed("one", &manifest("acme.one"));
    sample.installed("two", &manifest("acme.two"));

    let said = listing(&sample.discovered(), &sample.decided());

    assert_eq!(said.matches("nothing runs until").count(), 1, "{said}");
}
