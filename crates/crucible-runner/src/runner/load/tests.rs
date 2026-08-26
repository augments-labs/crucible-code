use crucible_core::{ToolOutput, ToolResult};

use super::*;

/// A prompt of `bytes` bytes that also named one file.
///
/// The file's own bytes are nowhere in it — a message holds a reference, and
/// that is the half of `C3` this module gets for free from the shape of the
/// type rather than from arithmetic.
fn attaching(bytes: usize) -> Message {
    Message::User {
        text: "x".repeat(bytes).into(),
        attachments: vec![crucible_core::Attachment {
            path: "/holiday.png".into(),
            modality: crucible_core::Modality::Image,
            media_type: "image/png".into(),
            hash: [0; 32],
        }]
        .into(),
    }
}

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
    load.recorded(&Message::said("x".repeat(300)));
    let tools = [ToolSchema {
        name: "read",
        schema: Box::leak("s".repeat(536).into_boxed_str()),
    }];

    load.requesting(Some(&"i".repeat(60)), &tools);

    // 300 transcript bytes + 60 instructions + 4 name + 536 schema + the
    // conservative 64-byte provider wrapper, at three bytes per token.
    assert_eq!(load.tokens(), 322);
    assert_eq!(load.left(Some(200_000), 0), Some(99));
}

#[test]
fn same_sized_new_request_overhead_is_shown_as_a_conservative_estimate() {
    let mut load = Load::default();
    load.recorded(&Message::said("request"));
    load.requesting(Some("system one"), &[]);
    load.responding(0);
    load.carried(Carried::new(100));
    // The ten fixed bytes are 59 tokens at this session's reported rate, and
    // they come out of the denominator: 100 of 141 transcript tokens left.
    assert_eq!(load.left(Some(200), 0), Some(70));

    load.requesting(Some("system two"), &[]);

    assert_eq!(load.left(Some(200), 0), Some(29));
    assert!(
        load.tokens() > 100,
        "the replacement overhead disappeared from the estimate"
    );
}

#[test]
fn a_provider_report_supersedes_system_and_tool_estimates_whole() {
    let mut load = Load::default();
    load.recorded(&Message::said("x".repeat(300)));
    let tools = [ToolSchema {
        name: "read",
        schema: Box::leak("s".repeat(536).into_boxed_str()),
    }];
    load.requesting(Some(&"i".repeat(60)), &tools);
    load.responding(0);

    load.carried(Carried::new(1_000));

    assert_eq!(load.tokens(), 1_000, "request overhead was counted twice");
    // The 664 fixed bytes are 689 tokens at the reported rate; what is left is
    // measured against the 1 311 the transcript may use.
    assert_eq!(load.left(Some(2_000), 0), Some(76));
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
    assert_eq!(load.left(Some(200_000), 0), Some(99));
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
    load.responding(0);
    load.carried(Carried::new(104_000));

    assert_eq!(load.tokens(), 104_000, "old output was counted twice");
}

#[test]
fn locally_estimated_tool_output_updates_an_older_exact_percentage() {
    let mut load = Load::default();
    load.carried(Carried::new(50_000));
    assert_eq!(load.left(Some(200_000), 0), Some(75));

    load.recorded(&results(30_000));

    assert_eq!(load.left(Some(200_000), 0), Some(69));
    assert!(load.tokens() > 50_000, "the estimate stopped counting");
}

#[test]
fn tool_results_cannot_appear_to_free_room_before_context_is_replaced() {
    let mut load = Load::default();
    load.carried(Carried::new(150_000));
    load.recorded(&results(42_000));
    assert_eq!(load.left(Some(200_000), 36_000), Some(0));

    // The next request reports a slightly lower exact count than the local
    // estimate. That correction still describes the same transcript plus these
    // results; it did not free context, so the visible reading cannot rise to 1%
    // before another result pushes it back to 0%.
    load.responding(0);
    load.carried(Carried::new(162_000));

    assert_eq!(load.left(Some(200_000), 36_000), Some(0));
}

#[test]
fn response_growth_is_estimated_until_output_usage_catches_up() {
    let mut load = Load::default();
    load.recorded(&results(200_000));
    load.responding(0);
    load.carried(Carried::new(50_000));
    load.produced(40_000);
    assert_eq!(load.left(Some(200_000), 0), Some(70));

    load.spent(Spend::new(10_000));
    assert_eq!(load.left(Some(200_000), 0), Some(70));
}

#[test]
fn a_late_input_report_preserves_unreported_response_growth() {
    let mut load = Load::default();
    load.recorded(&results(200_000));
    load.responding(0);
    load.produced(40_000);

    // Input and output counts are independent wire facts. An input count that
    // arrives after text covers the request, not the response bytes already
    // seen, so those bytes remain estimated at the new four-byte rate.
    load.carried(Carried::new(50_000));

    assert_eq!(load.left(Some(200_000), 0), Some(70));
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
    assert_eq!(load.left(Some(200_000), 0), Some(99));
}

#[test]
fn output_after_an_exact_partial_spend_is_the_only_part_estimated_on_recording() {
    let mut load = Load::default();
    load.recorded(&results(400_000));
    load.responding(0);
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
    assert_eq!(load.left(Some(200_000), 0), Some(49));
    assert!(!load.full(Some(100_151), 0));
}

#[test]
fn an_equal_overhead_resume_measurement_stays_exact_and_persistable() {
    let mut load = Load::default();
    load.recorded(&results(300));
    load.requesting(Some(&"i".repeat(500)), &[]);
    let reading = Calibration {
        carried: Carried::new(100),
        spent: Spend::new(10),
        sent: 806,
        overhead: 500,
    };

    load.measured(reading);

    assert_eq!(load.calibrated(), Some(reading));
}

#[test]
fn a_fresh_report_cannot_make_an_uncompacted_resumed_window_gain_visible_room() {
    let mut load = Load::default();
    load.recorded(&results(2_550_000));
    load.requesting(Some(&"i".repeat(15_000)), &[]);
    assert_eq!(load.left(Some(922_000), 36_000), Some(3));

    // The persisted reading is rejected because fixed content changed, then the
    // first request in this process proves the cautious recount over-counted.
    load.measured(Calibration {
        carried: Carried::new(711_628),
        spent: Spend::new(452),
        sent: 2_570_482,
        overhead: 16_431,
    });
    load.responding(0);
    load.carried(Carried::new(720_000));

    assert!(
        load.tokens() < 800_000,
        "the fresh provider report was ignored"
    );
    assert!(
        load.calibrated().is_some(),
        "the display floor prevented fresh accounting from becoming exact"
    );
    load.measured(Calibration {
        carried: Carried::new(900_000),
        spent: Spend::NONE,
        sent: 3_000_000,
        overhead: 20_000,
    });
    assert_eq!(
        load.left(Some(922_000), 36_000),
        Some(3),
        "the prompt claimed room was freed without compaction"
    );
}

#[test]
fn a_resume_without_a_persisted_measurement_still_holds_its_starting_reading() {
    let mut load = Load::default();
    load.recorded(&results(2_550_000));
    load.requesting(Some(&"i".repeat(15_000)), &[]);
    load.resumed();
    assert_eq!(load.left(Some(922_000), 36_000), Some(3));

    load.responding(0);
    load.carried(Carried::new(720_000));

    assert_eq!(load.left(Some(922_000), 36_000), Some(3));
}

#[test]
fn replacing_context_clears_the_resumed_display_floor() {
    let mut load = Load::default();
    load.recorded(&results(2_550_000));
    load.requesting(Some(&"i".repeat(15_000)), &[]);
    load.measured(Calibration {
        carried: Carried::new(711_628),
        spent: Spend::new(452),
        sent: 2_570_482,
        overhead: 16_431,
    });
    assert_eq!(load.left(Some(922_000), 36_000), Some(3));

    load.replaced();
    load.recorded(&results(600_000));

    assert_eq!(load.left(Some(922_000), 36_000), Some(77));
}

#[test]
fn a_resume_rejects_a_measurement_that_did_not_include_all_current_overhead() {
    let mut load = Load::default();
    load.recorded(&results(300));
    load.requesting(Some(&"i".repeat(700)), &[]);

    load.measured(Calibration {
        carried: Carried::new(100),
        spent: Spend::new(10),
        sent: 600,
        overhead: 500,
    });

    assert_eq!(load.calibrated(), None);
    assert_eq!(load.tokens(), 336, "the cautious recount was replaced");

    load.responding(0);
    load.carried(Carried::new(100));
    assert_eq!(
        load.tokens(),
        100,
        "the rejected resume measurement contaminated fresh accounting"
    );
}

#[test]
fn an_estimate_has_a_conservative_window_percentage() {
    let mut load = Load::default();
    load.recorded(&results(50_000));

    // 50 006 bytes at the cautious three-byte rate round up to 16 669 tokens.
    // The percentage rounds down in turn, so it never claims the fractional
    // room the estimate has already reserved.
    assert_eq!(load.left(Some(200_000), 0), Some(91));

    load.carried(Carried::new(50_000));
    assert_eq!(load.left(Some(200_000), 0), Some(75));
}

#[test]
fn how_much_is_left_is_nothing_at_all_where_no_window_is_known() {
    let mut load = Load::default();
    load.carried(Carried::new(50_000));

    assert_eq!(load.left(None, 0), None);
    assert_eq!(load.left(Some(200_000), 0), Some(75));
}

#[test]
fn a_window_already_over_full_reads_as_none_left_rather_than_wrapping() {
    let mut load = Load::default();
    load.carried(Carried::new(300_000));

    assert_eq!(load.left(Some(200_000), 0), Some(0));
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
fn the_reading_reaches_zero_at_the_same_reserve_boundary_as_compaction() {
    let mut load = Load::default();
    load.carried(Carried::new(164_000));

    assert_eq!(load.left(Some(200_000), 36_000), Some(0));
    assert!(load.full(Some(200_000), 36_000));
}

#[test]
fn the_reading_is_a_percentage_of_the_room_the_transcript_may_use() {
    let mut load = Load::default();

    assert_eq!(load.left(Some(200_000), 36_000), Some(100));
    load.carried(Carried::new(82_000));
    assert_eq!(load.left(Some(200_000), 36_000), Some(50));
}

#[test]
fn a_session_that_has_said_nothing_reads_as_a_whole_window() {
    // The system instructions and tool schemas are in the first request before
    // a word is typed, so a fresh session pays them however it goes on. They
    // are what the reading measures against rather than something already
    // spent: a session that has said nothing has all of its room left.
    let mut load = Load::default();
    let tools = [ToolSchema {
        name: "read",
        schema: Box::leak("s".repeat(30_000).into_boxed_str()),
    }];

    load.requesting(Some(&"i".repeat(6_000)), &tools);

    assert_eq!(load.left(Some(200_000), 36_000), Some(100));
    assert!(!load.full(Some(200_000), 36_000));
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

/// `C3`. A session that attached a 3 MB screenshot must go on estimating text
/// at the rate its own text established, and the two sides fail differently:
/// the file's bytes would move the divisor by a factor of thirty, and the
/// tokens the vendor charged for the picture would move the numerator the other
/// way. Both are corrected, so what is left is text against text.
///
/// The tolerance is the residual: the flat charge is deliberately below what a
/// real picture costs, so a little of the picture stays in the rate. Five per
/// cent is what that comes to beside a transcript of this size, and the
/// direction is the one this module has always preferred — an estimate that
/// leans high compacts early, which costs context nobody sees go rather than
/// the turn.
#[test]
fn a_three_megabyte_screenshot_does_not_move_the_rate_the_text_established() {
    // 90 KB of prompt, which is a session that has been going a while.
    const TEXT: usize = 90_000;
    // What that text alone truly cost, and what this vendor's own table says
    // the picture beside it cost: a 1000x1000 image, read on 2026-08-23.
    const TEXT_TOKENS: u64 = 30_000;
    const PICTURE_TOKENS: u64 = 1_296;

    let mut plain = Load::default();
    plain.recorded(&Message::said("x".repeat(TEXT)));
    plain.responding(0);
    plain.carried(Carried::new(TEXT_TOKENS));

    let mut holding = Load::default();
    holding.recorded(&attaching(TEXT));
    holding.responding(1);
    holding.carried(Carried::new(TEXT_TOKENS + PICTURE_TOKENS));

    let (text, with_picture) = (plain.bytes_to_tokens(3_000), holding.bytes_to_tokens(3_000));

    assert!(
        with_picture >= text,
        "text got cheaper because a picture was attached: {with_picture} < {text}"
    );
    assert!(
        with_picture * 100 <= text * 102,
        "the picture moved the rate by more than two per cent: {with_picture} vs {text}"
    );
}

/// The other half of `C3`, on its own so a later edit that puts an
/// attachment's bytes back into the byte measure fails here and says why.
#[test]
fn what_a_file_weighs_is_not_what_the_transcript_weighs() {
    let mut plain = Load::default();
    plain.recorded(&Message::said("x".repeat(300)));

    let mut holding = Load::default();
    holding.recorded(&attaching(300));

    assert_eq!(
        holding.tokens(),
        plain.tokens(),
        "a message that named a file weighs more than the same message without one"
    );
}

/// The charge follows what the request carried, which is what `T-RESOLVE`'s
/// ageing decides — not what the transcript still refers to.
#[test]
fn an_attachment_aged_out_of_the_request_stops_being_charged() {
    let mut load = Load::default();
    load.recorded(&Message::said("what is in these"));

    load.responding(2);
    let both = load.tokens();

    // The turn after, the ceiling admitted one of the two.
    load.responding(1);

    assert_eq!(
        both,
        load.tokens() + PER_ATTACHMENT,
        "the file the request left behind is still being charged for"
    );

    // And a turn that carried neither is back to the text alone.
    load.responding(0);

    assert_eq!(both, load.tokens() + PER_ATTACHMENT * 2);
}
