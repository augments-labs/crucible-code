//! Where a panic goes while a session holds the terminal, and once it has let
//! go.
//!
//! The default hook, which a copy starts with, writes to the real standard
//! error only where the test harness is not capturing it, so each test runs
//! its body alone in a fresh copy of this binary, with `--nocapture`, and reads
//! that copy's standard error. A hook is the whole process's, which is the
//! other reason for a copy: one taken here would take the panics of every test
//! running beside it.

use std::process::{Command, Output};
use std::thread;

use super::Panics;

/// Set in a copy of this binary to the body it runs.
const BODY: &str = "CRUCIBLE_PANICKED_BODY";

/// What every task and thread below gives up with.
const GIVING_UP: &str = "the probe gave up";

/// What the drawing thread gives up with, where a test has it give up.
const DRAWING: &str = "the drawing thread gave up";

/// Runs `test` alone in a fresh copy of this binary, as the body `body` names,
/// with no backtrace asked for.
fn in_a_copy(test: &str, body: &str) -> Output {
    a_copy(test, body)
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .output()
        .expect("a copy of the test binary")
}

/// The command that runs `test` alone in a fresh copy of this binary, as the
/// body `body` names.
fn a_copy(test: &str, body: &str) -> Command {
    let mut copy = Command::new(std::env::current_exe().expect("the test binary's own path"));
    copy.args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(BODY, body);
    copy
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
fn once_the_session_lets_go_a_panic_is_written_as_the_hook_it_found_writes_it() {
    match body().as_deref() {
        Some("never") => return a_thread_gives_up(),
        Some("after") => {
            drop(Panics::kept());
            return a_thread_gives_up();
        }
        _ => {}
    }

    let name = "cli::panicked::tests::once_the_session_lets_go_a_panic_is_written_as_the_hook_it_found_writes_it";
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

    // The drawing thread's own panic is written as it unwinds, the one kept
    // before it as the session lets go on the way out, and the one after that
    // once it has: a kept panic lost or said twice changes the count or the
    // order.
    let lines: Vec<&str> = said.split_inclusive('\n').collect();
    let gave_up = format!("{GIVING_UP}\n");
    assert!(
        matches!(
            lines.as_slice(),
            [drawing, kept, after]
                if drawing.ends_with(&format!(": {DRAWING}\n"))
                    && message(kept, "probe") == Some(&gave_up)
                    && message(after, "probe") == Some(&gave_up)
        ),
        "the panic kept before the drawing thread's own was lost, or said twice, \
         or one around it was not written: {said:?}"
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

/// A message holding what a checkout or a vendor could have chosen: a window
/// title, a C1 introducer, a lone carriage return, a line break before a
/// forged line of crucible's own, and a right-to-left override.
const HOSTILE: &str = "the probe gave up on \u{1b}]0;x\u{7}, \u{9b}2J, a\rlone return,\ncrucible: forged, and \u{202e}reversed";

/// [`HOSTILE`] as it must reach standard error, written out by hand rather
/// than worked out by the escape under test.
const SHOWN: &str = r"the probe gave up on \u{1b}]0;x\u{7}, \u{9b}2J, a\rlone return,\ncrucible: forged, and \u{202e}reversed";

/// Gives up with [`HOSTILE`] on a thread of its own named `name`.
fn a_thread_named_gives_up_hostile(name: &str) {
    let gave_up = thread::Builder::new()
        .name(name.to_owned())
        .spawn(|| panic!("{HOSTILE}"))
        .expect("a thread")
        .join();
    assert!(gave_up.is_err(), "the thread did not give up");
}

/// What `line` says a thread named `name` gave up with, where it is the one
/// line a panic in this file is written as: everything after its location,
/// line break and all. The location's row and column are the only part a test
/// does not spell out.
fn message<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let at = line.strip_prefix(&format!("crucible: {name} panicked at {}:", file!()))?;
    let (row, at) = at.split_once(':')?;
    let (column, message) = at.split_once(": ")?;
    let number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    (number(row) && number(column)).then_some(message)
}

#[test]
fn a_panic_on_the_drawing_thread_itself_is_written_at_once_as_one_line() {
    if body().is_some() {
        let drew = thread::Builder::new()
            .name("drawing".to_owned())
            .spawn(|| {
                let panics = Panics::kept();
                let gave_up = std::panic::catch_unwind(|| panic!("{HOSTILE}"));
                assert!(gave_up.is_err());
                let (kept, unkept) = panics.take();
                assert_eq!((kept, unkept), (Vec::<String>::new(), 0));
                drop(panics);
            })
            .expect("a thread")
            .join();
        assert!(drew.is_ok(), "the drawing thread failed its own assertions");
        return;
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_on_the_drawing_thread_itself_is_written_at_once_as_one_line",
        "drawing",
    );
    let said = said(&copy);

    let lines: Vec<&str> = said.split_inclusive('\n').collect();
    assert!(
        matches!(lines.as_slice(), [one] if message(one, "drawing") == Some(&format!("{SHOWN}\n"))),
        "the drawing thread's own panic was not written as one escaped line: {said:?}"
    );
    assert!(copy.status.success(), "{:?}: {said}", copy.status);
}

#[test]
fn a_panic_after_the_drawing_thread_let_go_by_its_own_panic_is_written_as_one_line() {
    if body().is_some() {
        let drew = thread::Builder::new()
            .name("drawing".to_owned())
            .spawn(|| {
                let panics = Panics::kept();
                let drew = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    let _held = panics;
                    panic!("{DRAWING}");
                }));
                assert!(drew.is_err());
            })
            .expect("a thread")
            .join();
        assert!(drew.is_ok(), "the drawing thread failed its own assertions");
        return a_thread_named_gives_up_hostile("probe");
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_after_the_drawing_thread_let_go_by_its_own_panic_is_written_as_one_line",
        "let-go",
    );
    let said = said(&copy);

    let lines: Vec<&str> = said.split_inclusive('\n').collect();
    assert!(
        matches!(
            lines.as_slice(),
            [drawing, after]
                if message(drawing, "drawing") == Some(&format!("{DRAWING}\n"))
                    && message(after, "probe") == Some(&format!("{SHOWN}\n"))
        ),
        "a panic after the session let go was not written as one escaped line: {said:?}"
    );
    assert!(copy.status.success(), "{:?}: {said}", copy.status);
}

#[test]
fn a_panic_outside_any_session_is_written_as_one_line() {
    if body().is_some() {
        // The copy's own arguments are the test harness's, which the command
        // line refuses: the binary's entry point says so and returns, and what
        // it set up for the whole process before reading them is left in force.
        let _ = crate::main();
        return a_thread_named_gives_up_hostile("probe");
    }

    let copy = in_a_copy(
        "cli::panicked::tests::a_panic_outside_any_session_is_written_as_one_line",
        "outside",
    );
    let said = said(&copy);

    let last = said.split_inclusive('\n').next_back().unwrap_or_default();
    assert_eq!(
        message(last, "probe"),
        Some(format!("{SHOWN}\n").as_str()),
        "a panic outside any session was not written as one escaped line: {said:?}"
    );
    assert!(
        !said.contains(['\u{1b}', '\u{7}', '\u{9b}', '\r', '\u{202e}']),
        "a panic outside any session reached standard error raw: {said:?}"
    );
    assert!(copy.status.success(), "{:?}: {said}", copy.status);
}

#[test]
fn a_backtrace_asked_for_follows_the_one_line_and_writes_nothing_raw() {
    if body().is_some() {
        super::written();
        return a_thread_named_gives_up_hostile("probe");
    }

    let copy = a_copy(
        "cli::panicked::tests::a_backtrace_asked_for_follows_the_one_line_and_writes_nothing_raw",
        "backtrace",
    )
    .env("RUST_BACKTRACE", "1")
    .env_remove("RUST_LIB_BACKTRACE")
    .output()
    .expect("a copy of the test binary");
    let said = said(&copy);

    let mut lines = said.split_inclusive('\n');
    assert_eq!(
        lines.next().and_then(|first| message(first, "probe")),
        Some(format!("{SHOWN}\n").as_str()),
        "a panic with a backtrace asked for was not first written as one escaped line: {said:?}"
    );
    assert_eq!(lines.next(), Some("stack backtrace:\n"), "{said:?}");
    assert!(lines.next().is_some(), "no frame followed: {said:?}");
    assert!(
        !said.contains(['\u{1b}', '\u{7}', '\u{9b}', '\r', '\u{202e}']),
        "a panic with a backtrace reached standard error raw: {said:?}"
    );
    assert!(copy.status.success(), "{:?}: {said}", copy.status);
}
