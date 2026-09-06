//! Private desktop for one restricted sandbox workload.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, CreateDesktopW, CreateWindowStationW, DESKTOP_CREATEMENU,
    DESKTOP_CREATEWINDOW, DESKTOP_ENUMERATE, DESKTOP_HOOKCONTROL, DESKTOP_JOURNALPLAYBACK,
    DESKTOP_JOURNALRECORD, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP, DESKTOP_WRITEOBJECTS,
    GetProcessWindowStation, HDESK, HWINSTA, SetProcessWindowStation,
};

use super::winutil::{SecurityDescriptor, error, sid_string, wide};

const DESKTOP_ACCESS: u32 = DESKTOP_READOBJECTS
    | DESKTOP_CREATEWINDOW
    | DESKTOP_CREATEMENU
    | DESKTOP_HOOKCONTROL
    | DESKTOP_JOURNALRECORD
    | DESKTOP_JOURNALPLAYBACK
    | DESKTOP_ENUMERATE
    | DESKTOP_WRITEOBJECTS
    | DESKTOP_SWITCHDESKTOP;
const WINDOW_STATION_ALL_ACCESS: u32 = 0x000f_037f;
const DESKTOP_ENVIRONMENT: &str = "CRUCIBLE_SANDBOX_DESKTOP";

pub(super) struct PrivateDesktop {
    station: HWINSTA,
    handle: HDESK,
    startup_name: Vec<u16>,
}

impl PrivateDesktop {
    pub(super) fn create(account_sid: &[u8]) -> io::Result<Self> {
        let account = sid_string(account_sid)?;
        let host = sid_string(&super::winutil::current_user_sid()?)?;
        let nonce = nonce()?;
        let station_name = format!("CrucibleSandbox-{nonce}");
        let desktop_name = "Default";
        let startup_name = wide(format!("{station_name}\\{desktop_name}"));
        let capability = sid_string(&super::plan::desktop_sid(
            account_sid,
            without_nul(&startup_name),
        ))?;
        let descriptor = SecurityDescriptor::from_sddl(format!(
            "D:P(A;;GA;;;{host})(A;;GA;;;{account})(A;;GA;;;{capability})"
        ))?;
        let station_encoded = wide(&station_name);
        let desktop_encoded = wide(desktop_name);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .map_err(|_| io::Error::other("invalid desktop security attributes size"))?,
            lpSecurityDescriptor: descriptor.as_ptr(),
            bInheritHandle: 0,
        };
        // SAFETY: the name, security descriptor and attributes remain live.
        let station = unsafe {
            CreateWindowStationW(
                station_encoded.as_ptr(),
                0,
                WINDOW_STATION_ALL_ACCESS,
                &raw const attributes,
            )
        };
        if station.is_null() {
            return Err(error("CreateWindowStationW"));
        }
        // SAFETY: this process always has a current station and `station` is a
        // live station created above. This broker is single-threaded here.
        let previous = unsafe { GetProcessWindowStation() };
        // SAFETY: `station` is the live station created above.
        let attached = !previous.is_null() && unsafe { SetProcessWindowStation(station) } != 0;
        if !attached {
            // SAFETY: station is the one owned handle created above.
            unsafe {
                CloseWindowStation(station);
            }
            return Err(error("SetProcessWindowStation(private)"));
        }
        // SAFETY: the desktop name is NUL terminated, the process is currently
        // attached to `station`, and the descriptor remains live.
        let handle = unsafe {
            CreateDesktopW(
                desktop_encoded.as_ptr(),
                null(),
                null(),
                0,
                DESKTOP_ACCESS,
                &raw const attributes,
            )
        };
        // SAFETY: previous was the live station returned for this process.
        let restored = unsafe { SetProcessWindowStation(previous) };
        if restored == 0 {
            // SAFETY: both handles were created above and are still owned.
            unsafe {
                if !handle.is_null() {
                    CloseDesktop(handle);
                }
                CloseWindowStation(station);
            }
            return Err(error("SetProcessWindowStation(restore)"));
        }
        if handle.is_null() {
            // SAFETY: station is the one owned handle created above.
            unsafe {
                CloseWindowStation(station);
            }
            return Err(error("CreateDesktopW"));
        }
        Ok(Self {
            station,
            handle,
            startup_name,
        })
    }

    pub(super) fn from_environment() -> io::Result<Self> {
        let name = std::env::var_os(DESKTOP_ENVIRONMENT).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Windows sandbox private desktop identity is missing",
            )
        })?;
        let text = name.to_str().ok_or_else(invalid_desktop)?;
        let Some(nonce) = text
            .strip_prefix("CrucibleSandbox-")
            .and_then(|value| value.strip_suffix("\\Default"))
        else {
            return Err(invalid_desktop());
        };
        if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_desktop());
        }
        Ok(Self {
            station: null_mut(),
            handle: null_mut(),
            startup_name: wide(text),
        })
    }

    pub(super) fn environment(&self) -> (&'static str, &[u16]) {
        (DESKTOP_ENVIRONMENT, without_nul(&self.startup_name))
    }

    pub(super) fn startup_name(&mut self) -> *mut u16 {
        self.startup_name.as_mut_ptr()
    }

    pub(super) fn capability_sid(&self, account_sid: &[u8]) -> Vec<u8> {
        super::plan::desktop_sid(account_sid, without_nul(&self.startup_name))
    }
}

impl Drop for PrivateDesktop {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: this wrapper uniquely owns the private desktop handle.
            unsafe {
                CloseDesktop(self.handle);
            }
        }
        if !self.station.is_null() {
            // SAFETY: this wrapper uniquely owns the private station handle.
            unsafe {
                CloseWindowStation(self.station);
            }
        }
    }
}

fn without_nul(value: &[u16]) -> &[u16] {
    value.strip_suffix(&[0]).unwrap_or(value)
}

fn invalid_desktop() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Windows sandbox private desktop identity is invalid",
    )
}

fn nonce() -> io::Result<String> {
    let mut random = [0_u8; 16];
    // SAFETY: random is writable for exactly the supplied byte count and the
    // system-preferred flag requires a null algorithm handle.
    let status = unsafe {
        BCryptGenRandom(
            null_mut(),
            random.as_mut_ptr(),
            u32::try_from(random.len())
                .map_err(|_| io::Error::other("invalid desktop nonce length"))?,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(io::Error::other(format!(
            "BCryptGenRandom failed: NTSTATUS 0x{:08X}",
            status.cast_unsigned()
        )));
    }
    let mut encoded = String::with_capacity(random.len() * 2);
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02X}")
            .map_err(|_| io::Error::other("desktop nonce formatting failed"))?;
    }
    Ok(encoded)
}
