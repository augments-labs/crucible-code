//! The directory this test binary was compiled in, and whether it is still there.
//!
//! `env!("CARGO_MANIFEST_DIR")` is resolved by the compiler, so a test binary
//! carries the crate directory it was *built* in rather than the one it is
//! being run from. A binary out of a reused build directory, a shared cache or
//! a checkout that has since been removed therefore names a place the reader
//! has never seen. A test that reads it without asking then either fails with
//! a path that looks like a fault in the code under test, or — if it writes —
//! writes into a tree nobody is looking at. Every test in this crate that
//! needs that path asks here, so there is one check and one sentence to read.

use std::path::Path;

/// The crate directory this test binary was compiled in.
///
/// # Panics
///
/// When it is no longer on disk. The build the binary came out of is then what
/// has to be thrown away, and nothing about the code under test can be
/// concluded from the run.
pub(crate) fn manifest_directory() -> &'static Path {
    match checked(Path::new(env!("CARGO_MANIFEST_DIR"))) {
        Ok(directory) => directory,
        Err(reason) => panic!("{reason}"),
    }
}

/// `directory` when it is still a directory, and otherwise what the reader has
/// to be told about the binary carrying it.
fn checked(directory: &Path) -> Result<&Path, String> {
    if directory.is_dir() {
        return Ok(directory);
    }

    Err(format!(
        "this test binary was compiled in {}, which is not there any more, so \
         the binary is older than the checkout running it. The path is fixed \
         when the crate is compiled and cannot be corrected at run time: build \
         again here, or clear the build directory this binary came out of. \
         Nothing is being reported about the code under test.",
        directory.display()
    ))
}

#[test]
fn a_baked_path_that_is_gone_is_reported_as_a_build_that_outlived_its_checkout() {
    let scratch = crate::sample::Scratch::new("baked-path-gone");
    let removed = scratch.at("removed/crates/crucible-config");

    // Without this check the reader is handed the workspace's own error, which
    // says the directory is not there and stops — true, and no help at all when
    // the directory is one the compiler chose and the reader has never seen.
    let workspace =
        crucible_core::Workspace::open(&removed).expect_err("a directory that was never created");
    assert!(
        !workspace.to_string().contains("test binary"),
        "{workspace}"
    );

    let reason = checked(&removed).expect_err("a directory that was never created");
    assert!(reason.contains(&removed.display().to_string()), "{reason}");
    assert!(reason.contains("test binary"), "{reason}");
    assert!(reason.contains("older than the checkout"), "{reason}");
}

#[test]
fn a_baked_path_that_is_still_there_is_handed_back() {
    let scratch = crate::sample::Scratch::new("baked-path-here");

    assert_eq!(checked(scratch.root()).unwrap(), scratch.root());
    assert!(manifest_directory().is_dir());
}
