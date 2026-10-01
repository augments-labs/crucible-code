//! What Kimi is sent and what it is read as, held to the bytes and events the
//! code wrote before its wire was shared with any other vendor.
//!
//! The expected files under `fixtures/` were written once, by the revision
//! this module arrived in, before any of the code they hold moved, and are not
//! written again: a change that alters one of them alters what Kimi receives
//! or what a turn is told, which is the thing these tests exist to catch.
//! Each request is the address, every header in the order it is set, and the
//! body; each response is the deltas it yields, one to a line, or the failure
//! it ends in. The one value a release changes, the version in the user agent,
//! is written as `{version}`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{Delta, DeltaStream};
use crucible_runtime::Cancel;

use super::*;
use crate::transport::Replay;

include!("fixtures/conversations.rs");

/// The key every fixture authorizes with. Fabricated, and written into the
/// request fixtures on purpose: which header carries it is part of what is sent.
const KEY: &str = "fabricated-kimi-key";

/// Where the expected file `name` is kept.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/moonshot/fixtures")
        .join(name)
}

/// Whether `actual` is what the fixture `name` holds.
fn agrees(name: &str, actual: &str) {
    let path = fixture(name);
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|problem| panic!("{}: {problem}", path.display()));
    assert_eq!(actual, expected, "{} differs", path.display());
}

/// Kimi at its coding address, answering every request with `status` and
/// `body`.
fn kimi(status: u16, body: &str) -> (Moonshot, std::sync::Arc<Replay>) {
    let replay = std::sync::Arc::new(Replay::new(status, body));
    (
        Moonshot::at(
            Moonshot::CODING,
            Box::new(HeaderKey::new(ApiKey::new(KEY), Header::bearer())),
            Box::new(std::sync::Arc::clone(&replay)),
        ),
        replay,
    )
}

/// What `stream` yields, one line each, ending with the failure if it failed.
///
/// A tool call's identity, name and arguments are written out: `Delta`'s own
/// `Debug` hides them, and they are what a call's fixture is about.
fn read(stream: &mut dyn DeltaStream) -> String {
    let mut lines = String::new();
    while let Some(next) = crucible_runtime::answered!(stream.next()) {
        let line = match next {
            Ok(Delta::ToolStarted { id, name }) => {
                format!("ToolStarted {{ id: {:?}, name: {name:?} }}", id.as_str())
            }
            Ok(Delta::ToolArgs(args)) => format!("ToolArgs({args:?})"),
            Ok(delta) => format!("{delta:?}"),
            Err(problem) => format!("failed: {problem:?}"),
        };
        lines.push_str(&line);
        lines.push('\n');
    }
    lines
}

#[test]
fn every_request_kimi_is_sent_is_the_one_it_was_sent_before() {
    let answer = streams()
        .into_iter()
        .find(|(name, ..)| *name == "text")
        .map(|(_, _, body)| body)
        .expect("the plain answer is a fixture");

    for (name, request) in conversations() {
        let (provider, replay) = kimi(200, answer);
        let cancel = Cancel::new();

        let mut stream = crucible_runtime::answered!(provider.stream(request, &cancel))
            .unwrap_or_else(|problem| panic!("{name}: {problem}"));
        read(stream.as_mut());

        let sent = replay.sent();
        let mut written = format!("POST {}\n", sent.url);
        for (header, value) in &sent.headers {
            let _ = writeln!(written, "{header}: {value}");
        }
        written.push('\n');
        written.push_str(&sent.body);
        written.push('\n');
        let written = written.replace(env!("CARGO_PKG_VERSION"), "{version}");

        agrees(&format!("requests/{name}.http"), &written);
    }
}

#[test]
fn every_response_kimi_sends_is_read_as_it_was_read_before() {
    for (name, status, body) in streams() {
        let (provider, _) = kimi(status, body);
        let cancel = Cancel::new();
        let request = conversations()
            .into_iter()
            .next()
            .map(|(_, request)| request)
            .expect("a conversation to ask with");

        let read = match crucible_runtime::answered!(provider.stream(request, &cancel)) {
            Ok(mut stream) => read(stream.as_mut()),
            Err(problem) => format!("refused: {problem:?}\n"),
        };

        agrees(&format!("streams/{name}.events"), &read);
    }
}
