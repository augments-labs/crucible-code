//! How fast a turn's requests are asked to be answered, what the vendor
//! said it served, and the one send more a refusal of fast gets.
//!
//! The speed is asked beside the request, through
//! [`crucible_models::Provider::stream_at`]. A vendor that refuses the fast
//! form does so before it has answered anything, so sending the same request
//! again at standard speed cannot repeat a word or a tool call: nothing of the
//! first was read. It is sent again exactly once, and the speed asked for is
//! standard from then on, so a refusal is never answered by another fast
//! request. Any other failure is the one it is, and nothing is sent again.

use crucible_models::{DeltaStream, Provider, ProviderError, Request, Served, Speed};

use super::{Listening, Runner};
use crate::Event;

/// The speed asked for, and what the last answer said it was served at.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Pace {
    /// What each request asks for until somebody chooses otherwise.
    asked: Speed,
    /// What the last answer's response said; unsaid until one has.
    pub(super) served: Served,
}

impl Runner {
    /// Asks every request from now on to be answered at `speed`, where the
    /// model has a fast form the request can carry.
    pub fn hasten(&mut self, speed: Speed) {
        self.pace.asked = speed;
    }

    /// The speed each request asks for.
    #[must_use]
    pub const fn speed(&self) -> Speed {
        self.pace.asked
    }

    /// What the last answer said about the speed it was served at.
    #[must_use]
    pub const fn served(&self) -> Served {
        self.pace.served
    }
}

/// Sends `request` at the speed `pace` asks for, and once more at standard
/// speed where the vendor refused the fast form.
///
/// Over the two fields it needs rather than the runner, because the request
/// borrows the runner's model while it is out.
pub(super) async fn sent(
    provider: &dyn Provider,
    pace: &mut Pace,
    request: Request<'_>,
    listening: &Listening<'_>,
) -> Result<Box<dyn DeltaStream>, ProviderError> {
    let cancel = listening.run.cancel();
    match provider.stream_at(request, pace.asked, cancel).await {
        Err(ProviderError::FastRefused {
            provider: named,
            message,
        }) => {
            pace.asked = Speed::Standard;
            listening.run.reporting().post(Event::FastRefused {
                provider: named,
                reason: message,
            });
            provider.stream_at(request, Speed::Standard, cancel).await
        }
        sent => sent,
    }
}
