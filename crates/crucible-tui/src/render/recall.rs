//! A wait on the keyboard that something outside the terminal can call off.
//!
//! The renderer's waits block on the keyboard with no clock, which is right
//! for a thread with nothing else to do until a key arrives. It is wrong for a
//! process told to stop from outside while it waits: nothing would look until
//! somebody pressed a key, so whoever told it would have to make it die where
//! it stood, with the terminal still in the modes it was put in. A [`Recall`]
//! is what the owner of that outside word hands the renderer. While one says
//! a wait is to be watched, the wait sleeps a beat at a time and asks between
//! beats and once more after it has said it is over, so a recall that comes
//! as the last beat runs out or the key is read is not left for nobody to
//! read. A wait called off ends in [`TerminalError::Recalled`], which
//! unwinds its caller the way a terminal that failed does, so every guard on
//! the way out hands back what it holds.
//!
//! What the outside word is, and when a wait is worth watching, are the
//! recall's to decide: this crate names neither.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::terminal::TerminalError;

/// How long a watched wait sleeps before it asks again whether it is called
/// off, and so the longest a recall is kept waiting.
const BEAT: Duration = Duration::from_millis(250);

/// Something outside the terminal that can call off a wait on the keyboard.
pub trait Recall: fmt::Debug + Send + Sync {
    /// Says a wait on the keyboard is starting, and answers whether this one
    /// is to be watched. [`Recall::waited`] follows a `true` however the wait
    /// ends.
    fn waiting(&self) -> bool;

    /// Whether the wait being watched is to be called off: asked between
    /// beats, and once more after [`Recall::waited`] where the wait ended with
    /// a key or with its patience spent.
    fn recalled(&self) -> bool;

    /// Says a watched wait has ended.
    fn waited(&self);
}

/// One wait on the keyboard, watched for a recall where the recall asked for
/// that, and said to be over by [`Watch::over`] or, failing that, when this is
/// dropped.
#[derive(Debug)]
pub(super) struct Watch(Option<Arc<dyn Recall>>);

impl Watch {
    /// Starts a wait, asking `recall` whether to watch it.
    pub(super) fn began(recall: Option<&Arc<dyn Recall>>) -> Self {
        Self(recall.filter(|recall| recall.waiting()).cloned())
    }

    /// How long the wait may sleep before it next looks: `patience`, cut to a
    /// beat where it is watched. A wait with no patience of its own sleeps a
    /// beat where it is watched, and otherwise until a key arrives.
    pub(super) fn patience(&self, patience: Option<Duration>) -> Option<Duration> {
        match self.0 {
            Some(_) => Some(patience.map_or(BEAT, |patience| patience.min(BEAT))),
            None => patience,
        }
    }

    /// Whether the wait goes on.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Recalled`] where it is watched and has been called off.
    pub(super) fn held(&self) -> Result<(), TerminalError> {
        match &self.0 {
            Some(recall) if recall.recalled() => Err(TerminalError::Recalled),
            _ => Ok(()),
        }
    }

    /// Ends the wait, then asks once more whether it was called off.
    ///
    /// After [`Recall::waited`], never before: a recall that comes once the
    /// wait has said it is over is the recall's to act on where it lands, and
    /// one that came earlier, however late, left its word for this to read.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Recalled`] where it was watched and called off.
    pub(super) fn over(mut self) -> Result<(), TerminalError> {
        match self.0.take() {
            Some(recall) => {
                recall.waited();
                if recall.recalled() {
                    Err(TerminalError::Recalled)
                } else {
                    Ok(())
                }
            }
            None => Ok(()),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(recall) = &self.0 {
            recall.waited();
        }
    }
}
