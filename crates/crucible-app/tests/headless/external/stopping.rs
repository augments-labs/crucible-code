//! What a stop the agent does not heed leaves for the prompt after it.
//!
//! Each case here holds a conversation on Claude Code's route, played by the
//! fake peer, whose first run says nothing of any work under way and then
//! ignores the stop it is sent. The turn ends cancelled once the stop's grace
//! has run out, with the run ended, and the prompt after it must reach the
//! agent rather than be lost to the run it could not use.

use crucible_external_harnesses::fake_peer::{Reply, scripts::claude};
use crucible_runner::Turned;
use crucible_types::StopReason;

use super::{Outside, connected, turn};
use crate::{Failed, Tree};

/// A prompt sent after a stop the agent ignored is heard by the agent, on a
/// run launched for it, and is answered.
#[test]
fn the_prompt_after_a_stop_the_agent_ignored_reaches_the_agent() -> Result<(), Failed> {
    let tree = Tree::new("external-unheeded-stop")?;
    let unheeding = Reply::new().hear(1).say(claude::init()).hear_out();
    let mut outside = Outside::new(&tree, &connected([unheeding, claude::clean()]))?;

    let (first, _) = turn(&mut outside, "one", true)?;
    assert!(
        matches!(&first, Ok(Turned::Ran(result)) if result.stop() == StopReason::Cancelled),
        "{first:?}"
    );
    let (second, _) = turn(&mut outside, "two", false)?;

    let heard = outside.heard();
    assert!(
        heard.iter().any(|line| line.contains("two")),
        "the prompt after the stop never reached the agent: {second:?}; the agent heard {heard:?}"
    );
    assert!(matches!(second, Ok(Turned::Ran(_))), "{second:?}");
    Ok(())
}
