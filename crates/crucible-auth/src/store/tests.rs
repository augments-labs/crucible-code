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
    let mut request = crucible_credentials::Outgoing::new();
    let header = crucible_credentials::Header::bare("x-api-key");
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

/// What is on the disk, as text. Every value in these tests is fabricated.
fn on_disk(scratch: &Scratch) -> String {
    fs::read_to_string(scratch.home().join(FILE)).expect("a store on the disk")
}

#[test]
fn a_write_keeps_every_name_this_build_does_not_serve_and_drops_a_field_it_does_not_know() {
    let scratch = Scratch::new("kept-names");
    let store = scratch.holding(
        r#"{"version":2,"keys":{"qwen@coding-plan.aliyun.com":"fabricated-plan-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}},"identities":{"moonshot@kimi.ai":"00000000-0000-4000-8000-000000000000"},"rows":{"added":"field"}}"#,
    );

    let read = store.read();
    // A file this test wrote with the default mode is tightened on its first
    // read, and the store says so once. Nothing else may be said.
    assert!(
        read.trouble().is_none_or(|said| said.contains("tightened")),
        "the store was read whole"
    );
    assert_eq!(read.providers().count(), 2, "both names were read");

    store.keep("openai", "fabricated-openai-key").unwrap();
    let text = on_disk(&scratch);
    assert!(text.contains(r#""qwen@coding-plan.aliyun.com":"fabricated-plan-key""#));
    assert!(text.contains(r#""moonshot@kimi.ai":{"access_token":"fabricated-access""#));
    assert!(text.contains("00000000-0000-4000-8000-000000000000"));
    assert!(text.contains(r#""openai":"fabricated-openai-key""#));
    assert!(text.contains(r#""version":2"#));
    assert!(
        !text.contains("rows"),
        "a field this build does not know is gone"
    );

    // A key for the provider whose other row is held: 0.43.3 stores it
    // beside that row rather than in its place.
    store.keep("moonshot", "fabricated-moonshot-key").unwrap();
    let text = on_disk(&scratch);
    assert!(text.contains(r#""moonshot":"fabricated-moonshot-key""#));
    assert!(text.contains(r#""moonshot@kimi.ai":{"access_token":"fabricated-access""#));
    assert!(store.read().trouble().is_none());

    // And forgetting the provider leaves the other name where it was.
    assert!(store.forget("moonshot").unwrap());
    let text = on_disk(&scratch);
    assert!(!text.contains("fabricated-moonshot-key"));
    assert!(text.contains(r#""moonshot@kimi.ai""#));
}

/// The names a build with two Kimi sites writes under, and one provider with a
/// single row. Every value in the tests below is fabricated.
fn named() -> Names {
    Names::new(["moonshot", "moonshot@kimi.ai", "openai", "anthropic"])
}

/// Every row of `named`, by the map its credential sits in and its name.
fn rows() -> Vec<(Kind, &'static str)> {
    let mut rows = Vec::new();
    for name in ["moonshot", "moonshot@kimi.ai", "openai", "anthropic"] {
        rows.push((Kind::Key, name));
        rows.push((Kind::Account, name));
    }
    rows
}

/// Writes `row`'s credential the way `/login` does.
fn give(store: &Store, (kind, name): (Kind, &str), canary: &str) -> Vec<Dropped> {
    match kind {
        Kind::Key => store.keep(name, &format!("key-{canary}")),
        Kind::Account => store.keep_subscription(name, subscription(canary)),
    }
    .expect("a writable store")
}

/// Every credential on the disk, by map and name, parsed the way 0.43.3
/// parses a store: the parser is the one it shipped.
fn on_the_disk(scratch: &Scratch) -> Vec<(Kind, String)> {
    let text = on_disk(scratch);
    assert!(text.len() <= MAX_STORE, "{} bytes", text.len());
    let document = document::parse(&text).expect("a file 0.43.3 reads whole");
    assert!(text.contains(r#""version":2"#), "{text}");
    document
        .keys
        .keys()
        .map(|name| (Kind::Key, name.clone()))
        .chain(
            document
                .subscriptions
                .keys()
                .map(|name| (Kind::Account, name.clone())),
        )
        .collect()
}

#[test]
fn every_pair_of_rows_leaves_a_provider_one_credential_and_says_which_went() {
    for first in rows() {
        for second in rows() {
            if provider_of(first.1) != provider_of(second.1) {
                continue;
            }
            let scratch = Scratch::new("one-each");
            let store = Store::in_home(scratch.home()).naming(named());
            give(&store, first, "first");
            let dropped = give(&store, second, "second");

            let held = on_the_disk(&scratch);
            let at = format!("{first:?} then {second:?}: {held:?}");
            assert_eq!(held, vec![(second.0, second.1.to_owned())], "{at}");
            let wanted: Vec<Dropped> = (first != second)
                .then(|| Dropped {
                    kind: first.0,
                    name: first.1.to_owned(),
                })
                .into_iter()
                .collect();
            assert_eq!(dropped, wanted, "{at}");
        }
    }
}

#[test]
fn a_write_for_one_provider_leaves_every_other_providers_credential() {
    let scratch = Scratch::new("others-kept");
    let store = Store::in_home(scratch.home()).naming(named());
    give(&store, (Kind::Key, "openai"), "openai");
    give(&store, (Kind::Account, "moonshot@kimi.ai"), "kimi");
    give(&store, (Kind::Key, "moonshot"), "moonshot");

    assert_eq!(
        on_the_disk(&scratch),
        vec![
            (Kind::Key, "moonshot".to_owned()),
            (Kind::Key, "openai".to_owned())
        ]
    );
}

#[test]
fn where_two_are_held_the_one_under_the_bare_name_serves_the_provider() {
    let scratch = Scratch::new("bare-wins");
    let store = scratch
        .holding(
            r#"{"version":2,"keys":{"moonshot":"fabricated-moonshot-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}},"identities":{}}"#,
        )
        .naming(named());

    assert_eq!(
        store.read().held("moonshot"),
        Some(Held {
            kind: Kind::Key,
            name: "moonshot".to_owned()
        })
    );
}

#[test]
fn a_name_this_build_does_not_write_is_never_used_and_never_removed() {
    let scratch = Scratch::new("unknown-name");
    let store = scratch
        .holding(
            r#"{"version":2,"keys":{"moonshot@kimi.cn":"fabricated-later-key"},"subscriptions":{},"identities":{}}"#,
        )
        .naming(named());

    assert_eq!(store.read().held("moonshot"), None);

    give(&store, (Kind::Account, "moonshot@kimi.ai"), "kimi");
    assert!(store.forget("moonshot").expect("a writable store"));
    let text = on_disk(&scratch);
    assert!(text.contains("fabricated-later-key"), "{text}");
    assert!(!text.contains("access-kimi"), "{text}");
}

#[test]
fn forgetting_a_provider_holding_two_takes_both_out_in_one_write() {
    let scratch = Scratch::new("forget-both");
    let store = scratch
        .holding(
            r#"{"version":2,"keys":{"moonshot":"fabricated-moonshot-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}},"identities":{}}"#,
        )
        .naming(named());

    assert!(store.forget("moonshot").expect("a writable store"));
    assert_eq!(on_the_disk(&scratch), Vec::new());
}

/// A store as 0.43.3 leaves one after a roll back: its own `moonshot` key
/// beside the kimi.ai sign-in the new release wrote.
const TWO_HELD: &str = r#"{"version":2,"keys":{"moonshot":"fabricated-moonshot-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}},"identities":{"moonshot@kimi.ai":"00000000-0000-4000-8000-000000000000"}}"#;

#[test]
fn a_start_that_finds_two_for_a_provider_keeps_the_bare_one_and_says_which_went() {
    let scratch = Scratch::new("settled");
    let store = scratch.holding(TWO_HELD).naming(named());

    let settled = store.settle();

    let Settled::Removed(dropped) = settled else {
        panic!("{settled:?}");
    };
    assert_eq!(
        dropped,
        vec![Dropped {
            kind: Kind::Account,
            name: "moonshot@kimi.ai".to_owned()
        }]
    );
    assert_eq!(
        on_the_disk(&scratch),
        vec![(Kind::Key, "moonshot".to_owned())]
    );
    // The installation's identity outlives the credential it was made for.
    assert!(on_disk(&scratch).contains("00000000-0000-4000-8000-000000000000"));
    // And a second start finds nothing to do.
    assert!(matches!(store.settle(), Settled::Nothing));
}

#[test]
fn a_start_that_cannot_write_goes_on_with_the_bare_one_and_leaves_the_file() {
    let scratch = Scratch::new("settle-unwritable");
    let store = scratch.holding(TWO_HELD).naming(named());
    // Read once so the file is tightened, then stand a directory where the
    // write puts its temporary: the write is refused, as a full disk would.
    let _ = store.read();
    let before = on_disk(&scratch);
    fs::create_dir(scratch.home().join(PARTIAL)).expect("a directory this test made");

    let settled = store.settle();

    assert!(
        matches!(&settled, Settled::Stayed { found, .. } if found.len() == 1),
        "{settled:?}"
    );
    assert_eq!(on_disk(&scratch), before);
    assert_eq!(
        store.read().held("moonshot").map(|held| held.kind),
        Some(Kind::Key)
    );
}

#[test]
fn what_a_provider_holds_is_read_by_name_and_kind_alone() {
    let scratch = Scratch::new("holding");
    let store = scratch.holding(TWO_HELD).naming(named());
    store
        .keep("openai", "fabricated-openai-key")
        .expect("a writable store");

    let holding = store.holding().expect("a store that reads whole");

    assert_eq!(
        holding,
        vec![
            Held {
                kind: Kind::Key,
                name: "moonshot".to_owned(),
            },
            Held {
                kind: Kind::Key,
                name: "openai".to_owned(),
            },
        ]
    );
    let said = format!("{holding:?}");
    assert!(!said.contains("fabricated"), "{said}");
}

#[test]
fn a_store_that_cannot_be_read_whole_is_said_before_anything_is_marked() {
    let unreadable = Scratch::new("holding-unreadable");
    let store = unreadable.holding("not json {").naming(named());
    assert!(
        matches!(store.holding(), Err(AuthError::Unreadable { .. })),
        "{:?}",
        store.holding()
    );

    let large = Scratch::new("holding-large");
    let store = large.holding(&"x".repeat(MAX_STORE + 1)).naming(named());
    assert!(
        matches!(store.holding(), Err(AuthError::TooLarge { .. })),
        "{:?}",
        store.holding()
    );

    let nothing = Scratch::new("holding-nothing");
    let store = Store::in_home(nothing.home()).naming(named());
    assert_eq!(store.holding().ok(), Some(Vec::new()));
}

/// Holds the store's lock the way another crucible writing does, until
/// dropped.
fn another_crucible_writing(scratch: &Scratch) -> Lock {
    Lock::take(&scratch.home().join(LOCK), &scratch.home().join(FILE))
        .expect("the lock, taken first")
}

#[test]
fn a_start_meeting_another_crucibles_lock_still_finds_the_second_credential() {
    let scratch = Scratch::new("settle-busy");
    let store = scratch.holding(TWO_HELD).naming(named());
    let _ = store.read();
    let before = on_disk(&scratch);
    let _held = another_crucible_writing(&scratch);

    let settled = store.settle();

    assert!(
        matches!(
            &settled,
            Settled::Stayed { found, why: AuthError::Busy { .. } }
                if found == &[Dropped { kind: Kind::Account, name: "moonshot@kimi.ai".to_owned() }]
        ),
        "{settled:?}"
    );
    assert_eq!(on_disk(&scratch), before);
}

#[test]
fn a_start_with_one_credential_each_waits_on_no_lock() {
    let scratch = Scratch::new("settle-idle");
    let store = scratch
        .holding(r#"{"version":2,"keys":{"moonshot":"fabricated-moonshot-key","openai":"fabricated-openai-key"},"subscriptions":{}}"#)
        .naming(named());
    let _ = store.read();
    let _held = another_crucible_writing(&scratch);

    let started = std::time::Instant::now();
    let settled = store.settle();

    assert!(matches!(settled, Settled::Nothing), "{settled:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_store_that_is_not_text_is_one_that_cannot_be_read_rather_than_reached() {
    // The file is there and opens; what it holds is not UTF-8. Moving it aside
    // is the way back in, not fixing permissions that are fine.
    let scratch = Scratch::new("holding-not-text");
    fs::write(scratch.home().join(FILE), [0x7b, 0xff, 0xfe, 0x7d]).expect("a store");
    let store = Store::in_home(scratch.home()).naming(named());

    assert!(
        matches!(store.holding(), Err(AuthError::Unreadable { .. })),
        "{:?}",
        store.holding()
    );
    assert!(
        matches!(
            store.keep("openai", "fabricated-openai-key"),
            Err(AuthError::Unreadable { .. })
        ),
        "a write refuses it the same way"
    );
}

#[test]
fn a_store_past_its_limit_is_too_large_wherever_the_limit_falls() {
    // The limit falls inside a two-byte character: what is read stops half
    // way through it, and the store is still one too large, not one that
    // cannot be read.
    let scratch = Scratch::new("too-large-mid-character");
    let mut text = "x".repeat(MAX_STORE);
    text.push('é');
    let store = scratch.holding(&text).naming(named());

    assert!(
        matches!(store.holding(), Err(AuthError::TooLarge { .. })),
        "{:?}",
        store.holding()
    );
}

/// What a letting-go hook was handed, and what the store's file held when it
/// was asked.
type Asked = Arc<std::sync::Mutex<Vec<(Vec<Dropped>, String)>>>;

/// A store that asks a hook recording what it was handed and what the file
/// said then, refusing when `refuse` is set.
fn asking(scratch: &Scratch, refuse: bool) -> (Store, Asked) {
    let asked: Asked = Arc::default();
    let file = scratch.home().join(FILE);
    let recording = Arc::clone(&asked);
    let store = Store::in_home(scratch.home())
        .naming(named())
        .letting_go(Arc::new(move |going: &[Dropped]| {
            let then = fs::read_to_string(&file).unwrap_or_default();
            recording.lock().unwrap().push((going.to_vec(), then));
            if refuse {
                Err("the yes could not be taken out".into())
            } else {
                Ok(())
            }
        }));
    (store, asked)
}

/// Every write that takes a credential out asks first, with the store's file
/// still as it was, and is handed exactly what goes: a key replaced by another
/// row's sign-in, a provider forgotten, a second credential settled. A write
/// that takes nothing out asks nothing.
#[test]
fn a_write_asks_before_it_takes_a_credential_out() {
    let scratch = Scratch::new("letting-go");
    let (store, asked) = asking(&scratch, false);

    give(&store, (Kind::Key, "moonshot"), "first");
    give(&store, (Kind::Key, "moonshot"), "again");
    give(&store, (Kind::Key, "openai"), "openai");
    assert!(asked.lock().unwrap().is_empty(), "nothing went yet");

    let before = on_disk(&scratch);
    give(&store, (Kind::Account, "moonshot@kimi.ai"), "second");
    let between = on_disk(&scratch);
    store.forget("openai").unwrap();

    let asked = asked.lock().unwrap().clone();
    assert_eq!(
        asked,
        [
            (vec![Held::new(Kind::Key, "moonshot")], before),
            (vec![Held::new(Kind::Key, "openai")], between),
        ],
        "asked with the store as it was before each write"
    );
}

#[test]
fn a_settle_asks_before_it_takes_the_second_credential_out() {
    let scratch = Scratch::new("letting-go-settle");
    scratch.holding(TWO_HELD);
    let (store, asked) = asking(&scratch, false);

    assert!(matches!(store.settle(), Settled::Removed(_)));
    let going: Vec<Vec<Dropped>> = asked
        .lock()
        .unwrap()
        .iter()
        .map(|(going, _)| going.clone())
        .collect();
    assert_eq!(going, [vec![Held::new(Kind::Account, "moonshot@kimi.ai")]]);
}

/// A hook that cannot do what it must leaves the store byte for byte, and the
/// write says why.
#[test]
fn a_refusal_before_a_credential_goes_leaves_the_store_as_it_was() {
    let scratch = Scratch::new("letting-go-refused");
    let (store, _) = asking(&scratch, true);
    give(
        &Store::in_home(scratch.home()).naming(named()),
        (Kind::Key, "moonshot"),
        "kept",
    );
    let before = on_disk(&scratch);

    let replaced = store.keep_subscription("moonshot@kimi.ai", subscription("refused"));
    let forgotten = store.forget("moonshot");

    assert_eq!(on_disk(&scratch), before);
    for failed in [replaced.err(), forgotten.err()] {
        let said = failed.map(|error| error.to_string()).unwrap_or_default();
        assert!(said.contains("the yes could not be taken out"), "{said}");
    }
}

/// What a moving hook was handed on each write, and what the file held then.
type Moved = Arc<std::sync::Mutex<Vec<(Vec<String>, String)>>>;

/// Every write that moves the credential a provider holds asks first, with the
/// file as it was, and is handed that provider: a first credential stored, one
/// replaced by another row's, one forgotten, a second settled. A key written
/// again under its own name moves nothing and asks nothing.
#[test]
fn a_write_asks_before_it_moves_the_credential_a_provider_holds() {
    let scratch = Scratch::new("moving");
    let moved: Moved = Arc::default();
    let file = scratch.home().join(FILE);
    let recording = Arc::clone(&moved);
    let store = Store::in_home(scratch.home())
        .naming(named())
        .moving(Arc::new(move |providers: &[&str]| {
            let then = fs::read_to_string(&file).unwrap_or_default();
            let providers = providers.iter().map(|one| (*one).to_owned()).collect();
            recording.lock().unwrap().push((providers, then));
            Ok(())
        }));

    let empty = String::new();
    give(&store, (Kind::Key, "moonshot"), "first");
    give(&store, (Kind::Key, "moonshot"), "again");
    let rekeyed = on_disk(&scratch);
    give(&store, (Kind::Account, "moonshot@kimi.ai"), "second");
    let second = on_disk(&scratch);
    store.forget("moonshot").unwrap();

    assert_eq!(
        *moved.lock().unwrap(),
        [
            (vec!["moonshot".to_owned()], empty),
            (vec!["moonshot".to_owned()], rekeyed),
            (vec!["moonshot".to_owned()], second),
        ]
    );
}

#[test]
fn a_refusal_before_a_credential_moves_leaves_the_store_as_it_was() {
    let scratch = Scratch::new("moving-refused");
    let store = Store::in_home(scratch.home())
        .naming(named())
        .moving(Arc::new(|_: &[&str]| {
            Err("the speed could not be taken out".into())
        }));

    let kept = store.keep("moonshot", "refused");

    assert!(!scratch.home().join(FILE).exists(), "nothing was written");
    let said = kept
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(said.contains("the speed could not be taken out"), "{said}");
}

/// Every file under `home`, with a digest of its bytes and, where modes
/// exist, its mode: what taking stock of a store must leave exactly as it
/// found it. A digest, so a failure prints no fabricated credential either.
fn everything(home: &Path) -> Vec<(PathBuf, u64, u32)> {
    use std::hash::{Hash as _, Hasher as _};

    let digest = |bytes: Vec<u8>| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    };
    let mode = |path: &Path| -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::symlink_metadata(path).unwrap().permissions().mode()
        }
        #[cfg(not(unix))]
        {
            u32::from(fs::symlink_metadata(path).unwrap().permissions().readonly())
        }
    };
    let mut found = vec![(home.to_path_buf(), 0, mode(home))];
    for entry in fs::read_dir(home).unwrap() {
        let path = entry.unwrap().path();
        let bytes = digest(fs::read(&path).unwrap_or_default());
        found.push((path.clone(), bytes, mode(&path)));
    }
    found.sort();
    found
}

/// A store holding a key and an account, both fabricated, at the default mode
/// a file is written at, in a directory others can list.
fn open_store(scratch: &Scratch) -> Store {
    let store = scratch.holding(&format!(
        r#"{{"version":2,"keys":{{"moonshot":"{SECRET}"}},"subscriptions":{{"moonshot@kimi.ai":{{"access_token":"access-{SECRET}","refresh_token":"refresh-{SECRET}","details":{{}},"expires_at":4102444800,"refreshed_at":1790000000}}}},"identities":{{}}}}"#
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(scratch.home(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(scratch.home().join(FILE), fs::Permissions::from_mode(0o644)).unwrap();
    }
    store.naming(named())
}

#[test]
fn taking_stock_of_a_store_writes_nothing_and_tightens_nothing() {
    let scratch = Scratch::new("stock-inert");
    let store = open_store(&scratch);
    let before = everything(scratch.home());

    let stock = store.inventory();

    assert_eq!(everything(scratch.home()), before);
    assert!(stock.present());
    assert_eq!(stock.trouble(), None);
    #[cfg(unix)]
    assert_eq!(stock.exposed(), Some(true));
}

#[test]
fn taking_stock_names_what_is_held_and_never_a_value() {
    let scratch = Scratch::new("stock-names");
    let stock = open_store(&scratch).inventory();

    assert_eq!(stock.count(), 2);
    assert_eq!(
        stock.held("moonshot"),
        Some(Held::new(Kind::Key, "moonshot"))
    );
    assert_eq!(stock.held("openai"), None);
    let shown = format!("{stock:?}");
    assert!(!shown.contains(SECRET), "{shown}");

    // The same answer reading the store gives, from the same names.
    let read = Store::in_home(scratch.home()).naming(named()).read();
    for provider in ["moonshot", "openai", "anthropic"] {
        assert_eq!(stock.held(provider), read.held(provider), "{provider}");
    }
}

#[test]
fn taking_stock_of_a_store_that_is_not_there_makes_no_directory() {
    let scratch = Scratch::new("stock-absent");
    let home = scratch.home().join("not-yet");

    let stock = Store::in_home(&home).inventory();

    assert!(!home.exists());
    assert!(!stock.present());
    assert_eq!(
        (stock.count(), stock.trouble(), stock.exposed()),
        (0, None, None)
    );
}

#[test]
fn a_store_that_cannot_be_taken_stock_of_says_why_without_its_path() {
    let scratch = Scratch::new("stock-trouble");
    for text in [
        "{not json".to_owned(),
        r#"{"version":9,"keys":{},"subscriptions":{}}"#.to_owned(),
        format!(
            r#"{{"version":1,"keys":{{"a":"{}"}}}}"#,
            "x".repeat(MAX_STORE)
        ),
    ] {
        let store = scratch.holding(&text);
        let before = everything(scratch.home());

        let stock = store.inventory();

        assert_eq!(everything(scratch.home()), before);
        assert!(stock.present());
        assert_eq!(stock.count(), 0);
        let said = stock.trouble().unwrap_or_default();
        assert!(said.contains(FILE), "{said}");
        assert!(!said.contains(&*scratch.home().to_string_lossy()), "{said}");
    }
}

#[cfg(unix)]
#[test]
fn a_store_that_is_a_symbolic_link_is_not_followed_when_stock_is_taken() {
    let scratch = Scratch::new("stock-link");
    let target = scratch.home().join("elsewhere.json");
    fs::write(
        &target,
        format!(r#"{{"version":1,"keys":{{"openai":"{SECRET}"}}}}"#),
    )
    .unwrap();
    std::os::unix::fs::symlink(&target, scratch.home().join(FILE)).unwrap();
    let before = everything(scratch.home());

    let stock = Store::in_home(scratch.home()).inventory();

    assert_eq!(everything(scratch.home()), before);
    assert_eq!(stock.count(), 0);
    assert!(stock.trouble().is_some());
}

#[test]
fn taking_stock_says_when_each_account_login_lapses_and_nothing_else_of_it() {
    let scratch = Scratch::new("stock-lapses");
    let stock = open_store(&scratch).inventory();

    assert_eq!(stock.lapses("moonshot@kimi.ai"), Some(4_102_444_800));
    // A key has no time to lapse at, and a name nothing holds has none.
    assert_eq!(stock.lapses("moonshot"), None);
    assert_eq!(stock.lapses("openai"), None);
    assert!(stock.holds(Kind::Account, "moonshot@kimi.ai"));
    assert!(stock.holds(Kind::Key, "moonshot"));
    assert!(!stock.holds(Kind::Key, "moonshot@kimi.ai"));
    let shown = format!("{stock:?}");
    assert!(!shown.contains(SECRET), "{shown}");
}

#[test]
fn forgetting_says_what_went_and_keeps_every_other_name_and_identity() {
    let scratch = Scratch::new("forgotten-names");
    let store = scratch
        .holding(
            r#"{"version":2,"keys":{"moonshot":"fabricated-moonshot-key","moonshot@kimi.cn":"fabricated-later-key","openai":"fabricated-openai-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}},"identities":{"moonshot@kimi.ai":"00000000-0000-4000-8000-000000000000"}}"#,
        )
        .naming(named());

    let went = store.forgotten("moonshot").expect("a writable store");

    assert_eq!(
        went,
        [
            Held::new(Kind::Key, "moonshot"),
            Held::new(Kind::Account, "moonshot@kimi.ai"),
        ]
    );
    let text = on_disk(&scratch);
    assert!(!text.contains("fabricated-moonshot-key"));
    assert!(!text.contains("fabricated-access"));
    // A name this build does not write, another provider and an identity
    // nothing here owns stay where they were.
    assert!(text.contains(r#""moonshot@kimi.cn":"fabricated-later-key""#));
    assert!(text.contains(r#""openai":"fabricated-openai-key""#));
    assert!(text.contains("00000000-0000-4000-8000-000000000000"));

    // Nothing left to forget is no write at all.
    let before = everything(scratch.home());
    assert_eq!(store.forgotten("moonshot").expect("a writable store"), []);
    assert_eq!(everything(scratch.home()), before);
}

#[test]
fn forgetting_while_another_crucible_writes_waits_and_then_takes_nothing() {
    let scratch = Scratch::new("forgotten-busy");
    let store = open_store(&scratch);
    let before = on_disk(&scratch);
    let _held = another_crucible_writing(&scratch);

    let refused = store.forgotten("moonshot");

    assert!(
        matches!(refused, Err(AuthError::Busy { .. })),
        "{refused:?}"
    );
    assert_eq!(on_disk(&scratch), before);
}
