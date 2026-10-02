//! What a settings menu reads off the layers: each row's value, and whether
//! somebody other than the user's own file decided it.
//!
//! A row the user cannot change from the menu is one a nearer layer states —
//! either project file, or the shell crucible was started in. Writing the
//! user's file under either would be an answer that looks taken and changes
//! nothing, so the menu shows the row locked instead, with who set it.
//!
//! Which rows a project file states is recorded while the layers are still
//! apart, because the merged value no longer says where anything came from.
//! The prompt cache is the one block read as a whole: its policy is checked
//! across its fields and across layers, and a user answer to any one of them
//! can turn a project's narrowing into a widening the next start refuses. So
//! while a project file says anything about the prompt cache, every one of its
//! rows is the project's.

use serde_json::Value;

use crate::document::{Document, Origin};
use crate::shape::rows::{Row, rows};

use super::Settings;
use super::variables;

/// Who decided a row's value, where it was not the user's own file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forced {
    /// The environment crucible was started in.
    Environment,
    /// A project configuration file, committed or local.
    Project,
}

/// The rows a workspace layer states, by key: at most one entry per row.
pub(super) fn pinned(documents: &[Document]) -> Vec<&'static str> {
    rows()
        .iter()
        .filter(|row| {
            documents
                .iter()
                .any(|document| document.origin() != Origin::User && held(row, document.value()))
        })
        .map(Row::key)
        .collect()
}

/// Whether `value`, one layer's document, decides `row`.
fn held(row: &Row, value: &Value) -> bool {
    if row.key().starts_with("promptCaching.") {
        return value.get("promptCaching").is_some();
    }
    let mut at = Some(value);
    for name in row.path() {
        at = at.and_then(|inside| inside.get(name));
    }
    at.is_some()
}

/// The variable a row is read from in the shell, where it is one of the `env`
/// block's.
fn variable(row: &Row) -> Option<&'static str> {
    row.key().strip_prefix("env.")
}

impl Settings {
    /// What `row` is set to across every layer, spelled as the menu shows a
    /// value before it words it: `true`, `dark`, `6`.
    ///
    /// `from` is the environment crucible was started in, which wins for a
    /// row that is one of crucible's variables, as it does at the start.
    /// `None` where nothing set it, and the row's default stands.
    #[must_use]
    pub fn stated(&self, row: &Row, from: &impl Fn(&str) -> Option<String>) -> Option<String> {
        if let Some(said) = variable(row).and_then(from) {
            return Some(said);
        }
        let mut at = Some(&self.value);
        for name in row.path() {
            at = at.and_then(|inside| inside.get(name));
        }
        match at? {
            Value::Bool(flag) => Some(flag.to_string()),
            held => variables::spelled(held).map(std::borrow::Cow::into_owned),
        }
    }

    /// Who decided `row`, where it was not the user's own file.
    #[must_use]
    pub fn forced(&self, row: &Row, from: &impl Fn(&str) -> Option<String>) -> Option<Forced> {
        if variable(row).and_then(from).is_some() {
            Some(Forced::Environment)
        } else if self.pinned.contains(&row.key()) {
            Some(Forced::Project)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{Document, Origin};
    use crate::shape::rows::row;

    use super::*;

    fn nothing(_: &str) -> Option<String> {
        None
    }

    fn rows_of(key: &str) -> &'static Row {
        row(key).expect("the test names a row")
    }

    #[test]
    fn a_settings_row_reads_the_value_every_layer_left() {
        let user = Document::sample(
            r#"{"output": {"scrollRail": false, "theme": "light"},
                "env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": 9}}"#,
            Origin::User,
        );
        let project = Document::sample(r#"{"output": {"theme": "dark"}}"#, Origin::Project);
        let settings = Settings::resolve(vec![user, project]);

        assert_eq!(
            settings.stated(rows_of("output.scrollRail"), &nothing),
            Some("false".to_owned())
        );
        assert_eq!(
            settings.stated(rows_of("output.theme"), &nothing),
            Some("dark".to_owned())
        );
        assert_eq!(settings.stated(rows_of("output.glyphs"), &nothing), None);

        let speed = rows_of("env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED");
        assert_eq!(settings.stated(speed, &nothing), Some("9".to_owned()));
        let shell =
            |name: &str| (name == "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED").then(|| "12".to_owned());
        assert_eq!(settings.stated(speed, &shell), Some("12".to_owned()));
    }

    #[test]
    fn a_settings_row_a_project_or_the_shell_decides_is_forced() {
        let user = Document::sample(
            r#"{"output": {"glyphs": "ascii"}, "updates": {"check": "never"}}"#,
            Origin::User,
        );
        let project = Document::sample(r#"{"output": {"theme": "dark"}}"#, Origin::Project);
        let local = Document::sample(
            r#"{"updates": {"check": "auto"}, "promptCaching": {"mode": "observeOnly"}}"#,
            Origin::ProjectLocal,
        );
        let settings = Settings::resolve(vec![user, project, local]);

        assert_eq!(
            settings.forced(rows_of("output.theme"), &nothing),
            Some(Forced::Project)
        );
        assert_eq!(
            settings.forced(rows_of("updates.check"), &nothing),
            Some(Forced::Project)
        );
        assert_eq!(settings.forced(rows_of("output.glyphs"), &nothing), None);
        // The cache's policy is one block across layers, so a project saying
        // anything in it holds every one of its rows.
        assert_eq!(
            settings.forced(rows_of("promptCaching.isolationScope"), &nothing),
            Some(Forced::Project)
        );

        let speed = rows_of("env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED");
        assert_eq!(settings.forced(speed, &nothing), None);
        let shell =
            |name: &str| (name == "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED").then(|| "12".to_owned());
        assert_eq!(settings.forced(speed, &shell), Some(Forced::Environment));
    }
}
