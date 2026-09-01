//! What counts as a line that only reports.

use super::only;

#[test]
fn a_program_that_only_ever_reports_reports() {
    assert!(only("ls -la src"));
    assert!(only("wc -l Cargo.toml"));
    assert!(only("/usr/bin/cat Cargo.toml"));
}

#[test]
fn a_program_that_writes_does_not() {
    assert!(!only("rm -rf build"));
    assert!(!only("mv one two"));
    assert!(!only("cargo build"));
}

#[test]
fn a_multiplexer_is_read_one_subcommand_at_a_time() {
    assert!(only("gh pr view 487"));
    assert!(!only("gh pr create"));
    assert!(only("git log --oneline"));
    assert!(!only("git commit -m x"));
}

/// The subcommand is matched whole, so a longer word that opens with it is a
/// different subcommand and is not claimed.
#[test]
fn a_longer_word_that_opens_with_a_subcommand_is_a_different_subcommand() {
    assert!(!only("gh pr viewers"));
    assert!(!only("git logs"));
}

/// A multiplexer with nothing after it says nothing about what it will do.
#[test]
fn a_multiplexer_with_no_subcommand_is_not_claimed() {
    assert!(!only("gh"));
    assert!(!only("git"));
}

#[test]
fn every_part_of_the_line_has_to_report() {
    assert!(only("ls src && cat Cargo.toml"));
    assert!(!only("ls src && rm -rf build"));
    assert!(only("git status | wc -l"));
    assert!(!only("cat one.txt | tee two.txt"));
}

/// What the scanner cannot read, this declines to claim.
#[test]
fn a_line_that_does_not_say_what_runs_is_not_claimed() {
    assert!(!only("ls $(cat targets)"));
    assert!(!only("cat one.txt > two.txt"));
    assert!(!only("eval ls"));
    assert!(!only(""));
}
