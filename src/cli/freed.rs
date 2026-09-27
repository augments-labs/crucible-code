//! Where a large block goes once it is freed.
//!
//! A turn is a task on the application's runtime and moves between its worker
//! threads from one await to the next, so what it allocates is spread over
//! several of glibc's per-thread arenas. glibc maps a block above a threshold
//! on its own and unmaps it when freed, but by default raises that threshold
//! each time such a block is freed, up to 32 MiB. Once it has risen, the
//! blocks a session carrying pictures holds are carved out of those arenas'
//! heaps instead, and a heap keeps what is freed in it: resident memory for
//! a session at its picture ceiling roughly tripled when turns began moving
//! between workers.
//!
//! So on glibc the threshold is fixed at its starting value before anything
//! else runs, which also stops it being raised: a block that large is mapped
//! on its own and handed back when freed, whichever thread frees it. Other
//! allocators have no such threshold, and nothing is done for them.

/// The size from which a block is mapped on its own and unmapped when freed:
/// glibc's starting value, kept from being raised.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const MAPPED_FROM: libc::c_int = 128 * 1024;

/// Fixes the size from which a freed block is handed back to the system.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(
    unsafe_code,
    reason = "glibc's allocator is tuned through mallopt, a C call with no safe wrapper"
)]
pub(super) fn handed_back() {
    // SAFETY: mallopt reads two integers and changes only the allocator's own
    // parameters, under the allocator's lock; M_MMAP_THRESHOLD with a value
    // below its 32 MiB maximum is a documented pair. It answers 0 only for a
    // value out of range, and the allocator then goes on as it was.
    let _ = unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, MAPPED_FROM) };
}

/// Nothing to fix where the allocator is not glibc's.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(super) fn handed_back() {}
