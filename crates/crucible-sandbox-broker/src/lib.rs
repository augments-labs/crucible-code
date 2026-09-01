//! Fixed protocol shared by the host launcher and the minimal PID-1 broker.

/// Bytes in one native-endian Linux wait status.
pub const WAIT_STATUS_BYTES: usize = size_of::<i32>();

/// Status sent when the broker could not create or reap the workload.
pub const BROKER_FAILURE_STATUS: i32 = i32::MIN;

/// Broker-to-host attestation that parsing and descriptor closure completed.
pub const READY_FRAME: [u8; 8] = *b"CRREADY1";

/// The sole host-to-broker authority to create the workload.
pub const GO_FRAME: [u8; 8] = *b"CRGO0001";

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
