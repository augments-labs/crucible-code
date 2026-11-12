//! Workload-only seccomp policy for local listener creation.

use std::collections::BTreeMap;
use std::io;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule,
};

#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: i64 = 0x4000_0000;

/// The listener calls a closed network refuses, and the `io_uring` calls,
/// which would reach the same sockets without passing a filter on them.
const LISTENING: [i64; 2] = [libc::SYS_bind, libc::SYS_listen];
const IO_URING: [i64; 3] = [
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
];
/// The socket families a private loopback means: Unix sockets, which the
/// namespace and the read-only filesystem keep its own, IP, which reaches only
/// its own `lo`, and netlink, which answers for its own namespace.
const PRIVATE_FAMILIES: [libc::c_int; 4] = [
    libc::AF_UNIX,
    libc::AF_INET,
    libc::AF_INET6,
    libc::AF_NETLINK,
];

pub(super) fn compile(allow_local_binding: bool) -> io::Result<Option<BpfProgram>> {
    if allow_local_binding {
        return Ok(None);
    }
    refusing(LISTENING.into_iter().chain(IO_URING)).map(Some)
}

/// The filter for a closed network whose own private loopback the process may
/// listen on: `bind` and `listen` pass, the `io_uring` calls stay refused, and
/// `socket` and `socketpair` open only a family in [`PRIVATE_FAMILIES`].
///
/// Bubblewrap's `--unshare-net` is what keeps the loopback private, but only
/// for families scoped to the network namespace: a vsock listener, for one, is
/// reachable from the host and the hypervisor. Refusing the family at `socket`
/// and `socketpair`, the two calls that create a socket, also refuses it before
/// the kernel loads a module to answer for it.
pub(super) fn compile_private_loopback() -> io::Result<BpfProgram> {
    let mut rules = refusals(IO_URING);
    // `domain` is an int, so only the argument's low 32 bits are its family.
    let other_family = PRIVATE_FAMILIES
        .into_iter()
        .map(|family| {
            let family = u64::try_from(family).map_err(error)?;
            SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Ne, family)
                .map_err(error)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let other_family = SeccompRule::new(other_family).map_err(error)?;
    for creating in [libc::SYS_socket, libc::SYS_socketpair] {
        rules.insert(creating, vec![other_family.clone()]);
        #[cfg(target_arch = "x86_64")]
        rules.insert(creating + X32_SYSCALL_BIT, vec![other_family.clone()]);
    }
    compiled(rules)
}

/// Answers each of `syscalls`, and its x32 number on `x86_64`, with `EPERM`.
fn refusing(syscalls: impl IntoIterator<Item = i64> + Clone) -> io::Result<BpfProgram> {
    compiled(refusals(syscalls))
}

/// Each of `syscalls`, and its x32 number on `x86_64`, refused whatever its
/// arguments.
fn refusals(syscalls: impl IntoIterator<Item = i64> + Clone) -> BTreeMap<i64, Vec<SeccompRule>> {
    let mut rules = BTreeMap::new();
    for syscall in syscalls.clone() {
        rules.insert(syscall, Vec::new());
    }
    #[cfg(target_arch = "x86_64")]
    {
        for syscall in syscalls {
            rules.insert(syscall + X32_SYSCALL_BIT, Vec::new());
        }
    }
    rules
}

/// Compiles `rules`, each answered with `EPERM` where it matches, for this
/// architecture; every other call is allowed.
fn compiled(rules: BTreeMap<i64, Vec<SeccompRule>>) -> io::Result<BpfProgram> {
    if !cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        return Err(io::Error::other(
            "Linux sandbox network filtering requires x86_64 or aarch64",
        ));
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

    /// The `io_uring` calls, each with arguments a kernel answers with an
    /// error of its own (a bad descriptor, a null pointer) where no filter
    /// refuses it first, so `EPERM` can only be the filter's.
    fn io_uring_answers() -> [(&'static str, i64); 3] {
        let null = std::ptr::null_mut::<libc::c_void>();
        // SAFETY: each call passes a null pointer or an invalid descriptor and
        // no length, so the kernel reads and writes no memory of this process.
        unsafe {
            [
                (
                    "io_uring_setup",
                    libc::syscall(libc::SYS_io_uring_setup, 0, null),
                ),
                (
                    "io_uring_enter",
                    libc::syscall(libc::SYS_io_uring_enter, -1, 0, 0, 0, null, 0),
                ),
                (
                    "io_uring_register",
                    libc::syscall(libc::SYS_io_uring_register, -1, 0, null, 0),
                ),
            ]
        }
    }

    #[test]
    fn the_private_loopback_filter_lifts_bind_and_listen_and_nothing_else() {
        if std::env::var_os(CHILD).is_some() {
            let filter = compile_private_loopback().expect("compile");
            apply(&filter).expect("install");
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .expect("bind and listen on loopback");
            std::net::TcpStream::connect(listener.local_addr().expect("listener address"))
                .expect("connect to the listener");
            let path = std::env::temp_dir().join(format!(
                "crucible-private-loopback-{}.sock",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&path);
            let unix = std::os::unix::net::UnixListener::bind(&path);
            let _ = std::fs::remove_file(&path);
            unix.expect("bind and listen on a Unix socket");
            for (call, answer) in io_uring_answers() {
                let errno = std::io::Error::last_os_error().raw_os_error();
                assert_eq!((call, answer, errno), (call, -1, Some(libc::EPERM)));
            }
            std::fs::write(
                std::env::var_os(CHILD_MARKER).expect("child marker path"),
                b"executed",
            )
            .expect("child marker");
            return;
        }
        // The refusing filter still names bind and listen, and local binding
        // still has no filter at all.
        assert!(compile(true).expect("compile").is_none());
        let refusing = compile(false).expect("compile").expect("filter");
        let lifted = compile_private_loopback().expect("compile");
        let named = |filter: &BpfProgram, syscall: i64| {
            let syscall = u32::try_from(syscall).expect("a syscall number");
            filter.iter().any(|instruction| instruction.k == syscall)
        };
        let mut lifted_calls = vec![libc::SYS_bind, libc::SYS_listen];
        let mut kept_calls = vec![
            libc::SYS_io_uring_setup,
            libc::SYS_io_uring_enter,
            libc::SYS_io_uring_register,
        ];
        #[cfg(target_arch = "x86_64")]
        {
            lifted_calls.extend([
                libc::SYS_bind + X32_SYSCALL_BIT,
                libc::SYS_listen + X32_SYSCALL_BIT,
            ]);
            kept_calls.extend([
                libc::SYS_io_uring_setup + X32_SYSCALL_BIT,
                libc::SYS_io_uring_enter + X32_SYSCALL_BIT,
                libc::SYS_io_uring_register + X32_SYSCALL_BIT,
            ]);
        }
        for syscall in &lifted_calls {
            assert!(named(&refusing, *syscall), "{syscall}");
            assert!(!named(&lifted, *syscall), "{syscall}");
        }
        for syscall in &kept_calls {
            assert!(named(&refusing, *syscall), "{syscall}");
            assert!(named(&lifted, *syscall), "{syscall}");
        }

        let marker = std::env::temp_dir().join(format!(
            "crucible-private-loopback-child-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let _turn = crate::broker::child_turn::take();
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "broker::binding::tests::the_private_loopback_filter_lifts_bind_and_listen_and_nothing_else",
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

    /// A family built into the kernels this runs on, so opening one where no
    /// filter refuses it loads no module; `libc` does not name it.
    const AF_MCTP: libc::c_int = 45;

    /// Opens a socket of `family` with `kind`, through the x32 number on
    /// `x86_64` when `x32`, and answers with the call's result and `errno`.
    fn opened(family: libc::c_int, kind: libc::c_int, x32: bool) -> (i64, Option<i32>) {
        let mut number = libc::SYS_socket;
        #[cfg(target_arch = "x86_64")]
        if x32 {
            number += X32_SYSCALL_BIT;
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = x32;
        // SAFETY: socket takes three integers and reads or writes no memory of
        // this process; a descriptor it returns is closed before it is reused.
        let answer = unsafe { libc::syscall(number, family, kind, 0) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        if let Ok(descriptor) = libc::c_int::try_from(answer)
            && descriptor >= 0
        {
            // SAFETY: `descriptor` is the one this call just opened.
            unsafe { libc::close(descriptor) };
        }
        (answer, errno)
    }

    /// Makes a connected pair of `family` sockets with `kind`, through the x32
    /// number on `x86_64` when `x32`, and answers with the call's result and
    /// `errno`.
    fn paired(family: libc::c_int, kind: libc::c_int, x32: bool) -> (i64, Option<i32>) {
        let mut number = libc::SYS_socketpair;
        #[cfg(target_arch = "x86_64")]
        if x32 {
            number += X32_SYSCALL_BIT;
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = x32;
        let mut ends: [libc::c_int; 2] = [-1, -1];
        // SAFETY: socketpair writes at most two ints into `ends`, which lives
        // for the whole call; the descriptors it returns are closed below.
        let answer = unsafe { libc::syscall(number, family, kind, 0, ends.as_mut_ptr()) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        if answer == 0 {
            for end in ends {
                // SAFETY: `end` is one this call just opened.
                unsafe { libc::close(end) };
            }
        }
        (answer, errno)
    }

    #[test]
    fn the_private_loopback_filter_opens_only_local_ip_and_netlink_sockets() {
        if std::env::var_os(CHILD).is_some() {
            let filter = compile_private_loopback().expect("compile");
            apply(&filter).expect("install");
            for (family, kind) in [
                (libc::AF_UNIX, libc::SOCK_STREAM),
                (libc::AF_INET, libc::SOCK_STREAM),
                (libc::AF_INET6, libc::SOCK_STREAM),
                (libc::AF_NETLINK, libc::SOCK_RAW),
            ] {
                let (answer, errno) = opened(family, kind, false);
                assert!(answer >= 0, "family {family}: {answer}, errno {errno:?}");
            }
            assert_eq!(
                opened(AF_MCTP, libc::SOCK_DGRAM, false),
                (-1, Some(libc::EPERM))
            );
            #[cfg(target_arch = "x86_64")]
            assert_eq!(
                opened(AF_MCTP, libc::SOCK_DGRAM, true),
                (-1, Some(libc::EPERM))
            );
            let (answer, errno) = paired(libc::AF_UNIX, libc::SOCK_STREAM, false);
            assert_eq!(answer, 0, "Unix socketpair: errno {errno:?}");
            assert_eq!(
                paired(AF_MCTP, libc::SOCK_DGRAM, false),
                (-1, Some(libc::EPERM))
            );
            #[cfg(target_arch = "x86_64")]
            assert_eq!(
                paired(AF_MCTP, libc::SOCK_DGRAM, true),
                (-1, Some(libc::EPERM))
            );
            std::fs::write(
                std::env::var_os(CHILD_MARKER).expect("child marker path"),
                b"executed",
            )
            .expect("child marker");
            return;
        }
        let marker = std::env::temp_dir().join(format!(
            "crucible-private-loopback-families-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let _turn = crate::broker::child_turn::take();
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "broker::binding::tests::the_private_loopback_filter_opens_only_local_ip_and_netlink_sockets",
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
