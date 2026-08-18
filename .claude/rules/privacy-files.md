---
paths:
  - "crates/crucible-privacy/**"
---

# Changing crucible-privacy

This crate is a lower platform boundary. It knows directories, files and the
current operating-system account; it must never learn what a credential or a
session is. Both consumers depend downward on it so Unix modes and Windows
access control lists have one implementation rather than two that can drift.

## Files are private when creation returns

Unix creation supplies `0600` to the open call. Windows first protects the
parent directory with an inheritable owner-only list, then protects each file
outright before returning its handle. Tightening after a caller has received a
new file is too late for secrets.

## Every fallible operation stays typed

The public boundary returns `PrivacyError`. Callers that own path-specific
errors explicitly recover its `io::Error`; this crate does not invent domain
sentences for files whose meaning it cannot know.

An existing sensitive file is opened without following its final symbolic link
or reparse point and validated through the returned handle. Callers that may
shorten it can request one read/write/append handle, recheck its hard-link count
immediately before mutation, and keep using that handle rather than resolving
the pathname again. The residual on platforms without a hard-link seal is a
new name created after the last recheck; it cannot redirect the opened handle.

`try_lock_identity` leaves readable content unlocked. Unix uses the advisory
whole-file primitive. Windows uses an exclusive, fail-immediately one-byte
range at 4 EiB, where mandatory locking cannot block a bounded caller's content
read; `unlock_identity` must use the exact same range. A caller using this
primitive keeps its content below that sentinel.

For replacement, the caller prepares and syncs a fresh sibling, then hands
both names to `replace`. Unix renames and syncs the parent directory. Windows
uses `MoveFileExW` with replace-existing and write-through flags, which asks
the operating system to complete the move before returning; Windows exposes no
separate parent-directory sync here, so this is not a claim of the Unix fsync
contract under every storage stack or power failure. Callers must not rebuild
either platform half.

## Unsafe code has one home

Only `windows.rs` opts into unsafe code. Each FFI call states the lifetime,
alignment and ownership facts that make it valid. Unix uses `std` modes and
must not acquire a second platform dependency without walking the dependency
ladder first.
