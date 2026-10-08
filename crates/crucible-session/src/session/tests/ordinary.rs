//! Which names in the sessions directory are read, and what a link or a pipe
//! under one costs.
//!
//! A name in the sessions directory is still a name anything that can write
//! there could have put a link or a pipe under. A link would continue whatever
//! it leads to, out of the directory, and append the next turn to it; a pipe
//! would hold the start until something wrote to it. Both are refused the way a
//! log that will not open is, and a log with a second hard name is not. The
//! files beside the logs — the index, its mark, the prompt history and a
//! deferred call's result — are read the same way, each refusing a link or a
//! pipe as it refuses a file of its own that will not open.

#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
use super::super::{Roots, discovered, index, prompts, remember, results};
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
#[cfg(unix)]
const OLDER: &str = "01887441-0c00-7000-8000-000000000000";

/// Moves the log `id` out of the sessions directory and leaves a link to it
/// under its name, returning where the log now is.
#[cfg(unix)]
fn linked_out(sample: &Sample, id: &str) -> PathBuf {
    let named = planted(sample, id, "read through a link");
    moved_out(sample, &named)
}

/// Moves the file `named` out of the sessions directory and leaves a link to
/// it under its name, returning where the file now is.
#[cfg(unix)]
fn moved_out(sample: &Sample, named: &Path) -> PathBuf {
    fs::create_dir_all(sample.home()).expect("a home");
    let outside = sample.home().join(named.file_name().expect("a file name"));
    fs::rename(named, &outside).expect("a file outside the sessions directory");
    std::os::unix::fs::symlink(&outside, named).expect("a link");
    outside
}

/// Leaves a pipe in the sessions directory under the log name `id`.
#[cfg(unix)]
fn piped(sample: &Sample, id: &str) -> PathBuf {
    let named = sample.logs().join(format!("{id}.jsonl"));
    pipe_at(&named);
    named
}

/// Leaves a pipe at `named`, wherever something was before it.
#[cfg(unix)]
fn pipe_at(named: &Path) {
    let _ = fs::remove_file(named);
    let made = std::process::Command::new("mkfifo")
        .arg(named)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
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
#[cfg(unix)]
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

/// The ids the index holds, newest first.
#[cfg(unix)]
fn indexed(sample: &Sample) -> Vec<String> {
    index::written(&sample.logs(), index::ENTRIES)
        .expect("an index that reads")
        .expect("an index")
        .into_iter()
        .map(|entry| entry.id.as_str().to_owned())
        .collect()
}

/// What `crucible sessions list` is handed for this sample's workspace, asked
/// on a thread of its own so a listing that waits on a pipe fails the test.
#[cfg(unix)]
fn listed(sample: &Sample) -> Result<crate::session::Discovery, SessionError> {
    let logs = sample.logs();
    let root = sample.workspace().root().to_path_buf();
    within_a_bound(move || discovered(&logs, Roots::These(&[root.as_path()]), 8))
}

/// An index under a pipe would hold the listing, and the welcome screen before
/// its first frame, until something wrote to it.
#[cfg(unix)]
#[test]
fn an_index_that_is_a_pipe_is_refused_without_waiting_for_a_writer() {
    let sample = Sample::new("ordinary-index-pipe");
    planted(&sample, NEWER, "the real one");
    pipe_at(&sample.logs().join("recent.sessions"));

    let outcome = listed(&sample);

    assert!(
        matches!(&outcome, Err(SessionError::Index { at, .. }) if at.contains("recent.sessions")),
        "{outcome:?}"
    );
}

/// An index under a link would list whatever sessions the file it leads to
/// names, as though they were this directory's.
#[cfg(unix)]
#[test]
fn an_index_that_is_a_link_is_refused_rather_than_followed() {
    let sample = Sample::new("ordinary-index-link");
    planted(&sample, NEWER, "the real one");
    index::ensure(&sample.logs()).expect("an index");
    moved_out(&sample, &sample.logs().join("recent.sessions"));

    let outcome = listed(&sample);

    assert!(
        matches!(&outcome, Err(SessionError::Index { at, .. }) if at.contains("recent.sessions")),
        "{outcome:?}"
    );
}

/// A mark that vouches for the index through a link would let a file outside
/// the directory say the index is in order, and leave a session out of it.
#[cfg(unix)]
#[test]
fn a_mark_that_is_a_link_vouches_for_nothing() {
    let sample = Sample::new("ordinary-mark-link");
    planted(&sample, OLDER, "indexed");
    index::ensure(&sample.logs()).expect("an index");
    planted(&sample, NEWER, "not indexed yet");
    moved_out(&sample, &sample.logs().join("recent.sessions.ordered"));

    index::ensure(&sample.logs()).expect("an index merged again");

    assert_eq!(indexed(&sample), [NEWER, OLDER]);
}

/// A mark under a pipe would hold every start, which asks the mark first,
/// until something wrote to it.
#[cfg(unix)]
#[test]
fn a_mark_that_is_a_pipe_vouches_for_nothing_without_waiting_for_a_writer() {
    let sample = Sample::new("ordinary-mark-pipe");
    planted(&sample, OLDER, "indexed");
    index::ensure(&sample.logs()).expect("an index");
    planted(&sample, NEWER, "not indexed yet");
    pipe_at(&sample.logs().join("recent.sessions.ordered"));
    let logs = sample.logs();

    let outcome = within_a_bound(move || index::ensure(&logs));

    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(indexed(&sample), [NEWER, OLDER]);
}

/// The history is read before the first frame, so a pipe under its name would
/// hold the start until something wrote to it.
#[cfg(unix)]
#[test]
fn a_prompt_history_that_is_a_pipe_is_refused_without_waiting_for_a_writer() {
    let sample = Sample::new("ordinary-prompts-pipe");
    pipe_at(&sample.logs().join("prompt.history"));
    let logs = sample.logs();
    let workspace = sample.workspace();

    let outcome = within_a_bound(move || prompts(&logs, &workspace));

    assert!(
        matches!(&outcome, Err(SessionError::History { at, .. }) if at.contains("prompt.history")),
        "{outcome:?}"
    );
}

/// A history under a link would offer back whatever lines the file it leads
/// to holds, as though they had been typed here.
#[cfg(unix)]
#[test]
fn a_prompt_history_that_is_a_link_is_refused_rather_than_followed() {
    let sample = Sample::new("ordinary-prompts-link");
    let workspace = sample.workspace();
    remember(&sample.logs(), &workspace, "read through a link").expect("a history");
    moved_out(&sample, &sample.logs().join("prompt.history"));

    let outcome = prompts(&sample.logs(), &workspace);

    assert!(
        matches!(&outcome, Err(SessionError::History { at, .. }) if at.contains("prompt.history")),
        "{outcome:?}"
    );
}

/// A session with one durable call result, and where that result is kept.
#[cfg(unix)]
fn deferred(sample: &Sample) -> (Session, PathBuf) {
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    session.append(&calling("call-1", "bash", "{}"));
    let key = CallResultKey::derive(Ancestry::new(), InvocationId::new(), &ToolId::new("call-1"));
    let result = ToolResult {
        id: ToolId::new("call-1"),
        output: RecordedToolOutput::ok("background job #1 accepted"),
    };
    crucible_runtime::answered!(session.put_call_result(key, &result)).expect("a durable result");
    let [record] = results::load(session.path())
        .expect("the result reads")
        .into_iter()
        .map(|stored| stored.path)
        .collect::<Vec<_>>()
        .try_into()
        .expect("one durable result");
    (session, record)
}

/// A result read through a link would be folded into the transcript from
/// wherever it leads.
#[cfg(unix)]
#[test]
fn a_durable_result_that_is_a_link_is_refused_rather_than_followed() {
    let sample = Sample::new("ordinary-result-link");
    let (session, record) = deferred(&sample);
    moved_out(&sample, &record);

    assert!(results::load(session.path()).is_err());
}

/// A result under a pipe would hold the replay until something wrote to it.
#[cfg(unix)]
#[test]
fn a_durable_result_that_is_a_pipe_is_refused_without_waiting_for_a_writer() {
    let sample = Sample::new("ordinary-result-pipe");
    let (session, record) = deferred(&sample);
    pipe_at(&record);
    let log = session.path().to_owned();

    let outcome = within_a_bound(move || results::load(&log).map(drop));

    assert!(outcome.is_err());
}
