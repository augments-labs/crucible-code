//! The runtime this crate's tests hand a hosted extension's streams to.
//!
//! One for every test, built the first time one asks and never shut down: a
//! test binary ends with its threads. Multi-thread, because a test speaks to an
//! extension synchronously from its own thread, which drives nothing; the
//! runtime's workers are what run the transport's tasks meanwhile.

use std::sync::LazyLock;

use tokio::runtime::{Builder, Handle, Runtime};

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime for the extension tests")
});

/// The runtime a test hands an extension's streams to.
pub(crate) fn runtime() -> Handle {
    RUNTIME.handle().clone()
}
