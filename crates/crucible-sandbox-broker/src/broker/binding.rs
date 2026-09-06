//! Workload-only seccomp policy for local listener creation.

use std::collections::BTreeMap;
use std::io;

use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};

#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: i64 = 0x4000_0000;

pub(super) fn compile(allow_local_binding: bool) -> io::Result<Option<BpfProgram>> {
    if allow_local_binding {
        return Ok(None);
    }
    if !cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        return Err(io::Error::other(
            "Linux sandbox network filtering requires x86_64 or aarch64",
        ));
    }
    let mut rules = BTreeMap::new();
    for syscall in [
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ] {
        rules.insert(syscall, Vec::new());
    }
    #[cfg(target_arch = "x86_64")]
    {
        for syscall in [
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ] {
            rules.insert(syscall + X32_SYSCALL_BIT, Vec::new());
        }
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
    .map(Some)
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
            let filter = compile(false).expect("compile").expect("filter");
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
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "broker::binding::tests::child_filter_denies_tcp_and_unix_bind",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(CHILD_MARKER, &marker)
            .status()
            .expect("spawn child");
        assert!(status.success(), "child status: {status}");
        assert_eq!(std::fs::read(&marker).expect("child marker"), b"executed");
        let _ = std::fs::remove_file(marker);
    }

    #[test]
    fn allow_binding_compiles_without_a_filter() {
        assert!(compile(true).expect("compile").is_none());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x32_listener_syscalls_are_present_in_the_compiled_filter() {
        let filter = compile(false).expect("compile").expect("filter");
        let bind = u32::try_from(libc::SYS_bind + X32_SYSCALL_BIT).expect("x32 syscall");
        let listen = u32::try_from(libc::SYS_listen + X32_SYSCALL_BIT).expect("x32 syscall");
        assert!(filter.iter().any(|instruction| instruction.k == bind));
        assert!(filter.iter().any(|instruction| instruction.k == listen));
    }
}
