//! The row a refusal of fast leaves in the transcript.

use super::*;

#[test]
fn a_refusal_of_fast_says_whose_it_was_why_and_what_became_of_the_message() {
    let screen = posted(Event::FastRefused {
        provider: "moonshot",
        reason: "your plan does not include\nkimi-for-coding-highspeed.".into(),
        resent: true,
    });

    // The vendor's own full stop is not doubled, and its line break is not
    // this program's row break.
    assert!(
        screen.contains(
            "⎿ moonshot refused fast: your plan does not include kimi-for-coding-highspeed. \
             Sent again at standard speed; fast is off."
        ),
        "{screen:?}"
    );
}

#[test]
fn a_refusal_whose_second_send_was_stopped_says_fast_is_off_and_nothing_was_sent() {
    let screen = posted(Event::FastRefused {
        provider: "openai",
        reason: "The requested service tier is not allowed for this project.".into(),
        resent: false,
    });

    assert!(
        screen.contains(
            "⎿ openai refused fast: The requested service tier is not allowed for this project. \
             Fast is off."
        ),
        "{screen:?}"
    );
    assert!(!screen.contains("Sent again"), "{screen:?}");
}
