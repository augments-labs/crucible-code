//! What reading a confined stream has to guarantee.

use std::collections::VecDeque;
use std::error::Error as _;
use std::fmt::Write as _;
use std::io;
use std::time::{Duration, Instant};

use crucible_core::{Over, SandboxOutput, SandboxRead, Speaking, Turn};

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
/// extension is exactly a process that is still there and has stopped talking.
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
fn what_an_extension_says_arrives_as_a_turn() {
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
/// every time an extension took a breath.
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
/// said. A budget for the whole conversation would kill an extension for the
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
fn an_extension_that_says_nothing_for_long_enough_is_given_up_on() {
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
/// reader has lost the boundary the extension was stating, so what is left is
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
