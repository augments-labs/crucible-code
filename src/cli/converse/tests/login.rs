//! `/login` and `/logout` over the names a row writes its credential under:
//! what each offers, and what each takes out of the store.

use std::io::Cursor;
use std::sync::Arc;

use crucible_runner::Tools;
use crucible_session::Session;
use crucible_tui::{Recording, Renderer};

use crate::cli::converse::{First, Terms, converse};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;

use super::{opening, paired, plain, saying, scripted};

/// A kimi.ai sign-in, fabricated, as the store holds one.
const KIMI_AI_SIGN_IN: &str = r#"{"version":2,"keys":{},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;

/// What typing `typed` wrote, over a store `seeded` put a credential in, and
/// what the store holds afterwards, by map and name.
fn typing(tree: &str, seeded: impl FnOnce(&Sample), typed: &str) -> (String, Vec<String>) {
    let sample = Sample::new(&format!("login-names-{tree}"));
    seeded(&sample);
    let terms = Terms {
        logins: sample.store(),
        ..plain()
    };
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(Script::new(vec![saying("answered")]), Tools::new(), session)
    });
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");

    let left = std::fs::read_to_string(sample.root().with_file_name("home").join("auth.json"))
        .unwrap_or_default();
    let names = ["\"moonshot@kimi.ai\"", "\"moonshot\""]
        .into_iter()
        .filter(|name| left.contains(name))
        .map(str::to_owned)
        .collect();
    (renderer.terminal().written().to_string(), names)
}

/// Keeps a kimi.ai key in `sample`'s store.
fn kimi_ai_key(sample: &Sample) {
    sample
        .store()
        .keep("moonshot@kimi.ai", "fabricated-kimi-ai-key")
        .expect("a writable home");
}

/// Puts a kimi.ai sign-in in `sample`'s store.
fn kimi_ai_sign_in(sample: &Sample) {
    let home = sample.root().with_file_name("home");
    std::fs::create_dir_all(&home).expect("a home");
    std::fs::write(home.join("auth.json"), KIMI_AI_SIGN_IN).expect("a store");
}

#[test]
fn logout_naming_moonshot_takes_out_a_credential_given_on_a_kimi_ai_row() {
    for (tree, seeded) in [
        ("key", kimi_ai_key as fn(&Sample)),
        ("sign-in", kimi_ai_sign_in as fn(&Sample)),
    ] {
        let (written, left) = typing(&format!("logout-{tree}"), seeded, "/logout moonshot\n");

        assert!(left.is_empty(), "{tree}: {left:?}");
        assert!(
            written.contains("removed the stored credential for moonshot"),
            "{tree}: {written}"
        );
        assert!(!written.contains("nothing is stored"), "{tree}: {written}");
    }
}

#[test]
fn logout_down_a_pipe_offers_a_credential_given_on_a_kimi_ai_row() {
    for (tree, seeded) in [
        ("key", kimi_ai_key as fn(&Sample)),
        ("sign-in", kimi_ai_sign_in as fn(&Sample)),
    ] {
        let (written, left) = typing(&format!("logout-piped-{tree}"), seeded, "/logout\n");

        assert!(written.contains("/logout moonshot"), "{tree}: {written}");
        assert!(!written.contains("nothing is stored"), "{tree}: {written}");
        assert_eq!(
            left,
            ["\"moonshot@kimi.ai\""],
            "{tree}: nothing is taken out unasked"
        );
    }
}
