//! Fixed protocol shared by the host launcher and the minimal PID 1 broker.

use std::io::{self, Read, Write};

/// Internal macOS launcher mode. It closes inherited descriptors and applies
/// hard limits before entering the system Seatbelt executable.
pub const MACOS_LAUNCH_MODE: &str = "--macos-seatbelt-launch";

/// Maximum UTF-8 bytes in one generated Seatbelt profile.
pub const MACOS_MAX_PROFILE_BYTES: usize = 256 * 1024;

/// Maximum path definitions passed to one Seatbelt invocation.
pub const MACOS_MAX_DEFINITIONS: usize = 256;

/// Maximum aggregate encoded bytes in the macOS launcher argument vector.
///
/// macOS counts the environment against its native exec limit too. Keeping the
/// protocol below 512 KiB leaves room for Crucible's separately bounded 128
/// KiB environment and the kernel's argument-pointer bookkeeping.
pub const MACOS_MAX_LAUNCH_ARGUMENT_BYTES: usize = 512 * 1024;

/// Bytes in one native-endian Linux wait status.
pub const WAIT_STATUS_BYTES: usize = size_of::<i32>();

/// Status sent when the broker could not create or reap the workload.
pub const BROKER_FAILURE_STATUS: i32 = i32::MIN;

/// Broker-to-host attestation that parsing and descriptor closure completed.
pub const READY_FRAME: [u8; 8] = *b"CRREADY1";

/// The sole host-to-broker authority to create the workload.
pub const GO_FRAME: [u8; 8] = *b"CRGO0001";

/// Host-to-broker request to kill and reap the released workload scope.
pub const CANCEL_FRAME: [u8; 8] = *b"CRCANCEL";

/// Broker-to-host typed refusal before the one-shot release boundary.
pub const REFUSED_FRAME: [u8; 8] = *b"CRREFU01";

/// Preparation refusal category: the complete merged-view scan failed.
pub const REFUSED_SCAN: u8 = 1;

/// Preparation refusal category: undeclared descriptor closure failed.
pub const REFUSED_DESCRIPTOR_CLOSURE: u8 = 2;

/// Begins one complete terminal semantic scan after the raw wait status.
pub const SCAN_FRAME: [u8; 8] = *b"CRSCAN01";

/// Proves that the complete bounded terminal scan was transmitted.
pub const SCAN_END_FRAME: [u8; 8] = *b"CREND001";

/// Opens one length-prefixed native Windows launch request.
pub const WINDOWS_LAUNCH_FRAME: [u8; 8] = *b"CRWIN001";

/// Maximum encoded Windows launch request, excluding its fixed frame header.
///
/// The request travels through the broker's standard input instead of the
/// Windows command line. This bound keeps a hostile SDK caller from turning
/// the trusted broker into an unbounded allocator before confinement begins.
pub const MAX_WINDOWS_LAUNCH_BYTES: usize = 512 * 1024;

/// Maximum opaque arguments carried by one Windows launch request.
pub const MAX_WINDOWS_ARGUMENTS: usize = 512;

/// Maximum environment entries carried by one Windows launch request.
pub const MAX_WINDOWS_ENVIRONMENT: usize = 128;

/// Maximum roots in each Windows filesystem access class.
pub const MAX_WINDOWS_ROOTS: usize = 128;

/// Maximum UTF-16 code units in one Windows path, argument, name, or value.
pub const MAX_WINDOWS_FIELD_UNITS: usize = 32_767;

/// One bounded command and filesystem plan handed to the trusted Windows
/// broker before it creates a restricted-account workload.
///
/// This type validates only the wire representation. The broker must load and
/// validate its installed setup record, then resolve path authority, the
/// environment, and the final Windows command line before it creates a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsLaunchRequest {
    working_directory: Vec<u16>,
    program: Vec<u16>,
    arguments: Vec<Vec<u16>>,
    environment: Vec<(Vec<u16>, Vec<u16>)>,
    writable_roots: Vec<Vec<u16>>,
    protected_roots: Vec<Vec<u16>>,
    unreadable_roots: Vec<Vec<u16>>,
}

impl WindowsLaunchRequest {
    /// Constructs one structurally valid Windows broker request.
    ///
    /// # Errors
    ///
    /// A field contains an interior NUL or exceeds a count or size bound.
    #[allow(
        clippy::too_many_arguments,
        reason = "the seven fixed arguments are the versioned wire fields whose order this constructor validates"
    )]
    pub fn new(
        working_directory: Vec<u16>,
        program: Vec<u16>,
        arguments: Vec<Vec<u16>>,
        environment: Vec<(Vec<u16>, Vec<u16>)>,
        writable_roots: Vec<Vec<u16>>,
        protected_roots: Vec<Vec<u16>>,
        unreadable_roots: Vec<Vec<u16>>,
    ) -> io::Result<Self> {
        let request = Self {
            working_directory,
            program,
            arguments,
            environment,
            writable_roots,
            protected_roots,
            unreadable_roots,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> io::Result<()> {
        if self.arguments.len() > MAX_WINDOWS_ARGUMENTS
            || self.environment.len() > MAX_WINDOWS_ENVIRONMENT
            || [
                self.writable_roots.len(),
                self.protected_roots.len(),
                self.unreadable_roots.len(),
            ]
            .into_iter()
            .any(|count| count > MAX_WINDOWS_ROOTS)
        {
            return Err(invalid_windows_request());
        }
        if self.working_directory.is_empty()
            || self.program.is_empty()
            || self.environment.iter().any(|(name, _)| name.is_empty())
            || self
                .writable_roots
                .iter()
                .chain(&self.protected_roots)
                .chain(&self.unreadable_roots)
                .any(Vec::is_empty)
            || self.all_fields().any(invalid_wide_field)
        {
            return Err(invalid_windows_request());
        }
        self.encoded_body_len().map(|_| ())
    }

    fn all_fields(&self) -> impl Iterator<Item = &[u16]> {
        [self.working_directory.as_slice(), self.program.as_slice()]
            .into_iter()
            .chain(self.arguments.iter().map(Vec::as_slice))
            .chain(
                self.environment
                    .iter()
                    .flat_map(|(name, value)| [name.as_slice(), value.as_slice()]),
            )
            .chain(self.writable_roots.iter().map(Vec::as_slice))
            .chain(self.protected_roots.iter().map(Vec::as_slice))
            .chain(self.unreadable_roots.iter().map(Vec::as_slice))
    }

    fn encoded_body_len(&self) -> io::Result<usize> {
        let count_bytes = 5 * size_of::<u32>();
        let length = self.all_fields().try_fold(count_bytes, |total, field| {
            let field_bytes = field
                .len()
                .checked_mul(size_of::<u16>())
                .and_then(|bytes| bytes.checked_add(size_of::<u32>()))
                .ok_or_else(invalid_windows_request)?;
            total
                .checked_add(field_bytes)
                .ok_or_else(invalid_windows_request)
        })?;
        if length > MAX_WINDOWS_LAUNCH_BYTES {
            return Err(invalid_windows_request());
        }
        Ok(length)
    }

    /// Command working directory.
    #[must_use]
    pub fn working_directory(&self) -> &[u16] {
        &self.working_directory
    }

    /// Requested program path.
    #[must_use]
    pub fn program(&self) -> &[u16] {
        &self.program
    }

    /// Opaque command arguments in order.
    #[must_use]
    pub fn arguments(&self) -> &[Vec<u16>] {
        &self.arguments
    }

    /// Requested command environment in wire order.
    #[must_use]
    pub fn environment(&self) -> &[(Vec<u16>, Vec<u16>)] {
        &self.environment
    }

    /// Requested roots to receive read and write access for this launch.
    #[must_use]
    pub fn writable_roots(&self) -> &[Vec<u16>] {
        &self.writable_roots
    }

    /// Requested readable roots whose mutation must stay denied.
    #[must_use]
    pub fn protected_roots(&self) -> &[Vec<u16>] {
        &self.protected_roots
    }

    /// Requested roots to deny both read and write access.
    #[must_use]
    pub fn unreadable_roots(&self) -> &[Vec<u16>] {
        &self.unreadable_roots
    }
}

/// Writes one complete Windows request without appending command-input bytes.
///
/// # Errors
///
/// The request does not fit its wire bound or the destination rejects a write.
pub fn encode_windows_launch(
    request: &WindowsLaunchRequest,
    destination: &mut impl Write,
) -> io::Result<()> {
    request.validate()?;
    let body_length = request.encoded_body_len()?;
    let mut body = Vec::with_capacity(body_length);
    write_field(&mut body, request.working_directory())?;
    write_field(&mut body, request.program())?;
    write_fields(&mut body, request.arguments())?;
    write_count(&mut body, request.environment().len())?;
    for (name, value) in request.environment() {
        write_field(&mut body, name)?;
        write_field(&mut body, value)?;
    }
    write_fields(&mut body, request.writable_roots())?;
    write_fields(&mut body, request.protected_roots())?;
    write_fields(&mut body, request.unreadable_roots())?;
    if body.len() != body_length {
        return Err(invalid_windows_request());
    }
    destination.write_all(&WINDOWS_LAUNCH_FRAME)?;
    destination.write_all(&u32_len(body.len())?.to_le_bytes())?;
    destination.write_all(&body)
}

/// Reads exactly one Windows request and leaves later command-input bytes in
/// the stream for the launched peer.
///
/// # Errors
///
/// The frame is truncated, malformed, over its bounds, or has trailing fields.
pub fn decode_windows_launch(source: &mut impl Read) -> io::Result<WindowsLaunchRequest> {
    let mut magic = [0_u8; WINDOWS_LAUNCH_FRAME.len()];
    source.read_exact(&mut magic)?;
    if magic != WINDOWS_LAUNCH_FRAME {
        return Err(invalid_windows_request());
    }
    let mut length = [0_u8; size_of::<u32>()];
    source.read_exact(&mut length)?;
    let length =
        usize::try_from(u32::from_le_bytes(length)).map_err(|_| invalid_windows_request())?;
    if length > MAX_WINDOWS_LAUNCH_BYTES {
        return Err(invalid_windows_request());
    }
    let mut body = vec![0_u8; length];
    source.read_exact(&mut body)?;
    let mut cursor = Cursor::new(&body);
    let request = WindowsLaunchRequest::new(
        cursor.field()?,
        cursor.field()?,
        cursor.fields(MAX_WINDOWS_ARGUMENTS)?,
        cursor.environment()?,
        cursor.fields(MAX_WINDOWS_ROOTS)?,
        cursor.fields(MAX_WINDOWS_ROOTS)?,
        cursor.fields(MAX_WINDOWS_ROOTS)?,
    )?;
    if cursor.remaining() != 0 {
        return Err(invalid_windows_request());
    }
    Ok(request)
}

fn invalid_wide_field(field: &[u16]) -> bool {
    field.len() > MAX_WINDOWS_FIELD_UNITS || field.contains(&0)
}

fn write_fields(output: &mut Vec<u8>, fields: &[Vec<u16>]) -> io::Result<()> {
    write_count(output, fields.len())?;
    for field in fields {
        write_field(output, field)?;
    }
    Ok(())
}

fn write_count(output: &mut Vec<u8>, count: usize) -> io::Result<()> {
    output.extend_from_slice(&u32_len(count)?.to_le_bytes());
    Ok(())
}

fn write_field(output: &mut Vec<u8>, field: &[u16]) -> io::Result<()> {
    if invalid_wide_field(field) {
        return Err(invalid_windows_request());
    }
    write_count(output, field.len())?;
    for unit in field {
        output.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(())
}

fn u32_len(length: usize) -> io::Result<u32> {
    u32::try_from(length).map_err(|_| invalid_windows_request())
}

fn invalid_windows_request() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid Windows sandbox request",
    )
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    fn count(&mut self) -> io::Result<usize> {
        let bytes = self.take(size_of::<u32>())?;
        let encoded: [u8; size_of::<u32>()] =
            bytes.try_into().map_err(|_| invalid_windows_request())?;
        usize::try_from(u32::from_le_bytes(encoded)).map_err(|_| invalid_windows_request())
    }

    fn field(&mut self) -> io::Result<Vec<u16>> {
        let units = self.count()?;
        if units > MAX_WINDOWS_FIELD_UNITS {
            return Err(invalid_windows_request());
        }
        let bytes = self.take(
            units
                .checked_mul(size_of::<u16>())
                .ok_or_else(invalid_windows_request)?,
        )?;
        let field: Vec<_> = bytes
            .chunks_exact(size_of::<u16>())
            .map(|unit| {
                let [low, high] = unit else {
                    return Err(invalid_windows_request());
                };
                Ok(u16::from_le_bytes([*low, *high]))
            })
            .collect::<io::Result<_>>()?;
        if invalid_wide_field(&field) {
            return Err(invalid_windows_request());
        }
        Ok(field)
    }

    fn fields(&mut self, maximum: usize) -> io::Result<Vec<Vec<u16>>> {
        let count = self.count()?;
        if count > maximum {
            return Err(invalid_windows_request());
        }
        (0..count).map(|_| self.field()).collect()
    }

    fn environment(&mut self) -> io::Result<Vec<(Vec<u16>, Vec<u16>)>> {
        let count = self.count()?;
        if count > MAX_WINDOWS_ENVIRONMENT {
            return Err(invalid_windows_request());
        }
        (0..count)
            .map(|_| Ok((self.field()?, self.field()?)))
            .collect()
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(invalid_windows_request)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or_else(invalid_windows_request)?;
        self.position = end;
        Ok(value)
    }
}

/// Maximum writable roots in one immutable broker plan.
pub const MAX_SCAN_ROOTS: usize = 64;

/// Maximum exact subtrees excluded because a narrower mount masks them.
pub const MAX_SCAN_EXCLUSIONS: usize = 4096;

/// Maximum entries retained across one projected root.
pub const MAX_SCAN_ENTRIES: usize = 262_144;

/// Maximum byte length of one raw relative path.
pub const MAX_SCAN_PATH_BYTES: usize = 16 * 1024;

/// Maximum byte length of a retained symbolic-link target.
pub const MAX_SCAN_SYMLINK_BYTES: usize = 16 * 1024;

/// Maximum ordered data extents retained for one regular file.
pub const MAX_SCAN_EXTENTS: usize = 131_072;

/// Maximum declared byte length of one regular file in a projected tree.
///
/// A sparse file costs its creator nothing, but every digest of it costs its
/// declared length. The bound is checked before any file is read on either
/// side, so a workload cannot buy hours of host hashing with one `truncate`.
pub const MAX_SCAN_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Wire tag for a directory semantic record.
pub const ENTRY_DIRECTORY: u8 = 1;

/// Wire tag for a regular-file semantic record.
pub const ENTRY_FILE: u8 = 2;

/// Wire tag for a symbolic-link semantic record.
pub const ENTRY_SYMLINK: u8 = 3;

/// Encodes the kernel wait status without flattening signals into exit codes.
#[must_use]
pub const fn encode_wait_status(status: i32) -> [u8; WAIT_STATUS_BYTES] {
    status.to_ne_bytes()
}

/// Decodes one complete broker status frame.
#[must_use]
pub const fn decode_wait_status(bytes: [u8; WAIT_STATUS_BYTES]) -> i32 {
    i32::from_ne_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_wait_status_round_trips_without_signal_flattening() {
        for status in [0, 17 << 8, 143 << 8, 15, BROKER_FAILURE_STATUS] {
            assert_eq!(decode_wait_status(encode_wait_status(status)), status);
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    fn request() -> WindowsLaunchRequest {
        WindowsLaunchRequest::new(
            wide(r"C:\work\crucible"),
            wide(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            vec![
                wide("-NoProfile"),
                wide("Write-Output 'ferrum ⚒'"),
                vec![0xd800],
            ],
            vec![(wide("PATH"), wide(r"C:\Windows\System32"))],
            vec![wide(r"C:\work\crucible")],
            vec![wide(r"C:\work\crucible\.git")],
            vec![wide(r"C:\Users\person\.ssh")],
        )
        .expect("request")
    }

    #[test]
    fn windows_launch_round_trips_and_leaves_peer_input_unread() {
        let request = request();
        let mut wire = Vec::new();
        encode_windows_launch(&request, &mut wire).expect("encode");
        wire.extend_from_slice(b"peer input");

        let mut source = std::io::Cursor::new(wire);
        assert_eq!(decode_windows_launch(&mut source).expect("decode"), request);
        let mut remaining = Vec::new();
        source.read_to_end(&mut remaining).expect("remaining input");
        assert_eq!(remaining, b"peer input");
    }

    #[test]
    fn windows_launch_rejects_unbounded_or_ambiguous_fields() {
        let mut too_many = request();
        too_many.arguments = vec![Vec::new(); MAX_WINDOWS_ARGUMENTS + 1];
        let mut wire = Vec::new();
        assert!(encode_windows_launch(&too_many, &mut wire).is_err());

        let mut nul = request();
        nul.program.push(0);
        assert!(encode_windows_launch(&nul, &mut wire).is_err());

        let oversized_body = WindowsLaunchRequest::new(
            wide(r"C:\work"),
            wide(r"C:\tool.exe"),
            vec![vec![u16::from(b'x'); MAX_WINDOWS_FIELD_UNITS]; 9],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert!(oversized_body.is_err());

        let mut oversized = Vec::from(WINDOWS_LAUNCH_FRAME);
        oversized.extend_from_slice(
            &u32::try_from(MAX_WINDOWS_LAUNCH_BYTES + 1)
                .expect("bounded test length")
                .to_le_bytes(),
        );
        assert!(decode_windows_launch(&mut oversized.as_slice()).is_err());
    }

    #[test]
    fn windows_launch_rejects_truncated_and_trailing_payload_fields() {
        let mut wire = Vec::new();
        encode_windows_launch(&request(), &mut wire).expect("encode");
        let truncated = wire
            .get(..wire.len().saturating_sub(1))
            .expect("truncated request");
        assert!(decode_windows_launch(&mut &*truncated).is_err());

        let length_start = WINDOWS_LAUNCH_FRAME.len();
        let length_end = length_start + size_of::<u32>();
        let length_bytes = wire
            .get(length_start..length_end)
            .expect("encoded body length");
        let body_length = u32::from_le_bytes(length_bytes.try_into().expect("length"));
        wire.extend_from_slice(&0_u32.to_le_bytes());
        wire.get_mut(length_start..length_end)
            .expect("encoded body length")
            .copy_from_slice(&(body_length + 4).to_le_bytes());
        assert!(decode_windows_launch(&mut wire.as_slice()).is_err());
    }
}
