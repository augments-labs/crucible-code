//! Stream exactly one framed request body, leaving any following request private.

use super::request::Body;
use std::io::{self, BufRead, Read, Write};

pub(super) fn forward(
    source: &mut impl BufRead,
    target: &mut impl Write,
    body: Body,
) -> io::Result<()> {
    match body {
        Body::Fixed(length) => fixed(source, target, length),
        Body::Chunked => chunked(source, target),
    }
}

fn fixed(source: &mut impl BufRead, target: &mut impl Write, length: u64) -> io::Result<()> {
    if io::copy(&mut source.take(length), target)? == length {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete sandbox proxy request body",
        ))
    }
}

fn chunked(source: &mut impl BufRead, target: &mut impl Write) -> io::Result<()> {
    loop {
        let line = line(source, super::request::MAX_HEADER_BYTES)?;
        let (used, length) = match httparse::parse_chunk_size(&line).map_err(|_| invalid())? {
            httparse::Status::Complete(value) => value,
            httparse::Status::Partial => return Err(invalid()),
        };
        if used != line.len() {
            return Err(invalid());
        }
        if length == 0 {
            trailers(source)?;
            target.write_all(b"0\r\n\r\n")?;
            return Ok(());
        }
        // Normalize the size and drop extensions. No attacker-supplied framing
        // or trailer field is forwarded to the authorized endpoint.
        write!(target, "{length:x}\r\n")?;
        fixed(source, target, length)?;
        let mut end = [0_u8; 2];
        source.read_exact(&mut end)?;
        if end != *b"\r\n" {
            return Err(invalid());
        }
        target.write_all(b"\r\n")?;
    }
}

fn trailers(source: &mut impl BufRead) -> io::Result<()> {
    let mut bytes = Vec::new();
    loop {
        let remaining = super::request::MAX_HEADER_BYTES.saturating_sub(bytes.len());
        let line = line(source, remaining)?;
        let last = line == b"\r\n";
        bytes.extend_from_slice(&line);
        if last {
            let mut slots = [httparse::EMPTY_HEADER; 64];
            match httparse::parse_headers(&bytes, &mut slots).map_err(|_| invalid())? {
                httparse::Status::Complete((used, _)) if used == bytes.len() => return Ok(()),
                _ => return Err(invalid()),
            }
        }
    }
}

fn line(source: &mut impl BufRead, maximum: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source
        .take(maximum as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > maximum || !bytes.ends_with(b"\r\n") {
        return Err(invalid());
    }
    Ok(bytes)
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or oversized sandbox proxy body framing",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bytes_after_the_single_framed_body_never_reach_the_authorized_endpoint() {
        let mut source = Cursor::new(b"helloGET http://denied.example.com/ HTTP/1.1\r\n\r\n");
        let mut target = Vec::new();
        forward(&mut source, &mut target, Body::Fixed(5)).unwrap();
        assert_eq!(target, b"hello");
        assert_eq!(source.position(), 5);
    }

    #[test]
    fn chunked_body_cannot_forward_a_second_request_or_unchecked_trailers() {
        let mut source = Cursor::new(b"5;tag=value\r\nhello\r\n0\r\nX-Finish: yes\r\n\r\nGET http://denied.example.com/ HTTP/1.1\r\n\r\n");
        let mut target = Vec::new();
        forward(&mut source, &mut target, Body::Chunked).unwrap();
        assert_eq!(target, b"5\r\nhello\r\n0\r\n\r\n");
        assert!(
            source
                .get_ref()
                .get(usize::try_from(source.position()).unwrap()..)
                .unwrap()
                .starts_with(b"GET ")
        );
        for bad in [
            b"5\r\nshort!\r\n0\r\n\r\n".as_slice(),
            b"x\r\n",
            b"0\r\nmalformed\r\n\r\n",
        ] {
            assert!(forward(&mut Cursor::new(bad), &mut Vec::new(), Body::Chunked).is_err());
        }
        let oversized = format!("1;{}\r\nx\r\n0\r\n\r\n", "x".repeat(16 * 1024));
        assert!(forward(&mut Cursor::new(oversized), &mut Vec::new(), Body::Chunked).is_err());
    }

    #[test]
    fn incomplete_bodies_are_errors() {
        assert!(forward(&mut Cursor::new(b"short"), &mut Vec::new(), Body::Fixed(9)).is_err());
    }
}
