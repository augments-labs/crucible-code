//! Random sandbox-account credentials protected by machine-scope DPAPI.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom, CRYPT_INTEGER_BLOB,
    CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

use super::winutil::error;

pub(super) struct SecretWide(Vec<u16>);

impl SecretWide {
    pub(super) fn generate() -> io::Result<Self> {
        let mut random = [0_u8; 32];
        // SAFETY: `random` is writable for exactly the supplied byte count;
        // a null algorithm handle is required with the system-preferred flag.
        let status = unsafe {
            BCryptGenRandom(
                null_mut(),
                random.as_mut_ptr(),
                32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(io::Error::other(format!(
                "BCryptGenRandom failed: NTSTATUS 0x{:08X}",
                status.cast_unsigned()
            )));
        }
        let mut password = String::from("Cr1!-");
        for byte in &random {
            append_hex(&mut password, *byte);
        }
        random.fill(0);
        let value = password.encode_utf16().chain(Some(0)).collect();
        // Erase the UTF-8 staging copy before returning the credential.
        let mut password = password.into_bytes();
        password.fill(0);
        Ok(Self(value))
    }

    pub(super) fn from_units(mut units: Vec<u16>) -> io::Result<Self> {
        let valid = units
            .split_last()
            .is_some_and(|(last, body)| *last == 0 && !body.is_empty() && !body.contains(&0))
            && units.len() <= 257;
        if !valid {
            units.fill(0);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Windows sandbox credential",
            ));
        }
        Ok(Self(units))
    }

    pub(super) fn as_ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }

    pub(super) fn protect(&self, entropy: &[u8]) -> io::Result<Vec<u8>> {
        let input_bytes = self
            .0
            .len()
            .checked_mul(size_of::<u16>())
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "credential too large"))?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: input_bytes,
            pbData: self.0.as_ptr().cast_mut().cast(),
        };
        let entropy = blob(entropy)?;
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        // SAFETY: every blob references a live slice for the duration of the
        // call and `output` is initialized storage owned by DPAPI on success.
        let protected = unsafe {
            CryptProtectData(
                &raw const input,
                null(),
                &raw const entropy,
                null(),
                null(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        if protected == 0 {
            return Err(error("CryptProtectData"));
        }
        let valid = !output.pbData.is_null() && output.cbData != 0;
        let bytes = if valid {
            // SAFETY: DPAPI returned `pbData` with exactly `cbData` initialized
            // bytes and transfers that LocalAlloc allocation to the caller.
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
        } else {
            Vec::new()
        };
        if !output.pbData.is_null() {
            // SAFETY: `pbData` is the DPAPI-owned LocalAlloc result just copied.
            unsafe {
                LocalFree(output.pbData as HLOCAL);
            }
        }
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "CryptProtectData returned an empty credential",
            ));
        }
        Ok(bytes)
    }

    pub(super) fn unprotect(protected: &[u8], entropy: &[u8]) -> io::Result<Self> {
        if protected.is_empty() || protected.len() > 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid protected sandbox credential",
            ));
        }
        let input = blob(protected)?;
        let entropy = blob(entropy)?;
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        // SAFETY: input and entropy reference live bounded slices and output
        // is initialized storage that DPAPI fills on success.
        let unprotected = unsafe {
            CryptUnprotectData(
                &raw const input,
                null_mut(),
                &raw const entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        if unprotected == 0 {
            return Err(error("CryptUnprotectData"));
        }
        let result = if output.pbData.is_null() || output.cbData == 0 {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "CryptUnprotectData returned an empty credential",
            ))
        } else if !output.cbData.is_multiple_of(2) {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid protected sandbox credential length",
            ))
        } else {
            // SAFETY: DPAPI returned `pbData` with exactly `cbData`
            // initialized bytes and the allocation remains live below.
            let bytes =
                unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
            let units = bytes
                .chunks_exact(2)
                .map(|pair| pair.try_into().ok().map(u16::from_le_bytes))
                .collect::<Option<Vec<_>>>();
            units.map_or_else(
                || {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid protected sandbox credential",
                    ))
                },
                Self::from_units,
            )
        };
        if !output.pbData.is_null() {
            // SAFETY: the DPAPI allocation is still live and writable for
            // `cbData` bytes; it is erased before LocalFree releases it.
            unsafe {
                std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
                LocalFree(output.pbData as HLOCAL);
            }
        }
        result
    }
}

impl Drop for SecretWide {
    fn drop(&mut self) {
        for unit in &mut self.0 {
            // SAFETY: `unit` is a live exclusive reference. Volatile writes
            // prevent the credential erasure from being optimized away.
            unsafe {
                std::ptr::write_volatile(unit, 0);
            }
        }
    }
}

fn append_hex(value: &mut String, byte: u8) {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
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

fn blob(bytes: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "DPAPI input too large"))?,
        pbData: bytes.as_ptr().cast_mut(),
    })
}
