//! `/cache`: redacted inspection and explicit persistent-resource cleanup.

use crucible_app::Conversation;
use crucible_app::switching::Retained;
use crucible_core::{Cancel, PromptCacheResourceError};
use crucible_tui::{Renderer, Terminal};

use crate::cli::Fatal;

/// Shows cache state, or performs one explicit bounded cleanup pass.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
) -> Result<(), Fatal> {
    match said {
        "" | "inspect" => inspect(renderer, conversation),
        "cleanup" => cleanup(renderer, conversation),
        _ => {
            renderer.commit("! /cache accepts only `inspect` or `cleanup`")?;
            Ok(())
        }
    }
}

fn inspect<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
) -> Result<(), Fatal> {
    let runner = conversation.runner();
    let policy = runner.prompt_cache_policy();
    renderer.commit(&format!(
        "cache policy: mode={}, isolation={}, retention={}, persistent={}",
        policy.mode().as_str(),
        policy.isolation().as_str(),
        policy.retention().class().as_str(),
        policy.persistent_resources().as_str(),
    ))?;

    let capabilities = runner.prompt_cache_capabilities();
    renderer.commit(&format!(
        "declared support: {:?}; capability record {}",
        capabilities.support(),
        capabilities.record_version(),
    ))?;
    if let Some(source) = capabilities.provenance() {
        renderer.commit(&format!(
            "capability provenance: reviewed {} from {} ({})",
            source.reviewed_on(),
            source.source_url(),
            source.record_version(),
        ))?;
    }

    if let Some(attempt) = runner.prompt_cache_attempt() {
        renderer.commit(&format!(
            "last attempt: eligibility={:?}, selected={:?}, wire={:?}, disposition={:?}, outcome={:?}",
            attempt.selection.eligibility(),
            attempt.selection.selected(),
            attempt.encoding,
            attempt.disposition,
            attempt.outcome,
        ))?;
        if let Some(usage) = &attempt.usage {
            renderer.commit(&format!(
                "normalized usage: input total={}, uncached={}, cache read={}, cache write={}, output={}, reasoning={}, storage token-hours={}",
                number(usage.input.total),
                number(usage.input.uncached),
                number(usage.input.cache_read),
                number(usage.input.cache_write_or_creation),
                number(usage.output),
                number(usage.reasoning),
                number(usage.storage_token_hours),
            ))?;
        } else {
            renderer.commit("normalized usage: unreported")?;
        }
        renderer.commit(&match attempt.cost.total {
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
        })?;
        if let Some(source) = attempt.cost.source_url {
            renderer.commit(&format!("pricing provenance: {source}"))?;
        }
    } else {
        renderer
            .commit("last attempt: none yet; predicted eligibility and wire outcome are unknown")?;
    }

    match conversation.prompt_cache_resources() {
        Ok(resources) if resources.is_empty() => {
            renderer.commit("persistent resources: none")?;
        }
        Ok(resources) => {
            for (index, resource) in resources.iter().enumerate() {
                let owner = resource.binding().owner();
                renderer.commit(&format!(
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
                ))?;
            }
        }
        Err(problem) => renderer.commit(&format!("! cache inspection: {problem}"))?,
    }
    Ok(())
}

fn cleanup<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
) -> Result<(), Fatal> {
    match conversation.clean_prompt_cache(&Cancel::new()) {
        Ok(result) => renderer.commit(&format!(
            "cache cleanup: inspected {}, deleted {}, ambiguous {}, orphaned {}",
            result.inspected, result.deleted, result.ambiguous, result.orphaned,
        ))?,
        Err(problem) => renderer.commit(&format!("! cache cleanup: {problem}"))?,
    }
    Ok(())
}

/// Says what a retirement ahead of an identity switch left behind, where it
/// left anything. Which resources are retired, and when, is the
/// conversation's; this is only the sentence.
pub(super) fn retained<T: Terminal>(
    renderer: &mut Renderer<T>,
    retained: Retained,
) -> Result<(), Fatal> {
    if retained.any() {
        renderer.commit(&format!(
            "! cache retirement retained {} ambiguous and {} orphaned resource(s)",
            retained.ambiguous, retained.orphaned,
        ))?;
    }
    Ok(())
}

/// Says that an identity switch stopped because the cache could not be retired.
pub(super) fn held<T: Terminal>(
    renderer: &mut Renderer<T>,
    problem: &PromptCacheResourceError,
) -> Result<(), Fatal> {
    renderer.commit(&format!("! cache retirement: {problem}"))?;
    Ok(())
}

fn number(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_core::PromptCacheResourceState;

    #[test]
    fn unknown_numbers_are_not_rendered_as_zero() {
        assert_eq!(number(None), "unknown");
        assert_eq!(number(Some(0)), "0");
        assert_ne!(PromptCacheResourceState::Ready.as_str(), "[redacted]");
    }
}
