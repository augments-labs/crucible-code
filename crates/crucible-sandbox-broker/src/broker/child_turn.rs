//! The turn a test holds while it has a child process of this test binary.
//!
//! The test harness runs every test as a thread of one process, and a child's
//! exit status belongs to that process, not to the thread that spawned it.
//! `scope::reap_until_workload` collects any exited child on purpose, so a
//! test driving it would take the status of a child another test is waiting
//! for by pid, and that test's `Command::status` or `Child::wait` fails with
//! `ECHILD` ("No child processes"). The reap can equally take another reaping
//! test's workload. Without this turn the failure is a rare, load-dependent
//! flake, not a defect in either test.
//!
//! So every test that spawns a child, whether it waits for it by pid or reaps
//! whatever has exited, holds the turn from before its first spawn until its
//! last child is collected. The mutex guards that time, not any data. A test
//! that panicked while holding it has already failed, and whatever child it
//! left is one nobody waits for, so a poisoned turn is taken as it stands
//! rather than failing every test that comes after.

use std::sync::{Mutex, MutexGuard, PoisonError};

static TURN: Mutex<()> = Mutex::new(());

/// Waits until no other test holds the turn, and holds it until the returned
/// guard is dropped. That no other test then has a child holds only while every
/// test that starts or reaps a child takes the turn first and keeps it until
/// the child is collected, and none has panicked and left a child behind.
pub(super) fn take() -> MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    /// The calls that start a child of this binary or collect any exited one,
    /// as they read with whitespace removed.
    const CHILD_CALLS: &[&str] = &[
        ".spawn()",
        ".status()",
        ".output()",
        "reap_until_workload(",
        "reap_available(",
        "process::wait(",
    ];
    const TAKE: &str = "child_turn::take()";

    /// Holds the broker's tests to the rule above. It reads `src/broker.rs`
    /// and the `.rs` files directly in `src/broker/`, not in directories
    /// below it, drops every line that starts with `//` and removes all
    /// whitespace. In what is left, a `#[test]` function's body runs from the
    /// first `{` after the attribute to the brace that closes it, counted by
    /// depth. A body containing one of `CHILD_CALLS` must contain `TAKE`
    /// before the first of them.
    ///
    /// It cannot see a child started or reaped inside a helper a test calls,
    /// a call spelled other than `CHILD_CALLS` lists, a turn taken under
    /// another name, or a guard dropped before the child is collected. A
    /// trailing or block comment, or a literal, that spells `TAKE` before the
    /// first child call counts as the turn, and so does a longer path ending
    /// in it, such as `not_child_turn::take()`. A lone `{` or `}` in a
    /// literal, a character, a comment, or an attribute before the `fn`
    /// moves where a test's body starts or ends, and can hide some or all of
    /// its calls. Those stay with review. Tests elsewhere in this binary
    /// cannot reach the turn, and none of them starts a child.
    #[test]
    fn every_broker_test_with_a_child_takes_the_turn_before_it() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = vec![src.join("broker.rs")];
        for entry in fs::read_dir(src.join("broker")).expect("broker sources") {
            let path = entry.expect("broker source entry").path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
        let mut with_a_child = Vec::new();
        let mut without_the_turn = Vec::new();
        for path in sources {
            let source = fs::read_to_string(&path).expect("broker source");
            let code: String = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .flat_map(str::chars)
                .filter(|character| !character.is_whitespace())
                .collect();
            for (name, body) in test_bodies(&code) {
                let Some(first) = CHILD_CALLS.iter().filter_map(|call| body.find(call)).min()
                else {
                    continue;
                };
                with_a_child.push(name.to_owned());
                if body.find(TAKE).is_none_or(|take| take > first) {
                    without_the_turn.push(format!("{}: {name}", path.display()));
                }
            }
        }
        for known in [
            "child_filter_denies_tcp_and_unix_bind",
            "reaping_returns_the_workload_status_and_discards_other_zombies",
        ] {
            assert!(
                with_a_child.iter().any(|name| name == known),
                "the check no longer sees {known} start or reap a child"
            );
        }
        assert!(
            without_the_turn.is_empty(),
            "these tests start or reap a child before taking the turn: \
             {without_the_turn:#?}"
        );
    }

    /// The name and body of every function marked `#[test]` in `code`, which
    /// has had its whitespace removed.
    fn test_bodies(code: &str) -> Vec<(&str, &str)> {
        code.match_indices("#[test]")
            .filter_map(|(at, _)| {
                let rest = code.get(at..)?;
                let open = rest.find('{')?;
                let (_, signature) = rest.get(..open)?.split_once("fn")?;
                let name = signature.split('(').next()?;
                Some((name, braced(rest.get(open..)?)))
            })
            .collect()
    }

    /// `code` from its opening brace through the brace that closes it, or all
    /// of it when none does.
    fn braced(code: &str) -> &str {
        let mut depth = 0_usize;
        for (at, character) in code.char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return code.get(..=at).unwrap_or(code);
                    }
                }
                _ => {}
            }
        }
        code
    }
}
