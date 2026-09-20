//! Which model answers for an agent, and how.

use crucible_models::Effort;
use crucible_types::Modalities;

/// Which model to ask, and how.
///
/// Model selection only. Stable operator instructions live on
/// [`crate::Instructions`]; the model name and effort are assembled separately
/// as typed context on every provider pass.
#[derive(Debug, Clone)]
pub struct Model {
    /// The model's name, as the provider spells it.
    pub name: Box<str>,
    /// Ceiling on one response.
    pub max_tokens: u32,
    /// How much this model accepts at once, in tokens, where anybody knows.
    ///
    /// `None` is not a large window — it is no answer, and a session runs
    /// without a proactive bound rather than against a number a loop made up.
    /// The wiring resolves it; a definition is handed the result.
    pub window: Option<u32>,
    /// What this model reads, where anybody knows.
    ///
    /// The model's half of what may be attached, and only that half: what a
    /// provider can put in a request is the provider's own answer, asked of it
    /// during a run rather than resolved here, because what a module can write
    /// today and what a vendor's table says are two facts that diverge.
    ///
    /// `None` is no answer rather than a permissive one. An attachment nothing
    /// can say this model reads is stood down and carries a line saying so —
    /// the alternative is bytes labelled with a shape the request has no word
    /// for, which is a wrong request rather than a refused one.
    pub accepts: Option<Modalities>,
    /// How hard to think, where somebody said. `None` leaves it to the vendor.
    pub effort: Option<Effort>,
}
