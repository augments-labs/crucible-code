//! The runtime this crate's tests hand the transport's tasks.
//!
//! One for every test, built the first time one asks and never shut down: a
//! test binary ends with its threads. Multi-thread, because the test's own
//! thread waits the way a synchronous host does, and a waiting thread drives
//! nothing; the runtime's workers are what run the tasks meanwhile.

use std::sync::LazyLock;

use tokio::runtime::{Builder, Handle, Runtime};

/// What the runtime's threads are called, so a test can tell work done on one
/// of them from work done on a thread of anybody else's.
const THREADS: &str = "transport-test-runtime";

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name(THREADS)
        .enable_all()
        .build()
        .expect("a runtime for the transport's tests")
});

/// The runtime a test hands the transport.
pub(crate) fn runtime() -> &'static Handle {
    RUNTIME.handle()
}

/// Whether the calling thread is one of that runtime's own.
pub(crate) fn on_runtime() -> bool {
    std::thread::current().name() == Some(THREADS)
}
