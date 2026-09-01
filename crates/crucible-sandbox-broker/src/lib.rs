//! Fixed protocol shared by the host launcher and the minimal PID 1 broker.

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
}
