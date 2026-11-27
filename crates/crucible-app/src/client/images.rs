//! Showing an image a turn named: read through the workspace, under an
//! [`Approved`] the permission engine settled, and handed back as a PNG.
//!
//! A front end asks for an image by the path an answer wrote or a read tool's
//! call kept; [`show`] answers with the picture or the reason it is not shown.
//! Everything about the file is hostile, the path first of all, so the order is
//! fixed and each step refuses before the next one runs:
//!
//! 1. The path is judged by its words before anything resolves it, since
//!    resolving it may reach another host before any refusal: a directory an
//!    automounter serves contacts the host a name under it names, and on
//!    Windows a path that opens with two separators, `\\` or `//` in any mix,
//!    names a share or a device. Joined to the root as the read tool joins it,
//!    and its `.` and `..` folded without asking the filesystem, it has to lie
//!    under the written spelling of a root the workspace grants; one that
//!    opens with two separators only under a root whose spelling does too, a
//!    workspace on a share. Only then is it resolved, by
//!    [`Workspace::existing`], which is what the read tool resolves it by. A
//!    link that leads out of the workspace and a file that is not there stop
//!    there.
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
//!    sixteen asked at once wait for the slot one at a time. A picture that
//!    was ready only past the deadline is late, however it ended.
//!
//! Every step that touches the filesystem, and the wait for the slot, runs on
//! the tool worker the host lends, where it lends one, and the deadline is
//! kept outside that work: what has started there cannot be stopped, so a step
//! still running at the deadline is left to end on its own and the answer is
//! given without it. Before the engine has approved the read, that answer is
//! [`ImageRefused::NotShown`], as it is for a job the worker had no room for or
//! would not start: a path it would refuse is answered so, and a slow or busy
//! worker must not tell such a path from the others. After approval it is
//! [`ImageRefused::NotInTime`].
//!
//! The route has at most one job of its own on that worker. Its [`Lane`] is
//! taken before an image's first job is handed over, carried into each job in
//! turn, and given back only as the job holding it returns, including one left
//! running past the deadline. An image asked for while the lane is held is
//! [`ImageRefused::NotShown`] at once, and nothing is queued for it: answer
//! text naming files on mounts that stopped answering, turn after turn, holds
//! one of the worker's places at most, and the tools keep the others.
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
//!   [`PER_TURN`]; this answers one request at a time and counts nothing but
//!   whether its lane is held.

use std::cell::Cell;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crucible_attachments::{AttachmentError, Opened, Taken};
use crucible_client_api::{ImageRef, ImageRefused, Shown};
use crucible_raster::{MOST_INPUT, Pixels, Refused, Slot};
use crucible_runtime::Cancel;
use crucible_tools::{
    Approved, Ask, Permission, Remember, Sensitivity, Settled, Target, ToolWorker, Unrun, Verdict,
};
use crucible_types::{ToolArgs, ToolCall, ToolId};
use crucible_workspace::{PathError, Workspace, WorkspacePath, written};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
/// read is decided by, the session's own rules and mode, `worker` the one the
/// read tool hands its blocking work to, and `lane` the route's one job on it,
/// all lent by the host. Resolving
/// the path, reading the file, waiting for the decode slot and decoding run
/// there, so a mount that stops answering holds a worker's place rather than
/// whoever asked; lent one, this answers within [`DEADLINE`] whether or not
/// that work has ended. The engine is never left waiting on a
/// person, since every question it would put is answered `deny` here.
///
/// # Errors
///
/// [`ImageRefused::NotShown`] for a path whose words lie outside every root
/// the workspace grants, one that opens with two separators outside a root on
/// a share, one that resolves outside the workspace, one a rule refuses or puts a question about, a file
/// that is not there or cannot be read, a worker that would not start the
/// work, a `lane` another image's job still holds, and [`DEADLINE`] passing before the engine allowed the read; for a
/// file the engine allowed, [`ImageRefused::TooLarge`] past
/// [`MOST_INPUT`] bytes or past what the decoder or encoder hold,
/// [`ImageRefused::NotAnImage`] for bytes no decoder reads,
/// [`ImageRefused::NotInTime`] past [`DEADLINE`], and
/// [`ImageRefused::Changed`] for a tool's image whose bytes are no longer the
/// ones the tool read.
pub async fn show(
    image: &ImageRef,
    workspace: &Workspace,
    permission: &mut Permission,
    worker: Option<&ToolWorker>,
    lane: &Lane,
) -> Result<Shown, ImageRefused> {
    showing(
        image,
        workspace,
        permission,
        (worker, lane),
        (Workspace::existing, read, crucible_raster::decode),
    )
    .await
}

/// The one job of its own the route may have on the worker, which a host
/// keeps for as long as the worker it lends.
///
/// Taken before the first of an image's jobs is handed to the worker, carried
/// into each job in turn, and given back only as the job holding it returns,
/// even one nobody waits for any longer. An image asked for while it is held
/// is not shown, at once: the worker's places are the tools', and a read stuck
/// on a mount that stopped answering may hold one, never more.
#[derive(Debug)]
pub struct Lane(Arc<Semaphore>);

impl Default for Lane {
    fn default() -> Self {
        Self(Arc::new(Semaphore::new(1)))
    }
}

impl Lane {
    /// The lane, where no job holds it.
    fn taken(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.0).try_acquire_owned().ok()
    }
}

/// How a path is resolved, its file read and its bytes decoded:
/// [`Workspace::existing`], [`read`] and the raster's own decoder outside the
/// tests, which put one that counts, a slow one or a stuck one in their place.
type Steps<R> = (
    fn(&Workspace, &str) -> Result<WorkspacePath, PathError>,
    R,
    fn(&[u8]) -> Result<Pixels, Refused>,
);

/// [`show`], reading and decoding by `steps`.
async fn showing<R>(
    image: &ImageRef,
    workspace: &Workspace,
    permission: &mut Permission,
    (worker, lane): (Option<&ToolWorker>, &Lane),
    (resolve, reader, decode): Steps<R>,
) -> Result<Shown, ImageRefused>
where
    R: FnOnce(&Approved, &Workspace, &WorkspacePath, Instant) -> Result<Taken, ImageRefused>
        + Send
        + 'static,
{
    let started = Instant::now();
    let requested = image.path().as_str();
    if !reaches(workspace, requested) {
        return Err(ImageRefused::NotShown);
    }
    let hash = match image {
        ImageRef::Tool { hash, .. } => Some(*hash),
        ImageRef::Answer { .. } => None,
    };
    // Carried into each job the worker runs for this image, and given back as
    // the job holding it returns, whoever still waits for it.
    let held = match worker {
        Some(_) => Some(lane.taken().ok_or(ImageRefused::NotShown)?),
        None => None,
    };
    // Until the engine has approved the read, every answer is the one a path
    // it would refuse gets, so running out of time or of room says nothing
    // about the file either.
    let (path, approved, held) = within(started, worker, ImageRefused::NotShown, async {
        let (owned, asked) = (workspace.clone(), requested.to_owned());
        let (resolved, held) = blocking(worker, move || {
            let resolved = resolve(&owned, &asked).map(|path| {
                let target = Target::resolved(&owned, &path);
                (path, Sensitivity::ReadOnly { target })
            });
            (resolved, held)
        })
        .await?;
        let (path, sensitivity) = resolved.map_err(|_| ImageRefused::NotShown)?;
        let call = ToolCall {
            id: ToolId::new("show-image"),
            name: TOOL.into(),
            args: ToolArgs::new(serde_json::json!({ "path": requested }).to_string()),
        };
        let Settled::Approved(approved) =
            permission.decide(&call, &sensitivity, &mut Unasked).await
        else {
            return Err(ImageRefused::NotShown);
        };
        Ok((path, approved, held))
    })
    .await?;
    let owned = workspace.clone();
    within(started, worker, ImageRefused::NotInTime, async {
        blocking(worker, move || {
            let _held = held;
            let taken = reader(&approved, &owned, &path, started)?;
            if hash.is_some_and(|hash| taken.hash() != hash) {
                return Err(ImageRefused::Changed);
            }
            pictured(taken.bytes(), started, decode)
        })
        .await?
    })
    .await
}

/// What `work` answers, or `late` once [`DEADLINE`] has passed since
/// `started`, whether or not the work has ended.
///
/// The deadline is kept here, outside the work, by the timer of the worker
/// the work runs on: what has started there cannot be stopped, so a read stuck
/// on a mount that stopped answering is left to end on its own and its answer
/// is not waited for. Lent no worker, the work runs on the calling thread,
/// which nothing could take back from it, and each step looks at the deadline
/// itself.
async fn within<T>(
    started: Instant,
    worker: Option<&ToolWorker>,
    late: ImageRefused,
    work: impl Future<Output = Result<T, ImageRefused>>,
) -> Result<T, ImageRefused> {
    let Some(worker) = worker else {
        return work.await;
    };
    let left = DEADLINE.saturating_sub(started.elapsed());
    worker.within(left, work).await.unwrap_or(Err(late))
}

/// What `job` answers, run on `worker` where one was lent, as the read tool's
/// blocking work is, and on the calling thread where none was.
///
/// A job the worker would not start, because its runtime is shutting down, is
/// [`ImageRefused::NotShown`]; one that comes apart unwinds through here, as
/// it would have on the calling thread.
async fn blocking<T, F>(worker: Option<&ToolWorker>, job: F) -> Result<T, ImageRefused>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let Some(worker) = worker else {
        return Ok(job());
    };
    match worker.run(&Cancel::new(), |_| job()).await {
        Ok(answer) => Ok(answer),
        Err(Unrun::Panicked) => std::panic::resume_unwind(Box::new(Unrun::Panicked.to_string())),
        Err(Unrun::Cancelled | Unrun::Stopped) => Err(ImageRefused::NotShown),
    }
}

/// Whether `requested`, by its words alone, lies under a root `workspace`
/// grants: joined to the root as [`Workspace::existing`] joins it, spelled as
/// [`written`] spells a path, and folded by [`folded`]. Nothing here asks the
/// filesystem.
fn reaches(workspace: &Workspace, requested: &str) -> bool {
    let asked = Path::new(requested);
    let joined = if asked.is_absolute() {
        asked.to_path_buf()
    } else {
        workspace.root().join(asked)
    };
    let roots: Vec<String> = workspace.roots().map(written).collect();
    under(&written(&joined), &roots, requested)
}

/// Whether `spelled`, the written spelling of the path `requested` names,
/// lies under one of `roots`' once both are folded: a path whose words open
/// with two separators only under a root whose spelling opens with them too.
fn under(spelled: &str, roots: &[String], requested: &str) -> bool {
    let Some(spelled) = folded(spelled) else {
        return false;
    };
    roots.iter().any(|root| {
        folded(root).is_some_and(|root| {
            let beneath = spelled.strip_prefix(root.as_str()).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with('/') || root.ends_with('/')
            });
            beneath && (!elsewhere(requested) || elsewhere(&root))
        })
    })
}

/// `spelled` with its `.` and empty names dropped and each `..` taking the
/// name before it away, the separators it opens with kept as they are; nothing
/// where a `..` has no name before it to take.
fn folded(spelled: &str) -> Option<String> {
    let names = spelled.trim_start_matches('/');
    let opening = spelled.strip_suffix(names).unwrap_or_default();
    let mut kept: Vec<&str> = Vec::new();
    for name in names.split('/') {
        match name {
            "" | "." => {}
            ".." => {
                kept.pop()?;
            }
            name => kept.push(name),
        }
    }
    Some(format!("{opening}{}", kept.join("/")))
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
fn pictured(
    bytes: &[u8],
    started: Instant,
    decode: fn(&[u8]) -> Result<Pixels, Refused>,
) -> Result<Shown, ImageRefused> {
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
    let pixels = decode(bytes).map_err(refused)?;
    let png = crucible_raster::png(&pixels).map_err(refused)?;
    // A decode or an encode that ran past the deadline is late however it
    // ended, so what is handed back always came within the two seconds.
    if started.elapsed() >= DEADLINE {
        return Err(ImageRefused::NotInTime);
    }
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
