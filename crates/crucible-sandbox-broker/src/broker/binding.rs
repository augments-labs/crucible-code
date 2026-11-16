//! Workload-only seccomp policy: local listeners, and attribute writes under a
//! disk ceiling.
//!
//! A run refused local binding cannot bind or listen. A run held to a disk
//! ceiling cannot set an extended attribute: tmpfs charges an attribute's value
//! to its inodes rather than to the size the root was mounted with, so a value
//! set on a file in the sized root would be held past the ceiling. `io_uring` is
//! refused under either, since it can bind and set an attribute without the
//! calls named here. A run that is refused neither gets no filter at all.

use std::collections::BTreeMap;
use std::io;

use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};

#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: i64 = 0x4000_0000;

/// `setxattrat(2)`, which this `libc` does not name. Calls added since Linux
/// 5.1 are numbered alike across architectures, and this one is 463 on both
/// that this filter is built for.
const SYS_SETXATTRAT: libc::c_long = 463;

/// What a workload's filter refuses.
#[derive(Clone, Copy, Default)]
pub(super) struct Refusals {
    /// Binding and listening, for a run refused local binding.
    pub(super) listeners: bool,
    /// Setting an extended attribute, for a run held to a disk ceiling.
    pub(super) attribute_writes: bool,
}

pub(super) fn compile(refusals: Refusals) -> io::Result<Option<BpfProgram>> {
    let mut refused = Vec::new();
    if refusals.listeners {
        refused.extend([libc::SYS_bind, libc::SYS_listen]);
    }
    if refusals.listeners || refusals.attribute_writes {
        refused.extend([
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ]);
    }
    if refusals.attribute_writes {
        refused.extend([
            libc::SYS_setxattr,
            libc::SYS_lsetxattr,
            libc::SYS_fsetxattr,
            SYS_SETXATTRAT,
        ]);
    }
    if refused.is_empty() {
        return Ok(None);
    }
    program(&refused).map(Some)
}

/// The program refusing each of `refused` with `EPERM`, and its x32 twin.
fn program(refused: &[libc::c_long]) -> io::Result<BpfProgram> {
    if !cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        return Err(io::Error::other(
            "Linux sandbox workload filtering requires x86_64 or aarch64",
        ));
    }
    let mut rules = BTreeMap::new();
    for &syscall in refused {
        rules.insert(syscall, Vec::new());
        #[cfg(target_arch = "x86_64")]
        rules.insert(syscall + X32_SYSCALL_BIT, Vec::new());
    }
    let arch = std::env::consts::ARCH.try_into().map_err(|source| {
        io::Error::other(format!("unsupported seccomp architecture: {source}"))
    })?;
    SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(error)?
    .try_into()
    .map_err(error)
}

pub(super) fn apply(filter: &[seccompiler::sock_filter]) -> io::Result<()> {
    // Keep the pre_exec failure conversion allocation-free: seccompiler's
    // installer itself issues only prctl and seccomp syscalls here.
    // This runs after fork: preserve the syscall error without allocating an
    // error string or taking an allocator lock in the workload child.
    seccompiler::apply_filter(filter).map_err(|error| match error {
        seccompiler::Error::Prctl(error) | seccompiler::Error::Seccomp(error) => error,
        _ => io::Error::from(io::ErrorKind::PermissionDenied),
    })
}

fn error(source: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("seccomp filter failure: {source}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const CHILD: &str = "CRUCIBLE_BINDING_CHILD";
    const CHILD_MARKER: &str = "CRUCIBLE_BINDING_CHILD_MARKER";

    #[test]
    fn child_filter_denies_tcp_and_unix_bind() {
        if std::env::var_os(CHILD).is_some() {
            let filter = compile(Refusals {
                listeners: true,
                attribute_writes: false,
            })
            .expect("compile")
            .expect("filter");
            apply(&filter).expect("install");
            assert!(std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).is_err());
            let path =
                std::env::temp_dir().join(format!("crucible-bind-{}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            assert!(std::os::unix::net::UnixListener::bind(&path).is_err());
            std::fs::write(
                std::env::var_os(CHILD_MARKER).expect("child marker path"),
                b"executed",
            )
            .expect("child marker");
            return;
        }
        let marker =
            std::env::temp_dir().join(format!("crucible-binding-child-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let _turn = crate::broker::child_turn::take();
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "broker::binding::tests::child_filter_denies_tcp_and_unix_bind",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(CHILD_MARKER, &marker)
            .spawn()
            .expect("spawn child");
        let status = child.wait().expect("wait for child");
        assert!(status.success(), "child status: {status}");
        assert_eq!(std::fs::read(&marker).expect("child marker"), b"executed");
        let _ = std::fs::remove_file(marker);
    }

    #[test]
    fn allow_binding_compiles_without_a_filter() {
        assert!(compile(Refusals::default()).expect("compile").is_none());
    }

    /// The constant each comparison in `program` tests the call number against.
    fn compared(program: &[seccompiler::sock_filter]) -> Vec<u32> {
        const JEQ: u16 = 0x15;
        program
            .iter()
            .filter(|instruction| instruction.code == JEQ)
            .map(|instruction| instruction.k)
            .collect()
    }

    fn numbered(syscall: libc::c_long) -> u32 {
        u32::try_from(syscall).expect("a call number")
    }

    #[test]
    fn a_filter_for_a_run_without_a_disk_ceiling_is_the_listener_filter_it_always_was() {
        // Written out rather than read back from `compile`: a run with no disk
        // ceiling keeps exactly the program it had before the ceiling refused
        // anything, and touches no extended-attribute call.
        let listeners = program(&[
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ])
        .expect("compile");
        let filter = compile(Refusals {
            listeners: true,
            attribute_writes: false,
        })
        .expect("compile")
        .expect("filter");
        assert_eq!(filter, listeners);
        for syscall in [
            libc::SYS_setxattr,
            libc::SYS_lsetxattr,
            libc::SYS_fsetxattr,
            SYS_SETXATTRAT,
        ] {
            assert!(!compared(&filter).contains(&numbered(syscall)), "{syscall}");
        }
    }

    #[test]
    fn a_disk_ceiling_filters_every_call_that_sets_an_attribute_even_where_binding_is_allowed() {
        // `io_uring` can set an attribute too, so it is refused under a ceiling
        // whether or not listeners are.
        let filter = compile(Refusals {
            listeners: false,
            attribute_writes: true,
        })
        .expect("compile")
        .expect("a filter although binding is allowed");
        let compared = compared(&filter);
        for syscall in [
            libc::SYS_setxattr,
            libc::SYS_lsetxattr,
            libc::SYS_fsetxattr,
            SYS_SETXATTRAT,
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ] {
            assert!(compared.contains(&numbered(syscall)), "{syscall}");
            #[cfg(target_arch = "x86_64")]
            assert!(
                compared.contains(&numbered(syscall + X32_SYSCALL_BIT)),
                "x32 {syscall}"
            );
        }
        for syscall in [libc::SYS_bind, libc::SYS_listen] {
            assert!(!compared.contains(&numbered(syscall)), "{syscall}");
        }
    }

    const ATTRIBUTE_CHILD: &str = "CRUCIBLE_ATTRIBUTE_CHILD";

    #[test]
    fn child_filter_refuses_every_attribute_write_and_still_lets_a_listener_bind() {
        #[repr(C)]
        struct Arguments {
            value: u64,
            size: u32,
            flags: u32,
        }
        if std::env::var_os(ATTRIBUTE_CHILD).is_some() {
            let path = std::env::var_os(ATTRIBUTE_CHILD).expect("a file to set attributes on");
            let path =
                std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()))
                    .expect("a path");
            let filter = compile(Refusals {
                listeners: false,
                attribute_writes: true,
            })
            .expect("compile")
            .expect("filter");
            apply(&filter).expect("install");
            let name = c"user.crucible";
            let value = [0_u8; 16];
            let file = std::fs::File::open(path.to_str().expect("utf8")).expect("open");
            let descriptor = std::os::fd::AsRawFd::as_raw_fd(&file);
            let arguments = Arguments {
                value: value.as_ptr() as u64,
                size: 16,
                flags: 0,
            };
            // SAFETY: each call reads only the live, NUL-terminated path and
            // name and the sixteen-byte value above, and writes nothing back.
            let results = unsafe {
                [
                    libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), 16, 0),
                    libc::lsetxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), 16, 0),
                    libc::fsetxattr(descriptor, name.as_ptr(), value.as_ptr().cast(), 16, 0),
                    libc::syscall(
                        SYS_SETXATTRAT,
                        libc::AT_FDCWD,
                        path.as_ptr(),
                        0,
                        name.as_ptr(),
                        &raw const arguments,
                        std::mem::size_of::<Arguments>(),
                    )
                    .try_into()
                    .expect("a status"),
                ]
            };
            for result in results {
                assert_eq!(result, -1);
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM)
                );
            }
            assert!(std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).is_ok());
            std::fs::write(
                std::env::var_os(CHILD_MARKER).expect("child marker path"),
                b"executed",
            )
            .expect("child marker");
            return;
        }
        let marker =
            std::env::temp_dir().join(format!("crucible-attribute-child-{}", std::process::id()));
        let target =
            std::env::temp_dir().join(format!("crucible-attribute-file-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        std::fs::write(&target, b"").expect("a file to set attributes on");
        let _turn = crate::broker::child_turn::take();
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "broker::binding::tests::child_filter_refuses_every_attribute_write_and_still_lets_a_listener_bind",
                "--nocapture",
            ])
            .env(ATTRIBUTE_CHILD, &target)
            .env(CHILD_MARKER, &marker)
            .status()
            .expect("run child");
        assert!(status.success(), "child status: {status}");
        assert_eq!(std::fs::read(&marker).expect("child marker"), b"executed");
        let _ = std::fs::remove_file(marker);
        let _ = std::fs::remove_file(target);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x32_listener_syscalls_are_present_in_the_compiled_filter() {
        let filter = compile(Refusals {
            listeners: true,
            attribute_writes: false,
        })
        .expect("compile")
        .expect("filter");
        let bind = u32::try_from(libc::SYS_bind + X32_SYSCALL_BIT).expect("x32 syscall");
        let listen = u32::try_from(libc::SYS_listen + X32_SYSCALL_BIT).expect("x32 syscall");
        assert!(filter.iter().any(|instruction| instruction.k == bind));
        assert!(filter.iter().any(|instruction| instruction.k == listen));
    }
}
