//! Z.ai's dialect, held to the examples its API reference and its streaming
//! guide print.

use crucible_models::{Delta, Effort};
use crucible_types::{InputTokenUsage, ProviderNumericDetail, ProviderUsage, StopReason};
use serde_json::json;

use crate::completions::testing::{asking, at, field, framed, question, read, sent};

use super::{Zai, ZaiChat};

/// The streamed example of the streaming guide, "Response Example", read
/// 2026-10-01, with the page's own elision line between its chunks.
const STREAM: &str = include_str!("fixtures/stream-text.sse");

#[test]
fn a_request_goes_to_the_site_of_the_key_with_no_field_the_vendor_does_not_document() {
    for (site, address) in [
        (Zai::ZAI, "https://api.z.ai/api/paas/v4/chat/completions"),
        (
            Zai::BIGMODEL,
            "https://open.bigmodel.cn/api/paas/v4/chat/completions",
        ),
    ] {
        let (provider, replay) = at::<ZaiChat>(site, 200, STREAM);

        read(
            &provider,
            asking("glm-5.3", question(), true, Some(Effort::Low)),
        )
        .expect("the answer reads");

        assert_eq!(replay.sent().url, address);
        let body = sent(&replay);
        assert_eq!(field(&body, "/max_tokens"), json!(512));
        assert_eq!(field(&body, "/reasoning_effort"), json!("low"));
        // Neither site documents the field, and the counts arrive unasked.
        assert_eq!(body.get("stream_options"), None);
        assert_eq!(body.get("tool_choice"), None);
    }
}

#[test]
fn the_documented_stream_reads_as_its_words_a_finish_and_its_counts() {
    let (provider, _) = at::<ZaiChat>(Zai::ZAI, 200, STREAM);

    let deltas =
        read(&provider, asking("glm-5.3", question(), false, None)).expect("the answer reads");

    let counted = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(8), Some(0)).expect("valid counts"),
        Some(262),
        None,
        Some(270),
        &[ProviderNumericDetail::new("cached_tokens", 0).expect("a detail")],
    )
    .expect("valid counts");
    assert_eq!(
        deltas,
        [
            Delta::Text("Spring".into()),
            Delta::Text(" comes".into()),
            Delta::Text(" with".into()),
            Delta::Usage(counted),
            Delta::Stopped(StopReason::Yielded),
        ]
    );
}

#[test]
fn the_reasons_to_stop_z_ai_has_words_of_its_own_for_are_read_as_they_mean() {
    for (reason, stop) in [
        ("sensitive", StopReason::Filtered),
        ("model_context_window_exceeded", StopReason::WindowExceeded),
        // A failure of the model's own, said where a reason goes: the turn
        // is unfinished, not complete.
        ("network_error", StopReason::Unknown),
    ] {
        let body = framed(&[
            json!({"choices": [{"index": 0, "delta": {"content": "Part"},
            "finish_reason": reason}]}),
        ]);
        let (provider, _) = at::<ZaiChat>(Zai::ZAI, 200, &body);

        let deltas =
            read(&provider, asking("glm-5.3", question(), false, None)).expect("the answer reads");

        assert_eq!(deltas.last(), Some(&Delta::Stopped(stop)), "{reason}");
    }
}

#[test]
fn reasoning_z_ai_clears_itself_is_not_kept() {
    let body = framed(&[
        json!({"choices": [{"index": 0, "delta": {"reasoning_content": "Hm."}}]}),
        json!({"choices": [{"index": 0, "delta": {"content": "Yes."}, "finish_reason": "stop"}]}),
    ]);
    let (provider, _) = at::<ZaiChat>(Zai::ZAI, 200, &body);

    let deltas =
        read(&provider, asking("glm-5.3", question(), false, None)).expect("the answer reads");

    assert_eq!(
        deltas,
        [
            Delta::Text("Yes.".into()),
            Delta::Stopped(StopReason::Yielded)
        ]
    );
}
