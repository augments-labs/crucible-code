use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// A key nobody would mistake for a real one, and the thing every leak test
/// greps for.
const SECRET: &str = "sk-do-not-log-me";

/// A tree that exists while the test does.
///
/// This crate's job is a file on a disk at a mode, so a fake filesystem would
/// only be testing the fake's answer to those questions.
struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("crucible-auth-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("a writable temporary directory");

        Self { base }
    }

    fn home(&self) -> &Path {
        &self.base
    }

    /// Puts a store on the disk without going through one, so a test can state
    /// the bytes it is about rather than the calls that would produce them.
    fn holding(&self, text: &str) -> Store {
        fs::write(self.base.join(FILE), text).expect("a writable temporary directory");

        Store::in_home(self.home())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// The key a store holds for `provider`, as text, for a test that has to
/// compare one — nothing outside this module can read a key back out.
fn held(store: &Store, provider: &str) -> Option<String> {
    let mut request = crucible_core::Outgoing::new();
    let header = crucible_core::Header::bare("x-api-key");
    store.read().get(provider)?.apply(&mut request, &header);

    request
        .headers()
        .iter()
        .find(|(name, _)| &**name == "x-api-key")
        .map(|(_, value)| value.to_string())
}

fn subscription(canary: &str) -> Tokens {
    Tokens::new(
        format!("access-{canary}").into(),
        format!("refresh-{canary}").into(),
        u64::MAX,
        1,
    )
    .with_detail("account_id", format!("account-{canary}"))
}

#[test]
fn a_key_in_the_file_is_the_key_read_back() {
    let scratch = Scratch::new("round-trip");
    let store = scratch.holding(&format!(
        r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#
    ));

    assert_eq!(held(&store, "openai").as_deref(), Some(SECRET));
}

#[test]
fn a_store_that_is_not_there_holds_nothing_and_says_nothing() {
    let scratch = Scratch::new("absent");
    let keys = Store::in_home(scratch.home()).read();

    assert_eq!(keys.providers().count(), 0);
    assert_eq!(
        keys.trouble(),
        None,
        "never having logged in is not a problem to report"
    );
}

#[test]
fn every_provider_logged_in_is_listed_in_name_order() {
    let scratch = Scratch::new("listed");
    let store =
        scratch.holding(r#"{"version":1,"keys":{"openai":"a","anthropic":"b","moonshot":"c"}}"#);

    let keys = store.read();
    let listed: Vec<_> = keys.providers().collect();
    assert_eq!(listed, ["anthropic", "moonshot", "openai"]);
}

#[test]
fn a_store_nobody_can_parse_reports_it_and_still_starts() {
    let scratch = Scratch::new("malformed");
    let store = scratch.holding("{not json at all");

    let keys = store.read();

    assert_eq!(keys.providers().count(), 0, "no stored credential was read");
    let said = keys.trouble().expect("a sentence for the user");
    assert!(
        said.contains(FILE),
        "the sentence has to name the file: {said}"
    );
}

#[test]
fn one_non_text_key_refuses_the_whole_store_and_is_never_rewritten() {
    let scratch = Scratch::new("non-text-key");
    let written = format!(r#"{{"version":1,"keys":{{"openai":"{SECRET}","moonshot":17}}}}"#);
    let store = scratch.holding(&written);

    let keys = store.read();
    assert_eq!(keys.providers().count(), 0, "a partial store was accepted");
    assert!(
        keys.trouble().is_some(),
        "the malformed key was not reported"
    );
    assert!(
        matches!(
            store.keep("anthropic", SECRET),
            Err(AuthError::Unreadable { .. })
        ),
        "writing over the part this build could not read was allowed"
    );
    assert_eq!(
        fs::read_to_string(scratch.home().join(FILE)).unwrap(),
        written
    );
}

#[test]
fn a_store_over_the_byte_bound_is_refused_before_it_can_choose_an_allocation() {
    let scratch = Scratch::new("too-large");
    let written = "x".repeat(MAX_STORE + 1);
    let store = scratch.holding(&written);

    let keys = store.read();
    assert_eq!(keys.providers().count(), 0);
    assert!(
        keys.trouble()
            .is_some_and(|said| said.contains(&MAX_STORE.to_string())),
        "the user was not told which bound was reached"
    );
    assert!(matches!(
        store.keep("openai", SECRET),
        Err(AuthError::TooLarge {
            maximum: MAX_STORE,
            ..
        })
    ));
    assert_eq!(
        fs::read_to_string(scratch.home().join(FILE)).unwrap(),
        written
    );
}

#[test]
fn a_store_from_a_later_version_is_left_alone_rather_than_guessed_at() {
    let scratch = Scratch::new("newer");
    let written = format!(r#"{{"version":99,"keys":{{"openai":"{SECRET}"}}}}"#);
    let store = scratch.holding(&written);

    let keys = store.read();

    assert_eq!(keys.providers().count(), 0, "no stored credential was read");
    assert!(keys.trouble().is_some(), "and the user is told why");
    assert_eq!(
        fs::read_to_string(scratch.home().join(FILE)).expect("still there"),
        written,
        "reading never rewrites what it could not understand"
    );
}

#[test]
fn a_store_that_does_not_say_its_version_is_not_guessed_at_either() {
    let scratch = Scratch::new("unversioned");
    let store = scratch.holding(&format!(r#"{{"keys":{{"openai":"{SECRET}"}}}}"#));

    let keys = store.read();

    assert_eq!(keys.providers().count(), 0, "no stored credential was read");
    assert!(keys.trouble().is_some(), "and the user is told why");
}

#[test]
fn a_key_appears_in_no_debug_output_anywhere() {
    let scratch = Scratch::new("redacted");
    let store = scratch.holding(&format!(
        r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#
    ));

    let keys = store.read();
    for printed in [format!("{keys:?}"), format!("{store:?}")] {
        assert!(
            !printed.contains(SECRET),
            "a key reached a Debug line: {printed}"
        );
    }
}

#[test]
fn a_key_written_down_is_the_key_read_back() {
    let scratch = Scratch::new("keep");
    let store = Store::in_home(scratch.home());

    store.keep("openai", SECRET).expect("a writable home");

    assert_eq!(held(&store, "openai").as_deref(), Some(SECRET));
}

#[test]
fn a_version_one_store_is_migrated_without_losing_a_key() {
    let scratch = Scratch::new("migrate-v1");
    let store = scratch.holding(&format!(
        r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#
    ));

    store.keep("moonshot", "moonshot-key").unwrap();

    assert_eq!(held(&store, "openai").as_deref(), Some(SECRET));
    assert_eq!(held(&store, "moonshot").as_deref(), Some("moonshot-key"));
    let written = fs::read_to_string(scratch.home().join(FILE)).unwrap();
    assert!(written.contains(r#""version":2"#), "{written}");
    assert!(written.contains(r#""subscriptions":{}"#), "{written}");
}

#[test]
fn selecting_a_subscription_replaces_only_that_providers_key() {
    let scratch = Scratch::new("subscription-over-key");
    let store = Store::in_home(scratch.home());
    store.keep("openai", SECRET).unwrap();
    store.keep("moonshot", "moonshot-key").unwrap();

    store
        .keep_subscription("openai", subscription("do-not-show"))
        .unwrap();

    let keys = store.read();
    assert!(keys.has("openai"));
    assert!(keys.get("openai").is_none());
    assert_eq!(held(&store, "moonshot").as_deref(), Some("moonshot-key"));
    assert!(!format!("{keys:?}").contains("do-not-show"));
}

#[test]
fn selecting_an_api_key_replaces_only_that_providers_subscription() {
    let scratch = Scratch::new("key-over-subscription");
    let store = Store::in_home(scratch.home());
    store
        .keep_subscription("openai", subscription("old"))
        .unwrap();

    store.keep("openai", SECRET).unwrap();

    let keys = store.read();
    assert_eq!(held(&store, "openai").as_deref(), Some(SECRET));
    assert!(!keys.has_subscription("openai"));
}

#[test]
fn forgetting_a_provider_removes_its_subscription() {
    let scratch = Scratch::new("forget-subscription");
    let store = Store::in_home(scratch.home());
    store
        .keep_subscription("openai", subscription("forgotten"))
        .unwrap();

    assert!(store.forget("openai").unwrap());
    assert!(!store.read().has("openai"));
}

#[test]
fn a_malformed_subscription_refuses_the_complete_store() {
    let scratch = Scratch::new("bad-subscription");
    let store = scratch.holding(
        r#"{"version":2,"keys":{"moonshot":"kept"},"subscriptions":{"openai":{"access_token":"only-one-field"}}}"#,
    );

    let keys = store.read();
    assert_eq!(keys.providers().count(), 0);
    assert!(keys.trouble().is_some());
    assert!(matches!(
        store.keep("anthropic", SECRET),
        Err(AuthError::Unreadable { .. })
    ));
}

#[test]
fn one_provider_replaces_its_own_key_and_leaves_the_others_alone() {
    let scratch = Scratch::new("replace");
    let store = Store::in_home(scratch.home());

    store.keep("openai", "first").expect("a writable home");
    store.keep("moonshot", SECRET).expect("a writable home");
    store.keep("openai", "second").expect("a writable home");

    assert_eq!(held(&store, "openai").as_deref(), Some("second"));
    assert_eq!(held(&store, "moonshot").as_deref(), Some(SECRET));
}

#[test]
fn forgetting_removes_one_provider_and_reports_whether_there_was_one() {
    let scratch = Scratch::new("forget");
    let store = Store::in_home(scratch.home());

    store.keep("openai", SECRET).expect("a writable home");
    store.keep("moonshot", SECRET).expect("a writable home");

    assert!(store.forget("openai").expect("a writable home"));
    assert!(
        !store.forget("openai").expect("a writable home"),
        "the second one had nothing to forget"
    );

    assert_eq!(held(&store, "openai"), None);
    assert_eq!(held(&store, "moonshot").as_deref(), Some(SECRET));
}

#[test]
fn writing_over_a_store_that_cannot_be_read_is_refused() {
    let scratch = Scratch::new("clobber");
    let store = scratch.holding("{not json at all");

    let refused = store.keep("openai", SECRET);

    assert!(
        matches!(refused, Err(AuthError::Unreadable { .. })),
        "a file this program cannot read is still the only copy of something"
    );
}

#[test]
fn a_written_key_appears_in_no_debug_output_anywhere() {
    let scratch = Scratch::new("write-redacted");
    let store = Store::in_home(scratch.home());
    store.keep("openai", SECRET).expect("a writable home");

    let keys = store.read();
    for printed in [format!("{keys:?}"), format!("{store:?}")] {
        assert!(
            !printed.contains(SECRET),
            "a key reached a Debug line: {printed}"
        );
    }
}

#[cfg(unix)]
#[test]
fn the_file_and_the_directory_it_makes_are_readable_only_by_their_owner() {
    let scratch = Scratch::new("modes");
    let home = scratch.home().join("never-made");
    let store = Store::in_home(&home);
    store.keep("openai", SECRET).expect("a writable home");

    assert_eq!(mode_of(&home.join(FILE)), 0o600);
    assert_eq!(mode_of(&home.join(LOCK)), 0o600);
    assert_eq!(mode_of(&home), 0o700);
}

#[cfg(unix)]
#[test]
fn an_existing_open_auth_directory_is_tightened_before_files_are_created() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("open-directory");
    fs::set_permissions(scratch.home(), fs::Permissions::from_mode(0o755)).unwrap();
    let store = Store::in_home(scratch.home());

    store.keep("openai", SECRET).expect("a writable home");

    assert_eq!(mode_of(scratch.home()), 0o700);
    assert_eq!(mode_of(&scratch.home().join(FILE)), 0o600);
    assert_eq!(mode_of(&scratch.home().join(LOCK)), 0o600);
}

#[cfg(unix)]
#[test]
fn an_existing_open_auth_directory_is_tightened_before_a_store_is_read() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("open-directory-read");
    let store = scratch.holding(&format!(
        r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#
    ));
    fs::set_permissions(scratch.home(), fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(held(&store, "openai").as_deref(), Some(SECRET));
    assert_eq!(mode_of(scratch.home()), 0o700);
}

#[cfg(unix)]
#[test]
fn a_store_that_cannot_be_tightened_is_refused_before_it_is_opened() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new("cannot-tighten");
    let path = scratch.home().join(FILE);
    symlink(scratch.home().join("missing-target"), &path).unwrap();
    let store = Store::in_home(scratch.home());

    let keys = store.read();
    assert_eq!(keys.providers().count(), 0);
    assert!(keys.trouble().is_some(), "the failed protection was hidden");
    assert!(
        matches!(
            store.keep("openai", SECRET),
            Err(AuthError::Unwritable { .. })
        ),
        "writing followed a path that could not be protected"
    );
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
}

#[cfg(unix)]
#[test]
fn a_store_symlinked_to_a_live_file_is_refused_without_touching_the_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let scratch = Scratch::new("live-store-link");
    let target = scratch.home().join("elsewhere.json");
    let text = format!(r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#);
    fs::write(&target, &text).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let path = scratch.home().join(FILE);
    symlink(&target, &path).unwrap();
    let store = Store::in_home(scratch.home());

    let keys = store.read();
    assert_eq!(keys.providers().count(), 0);
    assert!(keys.trouble().is_some());
    assert!(store.keep("openai", "replacement").is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), text);
    assert_eq!(mode_of(&target), 0o644);
}

#[cfg(windows)]
#[test]
fn auth_artifacts_stay_reachable_after_their_protected_lists_are_written() {
    let scratch = Scratch::new("windows-private");
    let store = Store::in_home(scratch.home());
    store.keep("openai", SECRET).expect("a writable home");

    assert_eq!(held(&store, "openai").as_deref(), Some(SECRET));
    assert!(scratch.home().join(LOCK).is_file());
    assert!(fs::read_dir(scratch.home()).unwrap().count() >= 2);
}

#[cfg(unix)]
#[test]
fn a_store_left_too_open_is_tightened_and_reported_rather_than_refused() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("loose");
    let store = Store::in_home(scratch.home());
    store.keep("openai", SECRET).expect("a writable home");

    let file = scratch.home().join(FILE);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).expect("a writable home");

    let keys = store.read();

    assert_eq!(
        held(&store, "openai").as_deref(),
        Some(SECRET),
        "a user who cannot log in without shell surgery is worse off"
    );
    assert!(keys.trouble().is_some(), "and is told it was too open");
    assert_eq!(mode_of(&file), 0o600);
}

/// The mode `path` is at, for a test that is about exactly that.
#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .expect("still there")
        .permissions()
        .mode()
        & 0o777
}

/// How long the crucible ahead of this one takes to finish its write, in the
/// test about waiting for it.
///
/// Longer than the rename it stands for, shorter than the budget it has to fit
/// inside. A machine slow enough to stretch this hold stretches the waiter's
/// own pauses by the same amount, so the order of the two never turns over and
/// the test cannot fail for being run somewhere slow.
const HELD: std::time::Duration = std::time::Duration::from_millis(1500);

#[test]
fn a_crucible_slow_to_write_does_not_cost_the_next_one_its_login() {
    // What the budget is really for. Not one rename: however many crucibles are
    // ahead of this one, each syncing a few hundred bytes to a disk somebody
    // else is using too. Sized for the rename alone it turns an ordinary queue
    // into a refusal, and a refusal here is a login nobody wrote down.
    let scratch = Scratch::new("slow-writer");
    let home = scratch.home().to_path_buf();

    let taken = Lock::take(&home.join(LOCK), &home.join(FILE)).expect("nobody else holds it");
    let ahead = std::thread::spawn(move || {
        std::thread::sleep(HELD);
        drop(taken);
    });

    Store::in_home(&home)
        .keep("openai", SECRET)
        .expect("a wait that outlasts the crucible ahead of it");

    ahead.join().expect("the one ahead finished");
    assert_eq!(
        held(&Store::in_home(&home), "openai").as_deref(),
        Some(SECRET)
    );
}

#[test]
fn a_second_crucible_writing_at_the_same_time_loses_nobody_a_login() {
    let scratch = Scratch::new("concurrent");
    let home = scratch.home().to_path_buf();

    let writers: Vec<_> = ["openai", "anthropic", "moonshot", "zed", "acme"]
        .into_iter()
        .map(|provider| {
            let home = home.clone();
            std::thread::spawn(move || {
                Store::in_home(&home)
                    .keep(provider, SECRET)
                    .expect("a writable home");
            })
        })
        .collect();

    for writer in writers {
        writer.join().expect("no writer panicked");
    }

    let store = Store::in_home(&home);
    let keys = store.read();
    let listed: Vec<_> = keys.providers().collect();
    assert_eq!(
        listed,
        ["acme", "anthropic", "moonshot", "openai", "zed"],
        "a write that read the file before another finished would have dropped one"
    );
}
