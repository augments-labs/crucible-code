//! Reading rule text into a rule.
//!
//! `read(src/**)`, `bash(cargo test)`, `bash(*)`, or a tool name on its own.
//! Nothing else is a rule, and anything else is refused here rather than
//! interpreted charitably — a rule that was silently read as something narrower
//! than it looks is the one failure this whole mechanism cannot survive.

use std::path::Path;

use globset::GlobBuilder;

use super::{Pattern, Rule, RuleError};

/// Reads one rule.
pub(super) fn rule(text: &str) -> Result<Rule, RuleError> {
    let trimmed = text.trim();

    let Some((tool, rest)) = trimmed.split_once('(') else {
        // A tool named on its own is the blanket, which is what `bash(*)` also
        // means. Two spellings of one rule, because both get written.
        return Ok(Rule {
            tool: named(trimmed, text)?,
            pattern: Pattern::Blanket,
        });
    };

    let Some(inside) = rest.strip_suffix(')') else {
        return Err(RuleError::Shape { text: text.into() });
    };

    Ok(Rule {
        tool: named(tool, text)?,
        pattern: pattern(inside, text)?,
    })
}

/// The tool part, which has to name one.
fn named(tool: &str, text: &str) -> Result<Box<str>, RuleError> {
    let tool = tool.trim();
    if tool.is_empty() || tool.contains(['(', ')']) {
        return Err(RuleError::NoTool { text: text.into() });
    }
    Ok(tool.into())
}

/// The part inside the brackets.
fn pattern(inside: &str, text: &str) -> Result<Pattern, RuleError> {
    let inside = inside.trim();
    if inside.is_empty() {
        return Err(RuleError::Shape { text: text.into() });
    }
    if inside == "*" {
        return Ok(Pattern::Blanket);
    }

    Ok(Pattern::Glob {
        path: glob(inside, text, true)?,
        text: glob(inside, text, false)?,
        absolute: Path::new(inside).is_absolute(),
    })
}

/// Compiles the pattern, with or without `*` stopping at a path separator.
fn glob(
    inside: &str,
    text: &str,
    literal_separator: bool,
) -> Result<globset::GlobMatcher, RuleError> {
    GlobBuilder::new(inside)
        .literal_separator(literal_separator)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|problem| RuleError::Pattern {
            text: text.into(),
            problem: problem.to_string().into(),
        })
}
