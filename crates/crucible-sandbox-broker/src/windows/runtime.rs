//! Two-hop native-Windows launch runtime.
//!
//! The owner-side broker validates machine setup and applies path ACLs before
//! logging on the dedicated account. The account-side broker creates the final
//! restricted token; credentials and identity never cross the wire.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, RawHandle};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, ERROR_NO_DATA, GetLastError, HANDLE,
    HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessWithLogonW, GetCurrentProcess, GetExitCodeProcess,
    LOGON_WITH_PROFILE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
    WaitForSingleObject,
};

use crate::{WindowsLaunchRequest, encode_windows_launch};

use super::plan::LaunchPlan;
use super::winutil::{OwnedHandle, error, wide};

const RELAY_POLL: Duration = Duration::from_millis(5);

pub(super) fn host(request: &WindowsLaunchRequest) -> io::Result<u32> {
    let candidate = super::current_launch_setup()?;
    let setup_lock = super::lock::SetupLock::acquire(&candidate.identity)?;
    drop(candidate);
    let setup = super::current_launch_setup()?;
    let plan = LaunchPlan::resolve(request)?;
    let broker = std::env::current_exe()?.canonicalize()?;
    if !plan.protects_broker(&broker) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows sandbox request does not protect its trusted broker",
        ));
    }
    super::acl::apply(plan.request(), &setup.record.account_sid)?;
    let mut desktop = super::desktop::PrivateDesktop::create(&setup.record.account_sid)?;

    let (child_input, host_input) = pipe()?;
    let process = logon_child(
        &broker,
        &setup.record.account_name,
        &setup.password,
        child_input.get(),
        &mut desktop,
    )?;
    drop(child_input);
    drop(setup_lock);

    let mut host_input = handle_file(host_input);
    encode_windows_launch(plan.request(), &mut host_input)?;
    host_input.flush()?;
    let relay_source = duplicate_standard_input()?;
    let finished = Arc::new(AtomicBool::new(false));
    let relay_finished = Arc::clone(&finished);
    let relay = thread::Builder::new()
        .name("crucible-windows-input-relay".into())
        .spawn(move || relay(relay_source, host_input, &relay_finished))?;

    // SAFETY: `process` owns the live process handle and no timeout is used;
    // the outer kill-on-close Job remains the cancellation authority.
    let waited = unsafe { WaitForSingleObject(process.get(), u32::MAX) };
    finished.store(true, Ordering::Release);
    let relay_result = relay
        .join()
        .map_err(|_| io::Error::other("Windows sandbox input relay panicked"))?;
    if waited == u32::MAX {
        return Err(error("WaitForSingleObject"));
    }
    relay_result?;
    let mut code = 0_u32;
    // SAFETY: the process handle remains live and `code` is initialized output
    // storage after the wait observed termination.
    if unsafe { GetExitCodeProcess(process.get(), &raw mut code) } == 0 {
        return Err(error("GetExitCodeProcess"));
    }
    Ok(code)
}

pub(super) fn child(request: &WindowsLaunchRequest) -> io::Result<u32> {
    let plan = LaunchPlan::from_host(request);
    let account_sid = super::winutil::current_user_sid()?;
    let mut desktop = super::desktop::PrivateDesktop::from_environment()?;
    let mut capabilities = plan.capability_sids(&account_sid);
    capabilities.push(desktop.capability_sid(&account_sid));
    let token = super::token::RestrictedToken::create(&mut capabilities, &account_sid)?;
    super::process::launch(&plan, &token, &mut desktop)
}

fn logon_child(
    broker: &std::path::Path,
    account: &str,
    password: &super::secret::SecretWide,
    input: HANDLE,
    desktop: &mut super::desktop::PrivateDesktop,
) -> io::Result<OwnedHandle> {
    let application = wide(broker.as_os_str());
    let mut command_line =
        command_line(&[broker.as_os_str(), OsStr::new(crate::WINDOWS_CHILD_MODE)]);
    let account = wide(account);
    let domain = wide(".");
    let current = broker.parent().map_or_else(|| wide("C:\\"), wide);
    let environment = environment(desktop)?;
    let mut startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>())
            .map_err(|_| io::Error::other("invalid Windows startup structure size"))?,
        dwFlags: STARTF_USESTDHANDLES,
        hStdInput: input,
        hStdOutput: standard_handle(STD_OUTPUT_HANDLE)?,
        hStdError: standard_handle(STD_ERROR_HANDLE)?,
        lpDesktop: desktop.startup_name(),
        ..STARTUPINFOW::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: all strings and the environment are live and NUL terminated,
    // the credential is retained by `setup` throughout this synchronous call,
    // and both output structures are initialized writable storage.
    let created = unsafe {
        CreateProcessWithLogonW(
            account.as_ptr(),
            domain.as_ptr(),
            password.as_ptr(),
            LOGON_WITH_PROFILE,
            application.as_ptr(),
            command_line.as_mut_ptr(),
            CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            current.as_ptr(),
            &raw mut startup,
            &raw mut process,
        )
    };
    if created == 0 {
        return Err(error("CreateProcessWithLogonW"));
    }
    let process_handle = OwnedHandle::new(process.hProcess, "CreateProcessWithLogonW process")?;
    let thread_handle = OwnedHandle::new(process.hThread, "CreateProcessWithLogonW thread")?;
    drop(thread_handle);
    Ok(process_handle)
}

fn environment(desktop: &super::desktop::PrivateDesktop) -> io::Result<Vec<u16>> {
    let (name, value) = desktop.environment();
    let mut environment = Vec::new();
    environment.extend(name.encode_utf16());
    environment.push(u16::from(b'='));
    environment.extend_from_slice(value);
    environment.extend_from_slice(&[0, 0]);
    if environment.len() > 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows sandbox desktop environment is too large",
        ));
    }
    Ok(environment)
}

fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
            .map_err(|_| io::Error::other("invalid Windows pipe attributes size"))?,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let mut reader = null_mut();
    let mut writer = null_mut();
    // SAFETY: both out handles and the attributes are initialized and live.
    if unsafe { CreatePipe(&raw mut reader, &raw mut writer, &raw const attributes, 0) } == 0 {
        return Err(error("CreatePipe"));
    }
    let reader = OwnedHandle::new(reader, "CreatePipe reader")?;
    let writer = OwnedHandle::new(writer, "CreatePipe writer")?;
    // SAFETY: the writer is live. Removing inheritance ensures the account-side
    // broker and target cannot retain the host's authority to signal EOF.
    if unsafe { SetHandleInformation(writer.get(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(error("SetHandleInformation"));
    }
    Ok((reader, writer))
}

fn duplicate_standard_input() -> io::Result<File> {
    let input = standard_handle(STD_INPUT_HANDLE)?;
    // SAFETY: the pseudo-handle is always valid in this process.
    let process = unsafe { GetCurrentProcess() };
    let mut duplicate = null_mut();
    // SAFETY: both process pseudo-handles are valid, input is a live standard
    // handle, and duplicate is initialized out-handle storage.
    if unsafe {
        DuplicateHandle(
            process,
            input,
            process,
            &raw mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(error("DuplicateHandle(stdin)"));
    }
    // SAFETY: the successful duplication transfers exactly one owned handle.
    Ok(unsafe { File::from_raw_handle(duplicate as RawHandle) })
}

fn relay(mut source: File, mut destination: File, finished: &AtomicBool) -> io::Result<()> {
    let mut buffer = [0_u8; 16 * 1024];
    while !finished.load(Ordering::Acquire) {
        let mut available = 0_u32;
        // SAFETY: the source handle is live and only the available-byte output
        // is requested. No data buffer is supplied to this non-consuming poll.
        if unsafe {
            PeekNamedPipe(
                source.as_raw_handle() as HANDLE,
                null_mut(),
                0,
                null_mut(),
                &raw mut available,
                null_mut(),
            )
        } == 0
        {
            // SAFETY: reads thread-local state immediately after the failed poll.
            let code = unsafe { GetLastError() };
            if code == ERROR_BROKEN_PIPE || code == ERROR_NO_DATA {
                return Ok(());
            }
            return Err(error("PeekNamedPipe(stdin)"));
        }
        if available == 0 {
            thread::sleep(RELAY_POLL);
            continue;
        }
        let amount = usize::try_from(available)
            .unwrap_or(buffer.len())
            .min(buffer.len());
        let target = buffer
            .get_mut(..amount)
            .ok_or_else(|| io::Error::other("invalid Windows relay read length"))?;
        let read = source.read(target)?;
        if read == 0 {
            return Ok(());
        }
        let bytes = buffer
            .get(..read)
            .ok_or_else(|| io::Error::other("invalid Windows relay write length"))?;
        if let Err(problem) = destination.write_all(bytes) {
            if matches!(problem.raw_os_error(), Some(code) if code == ERROR_BROKEN_PIPE.cast_signed() || code == ERROR_NO_DATA.cast_signed())
            {
                return Ok(());
            }
            return Err(problem);
        }
        destination.flush()?;
    }
    Ok(())
}

fn handle_file(handle: OwnedHandle) -> File {
    let raw = handle.into_raw();
    // SAFETY: ownership was removed from `handle` and is transferred once to File.
    unsafe { File::from_raw_handle(raw as RawHandle) }
}

fn standard_handle(kind: u32) -> io::Result<HANDLE> {
    // SAFETY: reads one process-global standard handle slot.
    let handle = unsafe { GetStdHandle(kind) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(error("GetStdHandle"))
    } else {
        Ok(handle)
    }
}

fn command_line(arguments: &[&OsStr]) -> Vec<u16> {
    let mut command = Vec::new();
    for argument in arguments {
        if !command.is_empty() {
            command.push(u16::from(b' '));
        }
        quote(argument, &mut command);
    }
    command.push(0);
    command
}

fn quote(argument: &OsStr, output: &mut Vec<u16>) {
    output.push(u16::from(b'"'));
    let mut slashes = 0_usize;
    for unit in argument.encode_wide() {
        if unit == u16::from(b'\\') {
            slashes = slashes.saturating_add(1);
            continue;
        }
        if unit == u16::from(b'"') {
            output.extend(std::iter::repeat_n(
                u16::from(b'\\'),
                slashes.saturating_mul(2).saturating_add(1),
            ));
        } else {
            output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes));
        }
        slashes = 0;
        output.push(unit);
    }
    output.extend(std::iter::repeat_n(
        u16::from(b'\\'),
        slashes.saturating_mul(2),
    ));
    output.push(u16::from(b'"'));
}
