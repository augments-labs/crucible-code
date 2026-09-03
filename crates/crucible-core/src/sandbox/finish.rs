//! How a confined process finished, and the wait that gives it the chance to.
//!
//! Every host that speaks to a confined process ends the same way and for the
//! same reason. Crucible's end of the pipe closes first, which is how the
//! program is told there is nothing further to wait for; then it is given a
//! grace to act on that, because a process killed while it was still tidying up
//! left whatever it was tidying half done; and only then is it stopped.
//!
//! Three endings rather than a success and a failure. Going quietly and being
//! stopped are both ordinary — one program exits on a closed pipe and another
//! waits to be told twice — and neither is anybody's problem afterwards. Not
//! being able to stop it is the third, and it is somebody's problem: the
//! sandbox could not confirm that everything the command owned is gone.

use std::io;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use super::SandboxProcess;

/// How long the wait for a process to finish sleeps between looks.
const WATCH: Duration = Duration::from_millis(5);

/// How a confined process finished.
#[derive(Debug)]
pub enum Finish {
    /// It ended on its own, within the grace it was given.
    Exited(ExitStatus),

    /// It did not, so it was stopped and its scope was reaped.
    Stopped,

    /// It did not, and stopping it failed.
    ///
    /// The sandbox could not confirm that everything the command owned is gone,
    /// which is the one ending that is somebody's problem afterwards.
    Unreaped(io::Error),
}

impl Finish {
    /// Waits out `grace` for `process` to finish, and stops it where it does not.
    ///
    /// The caller closes its end of the conversation first; this is only the
    /// waiting and the stopping.
    #[must_use]
    pub fn after(process: &mut dyn SandboxProcess, grace: Duration) -> Self {
        let began = Instant::now();
        loop {
            // An error here is not an ending, it is not knowing, and the remedy
            // for not knowing is the same as for a process that will not go:
            // stop it.
            if let Ok(Some(status)) = process.try_wait() {
                return Self::Exited(status);
            }
            let Some(left) = grace.checked_sub(began.elapsed()) else {
                break;
            };
            thread::sleep(left.min(WATCH));
        }
        match process.stop() {
            Ok(()) => Self::Stopped,
            Err(source) => Self::Unreaped(source),
        }
    }
}
