use crucible_core::{ToolOutput, ToolResult};

use super::*;

/// A tool result of `bytes` bytes, for filling a transcript.
fn results(bytes: usize) -> Message {
    Message::ToolResults(vec![ToolResult {
        id: crucible_core::ToolId::new("call-1"),
        output: ToolOutput::ok("x".repeat(bytes)),
    }])
}

#[test]
fn nothing_has_been_reported_so_the_load_is_what_was_appended_at_a_cautious_rate() {
    let mut load = Load::default();
    load.recorded(&results(3_000));

    // No response has been seen, so there is no rate to use but the pessimistic
    // one. Over-stating here costs an early compaction; under-stating costs the
    // turn.
    assert_eq!(load.tokens(), 1_002);
}

#[test]
fn an_unreported_request_includes_system_instructions_and_tool_schemas() {
    let mut load = Load::default();
    load.recorded(&Message::User("x".repeat(300).into()));
    let tools = [ToolSchema {
        name: "read",
        schema: Box::leak("s".repeat(536).into_boxed_str()),
    }];

    load.requesting(Some(&"i".repeat(60)), &tools);

    // 300 transcript bytes + 60 instructions + 4 name + 536 schema + the
    // conservative 64-byte provider wrapper, at three bytes per token.
    assert_eq!(load.tokens(), 322);
    assert_eq!(load.left(Some(200_000)), Some(99));
}

#[test]
fn same_sized_new_request_overhead_is_shown_as_a_conservative_estimate() {
    let mut load = Load::default();
    load.recorded(&Message::User("request".into()));
    load.requesting(Some("system one"), &[]);
    load.responding();
    load.carried(Carried::new(100));
    assert_eq!(load.left(Some(200)), Some(50));

    load.requesting(Some("system two"), &[]);

    assert_eq!(load.left(Some(200)), Some(20));
    assert!(
        load.tokens() > 100,
        "the replacement overhead disappeared from the estimate"
    );
}

#[test]
fn a_provider_report_supersedes_system_and_tool_estimates_whole() {
    let mut load = Load::default();
    load.recorded(&Message::User("x".repeat(300).into()));
    let tools = [ToolSchema {
        name: "read",
        schema: Box::leak("s".repeat(536).into_boxed_str()),
    }];
    load.requesting(Some(&"i".repeat(60)), &tools);
    load.responding();

    load.carried(Carried::new(1_000));

    assert_eq!(load.tokens(), 1_000, "request overhead was counted twice");
    assert_eq!(load.left(Some(2_000)), Some(50));
}

#[test]
fn a_reported_count_is_used_whole_rather_than_estimated() {
    let mut load = Load::default();
    load.carried(Carried::new(120_000));
    load.spent(Spend::new(2_000));

    // Nothing appended since, so the load is exactly what the provider said the
    // request carried plus what it produced. No estimate is involved at all.
    assert_eq!(load.tokens(), 122_000);
}

#[test]
fn what_was_appended_since_is_estimated_at_this_models_own_rate() {
    let mut load = Load::default();

    // A transcript of 400 000 bytes went out and the provider said it carried
    // 100 000 tokens, so this model reads four bytes to the token here.
    load.recorded(&results(400_000));
    load.carried(Carried::new(100_000));
    load.spent(Spend::new(1_000));
    load.recorded(&results(40_000));

    // Result IDs are request metadata too. At the calibrated rate, this
    // appended result conservatively rounds up to 10 002 tokens rather than
    // using the uncalibrated 13 336.
    assert_eq!(load.tokens(), 111_002);
}

#[test]
fn the_answer_is_counted_once_rather_than_measured_as_well() {
    let mut load = Load::default();
    load.carried(Carried::new(100_000));
    load.spent(Spend::new(5_000));

    let mut counted_twice = load;
    counted_twice.recorded(&Message::Agent {
        text: "x".repeat(20_000).into(),
        calls: Vec::new(),
        stop: None,
    });

    // The agent's own message costs what the provider said it cost. Estimating
    // its bytes as well would put one answer into the load twice.
    assert_eq!(counted_twice.tokens(), load.tokens());
}

#[test]
fn agent_prose_is_estimated_when_an_existing_transcript_is_recounted() {
    let mut load = Load::default();
    load.recounted(&Message::Agent {
        text: "x".repeat(3_000).into(),
        calls: Vec::new(),
        stop: None,
    });

    assert_eq!(load.tokens(), 1_000);
    assert_eq!(load.left(Some(200_000)), Some(99));
}

#[test]
fn a_response_supersedes_the_last_one_rather_than_adding_to_it() {
    let mut load = Load::default();
    load.carried(Carried::new(100_000));
    load.spent(Spend::new(2_000));
    load.recorded(&results(4_000));

    // The transcript goes whole to the provider every time, so the next
    // response's input already includes the preceding response's output and
    // everything appended after it. Neither may remain beside the new count.
    load.responding();
    load.carried(Carried::new(104_000));

    assert_eq!(load.tokens(), 104_000, "old output was counted twice");
}

#[test]
fn locally_estimated_tool_output_updates_an_older_exact_percentage() {
    let mut load = Load::default();
    load.carried(Carried::new(50_000));
    assert_eq!(load.left(Some(200_000)), Some(75));

    load.recorded(&results(30_000));

    assert_eq!(load.left(Some(200_000)), Some(69));
    assert!(load.tokens() > 50_000, "the estimate stopped counting");
}

#[test]
fn response_growth_is_estimated_until_output_usage_catches_up() {
    let mut load = Load::default();
    load.recorded(&results(200_000));
    load.responding();
    load.carried(Carried::new(50_000));
    load.produced(40_000);
    assert_eq!(load.left(Some(200_000)), Some(70));

    load.spent(Spend::new(10_000));
    assert_eq!(load.left(Some(200_000)), Some(70));
}

#[test]
fn a_late_input_report_preserves_unreported_response_growth() {
    let mut load = Load::default();
    load.recorded(&results(200_000));
    load.responding();
    load.produced(40_000);

    // Input and output counts are independent wire facts. An input count that
    // arrives after text covers the request, not the response bytes already
    // seen, so those bytes remain estimated at the new four-byte rate.
    load.carried(Carried::new(50_000));

    assert_eq!(load.left(Some(200_000)), Some(70));
}

#[test]
fn unreported_response_output_is_estimated_when_it_is_recorded() {
    let mut load = Load::default();
    load.recorded(&results(3_000));
    load.carried(Carried::new(1_000));
    load.produced(3_000);
    load.recorded(&Message::Agent {
        text: "x".repeat(3_000).into(),
        calls: Vec::new(),
        stop: None,
    });

    // The six-byte result ID is part of the calibration, so 3 000 bytes of
    // prose conservatively round up to 999 tokens at that request's rate.
    assert_eq!(load.tokens(), 1_999);
    assert_eq!(load.left(Some(200_000)), Some(99));
}

#[test]
fn output_after_an_exact_partial_spend_is_the_only_part_estimated_on_recording() {
    let mut load = Load::default();
    load.recorded(&results(400_000));
    load.responding();
    load.carried(Carried::new(100_000));

    // The provider counted the first 400 bytes exactly, then another 200 bytes
    // arrived before the complete 600-byte message was recorded.
    load.produced(400);
    load.spent(Spend::new(100));
    load.produced(200);
    load.recorded(&Message::Agent {
        text: "x".repeat(600).into(),
        calls: Vec::new(),
        stop: None,
    });

    assert_eq!(
        load.tokens(),
        100_150,
        "the exact prefix was estimated again"
    );
    assert_eq!(load.left(Some(200_000)), Some(49));
    assert!(!load.full(Some(100_151), 0));
}

#[test]
fn an_estimate_has_a_conservative_window_percentage() {
    let mut load = Load::default();
    load.recorded(&results(50_000));

    // 50 006 bytes at the cautious three-byte rate round up to 16 669 tokens.
    // The percentage rounds down in turn, so it never claims the fractional
    // room the estimate has already reserved.
    assert_eq!(load.left(Some(200_000)), Some(91));

    load.carried(Carried::new(50_000));
    assert_eq!(load.left(Some(200_000)), Some(75));
}

#[test]
fn how_much_is_left_is_nothing_at_all_where_no_window_is_known() {
    let mut load = Load::default();
    load.carried(Carried::new(50_000));

    assert_eq!(load.left(None), None);
    assert_eq!(load.left(Some(200_000)), Some(75));
}

#[test]
fn a_window_already_over_full_reads_as_none_left_rather_than_wrapping() {
    let mut load = Load::default();
    load.carried(Carried::new(300_000));

    assert_eq!(load.left(Some(200_000)), Some(0));
}

#[test]
fn bytes_become_tokens_at_the_rate_the_last_response_proved() {
    // Uncalibrated, the pessimistic divisor over-counts — the direction that
    // keeps less rather than the one that costs the turn.
    let mut load = Load::default();
    assert_eq!(load.bytes_to_tokens(3_000), 1_000);

    // Once a response has said what the result and its six-byte ID carried,
    // the same bytes are read at that rate instead. A fractional token rounds
    // up because this estimate protects a request boundary.
    load.recorded(&results(400_000));
    load.carried(Carried::new(100_000));
    assert_eq!(load.bytes_to_tokens(40_000), 10_000);
}

#[test]
fn the_reserve_is_the_answer_and_a_pass_of_tool_results() {
    // 16 000 for the answer, and two tool results at the rate above.
    assert_eq!(reserve(16_000, Some(1_000_000), None), 36_000);
}

#[test]
fn a_window_too_small_to_hold_the_reserve_keeps_half_of_itself_instead() {
    // Reserving 36 000 of an 8 000 window leaves nothing, which is a session
    // that compacts on its first turn and every turn after it.
    assert_eq!(reserve(16_000, Some(8_000), None), 4_000);
}

#[test]
fn a_reserve_somebody_wrote_down_is_the_one_that_is_used() {
    assert_eq!(reserve(16_000, Some(1_000_000), Some(90_000)), 90_000);

    // And is held to the same half-window ceiling, because the arithmetic that
    // makes an over-large reserve unusable does not care who chose it.
    assert_eq!(reserve(16_000, Some(8_000), Some(90_000)), 4_000);
}

#[test]
fn a_window_is_full_once_there_is_no_room_left_for_another_exchange() {
    let mut load = Load::default();
    load.carried(Carried::new(160_000));

    // Exactly the production rule: carried + reserve >= window, equivalently
    // carried >= window - reserve. One token below the boundary still fits;
    // the boundary itself compacts before another request is sent.
    assert!(!load.full(Some(200_000), 39_999));
    assert!(load.full(Some(200_000), 40_000));

    // And never, where no window is known: nothing here decides a session is
    // full against a number nobody stated.
    assert!(!load.full(None, 36_000));
}
