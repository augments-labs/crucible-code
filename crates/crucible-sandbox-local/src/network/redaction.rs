//! Bounded byte-preserving masking of echoed per-command proxy credentials.
//!
//! Output that could still grow into the password or its base64 form is held
//! back: masked one `*` per byte if it completes one, released as it is once
//! it cannot. When the command ends its own output, what is still held back is
//! released as it is. When crucible cut the output instead, by discarding past
//! the output limit or by stopping the command, the rest can never arrive, so
//! what is held back is masked. It could be the start of the credential or
//! only look like it: the base64 form always begins `Y3J1Y2libGU6`, so a final
//! `Y` of cut output shows as `*`.
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

use base64::Engine as _;
use crucible_sandbox::{SandboxOutput, SandboxRead};

/// Whether crucible cut short the command whose output this is, by its output
/// or command-time limit or by stopping it, rather than the command ending on
/// its own.
pub(super) type Interrupted = Box<dyn Fn() -> bool + Send>;

pub(super) struct ProtectedOutput {
    inner: Box<dyn SandboxOutput>,
    patterns: [Vec<u8>; 2],
    prefix: Vec<u8>,
    ready: VecDeque<u8>,
    discarded: usize,
    ended: bool,
    /// Asked when the stream ends. Without it, every end is the command's own.
    interrupted: Option<Interrupted>,
}

impl ProtectedOutput {
    pub(super) fn new(inner: Box<dyn SandboxOutput>, userinfo: &str) -> Self {
        Self {
            inner,
            prefix: Vec::with_capacity(100),
            ready: VecDeque::with_capacity(4096 + 100),
            discarded: 0,
            ended: false,
            interrupted: None,
            patterns: [
                userinfo
                    .split_once(':')
                    .map_or("", |(_, password)| password)
                    .as_bytes()
                    .to_vec(),
                base64::engine::general_purpose::STANDARD
                    .encode(userinfo)
                    .into_bytes(),
            ],
        }
    }

    pub(super) fn interrupted_by(mut self, interrupted: Interrupted) -> Self {
        self.interrupted = Some(interrupted);
        self
    }
}

impl ProtectedOutput {
    fn accept(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.prefix.push(*byte);
            if self.patterns.contains(&self.prefix) {
                self.ready
                    .extend(std::iter::repeat_n(b'*', self.prefix.len()));
                self.prefix.clear();
            } else {
                while !self.prefix.is_empty()
                    && !self
                        .patterns
                        .iter()
                        .any(|pattern| pattern.starts_with(&self.prefix))
                {
                    self.ready.push_back(self.prefix.remove(0));
                }
            }
        }
    }

    /// Masks what is held back, which can no longer complete.
    fn mask_held(&mut self) {
        self.ready
            .extend(std::iter::repeat_n(b'*', self.prefix.len()));
        self.prefix.clear();
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
                    self.ready.extend(self.prefix.drain(..));
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

    const USERINFO: &str =
        "crucible:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
        let mut output = ProtectedOutput::new(Box::new(Source { chunks, discarded }), USERINFO);
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
            USERINFO,
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
            USERINFO,
        );
        assert_eq!(
            read_all(&mut output, 1),
            ([b"id=".as_slice(), &[b'*'; 20]].concat(), 7)
        );
    }
}
