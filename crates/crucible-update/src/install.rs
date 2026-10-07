//! The installer's receipt, and the release layout it describes.
//!
//! A managed install keeps each release in a directory of its own under a
//! hidden prefix in the directory the installer was given, and one link names
//! the release that is active:
//!
//! ```text
//! <dir>/crucible -> .crucible-install/current/crucible
//! <dir>/cru -> crucible
//! <dir>/.crucible-install/
//!     current -> releases/<version>
//!     lock
//!     releases/<version>/crucible
//!     releases/<version>/crucible-sandbox-broker   (when the release has one)
//!     releases/<version>/receipt
//! ```
//!
//! The executable and its broker are one release unit, so a running process
//! finds its broker beside its own resolved executable and an activation
//! cannot pair it with another release's broker. The receipt is written once
//! the unit's executables are in place, and names the unit's installation,
//! platform, prefix and release and the SHA-256 of each executable it holds.
//! This module reads that layout and stages a release unit into it from the
//! release's archive, under `releases/` beside the active unit; the installer
//! writes the same layout and holds itself to the same shape.
//!
//! A receipt is evidence to check, not a list of paths to follow. Nothing
//! here opens a path the receipt names: the layout is found from where the
//! running executable is, each of its directories and files is checked for
//! type, owner and mode before it is read, the `current` link is read rather
//! than followed, and the receipt has to agree with the layout and
//! the files with their recorded hashes. A flat install, a package manager's
//! copy and a build tree have no receipt and are not a managed layout.
//!
//! Only Unix installs are managed this way; Windows has neither a receipt nor
//! this module.

mod layout;
mod receipt;
mod stage;

pub use layout::{EntryKind, LayoutEntry, LayoutError, ReceiptClaim, ReceiptLayout};
pub use receipt::{Digest, Installation, Receipt, ReceiptError, Target, Version};
pub use stage::{StageError, StagePart, StagedUnit};
