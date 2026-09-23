//! Bounded byte-preserving masking of the credentials a command was given.
//!
//! A stream is masked against a list of exact byte sequences: where the
//! command has a proxy, that proxy's password and the base64 form of its
//! userinfo, and, except on the standard output of a command crucible speaks
//! a protocol to, the value of every credential the command's environment
//! carries. Wherever one appears, overlapping or not, it reads as one `*` per
//! byte, so no byte count changes and nothing here parses the stream. A value
//! the command re-encodes, splits or transforms before printing it is not
//! matched. An empty sequence masks nothing.
//!
//! Output that could still grow into one of them is held back, and released
//! once it cannot: as it was printed, except that a byte inside a sequence it
//! completed reads `*`. What is held is shorter than the longest sequence, and
//! every sequence is bounded by the proxy's fixed credential or the
//! environment's byte ceiling. When the command ends its own output, what is
//! still held back is released the same way: as printed, but for any sequence
//! it completed. When crucible cut the output instead, by discarding past the
//! output limit or by stopping the command, the rest can never arrive, so what
//! is held back is masked whole. It could be the start of a credential or only
//! look like it: a proxy's base64 form always begins `Y3J1Y2libGU6`, so a
//! final `Y` of cut output shows as `*`.
//!
//! Each sequence is matched as a Knuth–Morris–Pratt automaton, so one byte
//! costs amortized constant work per sequence however long the sequence is,
//! and each byte is masked at most once per sequence however many of its
//! matches cover it. A credential can be as long as the environment's byte
//! ceiling, and a matcher that compared every held byte again on every new
//! one would spend that ceiling on each byte a command prints.
//!
//! Some ends crucible imposes are not marked, and each reads as the command's
//! own:
//!
//! - Its clean-up of what the command left running: the kill of the
//!   command's group once it exits, or on Linux the broker's sweep of the
//!   namespace once the workload exits. Both run after every exit, whether or
//!   not anything is left, and neither says whether it killed anything, so
//!   marking them would mask output that ended on its own.
//! - A CPU limit, when one is set: the kernel kills the command, and nothing
//!   records that as a limit it broke.
//! - A Linux broker that fails after the command started and exits: the
//!   kernel kills what is left of the command with it, and the readers see
//!   only the end.
//!
//! A start is released there only when a process killed this way wrote the
//! credential across more than one write.

use std::collections::VecDeque;
use std::io;

use crucible_sandbox::{SandboxOutput, SandboxRead};

/// Whether crucible cut short the command whose output this is, by its output
/// or command-time limit or by stopping it, rather than the command ending on
/// its own.
pub(crate) type Interrupted = Box<dyn Fn() -> bool + Send>;

/// One non-empty sequence, and how much of it the output so far ends with.
struct Sequence {
    bytes: Vec<u8>,
    /// For each length `i` of a prefix, the length of the longest proper
    /// prefix of that prefix that is also a suffix of it: where matching
    /// resumes when the next byte does not continue a partial match.
    fallback: Vec<usize>,
    /// The length of the longest suffix of the output so far that is a proper
    /// prefix of `bytes`, and so could still be completed.
    matched: usize,
    /// Where the last completed match ended: every byte of an earlier match
    /// is already `*`, so a match overlapping it masks only what follows.
    masked_to: usize,
}

impl Sequence {
    fn new(bytes: Vec<u8>) -> Self {
        let mut fallback = vec![0; bytes.len().saturating_add(1)];
        let mut border = 0;
        for (at, byte) in bytes.iter().enumerate().skip(1) {
            while border > 0 && bytes.get(border) != Some(byte) {
                border = fallback.get(border).copied().unwrap_or(0);
            }
            if bytes.get(border) == Some(byte) {
                border += 1;
            }
            if let Some(slot) = fallback.get_mut(at + 1) {
                *slot = border;
            }
        }
        Self {
            bytes,
            fallback,
            matched: 0,
            masked_to: 0,
        }
    }

    /// Takes one more byte of output, and says whether it completed the
    /// sequence.
    fn advance(&mut self, byte: u8) -> bool {
        while self.matched > 0 && self.bytes.get(self.matched) != Some(&byte) {
            self.matched = self.fallback.get(self.matched).copied().unwrap_or(0);
        }
        if self.bytes.get(self.matched) == Some(&byte) {
            self.matched += 1;
        }
        if self.matched < self.bytes.len() {
            return false;
        }
        // Kept below the whole length, so what is held back stays shorter
        // than the sequence and a match that overlaps this one still counts.
        self.matched = self.fallback.get(self.matched).copied().unwrap_or(0);
        true
    }
}

pub(crate) struct ProtectedOutput {
    inner: Box<dyn SandboxOutput>,
    /// The non-empty sequences masked wherever they appear.
    sequences: Vec<Sequence>,
    /// Output that could still be the start of a sequence, as it will be
    /// released: as printed, except that bytes inside a sequence already
    /// completed read `*`.
    shown: VecDeque<u8>,
    /// How many bytes of output have been taken in, so the end of `shown`.
    taken: usize,
    ready: VecDeque<u8>,
    discarded: usize,
    ended: bool,
    /// Asked when the stream ends. Without it, every end is the command's own.
    interrupted: Option<Interrupted>,
}

impl ProtectedOutput {
    pub(crate) fn new(inner: Box<dyn SandboxOutput>, patterns: Vec<Vec<u8>>) -> Self {
        let longest = patterns.iter().map(Vec::len).max().unwrap_or(0);
        Self {
            inner,
            sequences: patterns
                .into_iter()
                .filter(|pattern| !pattern.is_empty())
                .map(Sequence::new)
                .collect(),
            shown: VecDeque::with_capacity(longest),
            taken: 0,
            ready: VecDeque::with_capacity(4096 + longest),
            discarded: 0,
            ended: false,
            interrupted: None,
        }
    }

    pub(crate) fn interrupted_by(mut self, interrupted: Interrupted) -> Self {
        self.interrupted = Some(interrupted);
        self
    }
}

impl ProtectedOutput {
    fn accept(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.shown.push_back(*byte);
            self.taken = self.taken.saturating_add(1);
            let mut kept = 0;
            for sequence in &mut self.sequences {
                if sequence.advance(*byte) {
                    // Its bytes are all still held: none of them was released
                    // while it could complete the sequence. Only what no
                    // earlier match of the same sequence masked is masked.
                    let start = self.taken.saturating_sub(sequence.bytes.len());
                    let first = start.max(sequence.masked_to);
                    let held_from = self.taken.saturating_sub(self.shown.len());
                    for at in first..self.taken {
                        if let Some(shown) = self.shown.get_mut(at.saturating_sub(held_from)) {
                            *shown = b'*';
                        }
                    }
                    sequence.masked_to = self.taken;
                }
                kept = kept.max(sequence.matched);
            }
            // Release up to the first byte a sequence could still complete from.
            while self.shown.len() > kept {
                let Some(released) = self.shown.pop_front() else {
                    break;
                };
                self.ready.push_back(released);
            }
        }
    }

    /// Forgets every partial match, once what it was held for is gone.
    fn forget(&mut self) {
        self.shown.clear();
        for sequence in &mut self.sequences {
            sequence.matched = 0;
        }
    }

    /// Masks what is held back, which can no longer complete.
    fn mask_held(&mut self) {
        self.ready
            .extend(std::iter::repeat_n(b'*', self.shown.len()));
        self.forget();
    }

    /// Releases what is held back as it was printed, but for any sequence it
    /// already completed.
    fn release_held(&mut self) {
        self.ready.extend(self.shown.drain(..));
        self.forget();
    }

    fn drain(&mut self, bytes: &mut [u8]) -> SandboxRead {
        let mut copied = 0;
        for destination in bytes {
            let Some(byte) = self.ready.pop_front() else {
                break;
            };
            *destination = byte;
            copied += 1;
        }
        if self.discarded > 0 {
            SandboxRead::Limited {
                retained: copied,
                discarded: std::mem::take(&mut self.discarded),
            }
        } else if copied > 0 {
            SandboxRead::Bytes(copied)
        } else if self.ended {
            SandboxRead::End
        } else {
            SandboxRead::Pending
        }
    }
}

impl SandboxOutput for ProtectedOutput {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if buffer.is_empty() {
            return Ok(SandboxRead::Pending);
        }
        if !self.ready.is_empty() || self.ended || self.discarded > 0 {
            return Ok(self.drain(buffer));
        }
        // At most one bounded source read per call. A nonblocking Pending
        // retains only a possible secret prefix; ordinary output never waits
        // for a fixed lookahead window or changes its byte encoding/length.
        let mut incoming = [0; 4096];
        let count = match self.inner.read_ready(&mut incoming)? {
            SandboxRead::Bytes(count) => count,
            SandboxRead::Limited {
                retained,
                discarded,
            } => {
                self.discarded = discarded;
                retained
            }
            SandboxRead::Pending => return Ok(SandboxRead::Pending),
            SandboxRead::End => {
                self.ended = true;
                let cut = self.interrupted.as_ref().is_some_and(|cut| cut());
                if cut {
                    self.mask_held();
                } else {
                    self.release_held();
                }
                return Ok(self.drain(buffer));
            }
        };
        let bytes = incoming
            .get(..count)
            .ok_or_else(|| io::Error::other("sandbox output exceeded its buffer"))?;
        self.accept(bytes);
        if self.discarded > 0 {
            self.mask_held();
        }
        Ok(self.drain(buffer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use base64::Engine as _;

    const USERINFO: &str =
        "crucible:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// What a command whose proxy has `USERINFO` has masked.
    fn proxy() -> Vec<Vec<u8>> {
        crate::network::credential_forms(USERINFO)
    }

    struct Source {
        chunks: VecDeque<Vec<u8>>,
        discarded: usize,
    }
    impl SandboxOutput for Source {
        fn read_ready(&mut self, bytes: &mut [u8]) -> io::Result<SandboxRead> {
            let Some(chunk) = self.chunks.pop_front() else {
                return Ok(SandboxRead::End);
            };
            if chunk.is_empty() {
                return Ok(SandboxRead::Pending);
            }
            let count = bytes.len().min(chunk.len());
            bytes
                .get_mut(..count)
                .expect("fixture buffer")
                .copy_from_slice(chunk.get(..count).expect("fixture count"));
            if count < chunk.len() {
                self.chunks
                    .push_front(chunk.get(count..).expect("remainder").to_vec());
            }
            if self.chunks.is_empty() && self.discarded > 0 {
                Ok(SandboxRead::Limited {
                    retained: count,
                    discarded: std::mem::take(&mut self.discarded),
                })
            } else {
                Ok(SandboxRead::Bytes(count))
            }
        }
    }

    fn collect(chunks: VecDeque<Vec<u8>>, width: usize, discarded: usize) -> (Vec<u8>, usize) {
        let mut output = ProtectedOutput::new(Box::new(Source { chunks, discarded }), proxy());
        assert_eq!(
            output.read_ready(&mut []).expect("empty"),
            SandboxRead::Pending
        );
        read_all(&mut output, width)
    }

    /// Reads `output` to its end through a buffer `width` bytes wide.
    fn read_all(output: &mut ProtectedOutput, width: usize) -> (Vec<u8>, usize) {
        let mut retained = Vec::new();
        let mut lost = 0;
        let mut bytes = vec![0; width];
        for _ in 0..4096 {
            let count = match output.read_ready(&mut bytes).expect("read") {
                SandboxRead::Bytes(count) => count,
                SandboxRead::Limited {
                    retained,
                    discarded,
                } => {
                    lost += discarded;
                    retained
                }
                SandboxRead::Pending => continue,
                SandboxRead::End => return (retained, lost),
            };
            retained.extend_from_slice(bytes.get(..count).expect("bounded"));
        }
        panic!("bounded fixture did not terminate");
    }

    #[test]
    fn exact_echoes_are_masked_across_every_input_split_and_one_byte_reads() {
        let password = USERINFO.split_once(':').expect("fixture").1;
        let encoded = base64::engine::general_purpose::STANDARD.encode(USERINFO);
        for value in [password.as_bytes(), encoded.as_bytes()] {
            let mut input = b"\xffprefix=".to_vec();
            input.extend_from_slice(value);
            input.extend_from_slice(b"\x00end");
            let mut expected = b"\xffprefix=".to_vec();
            expected.extend(std::iter::repeat_n(b'*', value.len()));
            expected.extend_from_slice(b"\x00end");
            for split in 0..=input.len() {
                let chunks = [
                    input.get(..split).expect("left").to_vec(),
                    Vec::new(),
                    input.get(split..).expect("right").to_vec(),
                ]
                .into();
                assert_eq!(collect(chunks, 1, 0), (expected.clone(), 0));
            }
        }
    }

    #[test]
    fn nonsecret_bytes_are_available_without_waiting_for_a_lookahead_window() {
        let mut output = ProtectedOutput::new(
            Box::new(Source {
                chunks: [b"ready\n".to_vec(), Vec::new()].into(),
                discarded: 0,
            }),
            proxy(),
        );
        let mut bytes = [0; 128];
        assert_eq!(
            output.read_ready(&mut bytes).expect("ready"),
            SandboxRead::Bytes(6)
        );
        assert_eq!(&bytes[..6], b"ready\n");
    }

    #[test]
    fn masking_preserves_frame_lengths_and_limit_accounting() {
        let body = format!("{{\"credential\":\"{USERINFO}\"}}");
        let message = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let (output, discarded) = collect([message.as_bytes().to_vec()].into(), 3, 7);
        assert_eq!(output.len(), message.len());
        assert_eq!(discarded, 7);
        assert!(!String::from_utf8_lossy(&output).contains(USERINFO));
    }

    #[test]
    fn an_unmatched_prefix_at_eof_is_preserved() {
        assert_eq!(
            collect([b"01234".to_vec()].into(), 2, 0),
            (b"01234".to_vec(), 0)
        );
    }

    /// "id=" and the first 20 bytes of `value`, with the output limit
    /// discarding the 7 bytes that followed them.
    fn cut_short(value: &str) -> (Vec<u8>, usize) {
        let mut input = b"id=".to_vec();
        input.extend_from_slice(value.as_bytes().get(..20).expect("fixture length"));
        collect([input].into(), 1, 7)
    }

    #[test]
    fn a_password_the_output_limit_cut_short_is_masked() {
        let password = USERINFO.split_once(':').expect("fixture").1;
        assert_eq!(
            cut_short(password),
            ([b"id=".as_slice(), &[b'*'; 20]].concat(), 7)
        );
    }

    #[test]
    fn an_encoded_credential_the_output_limit_cut_short_is_masked() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(USERINFO);
        assert_eq!(
            cut_short(&encoded),
            ([b"id=".as_slice(), &[b'*'; 20]].concat(), 7)
        );
    }

    /// What a source reads next: bytes it keeps, or a discard on a read of its
    /// own, as when the budget ran out at a read boundary or on the other stream.
    enum Step {
        Bytes(Vec<u8>),
        Discard(usize),
    }

    struct Steps(VecDeque<Step>);

    impl SandboxOutput for Steps {
        fn read_ready(&mut self, bytes: &mut [u8]) -> io::Result<SandboxRead> {
            Ok(match self.0.pop_front() {
                None => SandboxRead::End,
                Some(Step::Bytes(chunk)) => {
                    bytes
                        .get_mut(..chunk.len())
                        .expect("fixture buffer")
                        .copy_from_slice(&chunk);
                    SandboxRead::Bytes(chunk.len())
                }
                Some(Step::Discard(discarded)) => SandboxRead::Limited {
                    retained: 0,
                    discarded,
                },
            })
        }
    }

    #[test]
    fn a_discard_on_a_read_of_its_own_masks_what_an_earlier_read_held_back() {
        let password = USERINFO.split_once(':').expect("fixture").1;
        let mut input = b"id=".to_vec();
        input.extend_from_slice(password.as_bytes().get(..20).expect("fixture length"));
        let mut output = ProtectedOutput::new(
            Box::new(Steps([Step::Bytes(input), Step::Discard(7)].into())),
            proxy(),
        );
        assert_eq!(
            read_all(&mut output, 1),
            ([b"id=".as_slice(), &[b'*'; 20]].concat(), 7)
        );
    }

    /// Everything `patterns` leaves of `chunks`, read one byte at a time to an
    /// end the command made itself.
    fn masked_by(patterns: &[&str], chunks: VecDeque<Vec<u8>>) -> Vec<u8> {
        let patterns = patterns.iter().map(|value| value.as_bytes().to_vec());
        let mut output = ProtectedOutput::new(
            Box::new(Source {
                chunks,
                discarded: 0,
            }),
            patterns.collect(),
        );
        let (kept, lost) = read_all(&mut output, 1);
        assert_eq!(lost, 0);
        kept
    }

    #[test]
    fn every_credential_value_is_masked_across_every_input_split_without_a_proxy() {
        let first = "first-credential-value";
        let second = "0123-another-one";
        let input = format!("\u{7f}a={first} b={second}\n{second}{first}");
        let expected = format!(
            "\u{7f}a={} b={}\n{}{}",
            "*".repeat(first.len()),
            "*".repeat(second.len()),
            "*".repeat(second.len()),
            "*".repeat(first.len()),
        );
        for split in 0..=input.len() {
            let chunks = [
                input.as_bytes().get(..split).expect("left").to_vec(),
                Vec::new(),
                input.as_bytes().get(split..).expect("right").to_vec(),
            ]
            .into();
            let kept = masked_by(&[first, second], chunks);
            let shown = String::from_utf8_lossy(&kept);
            assert_eq!(kept.len(), input.len(), "split at {split}: {shown}");
            assert!(!shown.contains(first) && !shown.contains(second), "{shown}");
            assert_eq!(shown, expected, "split at {split}");
        }
    }

    #[test]
    fn an_empty_value_masks_nothing() {
        let input = b"id= and nothing else\n".to_vec();
        assert_eq!(masked_by(&[""], [input.clone()].into()), input);
        assert_eq!(
            masked_by(&["", "nothing"], [input].into()),
            b"id= and ******* else\n"
        );
    }

    /// What one 4 KiB read costs against a value `length` bytes long, once the
    /// stream holds back as much of the value as it can: the best of five.
    ///
    /// The value is `a` repeated and a final `b`, and the stream is nothing
    /// but `a`, so every byte read could still be the start of it and the
    /// most is held back that ever can be.
    fn one_read_against(length: usize) -> std::time::Duration {
        let mut value = vec![b'a'; length.saturating_sub(1)];
        value.push(b'b');
        let priming = length.div_ceil(4096) + 1;
        let mut best = std::time::Duration::MAX;
        for _ in 0..5 {
            let chunks = std::iter::repeat_n(vec![b'a'; 4096], priming + 1).collect();
            let mut output = ProtectedOutput::new(
                Box::new(Source {
                    chunks,
                    discarded: 0,
                }),
                vec![value.clone()],
            );
            let mut bytes = vec![0; 4096];
            for _ in 0..priming {
                output.read_ready(&mut bytes).expect("priming read");
            }
            let started = std::time::Instant::now();
            let read = output.read_ready(&mut bytes).expect("measured read");
            best = best.min(started.elapsed());
            assert_eq!(
                read,
                SandboxRead::Bytes(4096),
                "the measured read released one read"
            );
        }
        best
    }

    /// A value the environment allows can be 128 KiB, and a read is 4 KiB: a
    /// matcher that pays for the value's length on every byte turns one read
    /// into half a gigabyte of comparisons. The cost of a read has to follow
    /// the read, not the value it is matched against.
    #[test]
    fn one_read_costs_the_same_against_a_long_value_as_against_a_short_one() {
        let short = one_read_against(4 * 1024);
        let long = one_read_against(128 * 1024);
        eprintln!(
            "one 4 KiB read: {short:?} against a 4 KiB value, {long:?} against a 128 KiB one"
        );
        assert!(
            long <= short * 4 + std::time::Duration::from_millis(5),
            "one 4 KiB read against a 128 KiB value took {long:?}, against a 4 KiB one {short:?}: \
             the matcher's cost grows with the value rather than the read"
        );
    }

    /// Every overlapping occurrence of every value, found by comparing it at
    /// every position: what the automaton masks has to be exactly this, for
    /// values that repeat themselves and inputs that nearly match them.
    #[test]
    fn the_automaton_masks_exactly_what_comparing_at_every_position_finds() {
        let sets: [&[&str]; 4] = [&["aab", "aba"], &["abab"], &["aa", "ab"], &["abaab", "b"]];
        for patterns in sets {
            for length in 0..=9u32 {
                for bits in 0..(1u32 << length) {
                    let input: Vec<u8> = (0..length)
                        .map(|at| if bits >> at & 1 == 1 { b'b' } else { b'a' })
                        .collect();
                    let mut expected = input.clone();
                    for pattern in patterns {
                        let pattern = pattern.as_bytes();
                        for start in 0..input.len() {
                            if input
                                .get(start..)
                                .is_some_and(|rest| rest.starts_with(pattern))
                            {
                                expected
                                    .get_mut(start..start + pattern.len())
                                    .expect("a match inside the input")
                                    .fill(b'*');
                            }
                        }
                    }
                    assert_eq!(
                        masked_by(patterns, [input.clone()].into()),
                        expected,
                        "an occurrence of {patterns:?} in {:?} was masked differently",
                        String::from_utf8_lossy(&input)
                    );
                }
            }
        }
    }

    /// A value can sit inside the start of another: it is masked whether the
    /// longer one then completes, breaks off, or is still unfinished when the
    /// command ends its output.
    #[test]
    fn a_value_inside_the_start_of_another_is_masked_however_the_longer_one_ends() {
        let longer = "token-abc123-xyz";
        let inner = "abc123";
        for (input, expected) in [
            ("token-abc123-xyz!", "****************!"),
            ("token-abc123!", "token-******!"),
            ("token-abc123", "token-******"),
        ] {
            assert_eq!(
                masked_by(&[longer, inner], [input.as_bytes().to_vec()].into()),
                expected.as_bytes(),
                "{input}"
            );
        }
    }
}
