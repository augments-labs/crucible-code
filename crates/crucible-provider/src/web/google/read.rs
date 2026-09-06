//! Whole-side-response ceilings below SSE framing.
//!
//! Event limits cannot bound a stream of tiny events or comments. The guard is
//! inside the stream's read loop so quiet/trickling responses still meet the
//! deadline, and a byte past the ceiling is a failure rather than a false EOF.

use std::io::{self, Read};
use std::time::{Duration, Instant};

use crucible_core::Cancel;

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

    fn check(&self) -> io::Result<()> {
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
}

impl Read for Limited {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        if into.is_empty() {
            return Ok(0);
        }
        self.check()?;
        if self.remaining == 0 {
            let read = self.body.read(&mut [0]);
            self.check()?;
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
        self.check()?;
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
}
