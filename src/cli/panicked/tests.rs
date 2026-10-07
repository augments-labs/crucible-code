//! Where a panic goes while a session holds the terminal, and once it has let
//! go.
//!
//! The default hook writes to the real standard error only where the test
//! harness is not capturing it, so each test runs its body alone in a fresh
//! copy of this binary, with `--nocapture`, and reads that copy's standard
//! error. A hook is the whole process's, which is the other reason for a copy:
//! one taken here would take the panics of every test running beside it.

use std::process::{Command, Output};
use std::thread;

use super::Panics;

/// Set in a copy of this binary to the body it runs.
const BODY: &str = "CRUCIBLE_PANICKED_BODY";

/// What every task and thread below gives up with.
const GIVING_UP: &str = "the probe gave up";

/// What the drawing thread gives up with, where a test has it give up.
const DRAWING: &str = "the drawing thread gave up";

/// Runs `test` alone in a fresh copy of this binary, as the body `body` names.
fn in_a_copy(test: &str, body: &str) -> Output {
    Command::new(std::env::current_exe().expect("the test binary's own path"))
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(BODY, body)
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .output()
        .expect("a copy of the test binary")
}

/// The body this process was started to run, where it is a copy.
fn body() -> Option<String> {
    std::env::var(BODY).ok()
}

/// Gives up, from one place, so every copy that calls it names the same
/// location.
fn give_up() {
    panic!("{GIVING_UP}");
}

/// Gives up on a thread of its own, named the same in every copy.
fn a_thread_gives_up() {
    a_thread_named_gives_up("probe");
}

/// Gives up on a thread of its own, named `name`.
fn a_thread_named_gives_up(name: &str) {
    let gave_up = thread::Builder::new()
        .name(name.to_owned())
        .spawn(give_up)
        .expect("a thread")
        .join();
    assert!(gave_up.is_err(), "the thread did not give up");
}

/// Standard error, read as text.
fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `said` without the number the default hook puts after a thread's name,
/// which is the one thing two copies panicking the same way cannot share.
fn unnumbered(said: &str) -> String {
    said.split("' (")
        .enumerate()
        .map(|(at, part)| match part.split_once(") ") {
            Some((number, rest)) if at > 0 && number.bytes().all(|b| b.is_ascii_digit()) => {
                format!("' {rest}")
            }
            _ if at > 0 => format!("' ({part}"),
            _ => part.to_owned(),
        })
        .collect()
}

#[test]
fn a_task_that_panics_while_a_session_holds_the_terminal_writes_nothing_to_standard_error() {
    if body().is_some() {
        let panics = Panics::kept();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .expect("a runtime");
        let ended = runtime.block_on(async { tokio::spawn(async { give_up() }).await });
        assert!(ended.is_err_and(|ended| ended.is_panic()));

        let (kept, unkept) = panics.take();
        assert_eq!(unkept, 0);
        assert!(
            matches!(kept.as_slice(), [one] if one.contains(GIVING_UP)),
            "the drawing thread was not handed the task's panic: {kept:?}"
        );
        return;
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_task_that_panics_while_a_session_holds_the_terminal_writes_nothing_to_standard_error",
        "held",
    );

    assert_eq!(
        said(&copy),
        "",
        "a task's panic reached standard error while a session held the terminal"
    );
    assert!(
        copy.status.success(),
        "{}",
        String::from_utf8_lossy(&copy.stdout)
    );
}

#[test]
fn a_panic_the_drawing_thread_never_said_is_written_once_the_session_lets_go() {
    if body().is_some() {
        let panics = Panics::kept();
        a_thread_gives_up();
        drop(panics);
        return;
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_the_drawing_thread_never_said_is_written_once_the_session_lets_go",
        "unsaid",
    );

    assert_eq!(
        said(&copy).matches(GIVING_UP).count(),
        1,
        "a panic nobody said was lost, or said twice: {:?}",
        said(&copy)
    );
    assert!(
        copy.status.success(),
        "{}",
        String::from_utf8_lossy(&copy.stdout)
    );
}

#[test]
fn a_panic_on_the_drawing_thread_itself_is_written_by_the_hook_it_found() {
    if body().is_some() {
        let panics = Panics::kept();
        let gave_up = std::panic::catch_unwind(give_up);
        assert!(gave_up.is_err());
        let (kept, _) = panics.take();
        assert_eq!(kept, Vec::<String>::new());
        drop(panics);
        return;
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_on_the_drawing_thread_itself_is_written_by_the_hook_it_found",
        "drawing",
    );

    assert!(
        said(&copy).contains(GIVING_UP),
        "the drawing thread's own panic was not written: {:?}",
        said(&copy)
    );
    assert!(
        copy.status.success(),
        "{}",
        String::from_utf8_lossy(&copy.stdout)
    );
}

#[test]
fn once_the_session_lets_go_a_panic_is_written_as_the_default_hook_writes_it() {
    match body().as_deref() {
        Some("never") => return a_thread_gives_up(),
        Some("after") => {
            drop(Panics::kept());
            return a_thread_gives_up();
        }
        _ => {}
    }

    let name = "cli::panicked::tests::once_the_session_lets_go_a_panic_is_written_as_the_default_hook_writes_it";
    let never = in_a_copy(name, "never");
    let after = in_a_copy(name, "after");

    assert!(said(&never).contains(GIVING_UP), "{:?}", said(&never));
    assert_eq!(
        unnumbered(&said(&after)),
        unnumbered(&said(&never)),
        "a session that has let go left a panic written differently"
    );
    assert!(never.status.success() && after.status.success());
}

#[test]
fn a_panic_on_the_drawing_thread_while_the_session_holds_the_terminal_loses_nothing_kept() {
    if body().is_some() {
        let panics = Panics::kept();
        a_thread_gives_up();
        let drew = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _held = panics;
            panic!("{DRAWING}");
        }));
        assert!(drew.is_err());
        return a_thread_gives_up();
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_on_the_drawing_thread_while_the_session_holds_the_terminal_loses_nothing_kept",
        "unwinding",
    );
    let said = said(&copy);

    assert_eq!(
        said.matches("crucible: probe panicked").count(),
        1,
        "the panic kept before the drawing thread's own was lost, or said twice: {said:?}"
    );
    assert_eq!(
        said.matches(DRAWING).count(),
        1,
        "the drawing thread's own panic was not written by the hook it found: {said:?}"
    );
    assert_eq!(
        said.matches(GIVING_UP).count(),
        2,
        "a panic after the session let go was not written by the hook it found: {said:?}"
    );
    assert!(copy.status.success(), "{:?}: {said}", copy.status);
}

#[test]
fn a_panic_the_drawing_thread_could_not_draw_is_put_back_under_the_same_ceiling() {
    if body().is_some() {
        let panics = Panics::kept();
        a_thread_named_gives_up("undrawn");
        let (said, unkept) = panics.take();
        for _ in 0..super::KEPT {
            a_thread_gives_up();
        }
        panics.put_back(said, unkept);
        drop(panics);
        return;
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_the_drawing_thread_could_not_draw_is_put_back_under_the_same_ceiling",
        "undrawn",
    );
    let said = said(&copy);

    assert!(
        said.starts_with("crucible: undrawn panicked"),
        "a panic put back was not said ahead of those kept since: {said:?}"
    );
    assert_eq!(
        said.matches("crucible: probe panicked").count(),
        super::KEPT - 1,
        "{said:?}"
    );
    assert!(
        said.contains("crucible: and 1 more panics"),
        "a panic put back past the ceiling was not counted: {said:?}"
    );
    assert!(copy.status.success(), "{:?}: {said}", copy.status);
}

#[test]
fn a_panic_written_once_the_session_lets_go_is_one_line_whatever_its_message_holds() {
    // A message is built from whatever the code that gave up was holding,
    // which can be text a checkout or a vendor chose.
    let hostile = "the probe gave up on a\ncrucible: forged \u{1b}]0;retitled\u{7}\u{2028}line";
    if body().is_some() {
        let panics = Panics::kept();
        let gave_up = thread::Builder::new()
            .name("probe".to_owned())
            .spawn(move || panic!("{hostile}"))
            .expect("a thread")
            .join();
        assert!(gave_up.is_err(), "the thread did not give up");
        drop(panics);
        return;
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_written_once_the_session_lets_go_is_one_line_whatever_its_message_holds",
        "hostile",
    );

    let said = said(&copy);
    let lines: Vec<&str> = said.lines().collect();
    assert!(
        matches!(lines.as_slice(), [one] if one.starts_with("crucible: probe panicked at ")),
        "a panic's message added a line of its own: {said:?}"
    );
    assert!(
        !said.contains(['\u{1b}', '\u{7}', '\u{2028}']),
        "a panic's message reached standard error with a control character in it: {said:?}"
    );
    assert!(copy.status.success(), "{:?}: {said}", copy.status);
}
