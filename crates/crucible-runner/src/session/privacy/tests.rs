//! Who can reach the session tree.
//!
//! Split the way the module is, because the two platforms do not have the same
//! thing to assert. A mode is a number the test can read back; a list is not.

use std::path::PathBuf;

use crucible_core::Message;

use crate::sample::Sample;
use crate::session::Session;

/// Records one message in a fresh session and returns where it was written.
fn record(sample: &Sample) -> PathBuf {
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let path = session.path().to_owned();
    session.append(&Message::User("hello".into()));

    // Dropping is what waits for the queue, so the file is complete after it.
    drop(session);
    path
}

/// What a file mode says.
#[cfg(unix)]
mod mode {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use super::record;
    use crate::sample::Sample;
    use crate::session::Session;

    #[test]
    fn a_log_is_readable_only_by_the_user_who_started_it() {
        // A transcript carries what was typed, the contents of files that were
        // read and everything a command printed. On a shared machine the default
        // 0644 hands all of it to anyone with an account.
        let sample = Sample::new("session-mode");
        let path = record(&sample);

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "log is {:o}", mode & 0o777);

        let directory = fs::metadata(sample.logs()).unwrap().permissions().mode();
        assert_eq!(directory & 0o077, 0, "directory is {:o}", directory & 0o777);
    }

    #[test]
    fn a_sessions_directory_left_open_is_narrowed_before_anything_is_written_to_it() {
        // `DirBuilderExt::mode` applies only when the call creates the directory, so
        // one made by an earlier build — or by a hand running `mkdir -p` — keeps
        // whatever the umask gave it, and every log written into it afterwards sits
        // somewhere the whole machine can list.
        let sample = Sample::new("session-widened-directory");
        fs::set_permissions(sample.logs(), fs::Permissions::from_mode(0o755))
            .expect("a temporary dir");

        record(&sample);

        let mode = fs::metadata(sample.logs()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "directory is {:o}", mode & 0o777);
    }

    #[test]
    fn a_sessions_directory_left_open_is_narrowed_when_a_session_is_continued() {
        // `start` is not the only way in. A run that goes straight to `--continue`
        // never reaches it, so a directory widened since the last session stays
        // widened for the whole of this one — and this is the path that goes
        // looking in it, so another account's log is what `--continue` might find.
        let sample = Sample::new("session-widened-directory-continued");
        record(&sample);
        fs::set_permissions(sample.logs(), fs::Permissions::from_mode(0o755))
            .expect("a temporary dir");

        let (session, _) =
            Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
        drop(session);

        let mode = fs::metadata(sample.logs()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "directory is {:o}", mode & 0o777);
    }

    #[test]
    fn a_sessions_directory_too_tight_to_use_is_repaired_rather_than_left_broken() {
        // Nothing is exposed by a directory the owner cannot write to, so it is not
        // a security problem — it is a session that cannot start, reported against
        // the log rather than against the directory that actually refused.
        let sample = Sample::new("session-tight-directory");
        fs::set_permissions(sample.logs(), fs::Permissions::from_mode(0o500))
            .expect("a temporary dir");

        record(&sample);

        let mode = fs::metadata(sample.logs()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "directory is {:o}", mode & 0o777);
    }

    #[test]
    fn a_mode_that_is_already_right_is_not_written_again() {
        // The whole point of reading the mode first is the `chmod` that then does
        // not happen — on a filesystem carrying no Unix modes that call fails, and
        // a startup dying over a permission which is already correct helps nobody.
        // A `chmod` leaves no trace a test can read, so the sticky bit stands in for
        // it: writing 0700 over this directory would take it off.
        let sample = Sample::new("session-untouched-directory");
        fs::set_permissions(sample.logs(), fs::Permissions::from_mode(0o1700))
            .expect("a temporary dir");

        record(&sample);

        let mode = fs::metadata(sample.logs()).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o1700, "directory is {:o}", mode & 0o7777);
    }

    #[test]
    fn a_log_left_open_is_narrowed_when_it_is_continued() {
        // Same for the log: `OpenOptionsExt::mode` says nothing about a file that
        // already exists, and `--continue` is exactly the path that opens one.
        let sample = Sample::new("session-widened-log");
        let path = record(&sample);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("a temporary file");

        let (session, _) =
            Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
        drop(session);

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "log is {:o}", mode & 0o777);
    }
}

/// What an access control list says.
///
/// Which accounts are on the list is what one would like to assert, and reading
/// it back to find out means a second FFI call in a test — proving the call
/// under test by making the same call again. So these assert the half that a
/// second call would not catch anyway: a list with the wrong access mask on it
/// shuts this account out of its own transcript, and nothing about the call that
/// wrote it says so. Both of these fail outright when that happens.
#[cfg(windows)]
mod list {
    use std::fs;

    use super::record;
    use crate::sample::Sample;
    use crate::session::Session;

    #[test]
    fn a_log_stays_readable_by_the_account_that_wrote_it() {
        let sample = Sample::new("session-list");
        let path = record(&sample);

        let held = fs::read_to_string(&path).expect("the log this account just wrote");
        assert!(held.contains("hello"), "log holds {held}");

        let entries = fs::read_dir(sample.logs()).expect("the directory this account just made");
        assert_eq!(entries.count(), 1);
    }

    #[test]
    fn a_log_stays_appendable_by_the_account_that_wrote_it() {
        // `--continue` opens the log for append and shortens it to what replay
        // reached, so a list granting read alone leaves a session that can be
        // started once and never continued.
        let sample = Sample::new("session-list-continued");
        record(&sample);

        let (session, transcript) =
            Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
        drop(session);

        assert_eq!(transcript.len(), 1);
    }
}
