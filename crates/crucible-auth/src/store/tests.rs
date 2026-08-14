use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// A key nobody would mistake for a real one, and the thing every leak test
/// greps for.
const SECRET: &str = "sk-do-not-log-me";

/// A tree that exists while the test does.
///
/// This crate's job is a file on a disk at a mode, so a fake filesystem would
/// only be testing the fake's answer to those questions.
struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("crucible-auth-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("a writable temporary directory");

        Self { base }
    }

    fn home(&self) -> &Path {
        &self.base
    }

    /// Puts a store on the disk without going through one, so a test can state
    /// the bytes it is about rather than the calls that would produce them.
    fn holding(&self, text: &str) -> Store {
        fs::write(self.base.join(FILE), text).expect("a writable temporary directory");

        Store::in_home(self.home())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// The key a store holds for `provider`, as text, for a test that has to
/// compare one — nothing outside this module can read a key back out.
fn held(store: &Store, provider: &str) -> Option<String> {
    let mut request = crucible_core::Outgoing::new();
    let header = crucible_core::Header::bare("x-api-key");
    store.read().get(provider)?.apply(&mut request, &header);

    request
        .headers()
        .iter()
        .find(|(name, _)| &**name == "x-api-key")
        .map(|(_, value)| value.to_string())
}

#[test]
fn a_key_in_the_file_is_the_key_read_back() {
    let scratch = Scratch::new("round-trip");
    let store = scratch.holding(&format!(
        r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#
    ));

    assert_eq!(held(&store, "openai").as_deref(), Some(SECRET));
}

#[test]
fn a_store_that_is_not_there_holds_nothing_and_says_nothing() {
    let scratch = Scratch::new("absent");
    let keys = Store::in_home(scratch.home()).read();

    assert_eq!(keys.providers().count(), 0);
    assert_eq!(
        keys.trouble(),
        None,
        "never having logged in is not a problem to report"
    );
}

#[test]
fn every_provider_logged_in_is_listed_in_name_order() {
    let scratch = Scratch::new("listed");
    let store =
        scratch.holding(r#"{"version":1,"keys":{"openai":"a","anthropic":"b","moonshot":"c"}}"#);

    let keys = store.read();
    let listed: Vec<_> = keys.providers().collect();
    assert_eq!(listed, ["anthropic", "moonshot", "openai"]);
}

#[test]
fn a_store_nobody_can_parse_reports_it_and_still_starts() {
    let scratch = Scratch::new("malformed");
    let store = scratch.holding("{not json at all");

    let keys = store.read();

    assert_eq!(keys.providers().count(), 0, "nobody is logged in");
    let said = keys.trouble().expect("a sentence for the user");
    assert!(
        said.contains(FILE),
        "the sentence has to name the file: {said}"
    );
}

#[test]
fn a_store_from_a_later_version_is_left_alone_rather_than_guessed_at() {
    let scratch = Scratch::new("newer");
    let written = format!(r#"{{"version":99,"keys":{{"openai":"{SECRET}"}}}}"#);
    let store = scratch.holding(&written);

    let keys = store.read();

    assert_eq!(keys.providers().count(), 0, "nobody is logged in");
    assert!(keys.trouble().is_some(), "and the user is told why");
    assert_eq!(
        fs::read_to_string(scratch.home().join(FILE)).expect("still there"),
        written,
        "reading never rewrites what it could not understand"
    );
}

#[test]
fn a_store_that_does_not_say_its_version_is_not_guessed_at_either() {
    let scratch = Scratch::new("unversioned");
    let store = scratch.holding(&format!(r#"{{"keys":{{"openai":"{SECRET}"}}}}"#));

    let keys = store.read();

    assert_eq!(keys.providers().count(), 0, "nobody is logged in");
    assert!(keys.trouble().is_some(), "and the user is told why");
}

#[test]
fn a_key_appears_in_no_debug_output_anywhere() {
    let scratch = Scratch::new("redacted");
    let store = scratch.holding(&format!(
        r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#
    ));

    let keys = store.read();
    for printed in [format!("{keys:?}"), format!("{store:?}")] {
        assert!(
            !printed.contains(SECRET),
            "a key reached a Debug line: {printed}"
        );
    }
}
