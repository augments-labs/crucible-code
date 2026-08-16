//! Reading a call's arguments, once, at the edge.
//!
//! Arguments arrive as the JSON text the model wrote. They are turned into the
//! shape a tool wants here and nowhere else, so no tool re-checks a field
//! another tool already checked, and a malformed call is refused before any of
//! it reaches the filesystem.
//!
//! Every problem is phrased for the model, because the model is what reads it
//! and what can send a corrected call.

use crucible_core::{ToolArgs, ToolError};
use serde_json::Value;

/// One call's arguments, parsed.
#[derive(Debug)]
pub(crate) struct Args {
    /// Named so every rejection says which tool rejected it.
    tool: &'static str,
    /// Where these arguments sit in the call, for one element of a list. A
    /// rejection carries it, because `find is required` about the second of
    /// four edits does not say which one to fix.
    within: Option<Box<str>>,
    value: Value,
}

impl Args {
    /// Parses the text the model sent.
    ///
    /// Nothing at all is an empty object: a tool whose fields are all optional
    /// is legitimately called with no arguments, and providers spell that
    /// several ways.
    pub(crate) fn parse(tool: &'static str, args: &ToolArgs) -> Result<Self, ToolError> {
        let text = args.as_str().trim();
        let whole = |value| Self {
            tool,
            within: None,
            value,
        };
        if text.is_empty() {
            return Ok(whole(Value::Object(serde_json::Map::new())));
        }

        let value: Value = serde_json::from_str(text).map_err(|problem| ToolError::Arguments {
            tool,
            problem: format!("arguments are not valid JSON: {problem}").into(),
        })?;

        if value.is_object() {
            Ok(whole(value))
        } else {
            Err(ToolError::Arguments {
                tool,
                problem: "arguments must be a JSON object".into(),
            })
        }
    }

    /// Whether the call mentions a field at all, however it filled it.
    ///
    /// For a tool that takes one of two shapes and has to say so rather than
    /// quietly read one of them.
    pub(crate) fn holds(&self, field: &str) -> bool {
        self.value.get(field).is_some_and(|found| !found.is_null())
    }

    /// The elements of a list field, each read as arguments of its own.
    ///
    /// An element is an object like the call itself, so the same helpers read
    /// it and its rejections read the same way — with `within` set, so one says
    /// which element it came from.
    ///
    /// Each element is copied out. The arguments are already held twice, as the
    /// text the model sent and as this tree, and one element is a fraction of
    /// either; what a borrow would buy is not worth a lifetime on every tool's
    /// arguments.
    pub(crate) fn list(&self, field: &str) -> Result<Option<Vec<Self>>, ToolError> {
        let Some(found) = self.value.get(field) else {
            return Ok(None);
        };
        if found.is_null() {
            return Ok(None);
        }

        let items = found
            .as_array()
            .ok_or_else(|| self.wrong(format!("{field} must be a list")))?;

        items
            .iter()
            .enumerate()
            .map(|(at, item)| {
                if item.is_object() {
                    Ok(Self {
                        tool: self.tool,
                        within: Some(format!("{field}[{at}]").into()),
                        value: item.clone(),
                    })
                } else {
                    Err(self.wrong(format!("{field}[{at}] must be a JSON object")))
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// A field that must be there and must not be blank.
    pub(crate) fn text(&self, field: &str) -> Result<&str, ToolError> {
        match self.optional_text(field)? {
            Some(text) => Ok(text),
            None => Err(self.wrong(format!("{field} is required"))),
        }
    }

    /// A field that must be there but may be empty.
    ///
    /// [`Args::text`] treats blank as missing, which is right for a path and
    /// wrong for content: writing an empty file and replacing text with
    /// nothing are both things a model asks for on purpose.
    pub(crate) fn exact(&self, field: &str) -> Result<&str, ToolError> {
        let Some(found) = self.value.get(field) else {
            return Err(self.wrong(format!("{field} is required")));
        };

        found
            .as_str()
            .ok_or_else(|| self.wrong(format!("{field} must be a string")))
    }

    /// A field that may be absent. Present-but-blank counts as absent, because
    /// an empty path and no path mean the same thing to every caller here.
    pub(crate) fn optional_text(&self, field: &str) -> Result<Option<&str>, ToolError> {
        let Some(found) = self.value.get(field) else {
            return Ok(None);
        };
        if found.is_null() {
            return Ok(None);
        }

        let text = found
            .as_str()
            .ok_or_else(|| self.wrong(format!("{field} must be a string")))?;

        Ok(Some(text).filter(|text: &&str| !text.is_empty()))
    }

    /// A count, which must be positive if it is given at all. A zero limit
    /// asks for no work to be done, which is a mistake worth reporting rather
    /// than an empty answer worth returning.
    pub(crate) fn count(&self, field: &str, default: usize) -> Result<usize, ToolError> {
        let Some(found) = self.value.get(field) else {
            return Ok(default);
        };
        if found.is_null() {
            return Ok(default);
        }

        let count = found
            .as_u64()
            .filter(|count| *count > 0)
            .ok_or_else(|| self.wrong(format!("{field} must be a positive whole number")))?;

        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// A count that may be none, absent meaning `default`.
    ///
    /// [`Args::count`] refuses zero because a limit of nothing asks for no work
    /// to be done. Zero is an answer rather than a mistake where the field asks
    /// for something extra: `"context": 0` is the plain search, and a model that
    /// worked the number out rather than writing it down sends it.
    pub(crate) fn whole(&self, field: &str, default: usize) -> Result<usize, ToolError> {
        let Some(found) = self.value.get(field) else {
            return Ok(default);
        };
        if found.is_null() {
            return Ok(default);
        }

        let count = found
            .as_u64()
            .ok_or_else(|| self.wrong(format!("{field} must be a whole number")))?;

        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// A field whose value is one of a fixed set of words, absent meaning
    /// `default`.
    ///
    /// What comes back is the entry from `allowed`, not the text the model
    /// sent, so the caller matches on words it wrote itself and a borrow of the
    /// arguments does not travel with the answer.
    ///
    /// A word outside the set is refused here rather than quietly read as the
    /// default. The set is small and named in the schema, so a call that missed
    /// it asked for something this tool does not do — and silently doing the
    /// other thing is the failure the model cannot see.
    pub(crate) fn choice(
        &self,
        field: &str,
        default: &'static str,
        allowed: &[&'static str],
    ) -> Result<&'static str, ToolError> {
        let Some(sent) = self.optional_text(field)? else {
            return Ok(default);
        };

        allowed
            .iter()
            .copied()
            .find(|word| *word == sent)
            .ok_or_else(|| self.wrong(format!("{field} must be one of {}", allowed.join(", "))))
    }

    /// A flag, absent meaning `default`.
    pub(crate) fn flag(&self, field: &str, default: bool) -> Result<bool, ToolError> {
        let Some(found) = self.value.get(field) else {
            return Ok(default);
        };
        if found.is_null() {
            return Ok(default);
        }

        found
            .as_bool()
            .ok_or_else(|| self.wrong(format!("{field} must be true or false")))
    }

    /// A rejection the model can act on.
    ///
    /// The arguments phrase it themselves. Which tool is rejecting the call is
    /// something they already know, and handing it back at every rejection is
    /// the sort of repetition one wrong argument hides in. Where in the call
    /// they sit is the other half of that, and it comes from the same place.
    pub(crate) fn wrong(&self, problem: impl std::fmt::Display) -> ToolError {
        ToolError::Arguments {
            tool: self.tool,
            problem: match &self.within {
                Some(within) => format!("{within} {problem}").into(),
                None => problem.to_string().into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(json: &str) -> Result<Args, ToolError> {
        Args::parse("test", &ToolArgs::new(json))
    }

    #[test]
    fn a_required_field_comes_back_as_written() {
        let args = args(r#"{"path":"src/main.rs"}"#).unwrap();
        assert_eq!(args.text("path").unwrap(), "src/main.rs");
    }

    #[test]
    fn a_missing_required_field_names_itself() {
        let args = args("{}").unwrap();
        let problem = args.text("path").unwrap_err().to_string();
        assert_eq!(problem, "test: path is required");
    }

    #[test]
    fn a_blank_required_field_is_missing_rather_than_empty() {
        // An empty path would resolve to the workspace root, which is not what
        // a model that forgot to fill the field meant.
        let args = args(r#"{"path":""}"#).unwrap();
        assert!(args.text("path").is_err());
    }

    #[test]
    fn a_field_of_the_wrong_type_says_which_type_it_wants() {
        let args = args(r#"{"path":7}"#).unwrap();
        assert_eq!(
            args.text("path").unwrap_err().to_string(),
            "test: path must be a string"
        );
    }

    #[test]
    fn content_that_is_deliberately_empty_is_still_content() {
        // Writing an empty file is a thing a model asks for; forgetting the
        // field is not the same event and must not look like one.
        assert_eq!(
            args(r#"{"content":""}"#).unwrap().exact("content").unwrap(),
            ""
        );
        assert_eq!(
            args("{}")
                .unwrap()
                .exact("content")
                .unwrap_err()
                .to_string(),
            "test: content is required"
        );
    }

    #[test]
    fn no_arguments_at_all_is_an_empty_object() {
        // A call with every field optional arrives this way.
        assert!(args("").unwrap().optional_text("path").unwrap().is_none());
        assert!(args("  ").unwrap().optional_text("path").unwrap().is_none());
    }

    #[test]
    fn arguments_that_are_not_json_are_refused_with_the_reason() {
        let problem = args(r#"{"path":"#).unwrap_err().to_string();
        assert!(
            problem.starts_with("test: arguments are not valid JSON"),
            "got {problem}"
        );
    }

    #[test]
    fn arguments_that_are_not_an_object_are_refused() {
        assert!(args("[1,2]").is_err());
        assert!(args(r#""just a string""#).is_err());
    }

    #[test]
    fn null_reads_as_absent() {
        // Providers fill an omitted optional field with null often enough that
        // treating it as "present, wrong type" would reject working calls.
        let args = args(r#"{"path":null,"limit":null,"deep":null}"#).unwrap();

        assert!(args.optional_text("path").unwrap().is_none());
        assert_eq!(args.count("limit", 40).unwrap(), 40);
        assert!(args.flag("deep", true).unwrap());
    }

    #[test]
    fn a_count_must_be_a_positive_whole_number() {
        assert_eq!(
            args(r#"{"limit":5}"#).unwrap().count("limit", 1).unwrap(),
            5
        );
        assert!(args(r#"{"limit":0}"#).unwrap().count("limit", 1).is_err());
        assert!(args(r#"{"limit":-3}"#).unwrap().count("limit", 1).is_err());
        assert!(args(r#"{"limit":"5"}"#).unwrap().count("limit", 1).is_err());
    }

    #[test]
    fn a_whole_number_may_be_none_of_something_but_never_less() {
        // Asking for no context is asking for the plain search, which is a
        // thing to do rather than a mistake to report.
        assert_eq!(
            args(r#"{"context":0}"#)
                .unwrap()
                .whole("context", 2)
                .unwrap(),
            0
        );
        assert_eq!(args("{}").unwrap().whole("context", 2).unwrap(), 2);
        assert_eq!(
            args(r#"{"context":null}"#)
                .unwrap()
                .whole("context", 2)
                .unwrap(),
            2
        );
        assert_eq!(
            args(r#"{"context":-1}"#)
                .unwrap()
                .whole("context", 2)
                .unwrap_err()
                .to_string(),
            "test: context must be a whole number"
        );
        assert!(
            args(r#"{"context":"3"}"#)
                .unwrap()
                .whole("context", 2)
                .is_err()
        );
    }

    #[test]
    fn a_choice_is_one_of_its_words_or_a_rejection_naming_them_all() {
        let words = ["content", "files"];

        assert_eq!(
            args(r#"{"mode":"files"}"#)
                .unwrap()
                .choice("mode", "content", &words)
                .unwrap(),
            "files"
        );
        assert_eq!(
            args("{}")
                .unwrap()
                .choice("mode", "content", &words)
                .unwrap(),
            "content"
        );
        assert_eq!(
            args(r#"{"mode":null}"#)
                .unwrap()
                .choice("mode", "content", &words)
                .unwrap(),
            "content"
        );
        assert_eq!(
            args(r#"{"mode":"paths"}"#)
                .unwrap()
                .choice("mode", "content", &words)
                .unwrap_err()
                .to_string(),
            "test: mode must be one of content, files"
        );
    }

    #[test]
    fn a_flag_falls_back_to_its_default() {
        assert!(!args("{}").unwrap().flag("deep", false).unwrap());
        assert!(
            args(r#"{"deep":true}"#)
                .unwrap()
                .flag("deep", false)
                .unwrap()
        );
        assert!(
            args(r#"{"deep":"yes"}"#)
                .unwrap()
                .flag("deep", false)
                .is_err()
        );
    }
}
