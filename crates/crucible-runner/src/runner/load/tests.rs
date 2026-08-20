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
    assert_eq!(load.tokens(), 1_000);
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

    // 40 000 bytes at that rate is 10 000 tokens — not the 13 333 the
    // uncalibrated divisor would have guessed.
    assert_eq!(load.tokens(), 111_000);
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
fn a_response_supersedes_the_last_one_rather_than_adding_to_it() {
    let mut load = Load::default();
    load.carried(Carried::new(100_000));
    load.recorded(&results(4_000));

    // The transcript goes whole to the provider every time, so the next
    // response's count already contains everything the last one's did.
    load.carried(Carried::new(102_000));

    assert_eq!(load.tokens(), 102_000, "the two counts were added together");
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

    // Once a response has said what 400 000 bytes carried, the same bytes are
    // read at that rate instead.
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

    assert!(!load.full(Some(200_000), 36_000));
    assert!(load.full(Some(196_000), 36_000));

    // And never, where no window is known: nothing here decides a session is
    // full against a number nobody stated.
    assert!(!load.full(None, 36_000));
}
