//! What `crucible agents install` writes.

use std::path::PathBuf;

use super::*;

#[test]
fn a_driver_is_named_as_externalagents_names_it() {
    assert_eq!(driver("antigravity"), Ok(Driver::Antigravity));
    assert_eq!(driver("claude"), Ok(Driver::Claude));
    assert_eq!(
        driver("Antigravity"),
        Err("expected one of claude, antigravity".to_owned())
    );
}

#[test]
fn an_install_says_where_the_release_is_and_how_to_use_it() {
    let installed = Installed {
        version: "1.3.0",
        executable: PathBuf::from("/home/me/.crucible/programs/antigravity/1.3.0/agy"),
        kept: false,
    };
    assert_eq!(
        answered(Driver::Antigravity, &installed),
        "Installed antigravity 1.3.0 at /home/me/.crucible/programs/antigravity/1.3.0/agy. \
         /login offers it under Use an installed coding agent.\n"
    );
    let kept = Installed {
        kept: true,
        ..installed
    };
    assert_eq!(
        answered(Driver::Antigravity, &kept),
        "antigravity 1.3.0 is already installed at \
         /home/me/.crucible/programs/antigravity/1.3.0/agy.\n"
    );
}

#[test]
fn a_terminal_is_told_each_new_percent_on_one_line() {
    let mut progress = Progress::new(Driver::Antigravity, true);
    assert_eq!(progress.ended(), "");
    assert_eq!(
        progress.heard(1_000_000, 4_000_000),
        "\rDownloading antigravity (4 MB):  25%"
    );
    assert_eq!(progress.heard(1_000_001, 4_000_000), "");
    assert_eq!(
        progress.heard(4_000_000, 4_000_000),
        "\rDownloading antigravity (4 MB): 100%"
    );
    assert_eq!(progress.ended(), "\n");
}

#[test]
fn anything_else_is_told_once_that_the_download_began() {
    let mut progress = Progress::new(Driver::Antigravity, false);
    assert_eq!(
        progress.heard(1_000_000, 4_000_000),
        "Downloading antigravity (4 MB)…\n"
    );
    assert_eq!(progress.heard(4_000_000, 4_000_000), "");
    assert_eq!(progress.ended(), "");
}
