//! Walking a parsed document against the shape.
//!
//! Everything here is about the sentence the reader gets back. The walk knows
//! the dotted path it is standing at and the source text it came from, so a
//! refusal can name the key, say where it is, and say what was accepted
//! instead — rather than reporting that a file is invalid and leaving the
//! search to the person who has it open.

#[cfg(test)]
mod tests;

use std::path::Path;

use serde_json::Value;

use crate::env;
use crate::error::{Accepted, At, ConfigError};
use crate::settings;
use crate::shape::Shape;

use super::Origin;

/// The key naming directories outside the root that tools may reach.
const DIRECTORIES: &str = "extraDirectories";

/// Where in a document the walk is standing.
///
/// The two travel together everywhere — a refusal names the path and points at
/// the position — so they are carried as one thing rather than as a pair of
/// parameters threaded through every arm.
#[derive(Clone, Copy)]
pub(super) struct Spot<'a> {
    /// The dotted route to this value.
    path: &'a str,
    /// Where the key holding it was found.
    at: At,
}

impl Spot<'_> {
    /// The root of a document, which no key holds and which has no path.
    pub(super) const ROOT: Self = Self {
        path: "",
        at: At::Ambiguous,
    };
}

/// One document being checked: the text it came from, what to call it, and
/// which of the layers it is.
///
/// The origin is held here rather than passed to the two checks that ask for
/// it, because it is a property of the document being read and not of one
/// question about it — and because a walk that has to be handed it at every
/// level is a walk somebody can forget to hand it to.
pub(super) struct Reader<'a> {
    /// The file as the user would name it, for the message.
    pub(super) file: &'a str,
    /// The source, so a key can be located in it.
    pub(super) text: &'a str,
    /// Which layer this file is, which decides what it is allowed to say.
    pub(super) origin: Origin,
}

impl Reader<'_> {
    /// Checks one value against the shape it is standing in.
    pub(super) fn check(
        &self,
        value: &Value,
        shape: &'static Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        match shape {
            Shape::Text => self.text_at(value, shape, spot),
            Shape::Choice(allowed) => self.choice(value, allowed, shape, spot),
            Shape::Fields(_) => self.fields(value, shape, spot),
            Shape::Named(inner) => self.named(value, inner, shape, spot),
            Shape::List(inner) => self.list(value, inner, shape, spot),
        }
    }

    /// A string, and nothing else. A number where a model name goes is a
    /// mistake worth stopping for rather than coercing.
    fn text_at(&self, value: &Value, shape: &Shape, spot: Spot<'_>) -> Result<(), ConfigError> {
        if value.is_string() {
            return Ok(());
        }
        Err(self.wrong_type(shape, spot))
    }

    /// A string from the fixed set.
    fn choice(
        &self,
        value: &Value,
        allowed: &'static [&'static str],
        shape: &Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        let Some(found) = value.as_str() else {
            return Err(self.wrong_type(shape, spot));
        };
        if allowed.contains(&found) {
            return Ok(());
        }
        Err(ConfigError::NotAChoice {
            file: self.file.into(),
            path: spot.path.into(),
            found: found.into(),
            at: spot.at,
            accepted: Accepted::new(allowed.to_vec()),
        })
    }

    /// An object whose keys crucible chose. An unrecognised one stops the read.
    ///
    /// And a key the shape marks as widening, in either layer inside the
    /// workspace. That is the same structural refusal `env` has had, moved onto
    /// the thing it was always about: either project filename can be committed,
    /// so a key in one that only ever loosens what crucible does unasked would
    /// be authority granted by a file nobody read.
    /// `permissions.mode` set to `fullAccess` there is every call in every
    /// session approved before the user has typed anything.
    ///
    fn fields(
        &self,
        value: &Value,
        shape: &'static Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        let Some(object) = value.as_object() else {
            return Err(self.wrong_type(shape, spot));
        };

        for (key, held) in object {
            if reserved(key) {
                continue;
            }
            let path = join(spot.path, key);
            let at = At::of(key, self.text);

            let declared = shape.declared(key).ok_or_else(|| ConfigError::UnknownKey {
                file: self.file.into(),
                path: path.as_str().into(),
                at,
                accepted: Accepted::new(shape.keys()),
            })?;

            // Before the value is walked, so the refusal names the key rather
            // than something written under it. What is wrong here is where the
            // key is, and that is true whatever it was set to.
            if declared.widens && self.origin.in_the_workspace() {
                return Err(ConfigError::Widening {
                    file: self.file.into(),
                    path: path.as_str().into(),
                    at,
                });
            }

            self.check(held, &declared.shape, Spot { path: &path, at })?;
        }
        Ok(())
    }

    /// An object whose keys the user chose — a provider name, a variable name.
    /// There is no unknown key here, only a value of the wrong kind.
    fn named(
        &self,
        value: &Value,
        inner: &'static Shape,
        shape: &Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        let Some(object) = value.as_object() else {
            return Err(self.wrong_type(shape, spot));
        };

        for (key, held) in object {
            if reserved(key) {
                continue;
            }
            let path = join(spot.path, key);
            let at = At::of(key, self.text);
            self.check(held, inner, Spot { path: &path, at })?;
        }
        Ok(())
    }

    /// An array, every element of the same shape.
    ///
    /// An element is named by the index it sits at and located at the key that
    /// holds the list. There is no key of its own to find in the source, and
    /// the position of some other `"read(src/**)"` in the file would send the
    /// reader to a line that is perfectly correct.
    fn list(
        &self,
        value: &Value,
        inner: &'static Shape,
        shape: &Shape,
        spot: Spot<'_>,
    ) -> Result<(), ConfigError> {
        let Some(items) = value.as_array() else {
            return Err(self.wrong_type(shape, spot));
        };

        for (index, held) in items.iter().enumerate() {
            let path = format!("{}[{index}]", spot.path);
            let spot = Spot {
                path: &path,
                at: spot.at,
            };
            self.check(held, inner, spot)?;
        }
        Ok(())
    }

    /// Checks the `env` block against the three rules that are not about shape.
    ///
    /// None of them belongs in the walk above: the first two are about *which
    /// name* was written rather than what kind of value it holds, and the third
    /// is about a meaning the shape has no way to state — the shape of every
    /// entry in `env` is the same string either way.
    ///
    /// The first is structural security. No file under the working directory
    /// may hold a value whose meaning crucible does not already fix, so a key
    /// cannot be leaked by a configuration file somebody committed without
    /// reading it — and, the sharper half, a repository cannot decide what the
    /// commands crucible runs actually are. `PATH` alone settles which program
    /// every one of them resolves to; `LD_PRELOAD` is not on the list a child
    /// inherits and is addable through this same block. Crucible's own
    /// namespace is exempt because those names are settings rather than
    /// secrets — see [`crate::env`]. Checked by origin rather than by filename,
    /// because the filename is the wiring's business and this rule is not.
    ///
    /// Both project files, not only the checked-in one. Which of them git
    /// carries is a convention written in the repository being cloned, so it is
    /// not a thing crucible can check — see [`Origin::in_the_workspace`]. The
    /// alternative was a list of the names that hand a program somebody else's
    /// code, and it is unbounded: `PATH`, `LD_*` and `DYLD_*` are only the
    /// start of it, `BASH_ENV` sources a file into every non-interactive shell,
    /// and every language runtime adds one more. A list of the names somebody
    /// guessed covers exactly the guesses — which is the argument the list of
    /// variables a command inherits is already built on, from the other side.
    ///
    /// It costs the honest user a real thing, and this is what: a variable for
    /// one project only — a `CARGO_TARGET_DIR`, a token a deploy script
    /// reads — now goes in the configuration file in their home directory,
    /// where it applies to every project, or in the shell they start crucible
    /// in. They are not the threat and they pay anyway, because the file they
    /// wrote and the file a repository committed are the same file to
    /// everything crucible can see.
    ///
    /// The second holds in every layer: a variable crucible has already read by
    /// the time it opens a file cannot be set from one, so it is refused instead
    /// of accepted and quietly dropped.
    ///
    /// The third is the value of one of crucible's own settings, which means
    /// something other than the string it was written as. Asked here rather
    /// than where the setting is read, because here the file is still open and
    /// the position can still be pointed at — by the time the layers have
    /// merged, neither can.
    pub(super) fn variables(&self, value: &Value) -> Result<(), ConfigError> {
        let Some(vars) = value.get("env").and_then(Value::as_object) else {
            return Ok(());
        };

        for (name, held) in vars {
            // The name, never the value beside it. Naming the variable is what
            // makes any of these messages actionable; quoting what was set
            // there would put a possible secret into an error string.
            if env::too_late(name) {
                return Err(ConfigError::TooLate {
                    file: self.file.into(),
                    name: name.as_str().into(),
                    at: At::of(name, self.text),
                });
            }

            if self.origin.in_the_workspace() && !env::ours(name) {
                return Err(ConfigError::ProjectEnv {
                    file: self.file.into(),
                    name: name.as_str().into(),
                    at: At::of(name, self.text),
                    namespace: env::NAMESPACE,
                });
            }

            // Only a string has an answer to read. Anything else was already
            // refused by the walk above, which ran first and says what shape an
            // entry here has to be.
            if let Some(written) = held.as_str()
                && let Some(accepted) = settings::refused(name, written)
            {
                return Err(ConfigError::Answer {
                    file: self.file.into(),
                    name: name.as_str().into(),
                    at: At::of(name, self.text),
                    accepted,
                });
            }
        }
        Ok(())
    }

    /// Checks that every directory the workspace is asked to reach is named by
    /// an absolute path.
    ///
    /// Not a shape rule: a relative path is a string like any other, and the
    /// shape has nothing to say about it. Not left to the workspace either,
    /// which refuses the same entry — by the time it does, the file the entry
    /// was written in is gone, and this is a message read with that file open.
    ///
    /// There is no schema `pattern` beside it. An absolute path does not begin
    /// with `/` on every platform, and a schema that is wrong about one is
    /// worse than a schema that says nothing.
    pub(super) fn directories(&self, value: &Value) -> Result<(), ConfigError> {
        let Some(entries) = value
            .get("permissions")
            .and_then(|block| block.get(DIRECTORIES))
            .and_then(Value::as_array)
        else {
            return Ok(());
        };

        for (index, held) in entries.iter().enumerate() {
            let Some(found) = held.as_str() else { continue };
            if Path::new(found).is_absolute() {
                continue;
            }
            return Err(ConfigError::Relative {
                file: self.file.into(),
                path: format!("permissions.{DIRECTORIES}[{index}]").into(),
                found: found.into(),
                at: At::of(DIRECTORIES, self.text),
            });
        }
        Ok(())
    }

    /// The same refusal from three places, built once.
    fn wrong_type(&self, shape: &Shape, spot: Spot<'_>) -> ConfigError {
        ConfigError::WrongType {
            file: self.file.into(),
            path: if spot.path.is_empty() {
                "the document".into()
            } else {
                spot.path.into()
            },
            wanted: shape.wanted(),
            at: spot.at,
        }
    }
}

/// Keys reserved by JSON Schema itself, which a document may carry anywhere and
/// which mean nothing to crucible.
///
/// `$schema` is how an editor knows which schema to complete against, so a
/// document that could not carry it would not get the completion this format
/// was chosen for. `$comment` is the specification's own answer to JSON having
/// no comments — the one real cost of the format, paid from the standard rather
/// than by inventing a dialect that standard parsers would reject.
///
/// These two by name rather than every key starting with a dollar, so that this
/// and the generated schema accept the same documents.
fn reserved(key: &str) -> bool {
    crate::shape::schema::RESERVED.contains(&key)
}

/// The dotted path to a key, for a message that has to say where it is.
fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}
