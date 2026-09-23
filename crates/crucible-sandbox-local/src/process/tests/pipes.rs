//! A real command spoken to and heard asynchronously through the process
//! handle, as a caller outside this crate would.
//!
//! What the waiting adapters carry is compared with what the synchronous ones
//! carry for the same command: the bytes, the output budget's accounting, and
//! the masking of a credential the command prints.

use super::super::*;

use crucible_sandbox::{SandboxInput, SandboxSpeech};

const WAIT: Duration = Duration::from_secs(20);

/// A runtime with every driver, for the tests that keep their own deadlines.
fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

/// Writes all of `bytes` through `input`.
async fn told(input: &mut dyn SandboxInput, bytes: &[u8]) -> io::Result<()> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let written = input.write(rest).await?;
        rest = rest
            .get(written..)
            .ok_or_else(|| io::Error::other("a write took more than it was given"))?;
    }
    Ok(())
}

/// Everything `output` gives until its end, waited for, and how much of it
/// the budget discarded.
async fn heard(output: &mut dyn SandboxOutput) -> io::Result<(Vec<u8>, usize)> {
    let mut kept = Vec::new();
    let mut discarded = 0;
    let mut buffer = [0; 4096];
    loop {
        let count = match output.read(&mut buffer).await? {
            SandboxRead::Bytes(count) => count,
            SandboxRead::Limited {
                retained,
                discarded: lost,
            } => {
                discarded += lost;
                retained
            }
            SandboxRead::Pending => return Err(io::Error::other("a waiting read was pending")),
            SandboxRead::End => return Ok((kept, discarded)),
        };
        let read = buffer
            .get(..count)
            .ok_or_else(|| io::Error::other("more bytes were reported than read"))?;
        kept.extend_from_slice(read);
    }
}

/// The same, read without waiting.
fn polled(output: &mut dyn SandboxOutput) -> io::Result<(Vec<u8>, usize)> {
    let mut kept = Vec::new();
    let mut discarded = 0;
    let mut buffer = [0; 4096];
    let deadline = Instant::now() + WAIT;
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::other("the output did not end"));
        }
        let count = match output.read_ready(&mut buffer)? {
            SandboxRead::Bytes(count) => count,
            SandboxRead::Limited {
                retained,
                discarded: lost,
            } => {
                discarded += lost;
                retained
            }
            SandboxRead::Pending => {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            SandboxRead::End => return Ok((kept, discarded)),
        };
        let read = buffer
            .get(..count)
            .ok_or_else(|| io::Error::other("more bytes were reported than read"))?;
        kept.extend_from_slice(read);
    }
}

/// The round trip: what crucible says asynchronously, the command hears, and
/// what it says back is heard asynchronously.
#[test]
fn a_command_spoken_to_asynchronously_hears_what_it_was_told() {
    let mut process = unconfined_child(
        shell("read line; printf '%s\\n' \"heard $line\""),
        SandboxSpeech::Held,
    )
    .expect("a peer");
    let mut input = process
        .take_async_stdin()
        .expect("a command built Held hands back an input");
    let mut output = process.take_stdout().expect("stdout");

    let said = runtime().expect("a test runtime").block_on(async {
        tokio::time::timeout(WAIT, async {
            told(input.as_mut(), b"a kettle\n").await?;
            drop(input);
            heard(output.as_mut()).await
        })
        .await
    });

    let (said, discarded) = said
        .expect("the exchange finished in time")
        .expect("the exchange");
    assert_eq!(
        (said.as_slice(), discarded),
        (b"heard a kettle\n".as_slice(), 0)
    );
    assert!(
        process.take_stdin().is_none(),
        "standard input was handed over twice"
    );
    crucible_runtime::answered!(process.stop()).expect("cleanup");
}

/// A command that never reads: the pipe fills and the write waits, and the
/// runtime's only thread goes on running other work meanwhile.
///
/// A writer that held the thread would hold it until the command exits, so
/// the runtime runs on a thread of its own and the test gives up on it rather
/// than hang with it.
#[test]
fn a_write_a_command_is_not_reading_waits_without_holding_the_runtime() {
    let mut process = unconfined_child(shell("exec sleep 3"), SandboxSpeech::Held).expect("a peer");
    let input = process
        .take_async_stdin()
        .expect("a command built Held hands back an input");

    let (gave_up, ticks) =
        writing_to_a_deaf_command(input).expect("the write held the runtime's only thread");

    assert!(gave_up, "a write to a command that never reads answered");
    assert_eq!(ticks, Some(10), "other work stopped while the write waited");
    crucible_runtime::answered!(process.stop()).expect("cleanup");
}

/// Writes to `input`, whose command never reads, for half a second on a
/// runtime with one thread, while another task on it ticks ten times: whether
/// the writes gave up, and how far the ticks got. `None` where the runtime was
/// still held after two seconds, which only a write that holds its thread
/// does; the runtime runs on a thread of its own so the test can give up on it.
pub(crate) fn writing_to_a_deaf_command(
    mut input: Box<dyn SandboxInput>,
) -> Option<(bool, Option<u32>)> {
    let (answer, answered) = std::sync::mpsc::channel();
    let running = thread::spawn(move || {
        let Ok(runtime) = runtime() else {
            return;
        };
        let ticked = runtime.block_on(async move {
            let ticks = tokio::spawn(async {
                let mut ticks = 0_u32;
                loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    ticks += 1;
                    if ticks == 10 {
                        return ticks;
                    }
                }
            });
            let chunk = vec![b'x'; 16 * 1024];
            let gave_up = tokio::time::timeout(Duration::from_millis(500), async {
                while input.write(&chunk).await.is_ok() {}
            })
            .await
            .is_err();
            (gave_up, ticks.await.ok())
        });
        let _ = answer.send(ticked);
    });
    let ticked = answered.recv_timeout(Duration::from_secs(2)).ok()?;
    running.join().ok()?;
    Some(ticked)
}

/// A quiet command's output is waited on through the reactor alone: the
/// runtime has no clock for a read to pause on.
#[test]
fn a_read_of_a_quiet_command_waits_on_the_pipe_rather_than_a_clock() {
    let mut process =
        unconfined_child(shell("sleep 0.2; printf late"), SandboxSpeech::Closed).expect("a step");
    let mut output = process.take_stdout().expect("stdout");
    let reactor_only = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("a runtime with a reactor and no clock");

    let said = reactor_only.block_on(heard(output.as_mut()));

    assert_eq!(said.expect("the output"), (b"late".to_vec(), 0));
    crucible_runtime::answered!(process.stop()).expect("cleanup");
}

/// The command's output budget counts what a waiting read takes exactly as
/// it counts what a read without waiting takes.
#[test]
fn the_output_budget_counts_a_waiting_read_as_a_read_without_waiting() {
    let script = "head -c 1048576 /dev/zero; exec sleep 5";
    let limited = |speech| {
        let mut plan = testing_plan(speech, None).expect("a plan");
        plan.limits.output_bytes = Some(1000);
        spawn(shell(script), plan).expect("a step")
    };

    let mut first = limited(SandboxSpeech::Closed);
    let mut stdout = first.take_stdout().expect("stdout");
    let (polled_kept, polled_discarded) = polled(stdout.as_mut()).expect("the output");
    let polled_violation = first.violation();
    crucible_runtime::answered!(first.stop()).expect("cleanup");

    let mut second = limited(SandboxSpeech::Closed);
    let mut stdout = second.take_stdout().expect("stdout");
    let (waited_kept, waited_discarded) = runtime()
        .expect("a test runtime")
        .block_on(async { tokio::time::timeout(WAIT, heard(stdout.as_mut())).await })
        .expect("the output ended in time")
        .expect("the output");
    let waited_violation = second.violation();
    crucible_runtime::answered!(second.stop()).expect("cleanup");

    assert_eq!(waited_kept, polled_kept);
    assert_eq!(waited_kept.len(), 1000);
    assert!(polled_discarded > 0 && waited_discarded > 0);
    assert_eq!(
        (waited_violation, polled_violation),
        (
            Some(SandboxViolation::Output),
            Some(SandboxViolation::Output)
        )
    );
}

/// A credential the command prints is masked on a waiting read as on a read
/// without waiting, on both streams: printed whole, and printed in two writes
/// with a pause between them, so a read can end between its two parts.
#[test]
fn a_printed_credential_is_masked_on_a_waiting_read_as_on_one_without_waiting() {
    let secret = "a-credential-value-0123456789";
    // Printed whole, then split across two writes, then on standard error.
    let script = "printf 'id=%s\\n' \"$SECRET\"; \
                  printf '%s' \"${SECRET%??????????}\"; sleep 0.05; \
                  printf '%s\\n' \"${SECRET#???????????????????}\"; \
                  printf 'err=%s\\n' \"$SECRET\" >&2";
    let (polled_out, polled_err, mut first) = masked(secret, script, &mut |output| {
        polled(output).expect("the output")
    })
    .expect("a step");
    crucible_runtime::answered!(first.stop()).expect("cleanup");
    let (waited_out, waited_err, mut second) = masked(secret, script, &mut |output| {
        runtime()
            .expect("a test runtime")
            .block_on(async { tokio::time::timeout(WAIT, heard(output)).await })
            .expect("the output ended in time")
            .expect("the output")
    })
    .expect("a step");
    crucible_runtime::answered!(second.stop()).expect("cleanup");
    let (polled_said, waited_said) = ((polled_out, polled_err), (waited_out, waited_err));

    let stars = "*".repeat(secret.len());
    assert!(
        waited_said.0.0 == format!("id={stars}\n{stars}\n").into_bytes(),
        "{waited_said:?}"
    );
    assert_eq!(
        waited_said.1.0,
        format!("err={stars}\n").into_bytes(),
        "standard error"
    );
    assert!(
        !String::from_utf8_lossy(&waited_said.0.0).contains(secret),
        "the credential was printed in the clear"
    );
    assert_eq!(waited_said, polled_said);
}

/// What one stream of a command left once it was read.
type Left = (Vec<u8>, usize);

/// Runs `script` with `secret` in its environment as a credential, reads its
/// standard output and then its standard error with `read`, and hands back
/// the process for the test to stop.
fn masked(
    secret: &str,
    script: &str,
    read: &mut dyn FnMut(&mut dyn SandboxOutput) -> Left,
) -> Result<(Left, Left, Box<dyn SandboxProcess>), crucible_sandbox::SandboxError> {
    let mut command = shell(script);
    command.env("SECRET", secret);
    let mut plan = testing_plan(SandboxSpeech::Closed, None)?;
    plan.credentials = vec![secret.as_bytes().to_vec()];
    let mut process = spawn(command, plan)?;
    let missing = || crucible_sandbox::SandboxError::Spawn(io::Error::other("a stream is missing"));
    let mut stdout = process.take_stdout().ok_or_else(missing)?;
    let mut stderr = process.take_stderr().ok_or_else(missing)?;
    Ok((read(stdout.as_mut()), read(stderr.as_mut()), process))
}
