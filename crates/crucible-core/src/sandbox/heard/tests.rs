//! What reading a confined stream has to guarantee.

use std::collections::VecDeque;
use std::error::Error as _;
use std::fmt::Write as _;
use std::io::{self, BufRead as _};
use std::time::{Duration, Instant};

use crate::{Cancel, Over, SandboxOutput, SandboxRead, Speaking, Turn};

use super::Heard;

/// How long a test sits through one silence. Long next to the pause between
/// polls, but not long enough to absorb several of them: an oversleeping
/// machine stretches every pause a test takes, so a test whose subject is not
/// timing should take none.
const PATIENCE: Duration = Duration::from_millis(50);

/// The pause between polls a test runs with. Short, because a test that waits
/// in real milliseconds should wait as few of them as it can.
const PAUSE: Duration = Duration::from_millis(1);

/// One thing a confined stream does when it is asked what it has.
enum Step {
    /// It has a frame, and the newline that ends one.
    Says(&'static str),
    /// It has nothing yet, and its writer is still there.
    Waits,
    /// It has a piece of what it said, and the rest went past the ceiling.
    Loses {
        /// What is still in the buffer.
        retained: &'static str,
        /// What was consumed and dropped.
        discarded: usize,
    },
    /// Its writer has gone.
    Closes,
}

/// A confined stream that does what it was told to, then goes quiet forever.
///
/// Running out of script is silence rather than an ending, because a hung
/// program is exactly a process that is still there and has stopped talking.
struct Says {
    /// What is left to do.
    steps: VecDeque<Step>,
}

impl Says {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
        }
    }
}

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        match self.steps.pop_front() {
            None | Some(Step::Waits) => Ok(SandboxRead::Pending),
            Some(Step::Closes) => Ok(SandboxRead::End),
            Some(Step::Says(frame)) => Ok(SandboxRead::Bytes(copied(frame, buffer))),
            Some(Step::Loses {
                retained,
                discarded,
            }) => Ok(SandboxRead::Limited {
                retained: copied(retained, buffer),
                discarded,
            }),
        }
    }
}

/// Puts one line into the caller's buffer, as a pipe would.
fn copied(frame: &str, buffer: &mut [u8]) -> usize {
    let said = format!("{frame}\n");
    let bytes = said.as_bytes();
    let taken = bytes.len().min(buffer.len());
    if let Some((into, from)) = buffer.get_mut(..taken).zip(bytes.get(..taken)) {
        into.copy_from_slice(from);
    }
    taken
}

/// A conversation with a stream that says these things.
fn hearing(steps: impl IntoIterator<Item = Step>) -> Speaking<Heard<Says>, Vec<u8>, &'static str> {
    Speaking::new(Heard::new(Says::new(steps), PATIENCE), Vec::new())
}

/// The same, waiting a chosen pause between polls.
fn patiently(
    steps: impl IntoIterator<Item = Step>,
    pause: Duration,
) -> Speaking<Heard<Says>, Vec<u8>, &'static str> {
    Speaking::new(
        Heard::with_pause(Says::new(steps), PATIENCE, pause),
        Vec::new(),
    )
}

/// What an ending said, all the way down to the operating system where there
/// is one.
fn because(over: &Over) -> String {
    let mut said = over.to_string();
    let mut next: Option<&dyn std::error::Error> = over.source();
    while let Some(source) = next {
        let _ = write!(said, "; {source}");
        next = source.source();
    }
    said
}

#[test]
fn what_a_program_says_arrives_as_a_turn() {
    let mut talk = hearing([
        Step::Says(r#"{"id":1,"method":"tools/list","params":{}}"#),
        Step::Closes,
    ]);

    let turn = talk.turn();

    assert!(
        matches!(turn, Ok(Turn::Asked { ref method, .. }) if &**method == "tools/list"),
        "a confined process's own words must reach the host whole: {turn:?}"
    );
}

/// The reason this type exists. A stream that answers "nothing yet" is not a
/// stream that has ended, and treating the two alike would end a conversation
/// every time a program took a breath.
///
/// How long a poll waits is not what is being asked here, so it waits for no
/// time at all. Three inherited pauses are fifteen milliseconds of the fifty
/// this case is allowed, which reads like room until a loaded machine stretches
/// each one and the case fails on the machine rather than on the code.
#[test]
fn a_pause_is_not_an_ending() {
    let mut talk = patiently(
        [
            Step::Waits,
            Step::Waits,
            Step::Waits,
            Step::Says(r#"{"method":"ready"}"#),
            Step::Closes,
        ],
        Duration::ZERO,
    );

    let turn = talk.turn();

    assert!(
        matches!(turn, Ok(Turn::Told { ref method, .. }) if &**method == "ready"),
        "waiting must not lose what was said afterwards: {turn:?}"
    );
}

/// Patience is spent on one silence and handed back whenever something is
/// said. A budget for the whole conversation would kill a program for the
/// crime of being useful for longer than crucible guessed.
#[test]
fn patience_is_for_one_silence_and_not_for_the_conversation() {
    let mut steps = Vec::new();
    for _ in 0..100 {
        steps.push(Step::Waits);
        steps.push(Step::Says(r#"{"method":"ready"}"#));
    }
    steps.push(Step::Closes);
    let mut talk = patiently(steps, PAUSE);

    let began = Instant::now();
    let mut ended = None;
    for said in 0..100 {
        if let Err(over) = talk.turn() {
            ended = Some(format!("frame {said} of 100: {}", because(&over)));
            break;
        }
    }
    let waited = began.elapsed();

    assert!(
        ended.is_none(),
        "a conversation must not run out of patience while it is being had: {ended:?}"
    );
    assert!(
        waited > PATIENCE,
        "the test proves nothing unless the whole conversation outlasts one \
         silence: waited {waited:?}, patience {PATIENCE:?}"
    );
}

/// A peer that has stopped reading and a peer that is merely slow look the
/// same from here, so the only honest answer is a deadline crucible chose.
#[test]
fn a_program_that_says_nothing_for_long_enough_is_given_up_on() {
    let mut talk = patiently([Step::Waits], PAUSE);

    let began = Instant::now();
    let over = talk.turn().expect_err("silence must end the conversation");
    let waited = began.elapsed();

    assert!(
        because(&over).contains("said nothing"),
        "the ending must say it was silence rather than a failure: {}",
        because(&over)
    );
    assert!(
        waited >= PATIENCE,
        "giving up before the patience is spent is not patience: {waited:?}"
    );
}

/// Bytes past the output ceiling are gone, and one of them was a newline. The
/// reader has lost the boundary the program was stating, so what is left is
/// not a shorter conversation but a stream that cannot be framed at all.
#[test]
fn a_stream_with_a_hole_in_it_cannot_be_framed() {
    let mut talk = hearing([
        Step::Loses {
            retained: r#"{"method":"rea"#,
            discarded: 4096,
        },
        Step::Says(r#"{"method":"ready"}"#),
        Step::Closes,
    ]);

    let over = talk.turn().expect_err("a hole must end the conversation");

    assert!(
        because(&over).contains("4096"),
        "the ending must say how much was lost: {}",
        because(&over)
    );
}

#[test]
fn a_closed_stream_is_the_ordinary_ending() {
    let mut talk = hearing([Step::Closes]);

    let over = talk
        .turn()
        .expect_err("a closed stream ends the conversation");

    assert!(
        matches!(over, Over::Silent),
        "a writer that has gone is nobody's fault: {over:?}"
    );
}

/// The whole reason the waiting is a poll loop and not a blocked read. A
/// patience answers "how long may a quiet program stay quiet"; an interrupt
/// answers "somebody wants this to stop", and those are different questions.
/// A reader that could only give the first answer would make escape mean *at
/// the deadline*, which for a request patience measured in minutes is not a
/// reader anybody can interrupt.
#[test]
fn a_reader_asked_to_stop_gives_up_on_the_silence_rather_than_waiting_it_out() {
    let cancel = Cancel::new();
    // Far longer than this case is willing to take, so that ending at the
    // patience and ending because somebody asked cannot be confused: only one
    // of the two can produce an answer inside the assertion below.
    let mut heard = Heard::with_pause(Says::new([Step::Waits]), Duration::from_secs(2), PAUSE);
    heard.abandoned_when(Some(cancel.clone()));
    cancel.request();

    let began = Instant::now();
    let stopped = heard.fill_buf().expect_err("the wait was interrupted");
    let waited = began.elapsed();

    assert_eq!(
        stopped.kind(),
        io::ErrorKind::ConnectionAborted,
        "an abandoned wait is not a program that timed out, and a caller that \
         has to tell them apart reads the kind: {stopped}"
    );
    assert_ne!(
        stopped.kind(),
        io::ErrorKind::Interrupted,
        "and it must not be the kind every std reader retries, or the caller \
         that gave up would be put straight back into the wait"
    );
    assert!(
        waited < Duration::from_millis(500),
        "a wait somebody asked to end must end there and then, not at the \
         patience: {waited:?}"
    );
}

/// The token is set around one exchange, so the reader has to put it down
/// again. A stream that stayed abandoned would refuse the next call for a
/// press that was spent on the last one.
#[test]
fn a_reader_handed_no_token_waits_out_a_silence_it_was_told_to_abandon_before() {
    let cancel = Cancel::new();
    let mut heard = Heard::with_pause(
        Says::new([Step::Waits, Step::Says(r#"{"method":"ready"}"#)]),
        PATIENCE,
        PAUSE,
    );
    heard.abandoned_when(Some(cancel.clone()));
    heard.abandoned_when(None);
    cancel.request();

    let arrived = heard.fill_buf().expect("no token is no interruption");

    assert!(
        std::str::from_utf8(arrived)
            .unwrap_or_default()
            .contains("ready"),
        "a spent press must not end the exchange after it"
    );
}

/// A stream that always has one more byte and never ends a frame.
///
/// The shape a patience cannot answer: it is never quiet for long enough to be
/// given up on, so a reader measuring silences alone waits on it for as long as
/// it cares to keep typing.
struct Dribbles;

impl SandboxOutput for Dribbles {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        std::thread::sleep(Duration::from_millis(1));
        let Some(byte) = buffer.first_mut() else {
            return Ok(SandboxRead::Pending);
        };
        // Not a newline: this is a frame that goes on forever, not a slow one.
        *byte = b'x';
        Ok(SandboxRead::Bytes(1))
    }
}

/// Reads one line from `reader` on a thread of its own, and gives up on the
/// whole test after `waiting`.
///
/// A reader that will not stop cannot be asked whether it stopped, so the
/// waiting happens here rather than in the reader: what comes back is either
/// the ending or the absence of one.
fn ended(mut reader: Heard<Dribbles>, waiting: Duration) -> Option<(io::Result<usize>, Duration)> {
    let (done, ending) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let began = Instant::now();
        let mut line = String::new();
        let read = reader.read_line(&mut line);
        drop(done.send((read, began.elapsed())));
    });
    ending.recv_timeout(waiting).ok()
}

#[test]
fn a_reader_given_a_deadline_gives_up_on_a_peer_that_never_stops_typing() {
    let mut reader = Heard::with_pause(Dribbles, Duration::from_secs(30), PAUSE);
    reader.bounded_until(Instant::now().checked_add(Duration::from_millis(100)));

    let (read, waited) = ended(reader, Duration::from_secs(5)).expect(
        "a deadline is a deadline however busy the far end is; without one this reader \
         is still going",
    );

    let ending = read.expect_err("a frame that never ended is not a line");
    assert_eq!(
        ending.kind(),
        io::ErrorKind::TimedOut,
        "it is the far end that failed to answer in the time it was given, which is \
         what a reader says about a peer that ran out of it: {ending}"
    );
    assert!(
        waited < Duration::from_secs(5),
        "and it ended at the deadline rather than at the patience thirty seconds away: \
         {waited:?}"
    );
}

#[test]
fn a_reader_asked_to_stop_while_bytes_keep_arriving_stops_anyway() {
    let cancel = Cancel::new();
    cancel.request();
    let mut reader = Heard::with_pause(Dribbles, Duration::from_secs(30), PAUSE);
    reader.abandoned_when(Some(cancel));

    let (read, waited) = ended(reader, Duration::from_secs(5))
        .expect("a press is answered whether or not the far end is saying anything");

    let ending = read.expect_err("a reader that was asked to stop did not finish the line");
    assert_eq!(
        ending.kind(),
        io::ErrorKind::ConnectionAborted,
        "a press ends the wait as the near end letting go, not as a slow peer: {ending}"
    );
    assert!(
        waited < Duration::from_secs(5),
        "and it is answered at the press rather than once the far end pauses for \
         breath: {waited:?}"
    );
}

/// A stream whose own read fails the way an abandonment is spelled.
struct Aborts;

impl SandboxOutput for Aborts {
    fn read_ready(&mut self, _buffer: &mut [u8]) -> io::Result<SandboxRead> {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the connection was aborted",
        ))
    }
}

#[test]
fn a_connection_the_backend_lost_is_not_reported_as_a_press_nobody_made() {
    // The kind that says crucible let go is crucible's to set, and a backend
    // that happens to fail with it would otherwise be indistinguishable from
    // the reader being asked to stop. What is decided on that difference is
    // whether a call in flight was abandoned by the near end or lost with the
    // far one.
    let mut reader = Heard::with_pause(Aborts, PATIENCE, PAUSE);
    let mut line = String::new();

    let ending = reader
        .read_line(&mut line)
        .expect_err("a stream that will not read is not a line");

    assert_eq!(
        ending.kind(),
        io::ErrorKind::BrokenPipe,
        "the far end went, and nobody pressed anything"
    );
    assert!(
        ending.to_string().contains("the connection was aborted"),
        "the backend's own account of it survives: {ending}"
    );
}
