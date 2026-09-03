//! What the listing says about what was found, and about what was not.

use crucible_config::Extensions;
use crucible_core::ExtensionProtocol;

use super::listing;
use crate::cli::sample::Sample;

/// The crucible these listings are drawn by.
///
/// Named here rather than taken from the crate, so that a release moving the
/// version does not move what these tests are about.
const RUNNING: &str = "0.34.0";

/// A manifest naming itself and asking for one capability.
fn manifest(id: &str) -> String {
    format!(
        r#"{{
  "id": "{id}",
  "version": "1.4.0",
  "protocol": "1.0",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "0.34.0",
  "capabilities": ["registerTools", "readRunContext"],
  "contributions": ["tools"]
}}"#
    )
}

/// The digest one installed manifest was read at.
///
/// Read back out of the sweep rather than written down here. It is the value a
/// person copies out of the listing into their own file, and a fixture holding
/// its own copy would be asserting the two agree by writing them both.
fn agreed(found: &Extensions, id: &str) -> String {
    found
        .found()
        .iter()
        .find(|one| &*one.manifest().identity().id == id)
        .expect("an installed extension")
        .manifest()
        .identity()
        .digest
        .to_string()
}

/// A manifest naming a protocol and the oldest crucible it says it works with.
fn speaking(protocol: &str, minimum: &str) -> String {
    format!(
        r#"{{
  "id": "acme.reviewer",
  "version": "1.4.0",
  "protocol": "{protocol}",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "{minimum}",
  "capabilities": [],
  "contributions": []
}}"#
    )
}

#[test]
fn nothing_installed_names_the_directory_that_was_looked_in() {
    // The usual mistake is having put the extension somewhere else, so an
    // empty answer that does not say where crucible looked is one nobody can
    // act on.
    let sample = Sample::new("extensions-listing-none");

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

    assert!(said.starts_with("no extensions in "), "{said}");
    assert!(said.contains("extensions"), "{said}");
}

#[test]
fn one_extension_is_listed_with_what_it_asked_for_and_where_it_came_from() {
    let sample = Sample::new("extensions-listing-one");
    sample.installed("reviewer", &manifest("acme.reviewer"));

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

    assert!(said.starts_with("1 extension in "), "{said}");
    assert!(said.contains("acme.reviewer 1.4.0"), "{said}");
    assert!(
        said.contains("protocol  1.0, needs crucible 0.34.0"),
        "{said}"
    );
    // What it wants is the whole reason to read a listing before trusting one.
    assert!(
        said.contains("asks for  registerTools, readRunContext"),
        "{said}"
    );
    assert!(said.contains("gives     tools"), "{said}");
    // Could this build run it at all, asked and answered before whether
    // anybody said it may: turning on an extension this crucible cannot speak
    // to would change nothing.
    assert!(said.contains("hosted    yes"), "{said}");
    // Installed is not permitted. Somebody reading this list is deciding
    // whether to trust the thing, so the answer to whether it already runs
    // belongs beside what it asked for — and it says which of the ways of not
    // being permitted this one is, because they need different things done.
    assert!(
        said.contains("may run   no; nobody has said this extension may run"),
        "{said}"
    );
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
  "minimumCrucible": "0.34.0",
  "capabilities": [],
  "contributions": []
}"#,
    );

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

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

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

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

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);
    let short = said
        .find("this list is short")
        .expect("a truncated sweep says so");

    assert!(short < said.find("acme.plugin0").expect("the first entry"));
}

#[test]
fn an_extension_the_home_file_agreed_to_at_its_own_digest_may_run() {
    let sample = Sample::new("extensions-listing-enabled");
    sample.installed("reviewer", &manifest("acme.reviewer"));
    let found = sample.discovered();
    let settings = sample.user(&format!(
        r#"{{"extensions": {{"acme.reviewer": {{"enabled": true,
             "digest": "{}"}}}}}}"#,
        agreed(&found, "acme.reviewer"),
    ));

    let said = listing(&found, &settings, RUNNING);

    assert!(said.contains("may run   yes"), "{said}");
    // The remedy is for the ones that are off. Said over a list where nothing
    // is, it sends a reader to change a file that is already right.
    assert!(!said.contains("nothing runs until"), "{said}");
}

#[test]
fn a_decision_that_names_no_manifest_or_names_another_one_is_told_apart() {
    // Both are enabled, so a listing that printed only that would show two
    // extensions as permitted when neither is. What separates them is what
    // whoever reads it has to do: write the digest down, or find out why the
    // file they agreed to is not the file that is there.
    let sample = Sample::new("extensions-listing-unpinned");
    sample.installed("one", &manifest("acme.one"));
    sample.installed("two", &manifest("acme.two"));
    let found = sample.discovered();
    let settings = sample.user(
        r#"{"extensions": {"acme.one": {"enabled": true},
                           "acme.two": {"enabled": true,
                                        "digest": "sha256:beef"}}}"#,
    );

    let said = listing(&found, &settings, RUNNING);

    assert!(
        said.contains("may run   no; no digest says which program was agreed to"),
        "{said}"
    );
    assert!(
        said.contains(
            "may run   no; the manifest has changed since it was agreed to at sha256:beef"
        ),
        "{said}"
    );
    // Neither is permitted, so the remedy still belongs at the foot of the list.
    assert_eq!(said.matches("nothing runs until").count(), 1, "{said}");
}

#[test]
fn a_list_holding_one_that_is_off_says_once_what_would_turn_it_on() {
    // Every entry carrying the same sentence is noise on the usual machine,
    // where nothing has been turned on yet and every entry says no.
    let sample = Sample::new("extensions-listing-remedy");
    sample.installed("one", &manifest("acme.one"));
    sample.installed("two", &manifest("acme.two"));

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

    assert_eq!(said.matches("nothing runs until").count(), 1, "{said}");
}

#[test]
fn an_extension_written_against_another_protocol_is_listed_as_not_hosted() {
    // Not something turning it on would cure, so the listing says which of the
    // two answers is the one standing in the way. Built from what this build
    // speaks rather than from a number written here, because the day that
    // number changes is the day a fixed one would start testing nothing.
    let host = ExtensionProtocol::HOST;
    let sample = Sample::new("extensions-listing-protocol");
    sample.installed(
        "reviewer",
        &speaking(&format!("{}.0", host.major().saturating_add(1)), RUNNING),
    );

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

    assert!(
        said.contains(&format!(
            "hosted    no; this crucible speaks protocol {host}"
        )),
        "{said}"
    );
}

#[test]
fn an_extension_that_needs_a_later_crucible_says_which_one_this_is() {
    let sample = Sample::new("extensions-listing-newer");
    sample.installed(
        "reviewer",
        &speaking(&ExtensionProtocol::HOST.to_string(), "9.0.0"),
    );

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

    // The crucible it wants and the crucible this is, one under the other.
    // Either alone leaves the reader working out the subtraction.
    assert!(said.contains("needs crucible 9.0.0"), "{said}");
    assert!(
        said.contains(&format!("hosted    no; this crucible is {RUNNING}")),
        "{said}"
    );
}

#[test]
fn an_extension_asking_for_more_reach_than_this_build_has_is_told_what_it_gets() {
    // Not a refusal — the pair settle on the smaller vocabulary — but the
    // author is the one who can tell whether the part it will not get matters,
    // and they cannot if the listing says only yes.
    let host = ExtensionProtocol::HOST;
    let sample = Sample::new("extensions-listing-reach");
    sample.installed(
        "reviewer",
        &speaking(
            &format!("{}.{}", host.major(), host.minor().saturating_add(4)),
            RUNNING,
        ),
    );

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

    assert!(
        said.contains(&format!("hosted    yes, speaking {host}")),
        "{said}"
    );
}

#[test]
fn the_settings_written_for_an_extension_are_named_but_never_quoted() {
    // Whoever is deciding about this extension wrote these names from its
    // documentation, and the listing is where they find out crucible read the
    // block they meant. The values stay out: crucible cannot tell which of
    // these names holds a key somebody pasted, and a listing is the wrong place
    // to find out it was one.
    let sample = Sample::new("extensions-listing-configured");
    sample.installed("reviewer", &manifest("acme.reviewer"));
    let found = sample.discovered();
    let settings = sample.user(&format!(
        r#"{{"extensions": {{"acme.reviewer": {{"enabled": true,
             "digest": "{}", "config": {{
             "style": "terse", "token": "sk-not-a-real-one"}}}}}}}}"#,
        agreed(&found, "acme.reviewer"),
    ));

    let said = listing(&found, &settings, RUNNING);

    // Directly under whether it may run, because both were written in the same
    // block of the same file and the pair read as one answer: allowed, and
    // told this.
    assert!(
        said.contains("may run   yes\n  config    style, token"),
        "{said}"
    );
    assert!(!said.contains("terse"), "{said}");
    assert!(!said.contains("sk-not-a-real-one"), "{said}");
}

#[test]
fn an_extension_nobody_wrote_settings_for_says_so_rather_than_nothing() {
    // A blank space beside the label reads like the listing failed to work it
    // out, the same way it would beside `asks for`. Nothing written is a real
    // and ordinary state and gets a word of its own.
    let sample = Sample::new("extensions-listing-unconfigured");
    sample.installed("reviewer", &manifest("acme.reviewer"));

    let said = listing(&sample.discovered(), &sample.decided(), RUNNING);

    assert!(said.contains("config    nothing"), "{said}");
}
