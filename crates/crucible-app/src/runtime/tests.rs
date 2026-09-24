//! How the runtime is shut down, and what it says when that runs out of time.

#[cfg(unix)]
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use super::{RuntimeOwner, Unstopped};

#[test]
fn a_runtime_nothing_asked_for_shuts_down_at_once() {
    let owner = RuntimeOwner::new();

    assert!(
        !owner.is_built(),
        "an owner built a runtime nobody asked for"
    );
    assert_eq!(owner.shutdown(), Ok(()));
}

#[test]
fn a_runtime_whose_threads_stop_in_time_shuts_down_cleanly() {
    let owner = RuntimeOwner::new();
    let runtime = owner.handle().unwrap();
    let answered = runtime.block_on(runtime.spawn(async { 5 }));

    assert_eq!(answered.map_err(|failed| failed.to_string()), Ok(5));
    assert_eq!(owner.shutdown(), Ok(()));
}

/// A blocking thread that will not return until the test lets it is the one
/// thing no shutdown can stop, so the bound runs out with it still running,
/// and the shutdown says so. It is let go afterwards, so it outlives the test
/// by no more than the time it takes to see its channel close.
#[test]
fn a_shutdown_that_runs_out_of_time_is_reported_as_failed_cleanup() {
    let owner = RuntimeOwner::new();
    let runtime = owner.handle().unwrap();
    let (release, held) = mpsc::channel::<()>();
    let (started, began) = mpsc::channel();
    let _stuck = runtime.spawn_blocking(move || {
        let _ = started.send(());
        let _ = held.recv();
    });
    began.recv_timeout(Duration::from_secs(5)).unwrap();

    let stopped = owner.shutdown_within(Duration::from_millis(50));
    drop(release);

    assert_eq!(
        stopped,
        Err(Unstopped {
            running: 1,
            waited: Duration::from_millis(50),
        })
    );
    assert_eq!(
        stopped.map_err(|unstopped| unstopped.to_string()),
        Err(
            "1 of the threads crucible runs its work on had not stopped 50 ms after they were \
             asked to, and were left running; what they were doing is unconfirmed"
                .to_owned()
        )
    );
}

/// A hosted program's pipes are read and written by tasks on this runtime, and
/// on Unix a pipe is waited on through the runtime's I/O driver. A runtime
/// built without one fails the first task that waits on a pipe, so the program
/// on the other end would never be heard; this is that wait, made the way the
/// local backend makes it, in work owned on the runtime.
#[cfg(unix)]
#[test]
fn a_pipe_waited_on_in_owned_work_makes_progress() {
    let owner = RuntimeOwner::new();
    let runtime = owner.handle().unwrap();
    let (answer, answered) = mpsc::channel();
    let work = runtime.spawn(async move {
        let _ = answer.send(through_a_pipe().await.map_err(|failed| failed.to_string()));
    });

    let heard = answered.recv_timeout(Duration::from_secs(5));

    assert_eq!(
        heard,
        Ok(Ok(*b"x")),
        "a byte written into a pipe by owned work on the runtime has to be read back out of it"
    );
    drop(work);
    assert_eq!(owner.shutdown(), Ok(()));
}

/// Writes one byte into a pipe and reads it back, waiting on each end.
#[cfg(unix)]
async fn through_a_pipe() -> io::Result<[u8; 1]> {
    let (sender, receiver) = tokio::net::unix::pipe::pipe()?;
    loop {
        sender.writable().await?;
        match sender.try_write(b"x") {
            Ok(_) => break,
            Err(problem) if problem.kind() == io::ErrorKind::WouldBlock => {}
            Err(problem) => return Err(problem),
        }
    }
    let mut byte = [0_u8; 1];
    loop {
        receiver.readable().await?;
        match receiver.try_read(&mut byte) {
            Ok(_) => return Ok(byte),
            Err(problem) if problem.kind() == io::ErrorKind::WouldBlock => {}
            Err(problem) => return Err(problem),
        }
    }
}

/// Work the run owns can talk over a socket: a task spawned onto the runtime
/// connects, and is woken when its peer has said something. A runtime built
/// without its I/O driver refuses the connection outright, saying I/O is
/// disabled.
#[test]
fn a_socket_in_work_on_the_runtime_makes_progress() {
    use std::io::Write as _;

    let owner = RuntimeOwner::new();
    let runtime = owner.handle().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"ping").unwrap();
    });

    let heard = runtime.block_on(runtime.spawn(async move {
        let stream = tokio::net::TcpStream::connect(address).await?;
        let mut said = Vec::new();
        let mut more = [0_u8; 4];
        while said.len() < more.len() {
            stream.readable().await?;
            match stream.try_read(&mut more) {
                Ok(0) => break,
                Ok(read) => said.extend(more.iter().take(read)),
                Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(problem) => return Err(problem),
            }
        }
        Ok::<_, std::io::Error>(said)
    }));
    peer.join().unwrap();

    assert_eq!(
        heard
            .map(|read| read.map_err(|problem| problem.to_string()))
            .map_err(|failed| failed.to_string()),
        Ok(Ok(b"ping".to_vec())),
        "a socket in a task on the runtime made no progress"
    );
    assert_eq!(owner.shutdown(), Ok(()));
}
