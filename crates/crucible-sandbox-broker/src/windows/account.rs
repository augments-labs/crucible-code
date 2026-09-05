//! Lifecycle of the dedicated local low-privilege sandbox account.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::NetworkManagement::NetManagement::{
    NERR_Success, NERR_UserExists, NERR_UserNotFound, NetApiBufferFree, NetUserAdd, NetUserDel,
    NetUserGetInfo, NetUserSetInfo, UF_ACCOUNTDISABLE, UF_DONT_EXPIRE_PASSWD, UF_NORMAL_ACCOUNT,
    UF_NOT_DELEGATED, UF_SCRIPT, USER_INFO_1, USER_INFO_1003, USER_INFO_1008, USER_PRIV_USER,
};
use windows_sys::Win32::Security::{
    LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, LogonUserW,
};

use super::secret::SecretWide;
use super::winutil::{account_sid, code_error, wide};

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
    // SAFETY: successful `LogonUserW` returned ownership of this token handle.
    unsafe {
        CloseHandle(token);
    }
    Ok(())
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

#[allow(
    clippy::cast_ptr_alignment,
    reason = "NetUserGetInfo allocates and aligns the level-1 structure even though its ABI exposes a byte pointer"
)]
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
    if buffer.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NetUserGetInfo returned no account record",
        ));
    }
    // SAFETY: a successful level-1 query returns an aligned `USER_INFO_1` in
    // the NetAPI allocation, which remains live until the free below.
    let flags = unsafe { (*buffer.cast::<USER_INFO_1>()).usri1_flags };
    // SAFETY: `buffer` is the allocation returned by `NetUserGetInfo`.
    unsafe {
        NetApiBufferFree(buffer.cast());
    }
    Ok(Some(flags))
}

fn net_success(operation: &str, status: u32) -> io::Result<()> {
    if status == NERR_Success {
        Ok(())
    } else {
        Err(code_error(operation, status))
    }
}
