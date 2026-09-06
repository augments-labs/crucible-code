//! Bounded byte-preserving masking of echoed per-command proxy credentials.

use std::collections::VecDeque;
use std::io;

use base64::Engine as _;
use crucible_core::{SandboxOutput, SandboxRead};

pub(super) struct ProtectedOutput {
    inner: Box<dyn SandboxOutput>,
    patterns: [Vec<u8>; 2],
    prefix: Vec<u8>,
    ready: VecDeque<u8>,
    discarded: usize,
    ended: bool,
}

impl ProtectedOutput {
    pub(super) fn new(inner: Box<dyn SandboxOutput>, userinfo: &str) -> Self {
        Self {
            inner,
            prefix: Vec::with_capacity(100),
            ready: VecDeque::with_capacity(4096 + 100),
            discarded: 0,
            ended: false,
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
                self.ready.extend(self.prefix.drain(..));
                return Ok(self.drain(buffer));
            }
        };
        let bytes = incoming
            .get(..count)
            .ok_or_else(|| io::Error::other("sandbox output exceeded its buffer"))?;
        self.accept(bytes);
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
}
