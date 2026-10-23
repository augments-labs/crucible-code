//! A catalog a test signs for itself.
//!
//! No key a test verifies under is ever written down: the signer is an Ed25519
//! key minted from the system's random source the first time a test in the
//! process asks for one, and kept only in memory, so every catalog a process
//! signs verifies under the same [`Keys`] and under no other process's. The
//! signature is minisign's own format, made over the catalog's BLAKE2b-512
//! digest as minisign makes it today, so what verifies here is what verifies
//! a catalog the publishing job signs with minisign. Compiled under the
//! `proof` feature alone, which only `[dev-dependencies]` turn on, so nothing
//! that ships carries a signer.

use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use blake2::{Blake2b512, Digest as _};
use ring::rand::{SecureRandom as _, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair as _};

use super::Keys;

/// How long after it was signed a test catalog expires when its JSON does not
/// say, as long as the publishing job gives a catalog.
const LIFETIME: Duration = Duration::from_hours(30 * 24);

/// A catalog, its detached signature and the keys that verify it.
#[derive(Debug)]
pub struct Signed {
    /// The catalog's bytes, as they were signed.
    pub index: Vec<u8>,
    /// Its minisign signature, as the text a `.minisig` file holds.
    pub signature: Vec<u8>,
    /// The one key it verifies under.
    pub keys: Keys,
}

/// The key this process signs with, minted on first use.
struct Signer {
    pair: Ed25519KeyPair,
    id: [u8; 8],
    keys: Keys,
}

/// `json` signed with this process's key.
///
/// When `json` is an object without `signed_at` or `expires`, the one missing
/// is filled in from the clock first, `signed_at` as now and `expires` thirty
/// days later, so a test of something else need not age. Anything else is
/// signed exactly as given, so a test can sign bytes no reader would take.
///
/// # Panics
///
/// When the system's random source cannot mint a key, which leaves the test
/// asking for a signature nothing to report.
#[must_use]
pub fn signed(json: &[u8]) -> Signed {
    let signer = signer();
    let index = dated(json).unwrap_or_else(|| json.to_vec());
    let digest = Blake2b512::digest(&index);
    let signature = signer.pair.sign(&digest);
    let trusted = "timestamp:0\tfile:catalog.json\thashed";
    let mut global = signature.as_ref().to_vec();
    global.extend_from_slice(trusted.as_bytes());
    let global = signer.pair.sign(&global);
    let mut algorithm = b"ED".to_vec();
    algorithm.extend_from_slice(&signer.id);
    algorithm.extend_from_slice(signature.as_ref());
    let text = format!(
        "untrusted comment: a test's own key\n{}\ntrusted comment: {trusted}\n{}\n",
        STANDARD.encode(&algorithm),
        STANDARD.encode(global.as_ref()),
    );
    Signed {
        index,
        signature: text.into_bytes(),
        keys: signer.keys,
    }
}

/// This process's signer, minted the first time it is asked for.
#[allow(clippy::expect_used)]
fn signer() -> &'static Signer {
    static SIGNER: OnceLock<Signer> = OnceLock::new();
    SIGNER.get_or_init(|| {
        let random = SystemRandom::new();
        let mut seed = [0_u8; 32];
        let mut id = [0_u8; 8];
        random.fill(&mut seed).expect("a seed for the test key");
        random.fill(&mut id).expect("an id for the test key");
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed).expect("a test key");
        let mut public = b"Ed".to_vec();
        public.extend_from_slice(&id);
        public.extend_from_slice(pair.public_key().as_ref());
        // Once per process, so the one key a test verifies under can be
        // handed out as the same `'static` keys the binary compiles in.
        let public: &'static str = STANDARD.encode(public).leak();
        let listed: &'static [&'static str] = Box::leak(Box::new([public]));
        Signer {
            pair,
            id,
            keys: Keys::new(listed),
        }
    })
}

/// `json` with `signed_at` and `expires` filled from the clock where it is an
/// object missing either, or `None` where nothing is to be filled.
fn dated(json: &[u8]) -> Option<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let object = value.as_object_mut()?;
    if object.contains_key("signed_at") && object.contains_key("expires") {
        return None;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    object
        .entry("signed_at")
        .or_insert_with(|| serde_json::Value::from(now));
    object
        .entry("expires")
        .or_insert_with(|| serde_json::Value::from(now + LIFETIME.as_secs()));
    serde_json::to_vec(&value).ok()
}
