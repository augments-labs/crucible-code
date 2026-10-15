//! Each mode starts the agent in the one permission mode mapped to it, and
//! only full access starts it with its prompts skipped.

use std::path::Path;

use crate::claude::{Claude, Mode, Modes, Permission};
use crate::{Dialect, Shape};

/// The arguments that would start the agent with its prompts skipped, or able
/// to skip them.
const SKIPPING: [&str; 3] = [
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "bypassPermissions",
];

/// Each mode runs in its mapped permission mode, and none of the agent's
/// other modes is a row.
#[test]
fn each_mode_runs_in_its_mapped_permission_mode() {
    assert_eq!(Modes::permission(Mode::Ask), Permission::Default);
    assert_eq!(Modes::permission(Mode::AllowEdits), Permission::AcceptEdits);
    assert_eq!(
        Modes::permission(Mode::FullAccess),
        Permission::BypassPermissions
    );
}

/// Whatever the mode, the agent's command line names its permission mode
/// explicitly, and only full access names a way to skip its prompts, by that
/// mode and by no flag beside it.
#[test]
fn only_full_access_launches_the_agent_with_its_prompts_skipped() {
    for mode in Mode::ALL {
        let mut agent = Claude::in_mode(mode);
        for resume in [None, Some("4f0c")] {
            let request = agent.request(Shape {
                resume,
                profile: Some(Path::new("/profiles/work")),
                workspace: Some(Path::new("/work")),
                model: None,
            });
            let named = Modes::permission(mode).name();
            assert!(
                request.arguments.windows(2).any(|pair| pair
                    .iter()
                    .map(String::as_str)
                    .eq(["--permission-mode", named])),
                "{mode:?}: {:?}",
                request.arguments
            );
            for skipping in SKIPPING {
                let skips = request
                    .arguments
                    .iter()
                    .any(|argument| argument.contains(skipping));
                assert_eq!(
                    skips,
                    mode == Mode::FullAccess && skipping == "bypassPermissions",
                    "{mode:?} {skipping}: {:?}",
                    request.arguments
                );
            }
        }
    }
}

/// A run that reports a mode asking at least as much as the one it was
/// started in is admitted, and one asking less is refused.
#[test]
fn a_run_is_admitted_in_a_mode_that_asks_at_least_as_much() {
    assert!(Permission::AcceptEdits.admits(Permission::Default));
    assert!(Permission::AcceptEdits.admits(Permission::Plan));
    assert!(Permission::AcceptEdits.admits(Permission::DontAsk));
    assert!(Permission::Default.admits(Permission::Plan));
    assert!(Permission::Default.admits(Permission::DontAsk));
    assert!(!Permission::Default.admits(Permission::AcceptEdits));
    assert!(!Permission::Default.admits(Permission::Auto));
    assert!(!Permission::AcceptEdits.admits(Permission::Auto));
    for started in Permission::ALL {
        assert!(started.admits(started), "{started:?}");
        assert_eq!(
            started.admits(Permission::BypassPermissions),
            started == Permission::BypassPermissions,
            "{started:?}"
        );
    }
}

/// A model chosen is named on the command line by its own argument, and none
/// chosen leaves the agent's own default, with no model named at all.
#[test]
fn the_model_chosen_is_the_one_the_agent_is_started_with() {
    for (model, named) in [(Some("opus"), Some("opus")), (None, None)] {
        let request = Claude::in_mode(Mode::Ask).request(Shape {
            resume: Some("4f0c"),
            model,
            ..Shape::default()
        });
        let at = request
            .arguments
            .iter()
            .position(|argument| argument == "--model");
        assert_eq!(
            at.and_then(|at| request.arguments.get(at + 1))
                .map(String::as_str),
            named,
            "{:?}",
            request.arguments
        );
        assert!(
            !request
                .arguments
                .iter()
                .any(|argument| argument.starts_with("--model=")),
            "{:?}",
            request.arguments
        );
    }
}
