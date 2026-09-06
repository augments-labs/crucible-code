//! Function results admit text and images, not the full user-input media union.
//!
//! Other resolved media follows all results in labelled user-input steps. This
//! client projection keeps the native call/result sequence together and names
//! the owning call without forging an unsupported function-result content type.
//! Attachment indexes are message-wide, so missing files cannot shift owners.

use super::input::attachment;
use crate::google::protocol;
use crate::json::Array;
use crucible_core::{Attached, Content, Modality, ProviderError, ToolResult};

pub(super) fn write(
    input: &mut Array<'_>,
    results: &[ToolResult],
    attached: &[Attached<'_>],
    message: usize,
) -> Result<(), ProviderError> {
    let mut index = 0;
    for result in results {
        let mut files = files(attached, message, &mut index, result)?
            .filter(|file| in_result(file))
            .peekable();
        let mut outcome = Ok(());
        input.object(|step| {
            step.text("type", "function_result");
            step.text("call_id", result.id.as_str());
            step.boolean("is_error", result.output.is_failed());
            if files.peek().is_none() {
                step.text("result", result.output.text());
            } else {
                step.array("result", |content| {
                    content.object(|part| {
                        part.text("type", "text");
                        part.text("text", result.output.text());
                    });
                    for file in files {
                        if outcome.is_ok() {
                            content.object(|part| outcome = attachment(part, file));
                        }
                    }
                });
            }
        });
        outcome?;
    }
    let mut index = 0;
    for result in results {
        let mut files = files(attached, message, &mut index, result)?
            .filter(|file| !in_result(file))
            .peekable();
        if files.peek().is_none() {
            continue;
        }
        let mut outcome = Ok(());
        input.object(|step| {
            step.text("type", "user_input");
            step.array("content", |content| {
                content.object(|part| {
                    part.text("type", "text");
                    part.text_with("text", |write| {
                        write("Attachments from tool call ");
                        write(result.id.as_str());
                        write(":");
                    });
                });
                for file in files {
                    if outcome.is_ok() {
                        content.object(|part| outcome = attachment(part, file));
                    }
                }
            });
        });
        outcome?;
    }
    Ok(())
}

fn in_result(file: &Attached<'_>) -> bool {
    matches!(file.content, Content::Instead(_))
        || matches!(file.modality, Modality::Text | Modality::Image)
}

fn files<'a, 'b>(
    attached: &'a [Attached<'b>],
    message: usize,
    index: &mut usize,
    result: &ToolResult,
) -> Result<impl Iterator<Item = &'a Attached<'b>>, ProviderError> {
    let start = *index;
    let end = start
        .checked_add(result.output.attachments().len())
        .ok_or_else(|| protocol("too many tool result attachments"))?;
    *index = end;
    Ok(attached
        .iter()
        .filter(move |file| file.message == message && (start..end).contains(&file.index)))
}
