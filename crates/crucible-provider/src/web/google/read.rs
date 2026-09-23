//! Whole-side-response ceilings below SSE framing.
//!
//! Event limits cannot bound a stream of tiny events or comments. The guard is
//! inside the stream's read loop so quiet/trickling responses still meet the
//! deadline, and a byte past the ceiling is a failure rather than a false EOF.
//!
//! The wait is checked before a read is attempted, never used to discard what
//! a read already returned: what a read hands back is used, not replaced by
//! the deadline error, and the wait's only power is over the next attempt.
//! The cancel check after a read stays, because a cancelled call uses nothing
//! it read.

use std::io::{self, Read};
use std::time::{Duration, Instant};

use crucible_runtime::Cancel;

pub(super) struct Limited {
    body: Box<dyn Read + Send>,
    cancel: Cancel,
    remaining: usize,
    started: Instant,
    wait: Duration,
}

impl Limited {
    pub(super) fn new(
        body: Box<dyn Read + Send>,
        cancel: Cancel,
        bytes: usize,
        wait: Duration,
    ) -> Self {
        Self {
            body,
            cancel,
            remaining: bytes,
            started: Instant::now(),
            wait,
        }
    }

    /// Whether a read may be attempted at all: not cancelled, and the wait
    /// not yet spent. Checked once per attempt, before that attempt's read.
    fn check_before(&self) -> io::Result<()> {
        if self.cancel.requested() {
            return Err(io::Error::other("Google web request cancelled"));
        }
        if self.started.elapsed() >= self.wait {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Google web response exceeded its deadline",
            ));
        }
        Ok(())
    }

    /// Whether a read that has already returned should still be honoured:
    /// only cancellation is looked at here, never the wait.
    fn checked(&self) -> io::Result<()> {
        if self.cancel.requested() {
            return Err(io::Error::other("Google web request cancelled"));
        }
        Ok(())
    }
}

impl Read for Limited {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        if into.is_empty() {
            return Ok(0);
        }
        self.check_before()?;
        if self.remaining == 0 {
            let read = self.body.read(&mut [0]);
            self.checked()?;
            return match read {
                Ok(0) => Ok(0),
                Ok(_) => Err(io::Error::other(
                    "Google web response exceeded its byte limit",
                )),
                Err(error) => Err(error),
            };
        }
        let take = into.len().min(self.remaining);
        let read = self.body.read(into.get_mut(..take).unwrap_or_default());
        self.checked()?;
        if let Ok(count) = read {
            self.remaining = self.remaining.saturating_sub(count);
        }
        read
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn exact_cap_is_eof_but_cap_plus_one_is_an_error() {
        for (body, valid) in [("abc", true), ("abcd", false)] {
            let mut reader = Limited::new(
                Box::new(Cursor::new(body)),
                Cancel::new(),
                3,
                Duration::from_secs(1),
            );
            let mut text = String::new();
            assert_eq!(reader.read_to_string(&mut text).is_ok(), valid);
            assert_eq!(text, "abc");
        }
    }

    #[test]
    fn expired_and_cancelled_reads_do_not_touch_the_body() {
        struct Untouched;
        impl Read for Untouched {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("body must not be read");
            }
        }
        let mut expired = Limited::new(Box::new(Untouched), Cancel::new(), 3, Duration::ZERO);
        assert_eq!(
            expired.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        let cancel = Cancel::new();
        cancel.request();
        let mut cancelled = Limited::new(Box::new(Untouched), cancel, 3, Duration::from_secs(1));
        assert!(cancelled.read(&mut [0]).is_err());
    }

    /// A body whose one read sleeps past `wait` and then answers with
    /// `then`, so the timing is driven by the read's own answer rather than
    /// a check racing a real deadline.
    struct Late {
        wait: Duration,
        then: Vec<u8>,
    }

    impl Read for Late {
        fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(self.wait.saturating_add(Duration::from_millis(20)));
            let took = self.then.len().min(into.len());
            into.get_mut(..took)
                .unwrap_or_default()
                .copy_from_slice(self.then.get(..took).unwrap_or_default());
            self.then.drain(..took);
            Ok(took)
        }
    }

    #[test]
    fn a_chunk_a_read_hands_over_as_the_wait_runs_out_is_kept() {
        // The bug this proves against checked the wait right after this
        // read, before looking at what it returned, so a chunk handed over
        // exactly there was discarded in favour of the deadline error.
        let wait = Duration::from_millis(5);
        let mut reader = Limited::new(
            Box::new(Late {
                wait,
                then: b"final".to_vec(),
            }),
            Cancel::new(),
            16,
            wait,
        );
        let mut into = [0_u8; 16];

        let read = reader.read(&mut into);

        assert_eq!(
            read.ok(),
            Some(5),
            "a late chunk was reported as a deadline instead of kept"
        );
        assert_eq!(into.get(..5), Some(&b"final"[..]));
    }

    #[test]
    fn a_clean_end_a_read_hands_over_after_the_wait_runs_out_is_not_a_deadline() {
        // A body that closes cleanly exactly as the wait runs out must be
        // honoured as `Ok(0)`, not replaced by the deadline error.
        let wait = Duration::from_millis(5);
        let mut reader = Limited::new(
            Box::new(Late {
                wait,
                then: Vec::new(),
            }),
            Cancel::new(),
            16,
            wait,
        );

        let read = reader.read(&mut [0; 16]);

        assert_eq!(
            read.ok(),
            Some(0),
            "a late clean end was reported as a deadline instead of Ok(0)"
        );
    }

    #[test]
    fn a_late_probe_confirming_the_byte_cap_was_not_exceeded_is_not_a_deadline() {
        // Once `remaining` is spent, a one-byte probe that closes cleanly
        // late must still say the cap was not exceeded.
        let wait = Duration::from_millis(5);
        let mut reader = Limited::new(
            Box::new(Late {
                wait,
                then: Vec::new(),
            }),
            Cancel::new(),
            0,
            wait,
        );

        let read = reader.read(&mut [0]);

        assert_eq!(
            read.ok(),
            Some(0),
            "a late probe end was reported as a deadline instead of Ok(0)"
        );
    }

    /// A peer that never closes and always has one more byte to give.
    struct Trickle;

    impl Read for Trickle {
        fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(2));
            if let Some(first) = into.first_mut() {
                *first = b'x';
                return Ok(1);
            }
            Ok(0)
        }
    }

    #[test]
    fn a_trickle_that_never_ends_still_meets_the_deadline() {
        // The wait still bounds a stall or a trickle: a read that keeps
        // handing over bytes without ever closing must not outlive the wait.
        let mut reader = Limited::new(
            Box::new(Trickle),
            Cancel::new(),
            usize::MAX,
            Duration::from_millis(20),
        );
        let mut into = [0_u8; 1];

        let mut hit_deadline = false;
        for _ in 0..100 {
            match reader.read(&mut into) {
                Ok(read) => assert_eq!(read, 1, "the trickle must not end on its own"),
                Err(error) => {
                    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                    hit_deadline = true;
                    break;
                }
            }
        }
        assert!(
            hit_deadline,
            "a trickle that never ends must eventually meet the deadline"
        );
    }

    /// A body whose one read sleeps past `wait` before failing with a real
    /// I/O error, mirroring `web/tests.rs`'s `SlowFailure`.
    struct SlowFailure {
        wait: Duration,
    }

    impl Read for SlowFailure {
        fn read(&mut self, _into: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(self.wait.saturating_add(Duration::from_millis(20)));
            Err(io::Error::other("upstream reset the connection"))
        }
    }

    #[test]
    fn a_real_read_failure_landing_as_the_wait_runs_out_keeps_its_own_message() {
        // `checked()` looks only at cancel, so a genuine error from this read
        // must come back as itself, not the deadline error a post-read wait
        // check would have replaced it with.
        let wait = Duration::from_millis(5);
        let mut reader = Limited::new(Box::new(SlowFailure { wait }), Cancel::new(), 16, wait);

        let error = reader
            .read(&mut [0; 16])
            .expect_err("a real read failure to still be reported");

        assert_ne!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(
            error.to_string().contains("upstream reset the connection"),
            "{error}"
        );
    }

    #[test]
    fn a_real_probe_read_failure_landing_as_the_wait_runs_out_keeps_its_own_message() {
        // The same proof against the `remaining == 0` probe path, which also
        // calls `checked()` after its own read of the body.
        let wait = Duration::from_millis(5);
        let mut reader = Limited::new(Box::new(SlowFailure { wait }), Cancel::new(), 0, wait);

        let error = reader
            .read(&mut [0])
            .expect_err("a real probe read failure to still be reported");

        assert_ne!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(
            error.to_string().contains("upstream reset the connection"),
            "{error}"
        );
    }

    #[test]
    fn cancelling_during_a_read_that_still_returns_bytes_wins_over_what_it_returned() {
        // The cancel check after the read stays: a cancel discovered the
        // instant a read returns must still discard what it handed over,
        // even though that read succeeded.
        struct CancelsDuringRead(Cancel);
        impl Read for CancelsDuringRead {
            fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
                self.0.request();
                if let Some(first) = into.first_mut() {
                    *first = b'x';
                }
                Ok(1)
            }
        }
        let cancel = Cancel::new();
        let mut reader = Limited::new(
            Box::new(CancelsDuringRead(cancel.clone())),
            cancel,
            16,
            Duration::from_secs(1),
        );

        let read = reader.read(&mut [0]);

        assert!(
            read.is_err(),
            "a cancel discovered right after a read must still win: {read:?}"
        );
    }
}
