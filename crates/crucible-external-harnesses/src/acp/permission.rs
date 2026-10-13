//! The options an agent offered for each permission it asked, and the one an
//! answer picks.
//!
//! An answer is sent under the id the agent gave the option it matches, and
//! only under one it gave: a decision no offered option carries is refused
//! rather than sent as a guess, and a question is answered once. A deny the
//! agent offered no one-time refusal for is sent as its lasting one, which
//! grants nothing more.

use crucible_execution::{Decision, Failure, Lasting};
use serde_json::{Value, json};

/// The options one question offered, each an id and its kind.
type Offered = Vec<(Box<str>, Box<str>)>;

/// The permissions an agent has asked and not yet been answered.
#[derive(Debug, Default)]
pub(crate) struct Permissions {
    /// Each question's native name and the options it offered, each an id and
    /// its kind.
    open: Vec<(Box<str>, Offered)>,
}

impl Permissions {
    /// Keeps that `native` was asked with `options`, each an id and its kind.
    pub(crate) fn offered(&mut self, native: &str, options: Offered) {
        self.open.push((native.into(), options));
    }

    /// The frame that answers `native` with `decision`, which settles it.
    ///
    /// # Errors
    ///
    /// A [`Failure`] where `native` is not open, or none of its options is the
    /// kind `decision` means.
    pub(crate) fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        let kinds: &[&str] = match decision {
            Decision::Allow(Lasting::Once) => &["allow_once"],
            Decision::Allow(Lasting::Session) => &["allow_always"],
            Decision::Deny => &["reject_once", "reject_always"],
            Decision::Answer(_) => {
                return Err(Failure::new(
                    "The agent asked for a permission, which a written answer cannot give.",
                ));
            }
        };
        let place = self.place(native)?;
        let option = self
            .open
            .get(place)
            .and_then(|(_, options)| {
                kinds
                    .iter()
                    .find_map(|kind| options.iter().find(|(_, offered)| &**offered == *kind))
            })
            .map(|(id, _)| id.clone())
            .ok_or_else(|| {
                Failure::new("The agent offered no option that carries that decision.")
            })?;
        self.open.remove(place);
        reply(
            native,
            &json!({"outcome": {"outcome": "selected", "optionId": &*option}}),
        )
    }

    /// The frame that tells `native` its turn was stopped, which settles it.
    ///
    /// # Errors
    ///
    /// A [`Failure`] where `native` is not open.
    pub(crate) fn withdraw(&mut self, native: &str) -> Result<String, Failure> {
        let place = self.place(native)?;
        self.open.remove(place);
        reply(native, &json!({"outcome": {"outcome": "cancelled"}}))
    }

    /// Where `native` is among the open questions.
    fn place(&self, native: &str) -> Result<usize, Failure> {
        self.open
            .iter()
            .position(|(asked, _)| &**asked == native)
            .ok_or_else(|| {
                Failure::new("The agent has no permission question open under that name.")
            })
    }
}

/// The response to the request `native` names, carrying `result`.
fn reply(native: &str, result: &Value) -> Result<String, Failure> {
    let id: Value = serde_json::from_str(native)
        .map_err(|_| Failure::new("The agent's permission question has no id a reply can name."))?;
    Ok(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
}
