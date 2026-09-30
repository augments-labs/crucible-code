//! How much building a Kimi request and reading a Kimi response allocates,
//! held to what the code allocated before its wire was shared with any other
//! vendor.
//!
//! Over the same conversations and responses as Kimi's differential fixtures,
//! and through nothing but what this crate makes public. The expected counts in
//! `src/moonshot/fixtures/allocations.txt` were written by the revision this
//! test arrived in; a case may allocate fewer times or fewer bytes than it
//! says, never more.
//!
//! No probe of `scripts/sh/bench.sh` builds a provider request or reads a
//! provider stream, which is why this is counted here.
//!
//! Counted on Linux `x86_64` alone, which is where the expected counts were
//! written. On macOS each case, a refused request included, makes one
//! allocation more than these counts: the same one in every case, which a
//! change to one path would not be, so it is the platform's cost on a path
//! every case shares. Held to another platform's numbers, the count measures
//! the platform rather than the change.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Whether this thread's allocations are being counted. Constant
    /// initialised and without a destructor, so reading it allocates nothing.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    /// How many allocations this thread made while counting, and their bytes.
    static COUNTED: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// The system allocator, telling this thread how often it is asked.
struct Counting;

/// Adds one allocation of `size` bytes to this thread's count, if it counts.
fn tally(size: usize) {
    let _ = COUNTING.try_with(|counting| {
        if counting.get() {
            let _ = COUNTED.try_with(|counted| {
                let (times, bytes) = counted.get();
                counted.set((times + 1, bytes + size as u64));
            });
        }
    });
}

// Counting what an allocation costs takes being the allocator, and being the
// allocator is `unsafe` whatever it does: `GlobalAlloc` is an unsafe trait with
// unsafe methods. The workspace denies `unsafe_code` so that each exception is
// written where it applies; this is one, in a test binary alone, and every
// method hands the same arguments to the system allocator unchanged.
#[allow(
    unsafe_code,
    reason = "a counting global allocator in a test binary; each method forwards to `System`"
)]
// SAFETY: every method forwards its arguments to `System` unchanged, so each
// upholds the contract its caller relied on; the tally beside it allocates
// nothing and never unwinds.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tally(layout.size());
        // SAFETY: the caller's contract for `alloc`, passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tally(layout.size());
        // SAFETY: the caller's contract for `alloc_zeroed`, passed on unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tally(new_size);
        // SAFETY: the caller's contract for `realloc`, passed on unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract for `dealloc`, passed on unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The cases, the transport they are answered over, and the count.
#[cfg(test)]
mod kimi {
    use std::cell::Cell;
    use std::io::Cursor;
    use std::path::Path;

    use crucible_credentials::{ApiKey, Header, HeaderKey, Outgoing};
    use crucible_models::Provider;
    use crucible_provider::{Moonshot, PostResponse, Transport, TransportError};
    use crucible_runtime::{BoxFuture, Cancel};

    use super::{COUNTED, COUNTING};

    include!("../src/moonshot/fixtures/conversations.rs");

    /// What `work` allocated on this thread: how many times, and how many bytes.
    fn counted(work: impl FnOnce()) -> (u64, u64) {
        COUNTED.with(|counted| counted.set((0, 0)));
        COUNTING.with(|counting| counting.set(true));
        work();
        COUNTING.with(|counting| counting.set(false));
        COUNTED.with(Cell::get)
    }

    /// Answers every request with one recorded response.
    #[derive(Debug)]
    struct Recorded {
        status: u16,
        body: &'static str,
    }

    impl Transport for Recorded {
        fn post<'a>(
            &'a self,
            _url: &'a str,
            _headers: &'a mut Outgoing,
            _body: String,
            _cancel: &'a Cancel,
        ) -> BoxFuture<'a, Result<PostResponse, TransportError>> {
            let response = PostResponse::recorded(self.status, Cursor::new(self.body.as_bytes()));
            Box::pin(async move { Ok(response) })
        }
    }

    /// Kimi at its coding address, answering with `status` and `body`.
    fn kimi(status: u16, body: &'static str) -> Moonshot {
        Moonshot::at(
            Moonshot::CODING,
            Box::new(HeaderKey::new(
                ApiKey::new("fabricated-kimi-key"),
                Header::bearer(),
            )),
            Box::new(Recorded { status, body }),
        )
    }

    /// Asks `provider` with `request` and reads whatever it answers, to the end.
    fn ask(provider: &Moonshot, request: crucible_models::Request<'static>) {
        let cancel = Cancel::new();
        if let Ok(mut stream) = crucible_runtime::answered!(provider.stream(request, &cancel)) {
            while crucible_runtime::answered!(stream.next()).is_some() {}
        }
    }

    /// Every case this counts, by the name its count is kept under, with what it
    /// allocated.
    fn every_count() -> Vec<(String, (u64, u64))> {
        let answer = streams()
            .into_iter()
            .find(|(name, ..)| *name == "text")
            .map(|(_, _, body)| body)
            .expect("the plain answer is a fixture");
        let mut counts = Vec::new();
        for (name, request) in conversations() {
            let provider = kimi(200, answer);
            counts.push((
                format!("request {name}"),
                counted(|| ask(&provider, request)),
            ));
        }
        for (name, status, body) in streams() {
            let provider = kimi(status, body);
            let request = conversations()
                .into_iter()
                .next()
                .map(|(_, request)| request)
                .expect("a conversation to ask with");
            counts.push((
                format!("response {name}"),
                counted(|| ask(&provider, request)),
            ));
        }
        counts
    }

    #[test]
    fn building_kimi_requests_and_reading_its_responses_allocates_no_more_than_before() {
        // Once to settle whatever is made the first time it is used, then counted.
        every_count();
        let counts = every_count();

        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/moonshot/fixtures/allocations.txt");
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|problem| panic!("{}: {problem}", path.display()));
        let mut over = Vec::new();
        for (name, (times, bytes)) in &counts {
            let line = expected
                .lines()
                .find(|line| line.starts_with(&format!("{name}: ")))
                .unwrap_or_else(|| panic!("{name} has no expected count"));
            let mut numbers = line
                .rsplit(": ")
                .next()
                .unwrap_or_default()
                .split(' ')
                .map(|number| number.parse::<u64>().expect("a count"));
            let (was_times, was_bytes) = (
                numbers.next().expect("allocations"),
                numbers.next().expect("bytes"),
            );
            if *times > was_times || *bytes > was_bytes {
                over.push(format!(
                    "{name}: {times} allocations of {bytes} bytes, was {was_times} of {was_bytes}"
                ));
            }
        }
        assert!(
            over.is_empty(),
            "allocates more than before:\n{}",
            over.join("\n")
        );
        assert_eq!(
            counts.len(),
            expected.lines().count(),
            "a counted case was added or taken away"
        );
    }
}
