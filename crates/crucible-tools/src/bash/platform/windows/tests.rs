//! Job lifetime observations use real Windows jobs and owned process handles.

use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::JobObjects::{
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
    QueryInformationJobObject,
};

use super::*;

const WAIT: Duration = Duration::from_secs(5);

struct Job {
    scope: Scope,
    leader: Child,
    descendant: Child,
}

impl Job {
    fn new(arguments: &[&str]) -> Self {
        let mut command = Command::new("cmd.exe");
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let scope = Scope::new(&mut command).expect("job");
        let leader = command.spawn().expect("suspended leader");

        // A suspended member cannot exit on its own or launch work outside this
        // fixture. Assignment supplies the same membership inherited by a real
        // descendant without making the test depend on shell startup timing.
        let descendant = Command::new(std::env::current_exe().expect("test executable"))
            .creation_flags(CREATE_SUSPENDED)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("suspended member");
        let fixture = Self {
            scope,
            leader,
            descendant,
        };
        assert_ne!(
            // SAFETY: both handles are owned by the fixture throughout this call.
            unsafe {
                AssignProcessToJobObject(
                    fixture.scope.0,
                    fixture.descendant.as_raw_handle() as HANDLE,
                )
            },
            0,
            "member assignment: {}",
            io::Error::last_os_error()
        );
        fixture.scope.attach(&fixture.leader).expect("start leader");
        fixture
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = Terminator(self.scope.0).stop();
        for child in [&mut self.leader, &mut self.descendant] {
            let _ = child.kill();
            let _ = exited(child);
        }
    }
}

fn exited(child: &mut Child) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fixture process exit",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn active(scope: &Scope) -> u32 {
    let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    assert_ne!(
        // SAFETY: the owned job is live and the writable buffer has exactly the
        // layout and size required by the selected information class.
        unsafe {
            QueryInformationJobObject(
                scope.0,
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut accounting).cast(),
                u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                    .expect("accounting size"),
                std::ptr::null_mut(),
            )
        },
        0,
        "job accounting: {}",
        io::Error::last_os_error()
    );
    accounting.ActiveProcesses
}

#[test]
fn accepted_termination_does_not_complete_a_job_that_still_has_members() {
    let mut job = Job::new(&["/d", "/c", "exit 0"]);
    exited(&mut job.leader).expect("leader exited normally");
    assert!(
        active(&job.scope) > 0,
        "member remains alive after leader exit"
    );

    // Deliberately signal another empty job: this injects successful termination
    // without extinction of the observed job. Production uses matching handles;
    // the mismatch makes asynchronous termination's pending state deterministic.
    let empty = Scope::new(&mut Command::new("cmd.exe")).expect("empty job");
    let signal = empty.terminator(&job.leader).expect("borrowed terminator");
    assert!(
        job.scope
            .try_wait(&mut job.leader, signal)
            .expect("poll")
            .is_none()
    );
    assert!(
        active(&job.scope) > 0,
        "the injected pending state remains real"
    );
}

#[test]
fn normal_leader_exit_is_reported_only_after_its_job_is_empty() {
    let mut job = Job::new(&["/d", "/c", "exit 0"]);
    exited(&mut job.leader).expect("leader exited normally");
    assert!(
        active(&job.scope) > 0,
        "member remains alive after leader exit"
    );
    let signal = job
        .scope
        .terminator(&job.leader)
        .expect("borrowed terminator");
    let deadline = Instant::now() + WAIT;
    loop {
        if job
            .scope
            .try_wait(&mut job.leader, signal)
            .expect("poll")
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "job did not finish");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        active(&job.scope),
        0,
        "reported completion with active members"
    );
    assert!(job.descendant.try_wait().expect("member exit").is_some());
}

#[test]
fn stopping_a_live_job_confirms_every_member_has_exited() {
    let mut job = Job::new(&["/d", "/c", "set /p VALUE="]);
    assert!(active(&job.scope) > 0, "the job begins alive");
    job.scope.stop(&mut job.leader).expect("stop live job");
    assert_eq!(active(&job.scope), 0, "stop returned with active members");
    assert!(job.leader.try_wait().expect("leader exit").is_some());
    assert!(job.descendant.try_wait().expect("member exit").is_some());
}
