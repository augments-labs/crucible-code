//! Exact-handle creation of the final restricted Windows process.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::WindowsLaunchRequest;

use super::desktop::PrivateDesktop;
use super::plan::LaunchPlan;
use super::token::RestrictedToken;
use super::winutil::{OwnedHandle, error};

pub(super) fn launch(
    plan: &LaunchPlan,
    token: &RestrictedToken,
    desktop: &mut PrivateDesktop,
) -> io::Result<u32> {
    let stdin = inheritable_standard_handle(STD_INPUT_HANDLE)?;
    let stdout = inheritable_standard_handle(STD_OUTPUT_HANDLE)?;
    let stderr = inheritable_standard_handle(STD_ERROR_HANDLE)?;
    let handles = [stdin.get(), stdout.get(), stderr.get()];
    let attributes = AttributeList::handles(&handles)?;
    let request = plan.request();
    let mut command_line = command_line(request);
    let mut application = request.program().to_vec();
    application.push(0);
    let mut current = request.working_directory().to_vec();
    current.push(0);
    let environment = environment(request)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>())
        .map_err(|_| io::Error::other("invalid extended startup structure size"))?;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin.get();
    startup.StartupInfo.hStdOutput = stdout.get();
    startup.StartupInfo.hStdError = stderr.get();
    startup.StartupInfo.lpDesktop = desktop.startup_name();
    startup.lpAttributeList = attributes.get();
    let mut process_info = PROCESS_INFORMATION::default();
    // SAFETY: all strings and the environment are live and NUL terminated;
    // the restricted primary token, desktop, allowlisted handles, attribute
    // list and writable process-information output remain live for the call.
    let created = unsafe {
        CreateProcessAsUserW(
            token.get(),
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            current.as_ptr(),
            (&raw const startup).cast(),
            &raw mut process_info,
        )
    };
    if created == 0 {
        return Err(error("CreateProcessAsUserW"));
    }
    let process = OwnedHandle::new(process_info.hProcess, "CreateProcessAsUserW process")?;
    let primary_thread = OwnedHandle::new(process_info.hThread, "CreateProcessAsUserW thread")?;
    // SAFETY: the returned primary thread was created with CREATE_SUSPENDED.
    if unsafe { ResumeThread(primary_thread.get()) } == u32::MAX {
        return Err(error("ResumeThread"));
    }
    // SAFETY: process remains live; the outer Job owns cancellation and this
    // account-side broker must relay the final status exactly.
    if unsafe { WaitForSingleObject(process.get(), u32::MAX) } == u32::MAX {
        return Err(error("WaitForSingleObject(target)"));
    }
    let mut code = 0_u32;
    // SAFETY: process is terminated and code is initialized output storage.
    if unsafe { GetExitCodeProcess(process.get(), &raw mut code) } == 0 {
        return Err(error("GetExitCodeProcess(target)"));
    }
    Ok(code)
}

fn inheritable_standard_handle(kind: u32) -> io::Result<OwnedHandle> {
    // SAFETY: reads the process-global standard handle slot.
    let source = unsafe { GetStdHandle(kind) };
    if source.is_null() || source == INVALID_HANDLE_VALUE {
        return Err(error("GetStdHandle(target)"));
    }
    // SAFETY: this process pseudo-handle is always valid.
    let process = unsafe { GetCurrentProcess() };
    let mut duplicate = null_mut();
    // SAFETY: the source is live, both pseudo-handles are valid, and duplicate
    // is initialized out-handle storage. Only these three copies are inheritable.
    if unsafe {
        DuplicateHandle(
            process,
            source,
            process,
            &raw mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(error("DuplicateHandle(target standard stream)"));
    }
    OwnedHandle::new(duplicate, "DuplicateHandle(target standard stream)")
}

fn command_line(request: &WindowsLaunchRequest) -> Vec<u16> {
    let mut command = Vec::new();
    for argument in
        std::iter::once(request.program()).chain(request.arguments().iter().map(Vec::as_slice))
    {
        if !command.is_empty() {
            command.push(u16::from(b' '));
        }
        quote(argument, &mut command);
    }
    command.push(0);
    command
}

fn quote(argument: &[u16], output: &mut Vec<u16>) {
    output.push(u16::from(b'"'));
    let mut slashes = 0_usize;
    for unit in argument {
        if *unit == u16::from(b'\\') {
            slashes = slashes.saturating_add(1);
            continue;
        }
        if *unit == u16::from(b'"') {
            output.extend(std::iter::repeat_n(
                u16::from(b'\\'),
                slashes.saturating_mul(2).saturating_add(1),
            ));
        } else {
            output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes));
        }
        slashes = 0;
        output.push(*unit);
    }
    output.extend(std::iter::repeat_n(
        u16::from(b'\\'),
        slashes.saturating_mul(2),
    ));
    output.push(u16::from(b'"'));
}

fn environment(request: &WindowsLaunchRequest) -> io::Result<Vec<u16>> {
    let mut block = Vec::new();
    for (name, value) in request.environment() {
        block.extend_from_slice(name);
        block.push(u16::from(b'='));
        block.extend_from_slice(value);
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    if block.len() > crate::MAX_WINDOWS_LAUNCH_BYTES / size_of::<u16>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows sandbox environment block exceeds its bound",
        ));
    }
    Ok(block)
}

struct AttributeList {
    storage: Vec<usize>,
}

impl AttributeList {
    fn handles(handles: &[HANDLE]) -> io::Result<Self> {
        let mut bytes = 0_usize;
        // SAFETY: a null list is the documented sizing call; bytes is writable.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &raw mut bytes);
        }
        if bytes == 0 {
            return Err(error("InitializeProcThreadAttributeList(size)"));
        }
        let words = bytes.div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let list = storage.as_mut_ptr().cast();
        // SAFETY: storage is aligned and writable for at least `bytes` and the
        // list requests exactly one attribute.
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &raw mut bytes) } == 0 {
            return Err(error("InitializeProcThreadAttributeList"));
        }
        let handle_bytes = handles
            .len()
            .checked_mul(size_of::<HANDLE>())
            .ok_or_else(|| io::Error::other("handle list size overflow"))?;
        // SAFETY: list is initialized; handles remains live and contains the
        // exact inheritable handles selected for the child.
        if unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                handle_bytes,
                null_mut(),
                null(),
            )
        } == 0
        {
            // SAFETY: list was successfully initialized above.
            unsafe {
                DeleteProcThreadAttributeList(list);
            }
            return Err(error("UpdateProcThreadAttribute(handle list)"));
        }
        Ok(Self { storage })
    }

    fn get(&self) -> *mut std::ffi::c_void {
        self.storage.as_ptr().cast_mut().cast()
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: construction succeeds only after this list is initialized.
        unsafe {
            DeleteProcThreadAttributeList(self.storage.as_mut_ptr().cast());
        }
    }
}
