//! Protected HKLM storage for the Windows sandbox setup record.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_BINARY,
    REG_OPTION_NON_VOLATILE, RRF_RT_REG_BINARY, RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW,
    RegGetValueW, RegOpenKeyExW, RegSetKeySecurity, RegSetValueExW,
};

use super::identity::SetupIdentity;
use super::record::{SetupRecord, Status};
use super::winutil::{SecurityDescriptor, code_error, wide};

const STATE_VALUE: &str = "State";
const WRITE_DAC_ACCESS: u32 = 0x0004_0000;

pub(super) fn load(identity: &SetupIdentity) -> io::Result<Option<SetupRecord>> {
    let key = match RegistryKey::open(&identity.registry_subkey, KEY_READ | KEY_WOW64_64KEY) {
        Ok(key) => key,
        Err(source) if source.raw_os_error() == Some(ERROR_FILE_NOT_FOUND.cast_signed()) => {
            return Ok(None);
        }
        Err(source) => return Err(source),
    };
    key.read_record()
}

pub(super) fn store(
    identity: &SetupIdentity,
    owner_sid_string: &str,
    record: &SetupRecord,
) -> io::Result<()> {
    let sddl = match record.status {
        // Until network denial is installed, the owner must not be able to
        // decrypt the account password and use an incompletely confined
        // identity. Administrators and SYSTEM retain repair access.
        Status::Pending => "D:P(A;;KA;;;SY)(A;;KA;;;BA)".to_owned(),
        Status::Installed => {
            format!("D:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;KR;;;{owner_sid_string})")
        }
    };
    let descriptor = SecurityDescriptor::from_sddl(sddl)?;
    let key = RegistryKey::create(&identity.registry_subkey, descriptor.as_ptr())?;
    key.set_security(descriptor.as_ptr())?;
    key.write_record(record)
}

pub(super) fn delete(identity: &SetupIdentity) -> io::Result<()> {
    let path = wide(&identity.registry_subkey);
    // SAFETY: `path` is a live NUL-terminated deterministic key path and the
    // selected 64-bit view is the one used by create/open below.
    let status = unsafe { RegDeleteKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), KEY_WOW64_64KEY, 0) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(code_error("RegDeleteKeyExW", status))
    }
}

struct RegistryKey(HKEY);

impl RegistryKey {
    fn open(path: &str, access: u32) -> io::Result<Self> {
        let path = wide(path);
        let mut key = null_mut();
        // SAFETY: `path` is NUL terminated and `key` is initialized out-handle
        // storage. A successful call transfers one closeable key handle.
        let status =
            unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, access, &raw mut key) };
        if status == ERROR_FILE_NOT_FOUND {
            return Err(io::Error::from_raw_os_error(status.cast_signed()));
        }
        if status != ERROR_SUCCESS {
            return Err(code_error("RegOpenKeyExW", status));
        }
        Ok(Self(key))
    }

    fn create(path: &str, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<Self> {
        let path = wide(path);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid security attributes size",
                )
            })?,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let mut key = null_mut();
        // SAFETY: all pointers reference live initialized values, `path` is
        // NUL terminated, and a successful call transfers one key handle.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE | KEY_WOW64_64KEY | WRITE_DAC_ACCESS,
                &raw const attributes,
                &raw mut key,
                null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(code_error("RegCreateKeyExW", status));
        }
        Ok(Self(key))
    }

    fn set_security(&self, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
        // SAFETY: `self` owns a live key handle and `descriptor` is a complete
        // self-relative descriptor kept alive by the caller during this call.
        let status = unsafe {
            RegSetKeySecurity(
                self.0,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(code_error("RegSetKeySecurity", status))
        }
    }

    fn read_record(&self) -> io::Result<Option<SetupRecord>> {
        let name = wide(STATE_VALUE);
        let mut length = 0_u32;
        // SAFETY: the key and NUL-terminated value name are live; null data
        // requests only the required byte count into initialized storage.
        let status = unsafe {
            RegGetValueW(
                self.0,
                null(),
                name.as_ptr(),
                RRF_RT_REG_BINARY,
                null_mut(),
                null_mut(),
                &raw mut length,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(code_error("RegGetValueW(size)", status));
        }
        if length == 0 || length > 32 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Windows sandbox registry record length",
            ));
        }
        let mut bytes = vec![0_u8; length as usize];
        // SAFETY: `bytes` is writable for the advertised `length`, which is
        // also supplied as the in/out buffer length.
        let status = unsafe {
            RegGetValueW(
                self.0,
                null(),
                name.as_ptr(),
                RRF_RT_REG_BINARY,
                null_mut(),
                bytes.as_mut_ptr().cast(),
                &raw mut length,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(code_error("RegGetValueW", status));
        }
        bytes.truncate(length as usize);
        SetupRecord::decode(&bytes).map(Some)
    }

    fn write_record(&self, record: &SetupRecord) -> io::Result<()> {
        let name = wide(STATE_VALUE);
        let bytes = record.encode()?;
        let length = u32::try_from(bytes.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "sandbox setup record too large")
        })?;
        // SAFETY: the key and NUL-terminated value name are live and `bytes`
        // contains exactly `length` initialized bytes.
        let status =
            unsafe { RegSetValueExW(self.0, name.as_ptr(), 0, REG_BINARY, bytes.as_ptr(), length) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(code_error("RegSetValueExW", status))
        }
    }
}

impl Drop for RegistryKey {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` is the unique live key handle owned here.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
}
