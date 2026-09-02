//! The broker binary refuses to run outside its launch protocol.
//!
//! This is also what makes `cargo test` build the binary itself, not only its
//! unit-test harness: the confinement tests in `crucible-tools` look for the
//! broker beside their own executable, and a workspace without an integration
//! test here would never produce it.

use std::process::Command;

#[test]
fn a_broker_started_without_its_status_channel_exits_without_supervising() {
    let output = Command::new(env!("CARGO_BIN_EXE_crucible-sandbox-broker"))
        .output()
        .expect("broker binary runs");
    assert_eq!(
        output.status.code(),
        Some(125),
        "a broker with no status descriptor must fail closed: {output:?}"
    );
    assert!(
        output.stdout.is_empty(),
        "the broker speaks only over its status channel: {output:?}"
    );
}
