//! Z.ai's dialect, held to the examples its API reference and its streaming
//! guide print.

use crucible_models::{Delta, Effort, ProviderError};
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

/// A refusal of Z.ai's error page's shape, with `code` as it prints one.
fn refusal(code: &str, message: &str) -> String {
    json!({"error": {"code": code, "message": message}}).to_string()
}

#[test]
fn a_prompt_too_long_for_the_model_is_a_request_that_outgrew_its_window() {
    // `1261` (400) "Prompt too long", from the error-code page: the refusal
    // the runner makes room for and asks again, not one that ends the turn.
    let (provider, _) = at::<ZaiChat>(Zai::ZAI, 400, &refusal("1261", "Prompt too long"));

    let answer = read(&provider, asking("glm-5.3", question(), false, None));

    assert!(
        matches!(
            answer,
            Err(ProviderError::WindowExceeded { provider: "zai" })
        ),
        "{answer:?}"
    );
}

#[test]
fn a_refused_parameter_is_not_read_as_a_prompt_too_long() {
    // `1214` (400) is a field the vendor would not take: compacting would
    // send the same field again.
    let (provider, _) = at::<ZaiChat>(
        Zai::ZAI,
        400,
        &refusal("1214", "Parameter `messages` is invalid."),
    );

    let answer = read(&provider, asking("glm-5.3", question(), false, None));

    assert!(
        matches!(
            answer,
            Err(ProviderError::Refused {
                provider: "zai",
                status: 400,
                ..
            })
        ),
        "{answer:?}"
    );
}

#[test]
fn z_ai_s_code_is_z_ai_s_alone() {
    // A number is a code only within the vendor that numbered it, so another
    // dialect on the wire reads the same body as an ordinary refusal.
    let (provider, _) = at::<crate::moonshot::Kimi>(
        crate::moonshot::Moonshot::PLATFORM,
        400,
        &refusal("1261", "Prompt too long"),
    );

    let answer = read(&provider, asking("k3", question(), false, None));

    assert!(
        matches!(
            answer,
            Err(ProviderError::Refused {
                provider: "moonshot",
                ..
            })
        ),
        "{answer:?}"
    );
}

/// Whether Z.ai answering `body` with a 400 is read as a request that outgrew
/// the window.
fn outgrows(body: &serde_json::Value) -> bool {
    let (provider, _) = at::<ZaiChat>(Zai::ZAI, 400, &body.to_string());
    let answer = read(&provider, asking("glm-5.3", question(), false, None));
    match answer {
        Err(ProviderError::WindowExceeded { provider: "zai" }) => true,
        Err(ProviderError::Refused {
            provider: "zai", ..
        }) => false,
        other => panic!("a refusal, not {other:?}"),
    }
}

// The error page prints the code as a string under `error`; the OpenAPI
// reference has it as an integer, at the top level with no wrapper. Each is
// read.

#[test]
fn a_prompt_too_long_with_an_integer_code_under_error_outgrew_the_window() {
    assert!(outgrows(
        &json!({"error": {"code": 1261, "message": "Prompt too long"}})
    ));
}

#[test]
fn a_prompt_too_long_with_an_integer_code_at_the_top_level_outgrew_the_window() {
    assert!(outgrows(
        &json!({"code": 1261, "message": "Prompt too long"})
    ));
}

#[test]
fn a_prompt_too_long_with_a_string_code_at_the_top_level_outgrew_the_window() {
    assert!(outgrows(
        &json!({"code": "1261", "message": "Prompt too long"})
    ));
}

#[test]
fn another_code_in_those_shapes_is_still_only_a_refusal() {
    for body in [
        json!({"error": {"code": 1214, "message": "Parameter is invalid."}}),
        json!({"code": 1214, "message": "Parameter is invalid."}),
        // A number that is not exactly the code is not the code.
        json!({"code": 12610, "message": "Prompt too long"}),
        json!({"code": 1261.5, "message": "Prompt too long"}),
    ] {
        assert!(!outgrows(&body), "{body}");
    }
}
