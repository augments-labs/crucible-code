//! The thread a Windows command's asynchronous input is written on belongs to
//! the process: its stop joins it, on every path, and a write first made after
//! the stop starts none.

use super::*;

use crucible_sandbox::SandboxSpeech;

/// `cmd` looping forever in itself, never reading its standard input, so the
/// pipe fills and killing `cmd` leaves nothing behind.
fn deaf() -> Command {
    let mut command = Command::new("cmd.exe");
    std::os::windows::process::CommandExt::raw_arg(
        command.args(["/d", "/c"]),
        "for /l %i in (0,0,1) do @rem",
    );
    command
}

fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// Writes to `input` until a write waits on a full pipe, and gives that write
/// up, leaving the thread parked in it; `false` where the pipe never filled.
fn parked(input: &mut dyn crucible_sandbox::SandboxInput) -> io::Result<bool> {
    let filled = runtime()?.block_on(async {
        let chunk = vec![b'x'; 4096];
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let wrote = tokio::time::timeout(Duration::from_millis(200), input.write(&chunk));
                if wrote.await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
    });
    Ok(filled)
}

/// The process keeps the thread its input's first write started, and its stop
/// joins it: neither the hand-over nor the join can be left out unseen.
#[test]
fn stopping_a_process_joins_the_thread_its_input_is_written_on() {
    let mut process = testing_local(deaf(), SandboxSpeech::Held, None).expect("a peer");
    let mut input = process
        .take_async_stdin()
        .expect("a command built Held hands back an input");
    assert!(
        parked(input.as_mut()).expect("a test runtime"),
        "the pipe never filled"
    );
    assert!(
        process.input_thread.started(),
        "the input's thread was not left with its process"
    );

    process.stop().expect("cleanup");

    assert!(
        process.input_thread.joined(),
        "the process's stop left its input's thread unjoined"
    );
    drop(input);
}

/// A stop whose scope could not be stopped still ends and joins the input's
/// thread, abandoning the parked write while the command runs on, and a
/// later stop that succeeds finds nothing of it left to do.
#[test]
fn a_stop_that_fails_still_joins_the_input_thread() {
    let mut process = testing_local(deaf(), SandboxSpeech::Held, None).expect("a peer");
    let mut input = process
        .take_async_stdin()
        .expect("a command built Held hands back an input");
    assert!(
        parked(input.as_mut()).expect("a test runtime"),
        "the pipe never filled"
    );
    process.test_stop = |_, _| Err(io::Error::other("the scope refused to stop"));

    process
        .stop()
        .expect_err("a stop whose scope refused reported success");

    assert!(
        process.input_thread.joined(),
        "a failed stop left the input's thread unjoined"
    );
    process.test_stop = stop_scope;
    process.stop().expect("the retried stop");
    drop(input);
}

/// Taken before the stop and first written after it: the write is refused,
/// and no thread starts that nothing would join.
#[test]
fn an_input_first_written_after_the_stop_starts_no_thread() {
    let mut process = testing_local(deaf(), SandboxSpeech::Held, None).expect("a peer");
    let mut input = process
        .take_async_stdin()
        .expect("a command built Held hands back an input");
    process.stop().expect("cleanup");

    let refused = runtime()
        .expect("a test runtime")
        .block_on(input.write(b"late"))
        .expect_err("a write after the stop was taken");

    assert_eq!(refused.kind(), io::ErrorKind::BrokenPipe);
    assert!(
        !process.input_thread.started(),
        "a thread started after the stop"
    );
}
