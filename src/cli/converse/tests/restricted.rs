//! A result read back for the reader is not what the model is sent.
//!
//! A vendor that restricts where its results may go has them cleared from what
//! is sent, and the log keeps the words. Reading a result back from the log
//! puts those words in front of the reader and nowhere else: the next request
//! holds the sentence left in their place.

use crucible_types::{Message, Transcript};

use crate::cli::fake::Sent;
use crate::cli::sample::Sample;

use super::*;

/// What stands in the transcript in place of the restricted result.
const CLEARED: &str = "[cleared, the vendor restricts where this may go]";

/// A session whose first result a vendor restricts, and forty after it long
/// enough that what is held has let go of the first by the end, recorded and
/// picked up again.
fn restricted_then_forty(sample: &Sample) -> (Session, Transcript) {
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    session.append(&Message::said("look at everything"));
    for number in 0..=40 {
        let id = ToolId::new(format!("r-{number}"));
        session.append(&Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: vec![crucible_types::ToolCall {
                id: id.clone(),
                name: "bash".into(),
                args: crucible_types::ToolArgs::new(r#"{"command":"make"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        });
        let said = if number == 0 {
            "the restricted words\n".repeat(1200)
        } else {
            format!("result {number} said a line\n").repeat(1000)
        };
        session.append(&Message::ToolResults(vec![crucible_types::ToolResult {
            id,
            output: crucible_types::RecordedToolOutput::ok(said),
        }]));
    }
    session.append(&Message::Agent {
        continuation: None,
        text: "done".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });
    session.restricted(25, &[ToolId::new("r-0")], CLEARED);
    drop(session);

    Session::resume(&sample.logs(), &sample.workspace()).expect("the session")
}

#[test]
fn a_restricted_result_the_log_still_holds_is_not_sent_to_the_model_after_a_resume() {
    // Picked up, the session is put back with a log to read the first result
    // back from. The prompt after it is a request, and what that request
    // sends of the first result is the sentence, not the words.
    let sample = Sample::new("restricted-resumed");
    let (session, transcript) = restricted_then_forty(&sample);

    let script = Script::new(vec![saying("answered")]);
    let sent: Sent = script.sent();
    let conversation = paired(Arc::new(session), |session| {
        scripted(script, Tools::new(), session).resuming(transcript)
    });
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"and now?\n".to_vec());

    converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");

    let sent = sent.lock().expect("the requests");
    let first = sent.first().expect("a request went");
    assert!(first.iter().any(|text| text == CLEARED), "{first:?}");
    assert!(
        first
            .iter()
            .all(|text| !text.contains("the restricted words")),
        "the restricted words were sent"
    );
}
