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

#[test]
fn directory_changes_and_print_only_sed_can_join_a_lookup_run() {
    for line in [
        "cd src && grep -n main lib.rs",
        "cd '../other tree' && sed -n '1,80p' src/lib.rs",
        "sed -n '20p' file | head -10",
    ] {
        assert!(only(line), "{line}");
        assert!(
            matches!(
                super::command::read(line),
                crucible_core::Command::Opaque(_)
            ),
            "display classification must not widen permission rules: {line}"
        );
    }
}

#[test]
fn scripts_and_sed_writes_are_still_individual_calls() {
    for line in [
        "cd src && rm file",
        "sed -i 's/a/b/' file",
        "sed -n '1,2w output' file",
        "sed -n '1p' -i file",
        "sed -n '1p' * data.txt",
        "sed -n '1p' ?i data.txt",
        "sed -n '1p' [-]i data.txt",
        "sed -n '1p; e touch file' file",
        "cd src && python3 -c 'pass'",
        "cd $(pwd) && cat file",
        "cat >file <<'EOF'\ntext\nEOF",
    ] {
        assert!(!only(line), "{line}");
    }
}
