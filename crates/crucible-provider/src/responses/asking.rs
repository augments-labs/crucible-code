//! Asking a vendor's plan how much of its limits is used.
//!
//! A vendor that keeps such a source names it in its dialect ([`Usage`]): an
//! address on its own host and how the answer there is read. The request is
//! a GET with the credential the provider already holds and nothing else,
//! through the same transport as a turn, so the same client and the same hold
//! decide whether it goes at all. The answer is read whole, within
//! [`MOST`] bytes and [`WAIT`], and parsed once, by the dialect.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crucible_credentials::{Credential, Outgoing};
use crucible_models::{Asked, ProviderError};
use crucible_runtime::Cancel;
use serde_json::Value;

use super::Usage;
use crate::transport::{PostBodyError, Transport};

/// The most bytes of an answer read: far more than a plan with every limit a
/// reading keeps says of them.
const MOST: usize = 64 * 1024;

/// How long the answer's body is waited for, once its head has arrived.
const WAIT: Duration = Duration::from_secs(10);

/// Asks `usage` with `credential` over `transport`, for the provider called
/// `provider`, with `extra` setting any header the vendor asks for beside the
/// credential.
///
/// A 401, 403 or 404 is [`Asked::Closed`]: the credential is refused there, or
/// the source is not there for it, and asking again would meet the same. Any
/// other failure is [`Asked::Failed`], with the exact credential sent redacted
/// from what it says.
pub(crate) async fn ask(
    provider: &'static str,
    usage: Usage,
    credential: Arc<dyn Credential>,
    transport: Arc<dyn Transport>,
    extra: fn(&mut Outgoing),
) -> Asked {
    // Nothing cancels the ask but dropping it, which closes the request.
    let cancel = Cancel::new();
    let mut outgoing = Outgoing::new();
    outgoing.set_header("accept", "application/json");
    extra(&mut outgoing);
    if let Err(source) = credential.authorize(&mut outgoing).await {
        return Asked::Failed(ProviderError::Credential { provider, source });
    }
    let response = transport.get(usage.url, &mut outgoing, &cancel).await;
    let redactions = outgoing.redactions();
    let response = match response {
        Ok(response) => response,
        Err(problem) => {
            return Asked::Failed(problem.for_provider(provider).redacted(&redactions));
        }
    };
    let arrived = SystemTime::now();
    match response.status() {
        200 => {}
        401 | 403 | 404 => return Asked::Closed,
        status => {
            return Asked::Failed(ProviderError::Refused {
                provider,
                status,
                message: "the plan's usage was not given".into(),
            });
        }
    }
    let body = match response.read_limited(MOST, WAIT).await {
        Ok(body) => body,
        Err(problem) => {
            let problem = match problem {
                PostBodyError::TooLarge
                | PostBodyError::Http(crucible_http::BodyError::TooLarge) => {
                    format!("the plan's usage ran past {MOST} bytes")
                }
                PostBodyError::Deadline
                | PostBodyError::Http(crucible_http::BodyError::Deadline) => {
                    "the plan's usage did not arrive in time".to_owned()
                }
                PostBodyError::Read(_) | PostBodyError::Http(_) => {
                    "the plan's usage broke off".to_owned()
                }
            };
            return Asked::Failed(ProviderError::Transport {
                provider,
                problem: problem.into(),
            });
        }
    };
    serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|answer| (usage.read)(&answer, arrived))
        .map_or_else(
            || {
                Asked::Failed(ProviderError::Protocol {
                    provider,
                    problem: "the plan's usage was not in a shape crucible reads".into(),
                })
            },
            Asked::Answered,
        )
}
