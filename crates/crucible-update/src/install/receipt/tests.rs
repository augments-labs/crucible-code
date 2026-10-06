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
        (
            "broker-not-hex",
            invalid(9, "sha256.crucible-sandbox-broker", HEX_64),
        ),
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
