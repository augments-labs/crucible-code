//! No mode starts the agent where it skips its own permission prompts.

use std::path::Path;

use crate::claude::{Claude, Mode, Modes, Permission, Row};
use crate::{Dialect, Shape};

/// The arguments that would start the agent with its prompts skipped, or able
/// to skip them.
const SKIPPING: [&str; 3] = [
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "bypassPermissions",
];

/// Every mode has a row, and no row runs the permission mode that skips
/// every prompt.
#[test]
fn no_mode_runs_the_agent_with_its_prompts_skipped() {
    for mode in Mode::ALL {
        for permission in Permission::ALL {
            if Modes::row(mode) == Row::Runs(permission) {
                assert_ne!(permission, Permission::BypassPermissions, "{mode:?}");
            }
        }
    }
}

/// Whatever the mode, the agent's command line names no way to skip its
/// prompts, and names the permission mode explicitly.
#[test]
fn no_launch_asks_the_agent_to_skip_its_prompts() {
    for mode in Mode::ALL {
        let Ok(mut agent) = Claude::in_mode(mode) else {
            continue;
        };
        for resume in [None, Some("4f0c")] {
            let request = agent.request(Shape {
                resume,
                profile: Some(Path::new("/profiles/work")),
                workspace: Some(Path::new("/work")),
            });
            for skipping in SKIPPING {
                assert!(
                    !request
                        .arguments
                        .iter()
                        .any(|argument| argument.contains(skipping)),
                    "{mode:?}: {:?}",
                    request.arguments
                );
            }
            assert!(
                request
                    .arguments
                    .windows(2)
                    .any(|pair| pair.first().map(String::as_str) == Some("--permission-mode")),
                "{mode:?}"
            );
        }
    }
}

/// A mode the table does not support is refused saying why, rather than run
/// in a wider one.
#[test]
fn an_unsupported_mode_is_refused_naming_why() {
    for mode in Mode::ALL {
        if let Row::Unsupported(why) = Modes::row(mode) {
            let refused = Claude::in_mode(mode).map(|_| ()).unwrap_err();
            assert_eq!(refused.reason().as_str(), why);
        }
    }
}

/// Asking keeps every prompt, and allowing edits skips only the ones for
/// edits.
#[test]
fn the_supported_modes_run_in_the_reviewed_permission_modes() {
    assert_eq!(Modes::row(Mode::Ask), Row::Runs(Permission::Default));
    assert_eq!(
        Modes::row(Mode::AllowEdits),
        Row::Runs(Permission::AcceptEdits)
    );
}
