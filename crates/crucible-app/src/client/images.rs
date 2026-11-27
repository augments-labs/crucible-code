//! Showing an image a turn named: read through the workspace, under an
//! [`Approved`] the permission engine settled, and handed back as a PNG.
//!
//! A front end asks for an image by the path an answer wrote or a read tool's
//! call kept; [`show`] answers with the picture or the reason it is not shown.
//! Everything about the file is hostile, the path first of all, so the order is
//! fixed and each step refuses before the next one runs:
//!
//! 1. A path that opens with two separators, `\\` or `//` in any mix, is
//!    refused from its words alone: on Windows it names another host's share
//!    or a device, and resolving it would reach the host first. Every other
//!    path is resolved by [`Workspace::existing`], which is what the read tool
//!    resolves it by. A path that leaves the workspace, a link that leads out
//!    of it, and a file that is not there all stop here.
//! 2. The permission engine decides the call the read tool would make for that
//!    path, with the sensitivity the read tool would give it. Whoever asks is
//!    never put a question: an `ask` rule is answered `deny`, so a picture
//!    shown at the end of a turn cannot stand a prompt in front of anybody.
//! 3. The file is read by [`read`], which takes the [`Approved`] the engine
//!    minted and nothing else, and is held to [`MOST_INPUT`] bytes, plus the
//!    one chunk it takes to tell that a file is longer, and to [`DEADLINE`].
//! 4. A tool's image is held to the hash the tool's call kept, so a file
//!    changed since the model was shown it is not shown as what it saw.
//! 5. It is decoded and encoded again as a PNG in the decode slot, taken with
//!    what is left of the deadline, so each image keeps its own two seconds and
//!    sixteen asked at once wait for the slot one at a time.
//!
//! Invariants:
//!
//! - Outside the workspace, under a rule that refuses the read, and missing
//!   are one refusal, [`ImageRefused::NotShown`], so a client cannot tell which
//!   it was and learns nothing about a file it may not read. Every other reason
//!   is told only about a file the engine allowed.
//! - Nothing is fetched from the network, since no path that could name
//!   another host is resolved, and nothing is remembered: a decision is asked
//!   for each image with [`Remember::Never`], and the PNG is handed back and
//!   kept nowhere here.
//! - How many images a turn shows is the front end's to count, at most
//!   [`PER_TURN`]; this answers one request at a time and counts nothing.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crucible_attachments::{AttachmentError, Opened, Taken};
use crucible_client_api::{ImageRef, ImageRefused, Shown};
use crucible_raster::{MOST_INPUT, Refused, Slot};
use crucible_tools::{Approved, Ask, Permission, Remember, Sensitivity, Settled, Target, Verdict};
use crucible_types::{ToolArgs, ToolCall, ToolId};
use crucible_workspace::{Workspace, WorkspacePath};

#[cfg(test)]
mod tests;

/// The most images a front end shows for one turn; the ninth and every one
/// after it are named with the words for being past it.
pub const PER_TURN: usize = 8;

/// How long one image may take, from the request to the PNG.
///
/// Never retried: an image that took longer is a link with its reason, and
/// asking again would only spend another two seconds of a turn's end on it.
pub const DEADLINE: Duration = Duration::from_secs(2);

/// The tool whose call an image is decided as. A rule written about reading a
/// file is a rule about showing it, so a file the model may not read is not
/// drawn either.
const TOOL: &str = "read";

/// How much of a file is read between two looks at whether to stop, which is
/// `crucible_attachments`'s own step. The tests hold the two together: a file
/// of [`MOST_INPUT`] bytes is read whole, and one a byte longer is refused.
const CHUNK: usize = 64 * 1024;

/// The picture `image` names, or why it is not shown.
///
/// `workspace` is where the path is resolved and `permission` the engine the
/// read is decided by, the session's own rules and mode, both lent by the
/// host. Settles as soon as it is asked: the engine is never left waiting on a
/// person, since every question it would put is answered `deny` here.
///
/// # Errors
///
/// [`ImageRefused::NotShown`] for a path that opens with two separators, one
/// outside the workspace, one a rule refuses or puts a question about, and a
/// file that is not there or cannot be read; for a file the engine allowed, [`ImageRefused::TooLarge`] past
/// [`MOST_INPUT`] bytes or past what the decoder or encoder hold,
/// [`ImageRefused::NotAnImage`] for bytes no decoder reads,
/// [`ImageRefused::NotInTime`] past [`DEADLINE`], and
/// [`ImageRefused::Changed`] for a tool's image whose bytes are no longer the
/// ones the tool read.
pub async fn show(
    image: &ImageRef,
    workspace: &Workspace,
    permission: &mut Permission,
) -> Result<Shown, ImageRefused> {
    let started = Instant::now();
    let requested = image.path().as_str();
    if elsewhere(requested) {
        return Err(ImageRefused::NotShown);
    }
    let path = workspace
        .existing(requested)
        .map_err(|_| ImageRefused::NotShown)?;
    let call = ToolCall {
        id: ToolId::new("show-image"),
        name: TOOL.into(),
        args: ToolArgs::new(serde_json::json!({ "path": requested }).to_string()),
    };
    let sensitivity = Sensitivity::ReadOnly {
        target: Target::resolved(workspace, &path),
    };
    let Settled::Approved(approved) = permission.decide(&call, &sensitivity, &mut Unasked).await
    else {
        return Err(ImageRefused::NotShown);
    };
    let taken = read(&approved, workspace, &path, started)?;
    if let ImageRef::Tool { hash, .. } = image
        && taken.hash() != *hash
    {
        return Err(ImageRefused::Changed);
    }
    pictured(taken.bytes(), started)
}

/// Whether `requested` opens with two separators, in any mix of `\` and `/`:
/// another host's share or a device on Windows, such as `\\host\share` or
/// `\\?\UNC\host`, which only resolving it would tell from a local path.
fn elsewhere(requested: &str) -> bool {
    let separator = |at: u8| matches!(at, b'/' | b'\\');
    matches!(requested.as_bytes(), [first, second, ..] if separator(*first) && separator(*second))
}

/// The file `approved` was settled about, read whole within [`MOST_INPUT`]
/// bytes and what is left of [`DEADLINE`] after `started`.
///
/// An [`Approved`] is the only way in, and only one settled about reading this
/// very path: anything else is refused as the path itself would have been.
/// Whatever has no proof cannot ask:
///
/// ```compile_fail,E0308
/// use std::time::Instant;
///
/// fn unproved(
///     workspace: &crucible_workspace::Workspace,
///     path: &crucible_workspace::WorkspacePath,
/// ) {
///     let _ = crucible_app::client::images::read(true, workspace, path, Instant::now());
/// }
/// ```
///
/// # Errors
///
/// [`ImageRefused::NotShown`] where `approved` is about another call or the
/// file cannot be opened as a regular file, [`ImageRefused::TooLarge`] past
/// [`MOST_INPUT`], and [`ImageRefused::NotInTime`] past the deadline.
pub fn read(
    approved: &Approved,
    workspace: &Workspace,
    path: &WorkspacePath,
    started: Instant,
) -> Result<Taken, ImageRefused> {
    let settled = Sensitivity::ReadOnly {
        target: Target::resolved(workspace, path),
    };
    if approved.tool() != TOOL || *approved.sensitivity() != settled {
        return Err(ImageRefused::NotShown);
    }
    // Asked before each chunk: past the deadline it stops at once, and past
    // the chunk that takes the read beyond the ceiling it stops too, so no more
    // than one chunk over the ceiling is ever held.
    let looks = Cell::new(0_usize);
    let late = || started.elapsed() >= DEADLINE;
    let stopped = || {
        looks.set(looks.get().saturating_add(1));
        late() || looks.get().saturating_sub(1).saturating_mul(CHUNK) > MOST_INPUT
    };
    let taken = Opened::reached(path)
        .and_then(|opened| opened.taken_until(stopped))
        .map_err(|error| match error {
            AttachmentError::Stopped if late() => ImageRefused::NotInTime,
            AttachmentError::Stopped | AttachmentError::TooLarge => ImageRefused::TooLarge,
            AttachmentError::NotFile
            | AttachmentError::Unread(_)
            | AttachmentError::Unreached(_) => ImageRefused::NotShown,
        })?;
    if taken.bytes().len() > MOST_INPUT {
        return Err(ImageRefused::TooLarge);
    }
    Ok(taken)
}

/// `bytes` decoded and encoded again as a PNG, in the decode slot taken with
/// what is left of the deadline.
///
/// Encoded again whatever it was, so what crosses is pixels this process made
/// and nothing else the file carried.
fn pictured(bytes: &[u8], started: Instant) -> Result<Shown, ImageRefused> {
    let left = DEADLINE.saturating_sub(started.elapsed());
    if left.is_zero() {
        return Err(ImageRefused::NotInTime);
    }
    let refused = |refused: Refused| match refused {
        Refused::TooLarge => ImageRefused::TooLarge,
        Refused::NotAnImage => ImageRefused::NotAnImage,
        Refused::OverTime => ImageRefused::NotInTime,
    };
    // Held through both steps, and re-entered by each: one image is decoded
    // and encoded at a time, and the wait for it comes out of this one's two
    // seconds rather than the slot's own.
    let _slot = Slot::take(left).map_err(refused)?;
    let pixels = crucible_raster::decode(bytes).map_err(refused)?;
    let png = crucible_raster::png(&pixels).map_err(refused)?;
    Ok(Shown {
        png,
        pixels: (pixels.width(), pixels.height()),
    })
}

/// The question an image's read would put, answered no and never remembered.
struct Unasked;

impl Ask for Unasked {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> crucible_runtime::BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async { (Verdict::Deny, Remember::Never) })
    }
}
