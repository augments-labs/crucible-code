//! Lifecycle of the dedicated local low-privilege sandbox account.

use std::io;
use std::ptr::NonNull;
use std::ptr::{null, null_mut};

use windows_sys::Win32::NetworkManagement::NetManagement::{
    NERR_Success, NERR_UserExists, NERR_UserNotFound, NetApiBufferFree, NetUserAdd, NetUserDel,
    NetUserGetInfo, NetUserSetInfo, UF_ACCOUNTDISABLE, UF_DONT_EXPIRE_PASSWD, UF_NORMAL_ACCOUNT,
    UF_NOT_DELEGATED, UF_SCRIPT, USER_INFO_1, USER_INFO_1003, USER_INFO_1008, USER_PRIV_USER,
};
use windows_sys::Win32::Security::{
    LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, LogonUserW,
};
use windows_sys::Win32::System::GroupPolicy::PI_NOUI;
use windows_sys::Win32::UI::Shell::{LoadUserProfileW, PROFILEINFOW, UnloadUserProfile};

use super::secret::SecretWide;
use super::winutil::{OwnedHandle, account_sid, code_error, wide};

const REQUIRED_FLAGS: u32 =
    UF_SCRIPT | UF_NORMAL_ACCOUNT | UF_DONT_EXPIRE_PASSWD | UF_NOT_DELEGATED;

pub(super) fn exists(name: &str) -> io::Result<bool> {
    Ok(read_flags(name)?.is_some())
}

/// Creates or repairs the account represented by an existing trusted record.
/// Returns whether this call created a new account.
pub(super) fn ensure(name: &str, password: &SecretWide) -> io::Result<bool> {
    let name_wide = wide(name);
    let info = USER_INFO_1 {
        usri1_name: name_wide.as_ptr().cast_mut(),
        usri1_password: password.as_ptr().cast_mut(),
        usri1_password_age: 0,
        usri1_priv: USER_PRIV_USER,
        usri1_home_dir: null_mut(),
        usri1_comment: null_mut(),
        usri1_flags: REQUIRED_FLAGS,
        usri1_script_path: null_mut(),
    };
    let mut parameter = 0_u32;
    // SAFETY: every pointer in `info` references a live NUL-terminated buffer
    // and the level-1 structure remains live for the duration of the call.
    let status = unsafe { NetUserAdd(null(), 1, (&raw const info).cast(), &raw mut parameter) };
    if status == NERR_Success {
        return Ok(true);
    } else if status == NERR_UserExists {
        let password_info = USER_INFO_1003 {
            usri1003_password: password.as_ptr().cast_mut(),
        };
        // SAFETY: the password and account buffers are live, NUL terminated,
        // and level 1003 consumes exactly `USER_INFO_1003`.
        net_success("NetUserSetInfo(password)", unsafe {
            NetUserSetInfo(
                null(),
                name_wide.as_ptr(),
                1003,
                (&raw const password_info).cast(),
                &raw mut parameter,
            )
        })?;
    } else {
        return Err(code_error("NetUserAdd", status));
    }
    let flags = USER_INFO_1008 {
        usri1008_flags: REQUIRED_FLAGS,
    };
    // SAFETY: the account buffer and level-1008 structure remain live for the
    // duration of the synchronous NetAPI call.
    net_success("NetUserSetInfo(flags)", unsafe {
        NetUserSetInfo(
            null(),
            name_wide.as_ptr(),
            1008,
            (&raw const flags).cast(),
            &raw mut parameter,
        )
    })?;
    Ok(false)
}

pub(super) fn sid(name: &str) -> io::Result<Vec<u8>> {
    account_sid(std::ffi::OsStr::new(name))
}

pub(super) fn probe(name: &str, expected_sid: &[u8], password: &SecretWide) -> io::Result<()> {
    let flags = read_flags(name)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Windows sandbox account is missing",
        )
    })?;
    if flags & REQUIRED_FLAGS != REQUIRED_FLAGS || flags & UF_ACCOUNTDISABLE != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows sandbox account flags are invalid",
        ));
    }
    if sid(name)? != expected_sid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows sandbox account SID does not match setup state",
        ));
    }
    logon(name, password)?;
    Ok(())
}

/// Makes the account's profile exist before anyone waits on a launch.
///
/// Every launch logs on with its profile, and Windows builds that profile the
/// first time it is asked for: over 23 seconds on a CI runner, against about
/// 0.15 seconds for each logon after it. Setup has someone waiting for it; a
/// launch holds the setup lock while it builds, and other launches wait on that
/// lock for 30 seconds at most.
pub(super) fn create_profile(name: &str, password: &SecretWide) -> io::Result<()> {
    let token = logon(name, password)?;
    let mut name = wide(name);
    let mut profile = PROFILEINFOW {
        dwSize: u32::try_from(size_of::<PROFILEINFOW>())
            .map_err(|_| io::Error::other("invalid Windows profile structure size"))?,
        dwFlags: PI_NOUI,
        lpUserName: name.as_mut_ptr(),
        ..PROFILEINFOW::default()
    };
    // SAFETY: `token` is a live logon token, and `name` outlives this
    // synchronous call through the structure that points at it.
    if unsafe { LoadUserProfileW(token.get(), &raw mut profile) } == 0 {
        return Err(super::winutil::error("LoadUserProfileW"));
    }
    // SAFETY: the same token and the profile handle its load just returned.
    if unsafe { UnloadUserProfile(token.get(), profile.hProfile) } == 0 {
        return Err(super::winutil::error("UnloadUserProfile"));
    }
    Ok(())
}

fn logon(name: &str, password: &SecretWide) -> io::Result<OwnedHandle> {
    let name = wide(name);
    let domain = wide(".");
    let mut token = null_mut();
    // SAFETY: all three strings are live and NUL terminated and `token` is
    // initialized out-parameter storage.
    let logged_on = unsafe {
        LogonUserW(
            name.as_ptr(),
            domain.as_ptr(),
            password.as_ptr(),
            LOGON32_LOGON_INTERACTIVE,
            LOGON32_PROVIDER_DEFAULT,
            &raw mut token,
        )
    };
    if logged_on == 0 {
        return Err(super::winutil::error("LogonUserW"));
    }
    OwnedHandle::new(token, "LogonUserW token")
}

pub(super) fn disable(name: &str) -> io::Result<()> {
    let Some(flags) = read_flags(name)? else {
        return Ok(());
    };
    let name = wide(name);
    let flags = USER_INFO_1008 {
        usri1008_flags: flags | UF_ACCOUNTDISABLE,
    };
    // SAFETY: the account buffer and level-1008 structure remain live for the
    // duration of the synchronous NetAPI call.
    net_success("NetUserSetInfo(disable)", unsafe {
        NetUserSetInfo(
            null(),
            name.as_ptr(),
            1008,
            (&raw const flags).cast(),
            null_mut(),
        )
    })
}

pub(super) fn delete(name: &str) -> io::Result<()> {
    let name = wide(name);
    // SAFETY: `name` is a live NUL-terminated local account name.
    let status = unsafe { NetUserDel(null(), name.as_ptr()) };
    if status == NERR_Success || status == NERR_UserNotFound {
        Ok(())
    } else {
        Err(code_error("NetUserDel", status))
    }
}

fn read_flags(name: &str) -> io::Result<Option<u32>> {
    let name = wide(name);
    let mut buffer = null_mut::<u8>();
    // SAFETY: `name` is NUL terminated and `buffer` is initialized out-pointer
    // storage which NetAPI owns on success.
    let status = unsafe { NetUserGetInfo(null(), name.as_ptr(), 1, &raw mut buffer) };
    if status == NERR_UserNotFound {
        return Ok(None);
    }
    net_success("NetUserGetInfo", status)?;
    let buffer = NetBuffer::<USER_INFO_1>::new(buffer.cast())?;
    Ok(Some(buffer.get().usri1_flags))
}

struct NetBuffer<T>(NonNull<T>);

impl<T> NetBuffer<T> {
    fn new(pointer: *mut T) -> io::Result<Self> {
        NonNull::new(pointer).map(Self).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "NetUserGetInfo returned no account record",
            )
        })
    }

    fn get(&self) -> &T {
        // SAFETY: a successful NetUserGetInfo call returned a non-null,
        // correctly aligned allocation containing the requested level's
        // structure. This wrapper retains that allocation for this borrow.
        unsafe { self.0.as_ref() }
    }
}

impl<T> Drop for NetBuffer<T> {
    fn drop(&mut self) {
        // SAFETY: this is the one release of the NetAPI allocation accepted by
        // `new`; `T` is only a typed view over that allocation.
        unsafe {
            NetApiBufferFree(self.0.as_ptr().cast());
        }
    }
}

fn net_success(operation: &str, status: u32) -> io::Result<()> {
    if status == NERR_Success {
        Ok(())
    } else {
        Err(code_error(operation, status))
    }
}
