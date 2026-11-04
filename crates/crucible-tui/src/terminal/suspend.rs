//! Giving the terminal to a child, and taking it back.
//!
//! A sign-in that needs a real terminal cannot be drawn by crucible while it
//! runs: the child needs the screen, the keys and the signals for itself. What
//! is left is to hand over everything the session holds, wait, and put back
//! exactly what was left.
//!
//! The modes are the guards' own. [`Screen`](super::screen::Screen),
//! [`Raw`](super::raw::Raw), [`Spelling`](super::keyboard::Spelling),
//! [`Pasting`](super::keyboard::Pasting) and
//! [`Reporting`](super::mouse::Reporting) go on holding them for the session;
//! this goes out through the same sequences without taking a second holder, so
//! the count in `mouse` and the state in every guard stay true. On the way back
//! it enters only what it left: a native session never took the alternate
//! screen or the pointer, and a session that holds neither is left with only
//! the foreground process group to give back.
//!
//! The child is put in a process group of its own before this is called, and
//! this makes that group the terminal's foreground group. Ctrl+C is then the
//! terminal's own: it is sent to the child and to nothing else. Taking the
//! foreground group back while the parent is in the background is the one
//! place the kernel would stop the parent with `SIGTTOU`, so that signal is
//! ignored for the length of the one call and the handler found is put back.
//!
//! Everything here is Unix: a process group, a controlling terminal and
//! `tcsetpgrp` are the platform's. The route is refused before a process
//! starts where they are not there.

use std::io;
use std::sync::Mutex;

use rustix::process::{Pid, getpgrp};
#[cfg(all(test, unix))]
use rustix::termios::tcgetpgrp;
use rustix::termios::tcsetpgrp;

use super::{Held, keyboard, mouse, raw, screen};

/// What can go wrong handing the terminal over or taking it back.
#[derive(Debug, thiserror::Error)]
pub enum SuspendError {
    /// The terminal could not be left for the child.
    #[error("the terminal could not be given to the sign-in: {0}")]
    Handed(#[source] io::Error),
    /// The terminal could not be taken back from the child.
    #[error("the terminal could not be taken back from the sign-in: {0}")]
    Resumed(#[source] io::Error),
}

impl Held {
    /// The modes in the order the guards enter them.
    fn taken(self) -> [Option<Mode>; 5] {
        [
            self.screen.then_some(Mode::Screen),
            self.raw.then_some(Mode::Raw),
            self.keyboard.then_some(Mode::Keyboard),
            self.paste.then_some(Mode::Paste),
            self.mouse.then_some(Mode::Mouse),
        ]
    }
}

/// One mode the terminal holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Screen,
    Raw,
    Keyboard,
    Paste,
    Mouse,
}

/// What a `Suspend` does to the terminal.
///
/// A trait rather than free functions so a test can watch every mode leave,
/// every mode come back, and the order they did it in without a terminal to do
/// it on — the shape every guard in this module already uses.
trait Effects: Send {
    fn leave(&self, mode: Mode) -> io::Result<()>;
    fn enter(&self, mode: Mode) -> io::Result<()>;
    fn foreground(&self, pid: u32) -> io::Result<()>;
    fn reclaim(&self) -> io::Result<()>;
}

/// The terminal this process actually has.
struct System;

impl Effects for System {
    fn leave(&self, mode: Mode) -> io::Result<()> {
        match mode {
            Mode::Screen => screen::leave(),
            Mode::Raw => raw::disable(),
            Mode::Keyboard => keyboard::pop(),
            Mode::Paste => keyboard::unbracket(),
            Mode::Mouse => mouse::silence(),
        }
    }

    fn enter(&self, mode: Mode) -> io::Result<()> {
        match mode {
            Mode::Screen => screen::enter(),
            Mode::Raw => raw::enable(),
            Mode::Keyboard => keyboard::push(),
            Mode::Paste => keyboard::bracket(),
            Mode::Mouse => mouse::report(),
        }
    }

    fn foreground(&self, pid: u32) -> io::Result<()> {
        let pid = i32::try_from(pid).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        let pid = Pid::from_raw(pid).ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        tcsetpgrp(rustix::stdio::stdin(), pid).map_err(Into::into)
    }

    fn reclaim(&self) -> io::Result<()> {
        // The kernel sends `SIGTTOU` to a background process that changes the
        // terminal's foreground group, and the parent is the background one
        // at this point. Ignoring it for the call is how every shell hands
        // control back after running a job. The handler found is restored
        // whatever the call did, so nothing outside this one `tcsetpgrp`
        // runs with a signal disposition this crate chose.
        //
        // SAFETY: `signal` is the C library call that changes one signal's
        // disposition; both arguments are constants from the same C library,
        // and the previous handler it returns is restored below.
        #[allow(unsafe_code)]
        let previous = unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) };
        let reclaimed = tcsetpgrp(rustix::stdio::stdin(), getpgrp());
        // SAFETY: as above, restoring the handler the call before this one
        // found rather than choosing a disposition.
        #[allow(unsafe_code)]
        unsafe {
            libc::signal(libc::SIGTTOU, previous);
        }
        if previous == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
        reclaimed.map_err(Into::into)
    }
}

/// The terminal, lent to a sign-in child for as long as it runs.
///
/// One is made before the child starts and dropped once it has ended or been
/// stopped. [`Suspend::handed`] leaves every mode `held` names and makes the
/// child's process group the foreground one; [`Suspend::resumed`] takes the
/// foreground group back, enters the modes again and calls `repaint`, which is
/// what makes the next frame write every row rather than trust a screen the
/// child may have changed.
pub struct Suspend {
    held: Held,
    effects: Box<dyn Effects>,
    state: Mutex<State>,
    /// What to ask the drawing thread for once the terminal is back.
    repaint: Box<dyn Fn() + Send + Sync>,
}

#[derive(Debug, Default)]
struct State {
    /// The modes this left, in the order it left them.
    left: Vec<Mode>,
    /// Whether the child holds the terminal.
    handed: bool,
}

impl std::fmt::Debug for Suspend {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Suspend")
            .field("held", &self.held)
            .field("left", &self.state.lock().map(|state| state.left.len()))
            .finish_non_exhaustive()
    }
}

impl Suspend {
    /// A handover of a terminal holding `held`, asking `repaint` to draw every
    /// row once the terminal is taken back.
    #[must_use]
    pub fn holding(held: Held, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        Self::over(held, Box::new(System), Box::new(repaint))
    }

    fn over(held: Held, effects: Box<dyn Effects>, repaint: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            held,
            effects,
            state: Mutex::new(State::default()),
            repaint,
        }
    }

    /// Leaves every mode this session holds and makes `pid`'s process group
    /// the terminal's foreground group, so the child owns the screen, the keys
    /// and Ctrl+C.
    ///
    /// # Errors
    ///
    /// `SuspendError::Handed` if a mode could not be left or the child could
    /// not be made the foreground group. The modes already left are entered
    /// again before this returns.
    pub fn handed(&self, pid: u32) -> Result<(), SuspendError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.handed {
            return Ok(());
        }
        state.handed = true;

        for mode in self.held.taken().into_iter().flatten().rev() {
            if let Err(why) = self.effects.leave(mode) {
                self.restore(&mut state);
                state.handed = false;
                return Err(SuspendError::Handed(why));
            }
            state.left.push(mode);
        }
        if let Err(why) = self.effects.foreground(pid) {
            self.restore(&mut state);
            state.handed = false;
            return Err(SuspendError::Handed(why));
        }
        Ok(())
    }

    /// Takes the terminal's foreground group back and enters every mode this
    /// left, then asks for a full repaint.
    ///
    /// # Errors
    ///
    /// `SuspendError::Resumed` if the foreground group could not be taken
    /// back or a mode could not be entered. Every mode is still given back
    /// before this returns.
    pub fn resumed(&self) -> Result<(), SuspendError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.handed {
            return Ok(());
        }
        state.handed = false;

        let mut trouble = self.effects.reclaim().err();
        self.restore(&mut state);
        (self.repaint)();
        match trouble.take() {
            Some(why) => Err(SuspendError::Resumed(why)),
            None => Ok(()),
        }
    }

    /// Puts back every mode in `state`, in the reverse order it was left, and
    /// forgets them.
    fn restore(&self, state: &mut State) {
        for mode in state.left.drain(..).rev() {
            // Best effort: the first failure is what the caller is told, and
            // the rest of the modes have to be tried whatever one does.
            let _ = self.effects.enter(mode);
        }
    }
}

impl Drop for Suspend {
    fn drop(&mut self) {
        // Best effort, like every other guard here: by the time this runs there
        // is nowhere left to report a terminal that would not come back, and
        // the modes that did come back are the ones that matter.
        let _ = self.resumed();
    }
}

/// The foreground process group of this process's terminal.
#[cfg(all(test, unix))]
pub(super) fn foreground_group() -> io::Result<Pid> {
    tcgetpgrp(rustix::stdio::stdin()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// One thing a handover did, in order.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Did {
        Leave(Mode),
        Enter(Mode),
        Foreground(u32),
        Reclaim,
    }

    /// A terminal that records what was asked of it.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Vec<Did>>>);

    impl Fake {
        fn did(&self) -> Vec<Did> {
            self.0.lock().map(|did| did.clone()).unwrap_or_default()
        }
    }

    impl Effects for Fake {
        fn leave(&self, mode: Mode) -> io::Result<()> {
            self.0.lock().unwrap().push(Did::Leave(mode));
            Ok(())
        }

        fn enter(&self, mode: Mode) -> io::Result<()> {
            self.0.lock().unwrap().push(Did::Enter(mode));
            Ok(())
        }

        fn foreground(&self, pid: u32) -> io::Result<()> {
            self.0.lock().unwrap().push(Did::Foreground(pid));
            Ok(())
        }

        fn reclaim(&self) -> io::Result<()> {
            self.0.lock().unwrap().push(Did::Reclaim);
            Ok(())
        }
    }

    fn over(held: Held) -> (Suspend, Fake, Arc<Mutex<usize>>) {
        let fake = Fake::default();
        let repainted = Arc::new(Mutex::new(0));
        let count = Arc::clone(&repainted);
        let suspend = Suspend::over(
            held,
            Box::new(fake.clone()),
            Box::new(move || {
                if let Ok(mut count) = count.lock() {
                    *count += 1;
                }
            }),
        );
        (suspend, fake, repainted)
    }

    fn all() -> Held {
        Held {
            screen: true,
            raw: true,
            keyboard: true,
            paste: true,
            mouse: true,
        }
    }

    fn repainted(count: &Arc<Mutex<usize>>) -> usize {
        count.lock().map(|count| *count).unwrap_or_default()
    }

    #[test]
    fn every_mode_the_session_holds_is_left_and_entered_again() {
        let (suspend, fake, repaints) = over(all());

        suspend.handed(41).expect("the terminal was given");
        assert_eq!(
            fake.did(),
            [
                Did::Leave(Mode::Mouse),
                Did::Leave(Mode::Paste),
                Did::Leave(Mode::Keyboard),
                Did::Leave(Mode::Raw),
                Did::Leave(Mode::Screen),
                Did::Foreground(41),
            ]
        );
        assert_eq!(
            repainted(&repaints),
            0,
            "it repainted while the child held it"
        );

        suspend.resumed().expect("the terminal came back");
        assert_eq!(
            fake.did().get(6..).unwrap_or_default(),
            [
                Did::Reclaim,
                Did::Enter(Mode::Screen),
                Did::Enter(Mode::Raw),
                Did::Enter(Mode::Keyboard),
                Did::Enter(Mode::Paste),
                Did::Enter(Mode::Mouse),
            ]
        );
        assert_eq!(repainted(&repaints), 1, "the frame was not repainted");
    }

    #[test]
    fn a_native_session_leaves_no_screen_or_pointer_and_enters_only_what_it_left() {
        let (suspend, fake, _) = over(Held::default());

        suspend.handed(7).expect("the terminal was given");
        suspend.resumed().expect("the terminal came back");

        assert_eq!(fake.did(), [Did::Foreground(7), Did::Reclaim]);
    }

    #[test]
    fn a_hand_over_the_stack_unwound_past_gives_the_terminal_back() {
        let (suspend, fake, repaints) = over(all());
        suspend.handed(9).expect("the terminal was given");

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _held = suspend;
            panic!("the door was handed a terminal that had gone");
        }));

        assert!(unwound.is_err(), "nothing unwound");
        assert_eq!(fake.did().last(), Some(&Did::Enter(Mode::Mouse)));
        assert_eq!(repainted(&repaints), 1, "the frame was not repainted");
    }

    #[test]
    fn a_mode_that_would_not_leave_is_not_reported_as_left() {
        struct Refuses;

        impl Effects for Refuses {
            fn leave(&self, _mode: Mode) -> io::Result<()> {
                Err(io::Error::other("the terminal went away"))
            }
            fn enter(&self, _mode: Mode) -> io::Result<()> {
                Ok(())
            }
            fn foreground(&self, _pid: u32) -> io::Result<()> {
                Ok(())
            }
            fn reclaim(&self) -> io::Result<()> {
                Ok(())
            }
        }

        let suspend = Suspend::over(all(), Box::new(Refuses), Box::new(|| {}));
        assert!(matches!(suspend.handed(3), Err(SuspendError::Handed(_))));
        assert_eq!(
            suspend
                .state
                .lock()
                .map(|state| state.left.len())
                .unwrap_or_default(),
            0,
            "a mode that never left was left for the drop to put back"
        );
    }

    /// The one place a real terminal has to be used: a child's process group
    /// becomes the foreground group, and Ctrl+C reaches it and nothing else.
    ///
    /// The case runs a helper in a pty of its own because this test process's
    /// controlling terminal is the one the harness is running in. The helper
    /// starts a session on the pty, puts a shell in a process group of its own,
    /// hands it the terminal, and waits. This test sends Ctrl+C down the pty
    /// and the shell's own trap says it arrived. The helper surviving to write
    /// its last line is what says the signal did not reach the parent.
    #[cfg(unix)]
    #[test]
    fn a_pty_child_is_the_foreground_group_and_ctrl_c_reaches_only_it() {
        use std::io::{Read as _, Write as _};
        use std::os::unix::ffi::OsStrExt as _;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        if std::env::var_os("CRUCIBLE_SUSPEND_PTY_HELPER").is_some() {
            return;
        }

        let master =
            rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
                .expect("a pty");
        rustix::pty::grantpt(&master).expect("the pty is granted");
        rustix::pty::unlockpt(&master).expect("the pty is unlocked");
        let name = rustix::pty::ptsname(&master, Vec::new()).expect("the pty's name");
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(name.as_bytes()));
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("the pty's other side");

        let mut running = Command::new(std::env::current_exe().expect("the test binary"))
            .arg("--exact")
            .arg("terminal::suspend::tests::a_pty_helper_runs_when_the_outer_case_asks")
            .arg("--nocapture")
            .env("CRUCIBLE_SUSPEND_PTY_HELPER", "1")
            .stdin(Stdio::from(
                slave.try_clone().expect("a descriptor for the child"),
            ))
            .stdout(Stdio::from(
                slave.try_clone().expect("a descriptor for the child"),
            ))
            .stderr(Stdio::from(slave))
            .spawn()
            .expect("the helper starts");

        let mut master = std::fs::File::from(master);
        let mut sending = master.try_clone().expect("a writer for the pty");
        let mut seen = String::new();
        let mut bytes = [0_u8; 256];
        let started = Instant::now();
        while !seen.contains("ready-for-interrupt") {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the helper never asked for the interrupt: {seen}"
            );
            let read = master.read(&mut bytes).expect("the pty says something");
            seen.push_str(&String::from_utf8_lossy(
                bytes.get(..read).unwrap_or_default(),
            ));
            if seen.len() > 64 * 1024 {
                seen = seen.split_off(seen.len() - 32 * 1024);
            }
        }
        sending.write_all(b"\x03").expect("Ctrl+C reaches the pty");
        sending.flush().expect("it leaves");

        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = running.try_wait().expect("the helper is waited for") {
                assert!(status.success(), "the helper failed: {seen}");
                break;
            }
            assert!(Instant::now() < until, "the helper did not finish: {seen}");
            while let Ok(read) = master.read(&mut bytes) {
                if read == 0 {
                    break;
                }
                seen.push_str(&String::from_utf8_lossy(
                    bytes.get(..read).unwrap_or_default(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(seen.contains("child-interrupted"), "{seen}");
        assert!(seen.contains("parent-still-here"), "{seen}");
    }

    /// The child half of [`a_pty_child_is_the_foreground_group_and_ctrl_c_reaches_only_it`],
    /// run in its own pty by that case.
    #[cfg(unix)]
    #[test]
    fn a_pty_helper_runs_when_the_outer_case_asks() {
        use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
        use std::process::Command;

        if std::env::var_os("CRUCIBLE_SUSPEND_PTY_HELPER").is_none() {
            return;
        }
        rustix::process::setsid().expect("a session of its own");
        rustix::process::ioctl_tiocsctty(rustix::stdio::stdin()).expect("a controlling terminal");

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("echo child-ready; while :; do sleep 1; done")
            .process_group(0)
            .spawn()
            .expect("the child starts");
        let pid = child.id();
        let suspend = Suspend::holding(Held::default(), || {});
        suspend.handed(pid).expect("the terminal is handed over");
        assert_eq!(
            foreground_group()
                .expect("the terminal reports its foreground group")
                .as_raw_pid(),
            i32::try_from(pid).expect("a pid this process can name"),
            "the child is not the terminal's foreground group"
        );
        println!("ready-for-interrupt");

        let status = child.wait().expect("the child is waited for");
        assert_eq!(
            status.signal(),
            Some(libc::SIGINT),
            "the child did not take Ctrl+C"
        );
        println!("child-interrupted");
        suspend.resumed().expect("the terminal comes back");
        assert_eq!(
            foreground_group()
                .expect("the terminal reports its foreground group")
                .as_raw_pid(),
            rustix::process::getpgrp().as_raw_pid(),
            "the parent did not take its terminal back"
        );
        println!("parent-still-here");
    }
}
