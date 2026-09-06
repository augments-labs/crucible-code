//! Stable machine identities for one host user's Windows sandbox setup.
//!
//! Windows limits local account names to twenty characters. The owner SID is
//! therefore hashed into a short account suffix while a longer, independently
//! domain-separated digest names registry state and every WFP filter. Setup
//! still stores and compares the complete owner SID, so a short-name collision
//! is a refusal rather than authority for the wrong user.

use sha2::{Digest as _, Sha256};
use windows_sys::core::GUID;

const ACCOUNT_DOMAIN: &[u8] = b"crucible/windows-sandbox/account/v1\0";
const REGISTRY_DOMAIN: &[u8] = b"crucible/windows-sandbox/registry/v1\0";
const ENTROPY_DOMAIN: &[u8] = b"crucible/windows-sandbox/dpapi/v1\0";
const FILTER_DOMAIN: &[u8] = b"crucible/windows-sandbox/wfp/v1\0";

const FILTER_LABELS: [&[u8]; 4] = [
    b"connect-v4",
    b"connect-v6",
    b"assignment-v4",
    b"assignment-v6",
];

pub(super) struct SetupIdentity {
    pub(super) owner_sid: Vec<u8>,
    pub(super) account_name: String,
    pub(super) registry_subkey: String,
    pub(super) entropy: [u8; 32],
    pub(super) filter_keys: [GUID; 4],
}

impl SetupIdentity {
    pub(super) fn for_owner(owner_sid: &[u8]) -> Self {
        let account_hash = digest(ACCOUNT_DOMAIN, owner_sid, &[]);
        let registry_hash = digest(REGISTRY_DOMAIN, owner_sid, &[]);
        let entropy = digest(ENTROPY_DOMAIN, owner_sid, &[]);
        let filter_keys = FILTER_LABELS.map(|label| {
            let bytes = digest(FILTER_DOMAIN, owner_sid, label);
            let mut value = [0_u8; 16];
            value.copy_from_slice(&bytes[..16]);
            // Give the deterministic value RFC 4122 version/variant bits. WFP
            // treats it as an opaque GUID, but conventional bits make dumps
            // and diagnostics less surprising.
            value[6] = (value[6] & 0x0f) | 0x50;
            value[8] = (value[8] & 0x3f) | 0x80;
            GUID::from_u128(u128::from_be_bytes(value))
        });
        Self {
            owner_sid: owner_sid.to_vec(),
            account_name: format!(
                "CrucibleSBX-{:02X}{:02X}{:02X}{:02X}",
                account_hash[0], account_hash[1], account_hash[2], account_hash[3]
            ),
            registry_subkey: format!(
                "SOFTWARE\\Augments Labs\\Crucible\\Sandbox\\Owners\\{}",
                hex(&registry_hash[..16])
            ),
            entropy,
            filter_keys,
        }
    }
}

fn digest(domain: &[u8], owner_sid: &[u8], label: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(owner_sid);
    hash.update([0]);
    hash.update(label);
    hash.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(
            DIGITS.get(usize::from(byte >> 4)).copied().unwrap_or(b'0'),
        ));
        value.push(char::from(
            DIGITS
                .get(usize::from(byte & 0x0f))
                .copied()
                .unwrap_or(b'0'),
        ));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_identity_is_stable_bounded_and_domain_separated() {
        let first = SetupIdentity::for_owner(b"owner sid one");
        let same = SetupIdentity::for_owner(b"owner sid one");
        let other = SetupIdentity::for_owner(b"owner sid two");

        assert_eq!(first.account_name, same.account_name);
        assert_eq!(first.registry_subkey, same.registry_subkey);
        assert_eq!(first.entropy, same.entropy);
        assert!(first.account_name.len() <= 20);
        assert_ne!(first.account_name, other.account_name);
        assert_ne!(first.registry_subkey, other.registry_subkey);
        assert_ne!(first.entropy, other.entropy);

        let tuple = |key: GUID| (key.data1, key.data2, key.data3, key.data4);
        let keys = first.filter_keys.map(tuple);
        assert_eq!(keys, same.filter_keys.map(tuple));
        assert_eq!(
            keys.into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4
        );
        assert_ne!(keys, other.filter_keys.map(tuple));
    }
}
