//! `/cache`: redacted inspection and explicit persistent-resource cleanup.

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::switching::Retained;
use crucible_client_api::Command;
use crucible_tui::{Renderer, Row, Terminal, fold};
use crucible_types::PromptCacheResourceError;

use crate::cli::Fatal;
use crate::cli::client::astray;

use super::{Laid, Terms};

/// Shows cache state, or performs one explicit bounded cleanup pass.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    match said {
        "" | "inspect" => inspect(renderer, conversation, terms),
        "cleanup" => cleanup(renderer, conversation, terms),
        _ => {
            reply(renderer, "! /cache accepts only `inspect` or `cleanup`")?;
            Ok(())
        }
    }
}

fn inspect<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let runner = conversation.runner();
    let policy = runner.prompt_cache_policy();
    reply(
        renderer,
        &format!(
            "cache policy: mode={}, isolation={}, retention={}, persistent={}",
            policy.mode().as_str(),
            policy.isolation().as_str(),
            policy.retention().class().as_str(),
            policy.persistent_resources().as_str(),
        ),
    )?;

    let capabilities = runner.prompt_cache_capabilities();
    reply(
        renderer,
        &format!(
            "declared support: {:?}; capability record {}",
            capabilities.support(),
            capabilities.record_version(),
        ),
    )?;
    if let Some(source) = capabilities.provenance() {
        reply(
            renderer,
            &format!(
                "capability provenance: reviewed {} from {} ({})",
                source.reviewed_on(),
                source.source_url(),
                source.record_version(),
            ),
        )?;
    }

    if let Some(attempt) = runner.prompt_cache_attempt() {
        reply(
            renderer,
            &format!(
                "last attempt: eligibility={:?}, selected={:?}, wire={:?}, disposition={:?}, outcome={:?}",
                attempt.selection.eligibility(),
                attempt.selection.selected(),
                attempt.encoding,
                attempt.disposition,
                attempt.outcome,
            ),
        )?;
        if let Some(usage) = &attempt.usage {
            reply(
                renderer,
                &format!(
                    "normalized usage: input total={}, uncached={}, cache read={}, cache write={}, output={}, reasoning={}, storage token-hours={}",
                    number(usage.input.total),
                    number(usage.input.uncached),
                    number(usage.input.cache_read),
                    number(usage.input.cache_write_or_creation),
                    number(usage.output),
                    number(usage.reasoning),
                    number(usage.storage_token_hours),
                ),
            )?;
        } else {
            reply(renderer, "normalized usage: unreported")?;
        }
        reply(
            renderer,
            &match attempt.cost.total {
                Some(total) => format!(
                    "normalized cost: {} femtocurrency ({}, {})",
                    total.femtocurrency(),
                    total.currency().as_str(),
                    attempt
                        .cost
                        .pricing_version
                        .unwrap_or("unknown pricing version"),
                ),
                None => "normalized cost: unknown".to_owned(),
            },
        )?;
        if let Some(source) = attempt.cost.source_url {
            reply(renderer, &format!("pricing provenance: {source}"))?;
        }
    } else {
        reply(
            renderer,
            "last attempt: none yet; predicted eligibility and wire outcome are unknown",
        )?;
    }

    let listed = match terms.perform(conversation, Command::InspectCache) {
        Performed::Cache(listed) => listed,
        other => return reply(renderer, &astray(&other)),
    };
    match listed {
        Ok(resources) if resources.is_empty() => {
            reply(renderer, "persistent resources: none")?;
        }
        Ok(resources) => {
            for (index, resource) in resources.iter().enumerate() {
                let owner = resource.binding().owner();
                reply(
                    renderer,
                    &format!(
                        "persistent resource {}: state={}, expires={}, owner={}/{}, provider={}",
                        index + 1,
                        resource.state().as_str(),
                        number(resource.expires_at()),
                        owner.isolation().as_str(),
                        if owner.exclusive() {
                            "exclusive"
                        } else {
                            "shared"
                        },
                        resource.binding().protocol(),
                    ),
                )?;
            }
        }
        Err(problem) => reply(renderer, &format!("! cache inspection: {problem}"))?,
    }
    Ok(())
}

fn cleanup<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let cleaned = match terms.perform(conversation, Command::CleanCache) {
        Performed::Cleaned(cleaned) => cleaned,
        other => return reply(renderer, &astray(&other)),
    };
    match cleaned {
        Ok(result) => reply(
            renderer,
            &format!(
                "cache cleanup: inspected {}, deleted {}, ambiguous {}, orphaned {}",
                result.inspected, result.deleted, result.ambiguous, result.orphaned,
            ),
        )?,
        Err(problem) => reply(renderer, &format!("! cache cleanup: {problem}"))?,
    }
    Ok(())
}

/// Says what a retirement ahead of an identity switch left behind, where it
/// left anything. Which resources are retired, and when, is the
/// conversation's; this is only the sentence.
pub(super) fn retained<T: Terminal>(
    renderer: &mut Renderer<T>,
    laid: Laid,
    retained: Retained,
) -> Result<(), Fatal> {
    if retained.any() {
        reply_at(
            renderer,
            laid,
            &format!(
                "! cache retirement retained {} ambiguous and {} orphaned resource(s)",
                retained.ambiguous, retained.orphaned,
            ),
        )?;
    }
    Ok(())
}

/// Says that an identity switch stopped because the cache could not be retired.
pub(super) fn held<T: Terminal>(
    renderer: &mut Renderer<T>,
    laid: Laid,
    problem: &PromptCacheResourceError,
) -> Result<(), Fatal> {
    reply_at(renderer, laid, &format!("! cache retirement: {problem}"))
}

/// Writes one line of the reply, folded short of the mark it is hung under.
///
/// The reply is hung under the line that asked once it is written, and a line
/// left to the window to fold runs over onto rows back at the left edge, out
/// from under the mark. So each line is laid out here, short of the window by
/// the mark, as `/context` and `/usage` lay out theirs.
fn reply<T: Terminal>(renderer: &mut Renderer<T>, said: &str) -> Result<(), Fatal> {
    reply_at(renderer, Laid::Hung, said)
}

/// Writes one line, folded to where it will stand: hung, or at the left edge
/// for a `/model` pick applied as the turn it was made over ends.
fn reply_at<T: Terminal>(renderer: &mut Renderer<T>, laid: Laid, said: &str) -> Result<(), Fatal> {
    let rows: Vec<Row> = fold(said, laid.columns(renderer))
        .into_iter()
        .map(Row::plain)
        .collect();

    Ok(renderer.present(&rows)?)
}

fn number(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_tui::{Glyphs, Recording, ScreenMode};
    use crucible_types::PromptCacheResourceState;

    #[test]
    fn unknown_numbers_are_not_rendered_as_zero() {
        assert_eq!(number(None), "unknown");
        assert_eq!(number(Some(0)), "0");
        assert_ne!(PromptCacheResourceState::Ready.as_str(), "[redacted]");
    }

    /// The rows a window forty columns wide drawn in `mode` shows once `said`
    /// has written its line, hung as a command's reply is hung.
    ///
    /// Native mode's rows are read from the last frame it drew, which redraws
    /// the whole region: nothing here waits for a key, so nothing has been
    /// sealed into the scrollback above it.
    fn hung(
        mode: ScreenMode,
        said: impl FnOnce(&mut Renderer<Recording>) -> Result<(), Fatal>,
    ) -> Vec<String> {
        let mut renderer = Renderer::drawing(Recording::new(40, 40), mode);
        let start = renderer.lines();
        renderer.hangs(Glyphs::Unicode);
        said(&mut renderer).expect("the line to be written");
        renderer
            .subordinate(start, Glyphs::Unicode)
            .expect("the reply to be hung");

        match mode {
            ScreenMode::Fullscreen => renderer.terminal().picture().rows(),
            ScreenMode::Native => last_frame(renderer.terminal().written()),
        }
    }

    /// The rows the last native frame wrote, top first: what follows the
    /// erase that opens it, with every control sequence read past.
    fn last_frame(written: &str) -> Vec<String> {
        let drawn = written.rsplit("\x1b[J").next().unwrap_or_default();
        let mut text = String::new();
        let mut left = drawn.chars();
        while let Some(character) = left.next() {
            if character != '\x1b' {
                text.push(character);
                continue;
            }
            if left.next() == Some('[') {
                for byte in left.by_ref() {
                    if ('@'..='~').contains(&byte) {
                        break;
                    }
                }
            }
        }
        text.split("\r\n")
            .map(|row| row.trim_end().to_owned())
            .collect()
    }

    /// The reply's rows, from the one carrying the mark to the first blank.
    fn reply_rows(mode: ScreenMode, rows: &[String]) -> Vec<String> {
        let opened = rows
            .iter()
            .position(|row| row.starts_with(Glyphs::Unicode.hangs()))
            .unwrap_or_else(|| panic!("{mode:?}: nothing was hung in {rows:#?}"));
        rows.iter()
            .skip(opened)
            .take_while(|row| !row.is_empty())
            .cloned()
            .collect()
    }

    /// The line `/model`, `/login` and `/logout` say when retiring the cache
    /// ahead of a switch left resources behind, in a window too narrow for it.
    ///
    /// Folded at the window's forty columns, the first row would end on
    /// "ambiguous", one column past what is left beside the mark; folded short
    /// of the mark, it ends a word earlier and the rest wraps under the mark.
    fn retained_keeps_its_indent(mode: ScreenMode) {
        let rows = hung(mode, |renderer| {
            retained(
                renderer,
                Laid::Hung,
                Retained {
                    ambiguous: 3,
                    orphaned: 2,
                },
            )
        });

        assert_eq!(
            reply_rows(mode, &rows),
            [
                "⎿ ! cache retirement retained 3",
                "  ambiguous and 2 orphaned resource(s)",
            ],
            "{mode:?}: in {rows:#?}"
        );
    }

    /// The line `/model`, `/login` and `/logout` say when an identity switch
    /// stopped because the cache could not be retired, in a window too narrow
    /// for it.
    ///
    /// Folded at forty columns, the second row would carry "deadline" as well;
    /// folded short of the mark, that word wraps onto a third row of its own.
    fn held_keeps_its_indent(mode: ScreenMode) {
        let rows = hung(mode, |renderer| {
            held(renderer, Laid::Hung, &PromptCacheResourceError::Deadline)
        });

        assert_eq!(
            reply_rows(mode, &rows),
            [
                "⎿ ! cache retirement: prompt-cache",
                "  resource operation reached its",
                "  deadline",
            ],
            "{mode:?}: in {rows:#?}"
        );
    }

    /// The same line said where nothing is hung, as for a `/model` pick
    /// applied once the turn it was made over ends: folded at the window's
    /// forty columns, the first row carries "ambiguous".
    #[test]
    fn retained_resources_said_at_the_left_edge_fold_at_the_whole_window() {
        let mut renderer = Renderer::drawing(Recording::new(40, 40), ScreenMode::Fullscreen);
        retained(
            &mut renderer,
            Laid::Flush,
            Retained {
                ambiguous: 3,
                orphaned: 2,
            },
        )
        .expect("the line to be written");

        let rows = renderer.terminal().picture().rows();
        let opened = rows
            .iter()
            .position(|row| row.starts_with("! cache retirement"))
            .unwrap_or_else(|| panic!("nothing was said in {rows:#?}"));
        assert_eq!(
            rows.get(opened..opened + 2).unwrap_or_default(),
            [
                "! cache retirement retained 3 ambiguous",
                "and 2 orphaned resource(s)",
            ],
            "{rows:#?}"
        );
    }

    #[test]
    fn retained_resources_keep_the_indent_where_the_line_wraps() {
        retained_keeps_its_indent(ScreenMode::Fullscreen);
    }

    #[test]
    fn retained_resources_keep_the_indent_where_the_line_wraps_in_native_mode() {
        retained_keeps_its_indent(ScreenMode::Native);
    }

    #[test]
    fn a_held_retirement_keeps_the_indent_where_the_line_wraps() {
        held_keeps_its_indent(ScreenMode::Fullscreen);
    }

    #[test]
    fn a_held_retirement_keeps_the_indent_where_the_line_wraps_in_native_mode() {
        held_keeps_its_indent(ScreenMode::Native);
    }
}
