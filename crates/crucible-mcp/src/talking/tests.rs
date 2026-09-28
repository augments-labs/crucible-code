//! Conversations a server could hold, including the ones it should not.

use std::collections::VecDeque;
use std::io::{self, Cursor};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use crucible_runtime::Cancel;
use crucible_sandbox::{SandboxOutput, SandboxRead};
use crucible_transport::Heard;
use serde_json::{Value, json};

use super::{ASIDES, Talking, Trouble};
use crate::testing::runtime;
use crate::wire::NO_SUCH_METHOD;

/// Holds a conversation against a fixed script and hands back what crucible
/// said.
///
/// The script is what the server sends, one frame per line, and it is written
/// out in full before crucible reads a byte — which is exactly the shape a
/// server that answers before crucible has finished asking would have, and one
/// this crate has to be right about either way.
fn against(script: &str, calls: &[(&str, Value)]) -> (Vec<Result<Value, Trouble>>, Vec<String>) {
    let mut said = Vec::new();
    let answers = {
        let mut talking = Talking::new(Cursor::new(script.to_owned()), &mut said);
        calls
            .iter()
            .map(|(method, params)| talking.ask(method, params))
            .collect()
    };
    let said = String::from_utf8(said).expect("crucible writes text");
    (
        answers,
        said.lines().map(ToOwned::to_owned).collect::<Vec<_>>(),
    )
}

/// Holds the same conversation as [`against`], awaited.
fn against_async(
    script: &str,
    calls: &[(&str, Value)],
) -> (Vec<Result<Value, Trouble>>, Vec<String>) {
    let mut said = Vec::new();
    let answers = runtime().block_on(async {
        let mut talking = Talking::new(Cursor::new(script.to_owned()), &mut said);
        let mut answers = Vec::new();
        for (method, params) in calls {
            answers.push(talking.ask_async(method, params).await);
        }
        answers
    });
    let said = String::from_utf8(said).expect("crucible writes text");
    (
        answers,
        said.lines().map(ToOwned::to_owned).collect::<Vec<_>>(),
    )
}

#[test]
fn a_call_comes_back_with_what_the_server_answered() {
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n",
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().map(|answer| answer.as_ref().ok()),
        Some(Some(&json!({ "tools": [] })))
    );

    let sent: Value = serde_json::from_str(said.first().expect("one frame")).expect("json");
    assert_eq!(
        sent.get("method").and_then(Value::as_str),
        Some("tools/list")
    );
    assert_eq!(sent.get("id").and_then(Value::as_u64), Some(1));
}

#[test]
fn each_call_is_numbered_afresh_so_two_answers_cannot_be_swapped() {
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"first\"}\n\
         {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"second\"}\n",
        &[("one", json!({})), ("two", json!({}))],
    );

    let read = |answer: &Result<Value, Trouble>| answer.as_ref().expect("answered").clone();
    assert_eq!(
        answers.iter().map(read).collect::<Vec<_>>(),
        ["first", "second"]
    );

    let numbers = said
        .iter()
        .map(|frame| {
            serde_json::from_str::<Value>(frame)
                .expect("json")
                .get("id")
                .and_then(Value::as_u64)
        })
        .collect::<Vec<_>>();
    assert_eq!(numbers, [Some(1), Some(2)]);
}

#[test]
fn an_answer_to_a_call_crucible_is_not_waiting_on_stops_the_conversation() {
    // Reading on would mean matching every later answer to the wrong question,
    // and the caller would never be told the two ends had drifted apart.
    let (answers, _) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":\"stale\"}\n",
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(
            answers.first(),
            Some(Err(Trouble::Astray { call, found }))
                if call.number() == 1 && found.number() == 9
        ),
        "{:?}",
        answers.first()
    );
}

#[test]
fn a_question_the_server_asks_is_refused_before_the_answer_is_waited_for() {
    // The server is waiting on it, and what it is waiting to get on with is
    // the call crucible made. Going quiet would deadlock both ends.
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"sampling/createMessage\"}\n\
         {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n",
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().and_then(|answer| answer.as_ref().ok()),
        Some(&json!("done"))
    );

    let refusal: Value = serde_json::from_str(said.get(1).expect("a refusal")).expect("json");
    assert_eq!(refusal.get("id").and_then(Value::as_u64), Some(4));
    assert_eq!(
        refusal.pointer("/error/code").and_then(Value::as_i64),
        Some(NO_SUCH_METHOD)
    );
}

#[test]
fn a_notification_while_waiting_is_dropped_and_the_answer_still_arrives() {
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\
         {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n",
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().and_then(|answer| answer.as_ref().ok()),
        Some(&json!("done"))
    );
    assert_eq!(said.len(), 1, "nothing is owed a notification: {said:?}");
}

#[test]
fn a_server_that_says_anything_but_the_answer_is_stopped_rather_than_waited_on() {
    let chatter = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n"
        .repeat(ASIDES + 1);
    let (answers, _) = against(
        &format!("{chatter}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"late\"}}\n"),
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(answers.first(), Some(Err(Trouble::Talkative { most, .. })) if *most == ASIDES),
        "{:?}",
        answers.first()
    );
}

#[test]
fn exactly_as_much_chatter_as_the_bound_allows_still_gets_its_answer() {
    // The awkward legal case. A bound that refused the last frame it says it
    // allows would be a bound nobody could write a server against.
    let chatter =
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n".repeat(ASIDES);
    let (answers, _) = against(
        &format!("{chatter}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"in time\"}}\n"),
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().and_then(|answer| answer.as_ref().ok()),
        Some(&json!("in time"))
    );
}

#[test]
fn a_server_that_stops_before_answering_says_which_call_it_left_open() {
    let (answers, _) = against("", &[("tools/list", json!({}))]);

    assert!(
        matches!(answers.first(), Some(Err(Trouble::Stopped { call })) if call.number() == 1),
        "{:?}",
        answers.first()
    );
}

#[test]
fn a_failure_comes_back_as_the_code_and_the_words_the_server_gave() {
    let (answers, _) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32000,\"message\":\"no catalogue\"}}\n",
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(
            answers.first(),
            Some(Err(Trouble::Refused { code, said, .. }))
                if *code == -32000 && &**said == "no catalogue"
        ),
        "{:?}",
        answers.first()
    );
}

#[test]
fn a_frame_that_is_not_a_message_stops_the_call_rather_than_being_skipped() {
    // Skipping it would mean crucible carrying on against a server it has
    // already failed to understand once.
    let (answers, _) = against(
        "{\"jsonrpc\":\"2.0\"}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n",
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(answers.first(), Some(Err(Trouble::Garbled(_)))),
        "{:?}",
        answers.first()
    );
}

/// A pipe that refuses a frame with a given ending.
///
/// Standing in for [`Said`](crucible_transport::Said), whose two ways of refusing
/// mean opposite things about the far end: a write that timed out left the
/// bytes with the task that owns the pipe, and a broken one left them
/// nowhere.
///
/// [`Said`]: crucible_transport::Said
struct Refuses(io::ErrorKind);

impl io::Write for Refuses {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(self.0, "the frame did not go"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(self.0, "the frame did not go"))
    }
}

/// Asks one call over a pipe that refuses the frame that way.
fn refused(ending: io::ErrorKind) -> Trouble {
    Talking::new(Cursor::new(String::new()), Refuses(ending))
        .ask("tools/call", &json!({}))
        .expect_err("a pipe that will not take a frame")
}

#[test]
fn a_frame_the_pipe_timed_out_on_is_outstanding_because_the_bytes_may_yet_land() {
    let trouble = refused(io::ErrorKind::TimedOut);

    assert!(
        matches!(trouble, Trouble::Unsent { .. }),
        "a frame the pipe would not take did not go, whatever became of it: {trouble}"
    );
    assert!(
        trouble.outstanding(),
        "a patience spent waiting for the pipe to take a frame ends with those \
         bytes still in the writer's hands, so the server may have been asked \
         and this is not a call anybody may ask again: {trouble}"
    );
}

#[test]
fn a_frame_the_pipe_broke_on_is_not_outstanding_because_nothing_was_left_to_read_it() {
    let trouble = refused(io::ErrorKind::BrokenPipe);

    assert!(
        matches!(trouble, Trouble::Unsent { .. }),
        "a broken pipe is a frame that did not go: {trouble}"
    );
    assert!(
        !trouble.outstanding(),
        "a pipe with nobody on the other end of it cannot have delivered the \
         frame, which is the one ending crucible can prove was harmless: {trouble}"
    );
}

/// Asks one call over a pipe that refuses the frame that way, awaited.
fn refused_async(ending: io::ErrorKind) -> Trouble {
    runtime()
        .block_on(
            Talking::new(Cursor::new(String::new()), Refuses(ending))
                .ask_async("tools/call", &json!({})),
        )
        .expect_err("a pipe that will not take a frame")
}

impl tokio::io::AsyncWrite for Refuses {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::new(self.0, "the frame did not go")))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(self.0, "the frame did not go")))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(self.0, "the frame did not go")))
    }
}

/// Every script the conversations above are held against, and what crucible
/// asks during each.
fn scripts() -> Vec<(String, Vec<(&'static str, Value)>)> {
    let notification = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n";
    let list = || vec![("tools/list", json!({}))];
    vec![
        (
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n".to_owned(),
            list(),
        ),
        (
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"first\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"second\"}\n"
                .to_owned(),
            vec![("one", json!({})), ("two", json!({}))],
        ),
        (
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"early\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"late\"}\n"
                .to_owned(),
            vec![("one", json!({})), ("two", json!({}))],
        ),
        (
            "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":\"stale\"}\n".to_owned(),
            list(),
        ),
        (
            "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"sampling/createMessage\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n"
                .to_owned(),
            list(),
        ),
        (
            format!("{notification}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}}\n"),
            list(),
        ),
        (
            format!(
                "{}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"late\"}}\n",
                notification.repeat(ASIDES + 1)
            ),
            list(),
        ),
        (
            format!(
                "{}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"in time\"}}\n",
                notification.repeat(ASIDES)
            ),
            list(),
        ),
        (String::new(), list()),
        (
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32000,\"message\":\"no catalogue\"}}\n"
                .to_owned(),
            vec![("tools/list", json!({})), ("tools/list", json!({}))],
        ),
        (
            "{\"jsonrpc\":\"2.0\"}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n".to_owned(),
            list(),
        ),
        ("{\"jsonrpc\":\"2.0\",\"id\":1,\"res".to_owned(), list()),
    ]
}

#[test]
fn an_awaited_conversation_comes_to_what_a_waited_one_does_on_every_script() {
    // Sequencing and error mapping are the conversation's, not the stream's:
    // the same server saying the same things has to be answered and reported
    // the same way whichever kind of stream crucible heard it over.
    for (script, calls) in scripts() {
        let (waited, waited_said) = against(&script, &calls);
        let (awaited, awaited_said) = against_async(&script, &calls);

        assert_eq!(
            format!("{awaited:?}"),
            format!("{waited:?}"),
            "the answers to {script:?}"
        );
        assert_eq!(
            awaited_said, waited_said,
            "what crucible said to {script:?}"
        );
    }
}

#[test]
fn an_awaited_answer_to_a_call_crucible_is_not_waiting_on_stops_the_conversation() {
    let (answers, _) = against_async(
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"early\"}\n\
         {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"late\"}\n",
        &[("one", json!({})), ("two", json!({}))],
    );

    assert!(
        matches!(
            answers.first(),
            Some(Err(Trouble::Astray { call, found }))
                if call.number() == 1 && found.number() == 2
        ),
        "{answers:?}"
    );
}

#[test]
fn an_awaited_handshake_numbers_its_calls_and_leaves_its_notification_unnumbered() {
    let script = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\
                  {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}\n";
    let mut said = Vec::new();
    let answers = runtime().block_on(async {
        let mut talking = Talking::new(Cursor::new(script), &mut said);
        let greeted = talking.ask_async("initialize", &json!({})).await;
        let told = talking
            .tell_async("notifications/initialized", &json!({}))
            .await;
        let listed = talking.ask_async("tools/list", &json!({})).await;
        (greeted, told, listed)
    });

    assert!(
        matches!(&answers, (Ok(_), Ok(()), Ok(listed)) if *listed == json!({ "tools": [] })),
        "{answers:?}"
    );
    let sent = String::from_utf8(said)
        .expect("crucible writes text")
        .lines()
        .map(|frame| serde_json::from_str::<Value>(frame).expect("json"))
        .map(|frame| {
            (
                frame
                    .get("method")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                frame.get("id").and_then(Value::as_u64),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sent,
        [
            (Some("initialize".to_owned()), Some(1)),
            (Some("notifications/initialized".to_owned()), None),
            (Some("tools/list".to_owned()), Some(2)),
        ]
    );
}

#[test]
fn an_awaited_frame_the_pipe_timed_out_on_is_outstanding_as_a_waited_one_is() {
    let trouble = refused_async(io::ErrorKind::TimedOut);

    assert!(matches!(trouble, Trouble::Unsent { .. }), "{trouble}");
    assert_eq!(
        trouble.outstanding(),
        refused(io::ErrorKind::TimedOut).outstanding()
    );
    assert!(trouble.outstanding(), "{trouble}");
}

#[test]
fn an_awaited_frame_the_pipe_broke_on_is_not_outstanding_as_a_waited_one_is_not() {
    let trouble = refused_async(io::ErrorKind::BrokenPipe);

    assert!(matches!(trouble, Trouble::Unsent { .. }), "{trouble}");
    assert_eq!(
        trouble.outstanding(),
        refused(io::ErrorKind::BrokenPipe).outstanding()
    );
    assert!(!trouble.outstanding(), "{trouble}");
}

/// How long the tests over a real stream sit through one silence.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a caller in these tests waits before it gives up on an answer.
///
/// Far inside [`PATIENCE`], so what ends the wait is the caller and never the
/// stream.
const GIVE_UP: Duration = Duration::from_millis(50);

/// A server's standard output that says what a test hands it, when the test
/// hands it, and nothing until then.
#[derive(Clone, Default)]
struct Later(Arc<Mutex<VecDeque<String>>>);

impl Later {
    /// Has the server say `frame` from here on.
    fn say(&self, frame: &str) {
        self.0
            .lock()
            .expect("a test's own lock")
            .push_back(format!("{frame}\n"));
    }
}

impl SandboxOutput for Later {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        let Some(said) = self.0.lock().expect("a test's own lock").pop_front() else {
            return Ok(SandboxRead::Pending);
        };
        let into = buffer
            .get_mut(..said.len())
            .expect("a test's frames fit one read");
        into.copy_from_slice(said.as_bytes());
        Ok(SandboxRead::Bytes(said.len()))
    }
}

/// A conversation over a server's output as the transport hears it, and a
/// writer that keeps what crucible said.
fn over(later: &Later) -> Talking<Heard<Later>, Vec<u8>> {
    Talking::new(Heard::new(later.clone(), PATIENCE, &runtime()), Vec::new())
}

#[test]
fn an_answer_to_an_awaited_call_given_up_on_is_refused_rather_than_taken_for_the_next() {
    // The out-of-order answer an awaited conversation can meet that a waited
    // one cannot: a caller that stops awaiting a call leaves its answer on the
    // way, and it arrives while crucible waits on the one after.
    let later = Later::default();
    let (given_up, next) = runtime().block_on(async {
        let mut talking = over(&later);
        let given_up =
            tokio::time::timeout(GIVE_UP, talking.ask_async("tools/call", &json!({}))).await;
        later.say("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"the first\"}");
        later.say("{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"the second\"}");
        (given_up, talking.ask_async("tools/list", &json!({})).await)
    });

    assert!(
        given_up.is_err(),
        "nothing answered the first call: {given_up:?}"
    );
    assert!(
        matches!(
            &next,
            Err(Trouble::Astray { call, found }) if call.number() == 2 && found.number() == 1
        ),
        "an answer to a call nobody awaits any more is not an answer to the next: {next:?}"
    );
}

/// What a wait somebody stopped came to, and what the call after it came to
/// once the stopped call's answer, and one for the call after it, arrived.
fn after_a_press(awaited: bool) -> (Trouble, Result<Value, Trouble>) {
    let later = Later::default();
    let cancel = Cancel::new();
    let mut talking = over(&later);
    talking.heard_mut().abandoned_when(Some(cancel.clone()));
    let pressing = thread::spawn({
        let cancel = cancel.clone();
        move || {
            thread::sleep(GIVE_UP);
            cancel.request();
        }
    });
    let stopped = if awaited {
        runtime().block_on(talking.ask_async("tools/call", &json!({})))
    } else {
        talking.ask("tools/call", &json!({}))
    };
    pressing.join().expect("the press is made");
    talking.heard_mut().abandoned_when(None);
    later.say("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"the first\"}");
    later.say("{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"the second\"}");
    let next = if awaited {
        runtime().block_on(talking.ask_async("tools/list", &json!({})))
    } else {
        talking.ask("tools/list", &json!({}))
    };
    (stopped.expect_err("a wait somebody stopped"), next)
}

#[test]
fn an_awaited_wait_somebody_stopped_ends_at_the_press_as_a_waited_one_does() {
    let (awaited, awaited_next) = after_a_press(true);
    let (waited, waited_next) = after_a_press(false);

    for trouble in [&awaited, &waited] {
        assert!(
            trouble.interrupted(),
            "a press, not a slow server: {trouble}"
        );
        assert!(trouble.outstanding(), "the call went: {trouble}");
        assert!(!trouble.settled(), "nothing answered it: {trouble}");
    }
    assert_eq!(format!("{awaited:?}"), format!("{waited:?}"));
    // A press ends the reading as any failed read does: the reader has let go
    // of whatever boundary the server was in the middle of, so the answers
    // that arrive afterwards are never read as anybody's.
    assert!(
        matches!(&awaited_next, Err(Trouble::Stopped { call }) if call.number() == 2),
        "{awaited_next:?}"
    );
    assert_eq!(format!("{awaited_next:?}"), format!("{waited_next:?}"));
}
