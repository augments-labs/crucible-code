//! Where a tool result stands in a session log, and reading it back from
//! there.
//!
//! What a screen keeps of a result it had to cut is a place rather than the
//! words: the call the result answered, and where in the log the record
//! holding it begins. A running session only adds to its log, so a position
//! stays true through every compaction, pruning and restriction, each of which
//! adds a line and changes none. Picking a session up can cut a torn tail off
//! first, but that is before anything is read or written here, and the only
//! places it could move are the ones that tail held.
//!
//! A place is handed out two ways. Replaying a log for the reader says where
//! each message it hands over was read from — see
//! [`super::DisplayHistory::placed`] — and the thread that appends to the log
//! keeps where each result it wrote landed, for [`Session::take_landed`] and
//! [`Session::take_placed`].
//!
//! Reading back is one record at one position, bounded as every record is,
//! and accepted only where that record holds the call the place names. Nothing
//! is added to the log, though what the session has queued is written first,
//! and what is read is what the log holds: the words the result
//! held when it arrived, whatever a pruning or a restriction has since cleared
//! from what the model is sent.

use std::fs::File;
use std::io::{self, BufReader, Seek as _, SeekFrom};
use std::sync::PoisonError;
use std::sync::mpsc::sync_channel;

use crucible_types::{Message, RecordedToolOutput, ToolId};

use super::{LogRequest, Session, SessionError, replay, wire};

/// Where one tool result stands in a session log.
///
/// It holds no text of the result, which is the point of it: a place is what
/// is left of a result a screen could not keep the words of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    call: ToolId,
    position: u64,
}

impl Place {
    /// The result of `call`, in the record that begins `position` bytes into
    /// the log.
    ///
    /// Anybody may make one. A place the log does not bear out reads nothing,
    /// and one it does reads only what the log holds for that call: a made-up
    /// place reaches no further than a result of that call in this log.
    #[must_use]
    pub const fn new(call: ToolId, position: u64) -> Self {
        Self { call, position }
    }

    /// The call whose result this is.
    #[must_use]
    pub const fn call(&self) -> &ToolId {
        &self.call
    }

    /// Where in the log the record holding it begins.
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }
}

impl Session {
    /// Reads the result `place` names back from the log.
    ///
    /// Whatever this session has queued is written first, so a result it
    /// appended is there to be read. The record at the place is read whole or
    /// not at all: a line a crash cut short, a line that is not a message, and
    /// a message that holds no result of the call the place names all read
    /// nothing.
    ///
    /// # Errors
    ///
    /// [`SessionError::Log`] where the session records nothing, where its
    /// writer has stopped, where the log cannot be opened or read there, or
    /// where it holds no result of that call at that place.
    pub fn read_back(&self, place: &Place) -> Result<RecordedToolOutput, SessionError> {
        let trouble = |source| SessionError::Log {
            at: self.path.display().to_string().into(),
            source,
        };
        if self.to.is_none() {
            return Err(trouble(io::Error::other("this session records nothing")));
        }

        // Behind whatever is queued, so a result appended a moment ago has
        // landed before the file is opened to read it.
        if !self.caught_up() {
            return Err(trouble(io::Error::other(
                "the session's writer has stopped",
            )));
        }

        let mut file = File::open(&self.path).map_err(trouble)?;
        file.seek(SeekFrom::Start(place.position))
            .map_err(trouble)?;
        let mut raw = Vec::new();
        replay::read_record(&mut BufReader::new(file), &mut raw, replay::RECORD_BYTES)
            .map_err(trouble)?;

        let refused = || trouble(io::Error::new(io::ErrorKind::InvalidData, "no such result"));
        if !raw.ends_with(b"\n") {
            return Err(refused());
        }
        let text = std::str::from_utf8(&raw).map_err(|_| refused())?.trim_end();
        let Some(Message::ToolResults(results)) = wire::message(text) else {
            return Err(refused());
        };

        results
            .into_iter()
            .find(|result| result.id == place.call)
            .map(|result| result.output)
            .ok_or_else(refused)
    }

    /// The places of the results this session's writer appended since this
    /// was last asked, oldest first, once everything queued before this has
    /// been written.
    ///
    /// Waiting makes every result appended so far have a place, which is what
    /// opening a row asks for. Taken rather than read: each place is handed
    /// over once, and what is left here is bounded by the writer however long
    /// nobody asks.
    #[must_use]
    pub fn take_placed(&self) -> Vec<Place> {
        if self.to.is_none() || !self.caught_up() {
            return Vec::new();
        }
        self.take_landed()
    }

    /// The places of the results this session's writer has already written
    /// since this was last asked, oldest first, waiting for nothing.
    ///
    /// What a screen asks as each result arrives: a result still queued has
    /// no place yet and is handed over by a later take.
    #[must_use]
    pub fn take_landed(&self) -> Vec<Place> {
        self.placed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    /// Waits for the writer to take everything queued before this, and says
    /// whether it did: `false` where there is no writer left to ask.
    fn caught_up(&self) -> bool {
        let Some(to) = &self.to else { return false };
        let (done, waiting) = sync_channel(1);
        to.send(LogRequest::Barrier(done)).is_ok() && waiting.recv().is_ok()
    }
}
