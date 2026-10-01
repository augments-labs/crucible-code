//! What the `/fast` cases start crucible over: a configuration asking for a
//! model with a fast form, and a home holding a `ChatGPT` sign-in. Nothing is
//! sent: the panel stands before any request.

use std::path::PathBuf;

/// The Gemini key the key cases are served by, never sent.
pub(crate) const KEY: &str = "fabricated-gemini-key-never-sent";

/// A configuration asking `provider` for `model`, drawn with `ascii` marks or
/// not.
pub(crate) fn document(provider: &str, model: &str, ascii: bool) -> String {
    let glyphs = if ascii { "ascii" } else { "unicode" };
    let document = serde_json::json!({
        "updates": {"check": "never"},
        "output": {"glyphs": glyphs},
        "provider": provider,
        "providers": {provider: {"model": model}},
    });
    serde_json::to_string_pretty(&document).expect("a configuration document")
}

/// A home holding a stored `ChatGPT` sign-in, owner-only as crucible keeps it,
/// for `case` to be started over. The caller removes it once started.
pub(crate) fn signed_in_home(case: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let home =
        std::env::temp_dir().join(format!("crucible-fast-home-{case}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("a home to copy in");
    let store = home.join("auth.json");
    std::fs::write(&store, crate::SIGN_IN_HELD).expect("a store");
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o600))
        .expect("an owner-only store");
    home
}
