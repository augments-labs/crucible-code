//! Which names `--continue` and `--resume` read as a log.
//!
//! A name in the sessions directory is still a name anything that can write
//! there could have put a link or a pipe under. A link would continue whatever
//! it leads to, out of the directory, and append the next turn to it; a pipe
//! would hold the start until something wrote to it. Both are refused the way a
//! log that will not open is, and a log with a second hard name is not.

use super::*;

/// One of this sample's workspace's sessions, recorded under `id` with one
/// prompt in it.
fn planted(sample: &Sample, id: &str, asked: &str) -> PathBuf {
    sample.plant(
        id,
        &[sample.header(wire::FORMAT, id), wire::line(&said(asked))],
    )
}

/// The newest name this file plants, so `--continue` reaches it first.
const NEWER: &str = "01941f29-7c00-7000-8000-000000000000";

/// An older session of the same workspace, which a refusal must not stand in
/// for.
const OLDER: &str = "01887441-0c00-7000-8000-000000000000";

/// Moves the log `id` out of the sessions directory and leaves a link to it
/// under its name, returning where the log now is.
#[cfg(unix)]
fn linked_out(sample: &Sample, id: &str) -> PathBuf {
    let named = planted(sample, id, "read through a link");
    fs::create_dir_all(sample.home()).expect("a home");
    let outside = sample.home().join(format!("{id}.jsonl"));
    fs::rename(&named, &outside).expect("a log outside the sessions directory");
    std::os::unix::fs::symlink(&outside, &named).expect("a link");
    outside
}

/// Leaves a pipe in the sessions directory under the log name `id`.
#[cfg(unix)]
fn piped(sample: &Sample, id: &str) -> PathBuf {
    let named = sample.logs().join(format!("{id}.jsonl"));
    let made = std::process::Command::new("mkfifo")
        .arg(&named)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    named
}

/// Runs `continuing` on a thread of its own, so one that waits on a pipe fails
/// the test rather than holding it forever.
#[cfg(unix)]
fn within_a_bound<T: Send + 'static>(continuing: impl FnOnce() -> T + Send + 'static) -> T {
    let (send, came) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(continuing());
    });
    came.recv_timeout(std::time::Duration::from_secs(10))
        .expect("an answer that came back rather than waiting on a pipe")
}

/// Where `--continue` took the session from, or why it refused.
fn continued(sample: &Sample) -> Result<PathBuf, SessionError> {
    Session::resume(&sample.logs(), &sample.workspace()).map(|(session, _)| session.path().into())
}

#[cfg(unix)]
#[test]
fn a_log_that_is_a_link_is_refused_by_continue_rather_than_followed() {
    let sample = Sample::new("ordinary-continue-link");
    planted(&sample, OLDER, "the real one");
    let outside = linked_out(&sample, NEWER);
    let before = fs::read(&outside).expect("the log outside");

    let outcome = continued(&sample);

    assert!(
        matches!(&outcome, Err(SessionError::Log { at, .. }) if at.contains(NEWER)),
        "{outcome:?}"
    );
    assert_eq!(fs::read(&outside).expect("the log outside"), before);
}

#[cfg(unix)]
#[test]
fn a_log_that_is_a_pipe_is_refused_by_continue_without_waiting_for_a_writer() {
    let sample = Sample::new("ordinary-continue-pipe");
    planted(&sample, OLDER, "the real one");
    piped(&sample, NEWER);
    let logs = sample.logs();
    let workspace = sample.workspace();

    let outcome = within_a_bound(move || {
        Session::resume(&logs, &workspace).map(|(session, _)| session.path().to_owned())
    });

    assert!(
        matches!(&outcome, Err(SessionError::Log { at, .. }) if at.contains(NEWER)),
        "{outcome:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_log_that_is_a_link_is_refused_by_resume_rather_than_followed() {
    let sample = Sample::new("ordinary-resume-link");
    let outside = linked_out(&sample, NEWER);
    let before = fs::read(&outside).expect("the log outside");
    let id = SessionId::from_str(NEWER).expect("a well-formed session id");

    let outcome = Session::reopen(&sample.logs(), &sample.workspace(), &id)
        .map(|(session, _)| session.path().to_owned());

    assert!(
        matches!(&outcome, Err(SessionError::Log { at, .. }) if at.contains(NEWER)),
        "{outcome:?}"
    );
    assert_eq!(fs::read(&outside).expect("the log outside"), before);
}

/// The replay itself refuses a pipe, whatever was asked of the name before
/// it: the header read and the replay are two opens of one name.
#[cfg(unix)]
#[test]
fn a_replay_of_a_pipe_is_refused_without_waiting_for_a_writer() {
    let sample = Sample::new("ordinary-replay-pipe");
    let named = piped(&sample, NEWER);

    let outcome = within_a_bound(move || super::super::replay(&named).map(drop));

    assert!(
        matches!(&outcome, Err(SessionError::Log { at, .. }) if at.contains(NEWER)),
        "{outcome:?}"
    );
}

/// A backup made with hard links gives every log a second name, and that is
/// no reason for `--continue` to stop finding it.
#[test]
fn a_log_with_a_second_name_is_still_continued() {
    let sample = Sample::new("ordinary-continue-hard-link");
    let named = planted(&sample, NEWER, "kept under two names");
    fs::hard_link(&named, sample.logs().join(format!("{NEWER}.jsonl.kept")))
        .expect("a second name for the log");

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("kept under two names")]);
}
