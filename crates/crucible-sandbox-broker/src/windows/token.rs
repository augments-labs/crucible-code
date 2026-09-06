//! Restricted primary token for the dedicated sandbox account.

use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
    TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE,
    GetTokenInformation, LUA_TOKEN, LookupPrivilegeValueW, SE_CHANGE_NOTIFY_NAME,
    SE_PRIVILEGE_ENABLED, SID_AND_ATTRIBUTES, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DEFAULT_DACL,
    TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenDefaultDacl, TokenGroups,
    WRITE_RESTRICTED, WinWorldSid,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::winutil::{LocalMemory, OwnedHandle, copy_sid, error};

const SE_GROUP_LOGON_ID: u32 = 0xc000_0000;
const GENERIC_ALL: u32 = 0x1000_0000;

pub(super) struct RestrictedToken {
    handle: OwnedHandle,
}

impl RestrictedToken {
    pub(super) fn create(capabilities: &mut [Vec<u8>], account_sid: &[u8]) -> io::Result<Self> {
        if capabilities.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows sandbox token has no path capabilities",
            ));
        }
        let base = primary_token()?;
        let mut logon_sid = logon_sid(base.get())?;
        let mut world_sid = world_sid()?;
        let mut restricting: Vec<SID_AND_ATTRIBUTES> = capabilities
            .iter_mut()
            .map(|sid| SID_AND_ATTRIBUTES {
                Sid: sid.as_mut_ptr().cast(),
                Attributes: 0,
            })
            .collect();
        restricting.push(SID_AND_ATTRIBUTES {
            Sid: logon_sid.as_mut_ptr().cast(),
            Attributes: 0,
        });
        // The account SID is an identity marker for Windows runtime objects,
        // not a path capability. It remains absent from the default DACL so a
        // newly created object cannot turn identity into ambient authority.
        restricting.push(SID_AND_ATTRIBUTES {
            Sid: account_sid.as_ptr().cast_mut().cast(),
            Attributes: 0,
        });
        // Windows runtime objects commonly grant their baseline access to
        // Everyone. WRITE_RESTRICTED still applies the dedicated account's
        // ordinary access check first, so this cannot grant authority that the
        // low-privilege account does not already hold.
        restricting.push(SID_AND_ATTRIBUTES {
            Sid: world_sid.as_mut_ptr().cast(),
            Attributes: 0,
        });
        let mut restricted = null_mut();
        let count = u32::try_from(restricting.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "too many token capabilities")
        })?;
        // SAFETY: the base token is live; every SID_AND_ATTRIBUTES points into
        // a retained valid SID buffer; all omitted lists have zero counts; and
        // `restricted` is initialized out-handle storage.
        if unsafe {
            CreateRestrictedToken(
                base.get(),
                DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED,
                0,
                null(),
                0,
                null(),
                count,
                restricting.as_ptr(),
                &raw mut restricted,
            )
        } == 0
        {
            return Err(error("CreateRestrictedToken"));
        }
        let handle = OwnedHandle::new(restricted, "CreateRestrictedToken")?;
        set_default_dacl(handle.get(), &logon_sid, &world_sid, capabilities)?;
        enable_traverse(handle.get())?;
        Ok(Self { handle })
    }

    pub(super) fn get(&self) -> HANDLE {
        self.handle.get()
    }
}

fn primary_token() -> io::Result<OwnedHandle> {
    let mut token = null_mut();
    let access = TOKEN_DUPLICATE
        | TOKEN_QUERY
        | TOKEN_ASSIGN_PRIMARY
        | TOKEN_ADJUST_DEFAULT
        | TOKEN_ADJUST_PRIVILEGES
        | TOKEN_ADJUST_SESSIONID;
    // SAFETY: the process pseudo-handle is valid and token is initialized
    // out-handle storage receiving one owned primary-token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &raw mut token) } == 0 {
        return Err(error("OpenProcessToken(restricted)"));
    }
    OwnedHandle::new(token, "OpenProcessToken(restricted)")
}

fn enable_traverse(token: HANDLE) -> io::Result<()> {
    let mut privileges = TOKEN_PRIVILEGES::default();
    // SAFETY: the well-known privilege name is NUL terminated and the LUID
    // destination is writable for the synchronous lookup.
    if unsafe {
        LookupPrivilegeValueW(
            null(),
            SE_CHANGE_NOTIFY_NAME,
            &raw mut privileges.Privileges[0].Luid,
        )
    } == 0
    {
        return Err(error("LookupPrivilegeValueW(SeChangeNotifyPrivilege)"));
    }
    privileges.PrivilegeCount = 1;
    privileges.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
    // SAFETY: the restricted token is live and adjustable, and `privileges`
    // contains one initialized entry retained for the synchronous call.
    if unsafe { AdjustTokenPrivileges(token, 0, &raw const privileges, 0, null_mut(), null_mut()) }
        == 0
    {
        return Err(error("AdjustTokenPrivileges(SeChangeNotifyPrivilege)"));
    }
    // SAFETY: reads the thread-local result immediately after the successful
    // adjustment call; this is the documented partial-success condition.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(error("AdjustTokenPrivileges(SeChangeNotifyPrivilege)"));
    }
    Ok(())
}

fn logon_sid(token: HANDLE) -> io::Result<Vec<u8>> {
    let mut needed = 0_u32;
    // SAFETY: null output requests the required size for the live token.
    unsafe {
        GetTokenInformation(token, TokenGroups, null_mut(), 0, &raw mut needed);
    }
    // SAFETY: reads the sizing call's thread-local error immediately.
    if needed == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(error("GetTokenInformation(TokenGroups size)"));
    }
    let words = (needed as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0_usize; words];
    // SAFETY: the aligned buffer is writable for `needed` bytes.
    if unsafe {
        GetTokenInformation(
            token,
            TokenGroups,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    } == 0
    {
        return Err(error("GetTokenInformation(TokenGroups)"));
    }
    // SAFETY: a successful query initialized the group count followed by its
    // aligned SID_AND_ATTRIBUTES array inside the retained buffer.
    let groups = unsafe {
        &*buffer
            .as_ptr()
            .cast::<windows_sys::Win32::Security::TOKEN_GROUPS>()
    };
    let count = usize::try_from(groups.GroupCount)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid token group count"))?;
    let base = groups.Groups.as_ptr();
    for index in 0..count {
        // SAFETY: TokenGroups guarantees `GroupCount` initialized entries.
        let group = unsafe { &*base.add(index) };
        if group.Attributes & SE_GROUP_LOGON_ID == SE_GROUP_LOGON_ID {
            return copy_sid(group.Sid);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Windows sandbox account token has no logon SID",
    ))
}

fn world_sid() -> io::Result<Vec<u8>> {
    let mut needed = 0_u32;
    // SAFETY: null output requests the documented SID size.
    unsafe {
        CreateWellKnownSid(WinWorldSid, null_mut(), null_mut(), &raw mut needed);
    }
    if needed == 0 {
        return Err(error("CreateWellKnownSid(Everyone size)"));
    }
    let mut sid = vec![0_u8; needed as usize];
    // SAFETY: sid is writable for the size returned above and the domain SID
    // is not used for the machine-independent Everyone identity.
    if unsafe {
        CreateWellKnownSid(
            WinWorldSid,
            null_mut(),
            sid.as_mut_ptr().cast(),
            &raw mut needed,
        )
    } == 0
    {
        return Err(error("CreateWellKnownSid(Everyone)"));
    }
    sid.truncate(needed as usize);
    Ok(sid)
}

fn set_default_dacl(
    token: HANDLE,
    logon_sid: &[u8],
    world_sid: &[u8],
    capabilities: &[Vec<u8>],
) -> io::Result<()> {
    let mut entries = Vec::with_capacity(capabilities.len().saturating_add(2));
    entries.push(allow(logon_sid));
    entries.push(allow(world_sid));
    entries.extend(capabilities.iter().map(|sid| allow(sid)));
    let mut acl = null_mut();
    let count = u32::try_from(entries.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many default ACL entries"))?;
    // SAFETY: every trustee points into a retained SID slice; a null old ACL
    // starts a new canonical ACL and `acl` receives LocalAlloc ownership.
    let status = unsafe { SetEntriesInAclW(count, entries.as_ptr(), null_mut(), &raw mut acl) };
    if status != 0 {
        return Err(super::winutil::code_error(
            "SetEntriesInAclW(default)",
            status,
        ));
    }
    let acl = LocalMemory(acl.cast::<c_void>());
    let mut info = TOKEN_DEFAULT_DACL {
        DefaultDacl: acl.0.cast(),
    };
    let length = u32::try_from(size_of::<TOKEN_DEFAULT_DACL>())
        .map_err(|_| io::Error::other("invalid token default DACL size"))?;
    // SAFETY: the token is live and adjustable; info and the ACL it references
    // remain live for the synchronous copy performed by SetTokenInformation.
    if unsafe { SetTokenInformation(token, TokenDefaultDacl, (&raw mut info).cast(), length) } == 0
    {
        return Err(error("SetTokenInformation(TokenDefaultDacl)"));
    }
    Ok(())
}

fn allow(sid: &[u8]) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.as_ptr().cast_mut().cast(),
        },
    }
}
