//! Persistent path ACL capabilities consumed only by restricted launch tokens.

use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetSecurityInfo, REVOKE_ACCESS, SE_FILE_OBJECT,
    SET_ACCESS, SetEntriesInAclW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
    WRITE_OWNER,
};

use crate::WindowsLaunchRequest;

use super::plan::{Access, capability_sid};
use super::winutil::{LocalMemory, code_error, error, wide};

const READ: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
const WRITE: u32 = READ | FILE_GENERIC_WRITE;
const WRITE_DESCENDANT: u32 = WRITE | DELETE;
const DENY_WRITE: u32 = FILE_GENERIC_WRITE | DELETE | FILE_DELETE_CHILD | WRITE_DAC | WRITE_OWNER;
const DESCENDANTS: u32 = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE;
const SELF_AND_DESCENDANTS: u32 = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;

pub(super) fn apply(request: &WindowsLaunchRequest, account_sid: &[u8]) -> io::Result<()> {
    for root in request.readable_roots() {
        grant(
            &path(root),
            account_sid,
            Access::Read,
            root,
            Rights {
                direct: READ,
                descendants: READ,
            },
        )?;
    }
    for root in request.writable_roots() {
        grant(
            &path(root),
            account_sid,
            Access::Write,
            root,
            Rights {
                direct: WRITE,
                descendants: WRITE_DESCENDANT,
            },
        )?;
    }
    for root in request.protected_roots() {
        deny(
            &path(root),
            &capability_sid(account_sid, Access::DenyWrite, root),
            DENY_WRITE,
        )?;
    }
    Ok(())
}

fn grant(
    path: &Path,
    account_sid: &[u8],
    access: Access,
    encoded_path: &[u16],
    rights: Rights,
) -> io::Result<()> {
    let capability = capability_sid(account_sid, access, encoded_path);
    edit(path, |old| {
        // The dedicated account satisfies Windows' ordinary access check and
        // the request capability satisfies its restricted-SID check. Account
        // grants only accumulate: reducing one while another command uses the
        // same path would revoke authority from that live command. The token
        // also retains the account as a restricted identity because ordinary
        // developer runtimes require it, so callers disclose that a root used
        // by an earlier command remains reachable through that account grant.
        let revoked = set_entries(&[entry(&capability, 0, REVOKE_ACCESS, 0)], old)?;
        let direct = set_entries(
            &[
                entry(account_sid, rights.direct, GRANT_ACCESS, 0),
                entry(&capability, rights.direct, SET_ACCESS, 0),
            ],
            revoked.as_acl(),
        )?;
        set_entries(
            &[
                entry(account_sid, rights.descendants, GRANT_ACCESS, DESCENDANTS),
                entry(&capability, rights.descendants, GRANT_ACCESS, DESCENDANTS),
            ],
            direct.as_acl(),
        )
    })
}

#[derive(Clone, Copy)]
struct Rights {
    direct: u32,
    descendants: u32,
}

fn deny(path: &Path, denial_sid: &[u8], mask: u32) -> io::Result<()> {
    edit(path, |old| {
        let revoked = set_entries(&[entry(denial_sid, 0, REVOKE_ACCESS, 0)], old)?;
        set_entries(
            &[entry(denial_sid, mask, DENY_ACCESS, SELF_AND_DESCENDANTS)],
            revoked.as_acl(),
        )
    })
}

fn edit(
    path: &Path,
    build: impl FnOnce(*mut windows_sys::Win32::Security::ACL) -> io::Result<AclMemory>,
) -> io::Result<()> {
    let handle = open(path)?;
    let mut old = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: the owned handle names the already-canonical non-reparse object;
    // all unused out pointers are null and WFP-style LocalAlloc ownership of
    // the returned descriptor is retained below.
    let status = unsafe {
        GetSecurityInfo(
            handle.get(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &raw mut old,
            null_mut(),
            &raw mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(code_error("GetSecurityInfo", status));
    }
    let _descriptor = LocalMemory(descriptor);
    let replacement = build(old)?;
    // SAFETY: the handle remains live and `replacement` owns the complete ACL
    // for the duration of this synchronous update.
    let status = unsafe {
        SetSecurityInfo(
            handle.get(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            replacement.as_acl(),
            null(),
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(code_error("SetSecurityInfo", status))
    }
}

fn set_entries(
    entries: &[EXPLICIT_ACCESS_W],
    old: *mut windows_sys::Win32::Security::ACL,
) -> io::Result<AclMemory> {
    let mut replacement = null_mut();
    let count = u32::try_from(entries.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many ACL entries"))?;
    // SAFETY: every trustee points into a SID slice retained by the caller,
    // `old` belongs to the live security descriptor, and replacement is an
    // initialized out pointer transferred to LocalFree on success.
    let status = unsafe { SetEntriesInAclW(count, entries.as_ptr(), old, &raw mut replacement) };
    if status != ERROR_SUCCESS {
        return Err(code_error("SetEntriesInAclW", status));
    }
    AclMemory::new(replacement)
}

fn entry(sid: &[u8], mask: u32, mode: i32, inheritance: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.as_ptr().cast_mut().cast(),
        },
    }
}

fn open(path: &Path) -> io::Result<FileHandle> {
    let path = wide(path.as_os_str());
    // SAFETY: the path is NUL terminated; the flags open the named object
    // itself rather than traversing a final reparse point.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            READ_CONTROL | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        Err(error("CreateFileW(ACL)"))
    } else {
        Ok(FileHandle(handle))
    }
}

fn path(units: &[u16]) -> PathBuf {
    PathBuf::from(OsString::from_wide(units))
}

struct FileHandle(HANDLE);

impl FileHandle {
    const fn get(&self) -> HANDLE {
        self.0
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the successful CreateFileW handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

struct AclMemory(LocalMemory);

impl AclMemory {
    fn new(pointer: *mut windows_sys::Win32::Security::ACL) -> io::Result<Self> {
        if pointer.is_null() {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SetEntriesInAclW returned no ACL",
            ))
        } else {
            Ok(Self(LocalMemory(pointer.cast::<c_void>())))
        }
    }

    fn as_acl(&self) -> *mut windows_sys::Win32::Security::ACL {
        self.0.0.cast()
    }
}
