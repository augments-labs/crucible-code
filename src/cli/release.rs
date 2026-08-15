//! Whether there is a release newer than the one running.
//!
//! Two halves that never wait for each other. What is drawn under the welcome
//! comes off the disk, because the startup path is budgeted in milliseconds and
//! a socket is not something that fits in one; the question is put to GitHub on
//! a thread of its own, and its answer is what the *next* run draws. So the
//! first run after a release is quiet and the one after it says so, which is a
//! day late and costs the startup path nothing.
//!
//! Nothing here decides whether to ask. That is `updates.check` in the
//! configuration, read by the caller, because a program reaching a server it
//! was not asked to reach is the kind of thing that has to be refusable in one
//! line of a file.

use std::io::Read as _;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime};

use crucible_provider::Https;

/// Where the newest release is named.
///
/// The API rather than the page, because a page is layout and this is a fact.
const LATEST: &str = "https://api.github.com/repos/augments-labs/crucible-code/releases/latest";

/// Where somebody goes to get it. Drawn beside the version, because crucible
/// installs by download and there is no command that would update it.
const FROM: &str = "https://github.com/augments-labs/crucible-code/releases";

/// What the answer is kept in, under crucible's own directory.
const REMEMBERED: &str = "release";

/// How long an answer stands before it is worth asking again.
///
/// A release is a thing that happens a few times a month. Asking each start
/// would be a request per terminal window for an answer that changes weekly.
const ASK_AFTER: Duration = Duration::from_hours(24);

/// The absolute lifetime of release discovery, including its response body.
const CHECK_LIFETIME: Duration = Duration::from_secs(10);

/// The most of a response that is read before giving up on it.
///
/// The body is a few kilobytes of JSON. The ceiling is what stops a server that
/// answers with a stream from being read until this process runs out of memory.
const CEILING: u64 = 256 * 1024;

/// A release this machine has heard of and is not running.
#[derive(Debug, Clone)]
pub(crate) struct Newer {
    /// What it is called, without the `v` a tag carries.
    pub(crate) version: Box<str>,
    /// Where to get it.
    pub(crate) from: Box<str>,
}

/// What this machine last heard, where that is newer than what is running.
///
/// One small read on the startup path and no network at all. A file that is not
/// there, cannot be read, or names something no newer is all the same answer:
/// nothing to say.
pub(crate) fn newer(home: &Path, running: &str) -> Option<Newer> {
    let said = std::fs::read_to_string(home.join(REMEMBERED)).ok()?;
    let version = said.lines().next()?.trim();

    beyond(version, running).then(|| Newer {
        version: version.into(),
        from: FROM.into(),
    })
}

/// Asks again, on a thread of its own, when what is on disk is old enough.
///
/// Detached on purpose. Nothing is waiting for the answer — it is for the next
/// run — so this returns before the socket is even opened, and a process that
/// ends first simply leaves the file as it was.
pub(crate) fn refresh(home: &Path) {
    if !stale(home) {
        return;
    }

    let into = home.join(REMEMBERED);
    thread::Builder::new()
        .name("release".to_owned())
        .spawn(move || {
            // An answer that never came is still an answer asked for, and the
            // file is what records that: a machine with no network would
            // otherwise open a socket on every start for ever. What was already
            // known is written again rather than replaced — a release that
            // exists does not stop existing because the network was down, and
            // this release stands in only where nothing was known at all.
            let version = asked().unwrap_or_else(|| known(&into));

            remember(&into, &version);
        })
        .ok();
}

/// The release already written down, or this one where nothing is.
fn known(at: &Path) -> Box<str> {
    let said = std::fs::read_to_string(at).unwrap_or_default();
    let version = said.lines().next().unwrap_or_default().trim();

    if version.is_empty() {
        return env!("CARGO_PKG_VERSION").into();
    }

    version.into()
}

/// Whether the answer on disk is old enough to be worth asking again.
///
/// A file that is not there has never been asked for, which is the one case
/// that has to count as stale.
fn stale(home: &Path) -> bool {
    let asked = std::fs::metadata(home.join(REMEMBERED))
        .and_then(|about| about.modified())
        .ok();

    asked.is_none_or(|asked| {
        SystemTime::now()
            .duration_since(asked)
            .is_ok_and(|since| since >= ASK_AFTER)
    })
}

/// What GitHub says the newest release is called.
fn asked() -> Option<Box<str>> {
    // The variant that names the program, because GitHub refuses a request
    // that names nothing, and a version in it is what makes a request from an
    // old build tellable from a new one in somebody else's logs.
    let agent = format!("crucible-code/{}", env!("CARGO_PKG_VERSION"));
    let headers = [
        ("accept", "application/vnd.github+json"),
        ("user-agent", agent.as_str()),
    ];

    let response = Https::new().get(LATEST, &headers, CHECK_LIFETIME).ok()?;
    if response.status != 200 {
        return None;
    }

    let mut body = String::new();
    response.body.take(CEILING).read_to_string(&mut body).ok()?;

    let said: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = said.get("tag_name")?.as_str()?;

    Some(tag.trim().trim_start_matches('v').into())
}

/// Writes the answer down, whole or not at all.
///
/// Through a file beside it and a rename, because this runs on a thread the
/// process may end under: a half-written name read back next time would be a
/// version that was never released.
fn remember(into: &Path, version: &str) {
    let beside = into.with_extension("writing");

    if std::fs::write(&beside, format!("{version}\n")).is_ok()
        && std::fs::rename(&beside, into).is_err()
    {
        let _ = std::fs::remove_file(&beside);
    }
}

/// Whether `offered` is a later release than `running`.
///
/// Compared number by number rather than as text, which is the whole reason
/// this is not `!=`: `0.0.10` sorts before `0.0.9` as a string, and a user on
/// the newer of the two would be told for ever that an older one is available.
///
/// Anything after a `-` is dropped, so a pre-release is treated as its version.
/// A part that is not a number stops the comparison, and a name that is not
/// shaped like a version is not newer than anything.
fn beyond(offered: &str, running: &str) -> bool {
    let numbers = |version: &str| -> Vec<u64> {
        version
            .split('-')
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|part| part.trim().parse().unwrap_or(0))
            .collect()
    };

    let (offered, running) = (numbers(offered), numbers(running));
    if offered.iter().all(|part| *part == 0) {
        return false;
    }

    offered > running
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A directory of its own, deleted with the value.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let at = std::env::temp_dir()
                .join(format!("crucible-release-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&at);
            std::fs::create_dir_all(&at).expect("a temporary directory");

            Self(at)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_release_is_compared_number_by_number_and_not_as_text() {
        // The bug this exists to prevent: `0.0.10` is *before* `0.0.9` as text,
        // so a user who had upgraded would be told to upgrade for ever.
        assert!(beyond("0.0.10", "0.0.9"));
        assert!(!beyond("0.0.9", "0.0.10"));

        assert!(beyond("0.1.0", "0.0.9"));
        assert!(beyond("1.0.0", "0.9.9"));
        assert!(!beyond("0.0.9", "0.0.9"));
        assert!(!beyond("0.0.8", "0.0.9"));
    }

    #[test]
    fn a_pre_release_is_read_as_the_version_it_leads_to() {
        assert!(beyond("0.1.0-rc.1", "0.0.9"));
        assert!(!beyond("0.0.9-rc.1", "0.0.9"));
    }

    #[test]
    fn a_name_that_is_not_a_version_is_newer_than_nothing() {
        // A tag that does not parse reaches here as zeroes, and zeroes are
        // newer than nothing rather than newer than everything.
        for said in ["", "nightly", "latest"] {
            assert!(!beyond(said, "0.0.9"), "{said:?}");
        }
    }

    #[test]
    fn nothing_on_disk_is_nothing_to_say() {
        let scratch = Scratch::new("silent");

        assert!(newer(&scratch.0, "0.0.9").is_none());
    }

    #[test]
    fn a_release_remembered_is_read_back_and_drawn() {
        let scratch = Scratch::new("heard");
        remember(&scratch.0.join(REMEMBERED), "0.1.0");

        let found = newer(&scratch.0, "0.0.9").expect("a newer release");

        assert_eq!(&*found.version, "0.1.0");
        assert_eq!(&*found.from, FROM);
    }

    #[test]
    fn the_release_being_run_is_not_an_update() {
        let scratch = Scratch::new("current");
        remember(&scratch.0.join(REMEMBERED), "0.0.9");

        assert!(newer(&scratch.0, "0.0.9").is_none());
    }

    #[test]
    fn an_answer_just_written_is_not_asked_for_again() {
        let scratch = Scratch::new("fresh");
        remember(&scratch.0.join(REMEMBERED), "0.0.9");

        assert!(!stale(&scratch.0), "a file written now is not stale");
    }

    #[test]
    fn a_release_already_known_survives_a_question_that_could_not_be_asked() {
        // A machine that went offline for a day would otherwise be told the
        // release it is behind no longer exists.
        let scratch = Scratch::new("offline");
        remember(&scratch.0.join(REMEMBERED), "9.9.9");

        assert_eq!(&*known(&scratch.0.join(REMEMBERED)), "9.9.9");
    }

    #[test]
    fn a_machine_that_has_never_heard_an_answer_stands_on_this_release() {
        let scratch = Scratch::new("unheard");

        assert_eq!(
            &*known(&scratch.0.join(REMEMBERED)),
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn a_machine_that_has_never_asked_is_stale() {
        let scratch = Scratch::new("never");

        assert!(stale(&scratch.0));
    }
}
