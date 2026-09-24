//! A proxy credential held back when crucible cut the command's output.
//!
//! Each stream masks the credential on its own, but stdout and stderr share one
//! output budget and one status task. These tests drive both streams through the
//! shared control, and a real command through a stop, to show that the stream
//! holding the start of a credential learns that crucible cut it, even when the
//! other stream or a deadline was the cause.

use super::super::*;

use std::collections::VecDeque;

use base64::Engine as _;
use crucible_sandbox::{SandboxDomainPolicy, SandboxNetworkProvenance, SandboxSpeech};
use crucible_types::{Ancestry, ToolId};

use crate::network::Mediator;

/// A pipe that answers from a script: bytes, `None` for nothing yet, and its
/// end once the script runs out.
struct Script(VecDeque<Option<Vec<u8>>>);

impl Stream for Script {
    fn read<'a>(&'a mut self, _: &'a mut [u8]) -> BoxFuture<'a, io::Result<ReadState>> {
        Box::pin(std::future::ready(Err(io::Error::other(
            "a script is read through read_ready",
        ))))
    }

    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState> {
        let Some(step) = self.0.pop_front() else {
            return Ok(ReadState::End);
        };
        let Some(bytes) = step else {
            return Ok(ReadState::Pending);
        };
        let written = buffer
            .get_mut(..bytes.len())
            .ok_or_else(|| io::Error::other("script step exceeds the read buffer"))?;
        written.copy_from_slice(&bytes);
        Ok(ReadState::Bytes(bytes.len()))
    }
}

/// A listener with a fresh credential, and that credential's password.
fn proxy() -> io::Result<(Mediator, String)> {
    let policy = SandboxDomainPolicy::new([], [], false, [], SandboxNetworkProvenance::User)
        .map_err(io::Error::other)?;
    let proxy = Mediator::tcp(policy, SandboxId::new(), Some(Duration::from_secs(5)))?;
    let userinfo = proxy
        .authorization()
        .strip_prefix("Basic ")
        .map(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded))
        .ok_or_else(|| io::Error::other("authorization is not basic"))?
        .map_err(io::Error::other)?;
    let userinfo = String::from_utf8(userinfo).map_err(io::Error::other)?;
    let (_, password) = userinfo
        .split_once(':')
        .ok_or_else(|| io::Error::other("userinfo has no password"))?;
    Ok((proxy, password.to_owned()))
}

fn control(output_limit: Option<u64>) -> Arc<Control> {
    Arc::new(Control::new(
        output_limit,
        SandboxAudit::new(Ancestry::new(), ToolId::new("test-process")),
        SandboxId::new(),
    ))
}

/// One stream of the command `control` governs, masked as a command's is.
fn stream<const N: usize>(
    proxy: &Mediator,
    control: &Arc<Control>,
    script: [Option<Vec<u8>>; N],
) -> Box<dyn SandboxOutput> {
    let prepared = PreparedOutput::new(Box::new(Script(script.into())), Arc::clone(control));
    protect_output(Box::new(prepared), proxy.masked(), control)
}

/// "id=" and the first 20 bytes of the password: the start of a credential
/// whose rest has not been written.
fn started(password: &str) -> io::Result<Vec<u8>> {
    let start = password
        .as_bytes()
        .get(..20)
        .ok_or_else(|| io::Error::other("password is shorter than 20 bytes"))?;
    Ok([b"id=".as_slice(), start].concat())
}

fn masked() -> Vec<u8> {
    [b"id=".as_slice(), &[b'*'; 20]].concat()
}

/// Everything a stream still gives, and how much of it the limit discarded.
fn rest(output: &mut dyn SandboxOutput) -> io::Result<(Vec<u8>, usize)> {
    let mut kept = Vec::new();
    let mut discarded = 0;
    let mut buffer = [0; 256];
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::other("output did not end"));
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
            .ok_or_else(|| io::Error::other("read exceeds the buffer"))?;
        kept.extend_from_slice(read);
    }
}

/// The security clear's counterexample. stdout has printed the start of the
/// credential and is still quiet when stderr goes over the budget the two
/// share; the status task then kills the command, and stdout reads its end
/// without ever seeing a discard of its own.
#[test]
fn a_credential_start_on_stdout_is_masked_when_stderr_breaks_the_shared_limit() {
    let (mut proxy, password) = proxy().unwrap();
    let control = control(Some(32));
    let mut stdout = stream(&proxy, &control, [Some(started(&password).unwrap()), None]);
    // `-` begins neither the hex password nor its base64 form, so the cut
    // stderr keeps its last byte as it is.
    let mut stderr = stream(&proxy, &control, [Some(vec![b'-'; 100])]);

    let mut buffer = [0; 256];
    assert_eq!(
        stdout.read_ready(&mut buffer).unwrap(),
        SandboxRead::Bytes(3)
    );
    assert_eq!(rest(stderr.as_mut()).unwrap(), (vec![b'-'; 9], 91));
    assert_eq!(control.violation(), Some(SandboxViolation::Output));

    assert_eq!(rest(stdout.as_mut()).unwrap(), (masked().split_off(3), 0));
    proxy.stop().unwrap();
}

/// The same cut by the deadline: the status task marks the command as having
/// run too long, then kills it.
#[test]
fn a_credential_start_is_masked_when_the_deadline_ends_the_command() {
    let (mut proxy, password) = proxy().unwrap();
    let control = control(None);
    let mut stdout = stream(&proxy, &control, [Some(started(&password).unwrap()), None]);

    let mut buffer = [0; 256];
    assert_eq!(
        stdout.read_ready(&mut buffer).unwrap(),
        SandboxRead::Bytes(3)
    );
    control.mark(SandboxViolation::CommandTime);

    assert_eq!(rest(stdout.as_mut()).unwrap(), (masked().split_off(3), 0));
    proxy.stop().unwrap();
}

/// A command that ends its own output printed only the start, and that is
/// what it shows, byte for byte.
#[test]
fn a_credential_start_is_released_when_the_command_ends_its_own_output() {
    let (mut proxy, password) = proxy().unwrap();
    let control = control(Some(1024));
    let mut stdout = stream(&proxy, &control, [Some(started(&password).unwrap()), None]);

    assert_eq!(
        rest(stdout.as_mut()).unwrap(),
        (started(&password).unwrap(), 0)
    );
    assert_eq!(control.violation(), None);
    proxy.stop().unwrap();
}

/// A running command crucible stops — a tool's deadline, a cancelled turn —
/// cuts its output as surely as a limit does.
#[test]
fn a_credential_start_is_masked_when_a_running_command_is_stopped() {
    let (proxy, _) = proxy().unwrap();
    let mut command = std::process::Command::new("/bin/sh");
    command.args([
        "-c",
        "printf 'id=%.20s' \"${HTTP_PROXY#http://crucible:}\"; exec sleep 30",
    ]);
    command.envs(proxy.environment(proxy.address()));
    let mut plan = testing_plan(SandboxSpeech::Closed, None).unwrap();
    plan.network = Some(proxy);
    let mut process = spawn(command, plan).unwrap();
    let mut stdout = process.take_stdout().unwrap();

    let mut buffer = [0; 256];
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "the command printed nothing");
        match stdout.read_ready(&mut buffer).unwrap() {
            SandboxRead::Pending => thread::sleep(Duration::from_millis(1)),
            read => {
                assert_eq!(read, SandboxRead::Bytes(3));
                break;
            }
        }
    }
    crucible_runtime::answered!(process.stop()).unwrap();

    assert_eq!(rest(stdout.as_mut()).unwrap(), (masked().split_off(3), 0));
}
