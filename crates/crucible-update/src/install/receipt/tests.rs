use std::path::PathBuf;

use super::*;

fn fixture(verdict: &str, name: &str) -> Vec<u8> {
    let at = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/installer/receipts")
        .join(verdict)
        .join(format!("{name}.receipt"));
    std::fs::read(&at).unwrap_or_else(|error| panic!("{}: {error}", at.display()))
}

fn invalid(line: usize, key: &'static str, grammar: &'static str) -> ReceiptError {
    ReceiptError::Invalid { line, key, grammar }
}

fn expected(line: usize, key: &'static str) -> ReceiptError {
    ReceiptError::Expected { line, key }
}

#[test]
fn a_receipt_with_a_broker_is_read_into_its_values() {
    let receipt = Receipt::parse(&fixture("accepted", "linux-with-broker")).expect("a receipt");

    assert_eq!(
        receipt.installation().as_str(),
        "5c0f9d2e8a4b47e1b3d6a09f7c21e845"
    );
    assert_eq!(receipt.target(), Target::LinuxX86_64);
    assert_eq!(
        receipt.prefix(),
        Path::new("/home/someone/.local/bin/.crucible-install")
    );
    assert_eq!(receipt.version().as_str(), "0.46.0");
    assert_eq!(receipt.crucible().to_string().len(), 64);
    assert!(receipt.broker().is_some());
}

#[test]
fn a_receipt_without_a_broker_has_none() {
    let receipt =
        Receipt::parse(&fixture("accepted", "freebsd-without-broker")).expect("a receipt");

    assert_eq!(receipt.target(), Target::FreebsdX86_64);
    assert_eq!(receipt.broker(), None);
}

#[test]
fn a_prefix_that_is_not_utf_8_is_kept_as_its_bytes() {
    let receipt = Receipt::parse(&fixture("accepted", "prefix-not-utf-8")).expect("a receipt");

    assert!(receipt.prefix().as_os_str().as_bytes().contains(&0xff));
}

/// Each refused fixture, with the reason it is refused for.
fn reasons() -> Vec<(&'static str, ReceiptError)> {
    vec![
        ("empty", ReceiptError::NotAReceipt),
        ("unterminated", ReceiptError::Unterminated),
        ("larger-than-the-ceiling", ReceiptError::TooLarge),
        ("one-past-the-ceiling", ReceiptError::TooLarge),
        ("carriage-returns", ReceiptError::Control { line: 1 }),
        ("tab-in-prefix", ReceiptError::Control { line: 6 }),
        ("nul-in-prefix", ReceiptError::Control { line: 6 }),
        ("delete-in-prefix", ReceiptError::Control { line: 6 }),
        ("escape-in-version", ReceiptError::Control { line: 7 }),
        ("newer-format", ReceiptError::Newer),
        ("much-newer-format", ReceiptError::Newer),
        ("huge-format-number", ReceiptError::Newer),
        ("format-zero", ReceiptError::NotAReceipt),
        ("format-leading-zero", ReceiptError::NotAReceipt),
        ("format-not-a-number", ReceiptError::NotAReceipt),
        ("format-space-after", ReceiptError::NotAReceipt),
        ("other-header", ReceiptError::NotAReceipt),
        ("no-header", ReceiptError::NotAReceipt),
        ("only-header", ReceiptError::Ended { key: "manager" }),
        ("other-manager", invalid(2, "manager", "crucible-installer")),
        ("key-in-upper-case", expected(2, "manager")),
        ("spaces-around-equals", expected(2, "manager")),
        ("reordered-keys", expected(3, "installation")),
        ("unknown-key", expected(5, "layout")),
        ("missing-layout", expected(5, "layout")),
        ("blank-line-inside", expected(5, "layout")),
        ("missing-crucible", expected(8, "sha256.crucible")),
        ("repeated-key", expected(8, "sha256.crucible")),
        ("broker-before-crucible", expected(8, "sha256.crucible")),
        ("line-after-broker", ReceiptError::Trailing { line: 10 }),
        ("blank-line-at-end", ReceiptError::Trailing { line: 10 }),
        (
            "installation-upper-case",
            invalid(3, "installation", HEX_32),
        ),
        ("installation-short", invalid(3, "installation", HEX_32)),
        ("installation-long", invalid(3, "installation", HEX_32)),
        ("installation-empty", invalid(3, "installation", HEX_32)),
        ("target-windows", invalid(4, "target", TARGET)),
        ("target-other-arch", invalid(4, "target", TARGET)),
        ("target-freebsd-aarch64", invalid(4, "target", TARGET)),
        ("layout-flat", invalid(5, "layout", "versioned")),
        ("prefix-relative", invalid(6, "prefix", CANONICAL)),
        ("prefix-tilde", invalid(6, "prefix", CANONICAL)),
        ("prefix-empty", invalid(6, "prefix", CANONICAL)),
        ("prefix-root", invalid(6, "prefix", CANONICAL)),
        ("prefix-dot", invalid(6, "prefix", CANONICAL)),
        ("prefix-ends-in-dot", invalid(6, "prefix", CANONICAL)),
        ("prefix-dot-dot", invalid(6, "prefix", CANONICAL)),
        ("prefix-ends-in-dot-dot", invalid(6, "prefix", CANONICAL)),
        ("prefix-double-slash", invalid(6, "prefix", CANONICAL)),
        (
            "prefix-leading-double-slash",
            invalid(6, "prefix", CANONICAL),
        ),
        ("prefix-trailing-slash", invalid(6, "prefix", CANONICAL)),
        ("version-two-parts", invalid(7, "version", RELEASE)),
        ("version-four-parts", invalid(7, "version", RELEASE)),
        ("version-leading-zero", invalid(7, "version", RELEASE)),
        ("version-prerelease", invalid(7, "version", RELEASE)),
        ("version-v-prefix", invalid(7, "version", RELEASE)),
        ("version-empty-part", invalid(7, "version", RELEASE)),
        ("version-empty", invalid(7, "version", RELEASE)),
        ("crucible-upper-case", invalid(8, "sha256.crucible", HEX_64)),
        ("crucible-short", invalid(8, "sha256.crucible", HEX_64)),
        ("broker-not-hex", invalid(9, BROKER, HEX_64)),
        ("broker-empty", invalid(9, BROKER, HEX_64)),
        ("broker-upper-case", invalid(9, BROKER, HEX_64)),
        ("broker-short", invalid(9, BROKER, HEX_64)),
        ("broker-key-unknown", expected(9, BROKER)),
        ("broker-key-repeats-crucible", expected(9, BROKER)),
    ]
}

#[test]
fn each_refused_receipt_is_refused_for_its_own_reason() {
    for (name, reason) in reasons() {
        assert_eq!(
            Receipt::parse(&fixture("refused", name)),
            Err(reason),
            "{name}"
        );
    }
}

#[test]
fn every_refused_fixture_has_its_reason_stated() {
    let mut present: Vec<String> = std::fs::read_dir(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/installer/receipts/refused"),
    )
    .expect("the refused fixtures")
    .map(|entry| {
        let name = entry.expect("an entry").file_name();
        let name = name.to_str().expect("a UTF-8 name");
        name.strip_suffix(".receipt").expect("a receipt").to_owned()
    })
    .collect();
    present.sort();
    let mut stated: Vec<String> = reasons()
        .into_iter()
        .map(|(name, _)| name.to_owned())
        .collect();
    stated.sort();

    // A fixture added without a reason here would be held only to a verdict.
    assert_eq!(present, stated);
}

#[test]
fn a_receipt_as_large_as_the_ceiling_is_read() {
    let bytes = fixture("accepted", "exactly-the-ceiling");

    assert_eq!(bytes.len(), MAX_BYTES);
    assert!(Receipt::parse(&bytes).is_ok());
}

/// How many arms [`named`] has. A target added to the enum fails to compile
/// there until it is named, and this count is then held to [`Target::ALL`],
/// which the parser looks names up in.
const NAMED: usize = 5;

/// The name each target has, written as a match so that a new target cannot
/// be added without a name being decided here.
fn named(target: Target) -> &'static str {
    match target {
        Target::LinuxX86_64 => "linux-x86_64",
        Target::LinuxAarch64 => "linux-aarch64",
        Target::MacosX86_64 => "macos-x86_64",
        Target::MacosAarch64 => "macos-aarch64",
        Target::FreebsdX86_64 => "freebsd-x86_64",
    }
}

#[test]
fn every_target_has_one_name_and_an_accepted_receipt_that_uses_it() {
    let accepted: Vec<Vec<u8>> = std::fs::read_dir(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/installer/receipts/accepted"),
    )
    .expect("the accepted fixtures")
    .map(|entry| std::fs::read(entry.expect("an entry").path()).expect("a receipt"))
    .collect();

    let mut names: Vec<&str> = Target::ALL.iter().map(|target| target.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        Target::ALL.len(),
        NAMED,
        "a target is missing from the list"
    );
    assert_eq!(names.len(), Target::ALL.len(), "two targets share a name");
    for target in Target::ALL {
        assert_eq!(target.as_str(), named(target));
        assert_eq!(target.to_string(), named(target));
        let line = format!("\ntarget={}\n", named(target));
        assert!(
            accepted.iter().any(|receipt| receipt
                .windows(line.len())
                .any(|window| window == line.as_bytes())),
            "no accepted receipt names {target}"
        );
    }
    if let Some(running) = Target::running() {
        assert!(Target::ALL.contains(&running));
    }
}

#[test]
fn a_digest_is_written_as_lowercase_hex() {
    let bytes = std::array::from_fn(|at| match at {
        0 => 0xab,
        31 => 0x0f,
        _ => 0,
    });

    let text = Digest::new(bytes).to_string();

    assert!(text.starts_with("ab00"));
    assert!(text.ends_with("000f"));
    assert_eq!(text.len(), 64);
}
