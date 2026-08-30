//! What an agent *is*, as opposed to what running one does.
//!
//! Configuration, and only configuration: a name, what the agent is for, the
//! instructions it works under, and which model answers for it. Everything an
//! execution needs and this does not hold — the provider connection, the
//! transcript, the permission memory, the session log — belongs to whatever is
//! running the agent, because those are facts about one run rather than about
//! the agent, and two runs of the same agent must not share them.
//!
//! Configuration is not the same as constant. A session answers under the
//! instructions and the model it was started with until somebody changes them,
//! and both can be changed mid-session — so a runner rewrites those two fields
//! on the definition it holds. It holds its own because one is handed over by
//! value: the wiring gives its definition up, rather than keeping a second one
//! that would go stale the first time the session changed models. What a
//! definition may never acquire is one of the four things above; that is the
//! line this type draws.
//!
//! One of these is built during wiring today and there is exactly one agent to
//! build it for. The split is worth making before there are two: a definition
//! that had ever held a live connection could not be reused by a second run,
//! and by then every caller would depend on it holding one.

use crucible_core::AgentId;

use crate::runner::Model;

/// A reusable agent definition.
///
/// Handed over whole by the wiring, the way [`crate::Compaction`] is, so this
/// crate never learns that any of it has a spelling in a file.
#[derive(Debug)]
pub struct AgentSpec {
    /// What this agent is called where one is selected: a configuration
    /// document, a command line, or — later — another agent delegating to it.
    pub id: AgentId,

    /// The name a reader sees.
    ///
    /// Apart from the id because an id is an address and a name is for people:
    /// renaming an agent must not silently repoint everything that selected it.
    pub name: Box<str>,

    /// What this agent is for, in one sentence.
    ///
    /// Not decoration. Where an agent becomes something another agent can hand
    /// work to, this is what that decision is made on — so it says what the
    /// agent is good for rather than restating its name.
    pub description: Box<str>,

    /// The sentence the model is asked under, where the session has one.
    ///
    /// `None` is no instructions rather than empty ones: a request that carries
    /// no system field and one that carries an empty string are two different
    /// requests, and only the first is what "nobody said" means.
    ///
    /// The one field here that is not simply whatever the caller set, which is
    /// why it is the one that is not written directly. A rule stated in prose
    /// beside a public field holds wherever somebody remembered it; [`told`] is
    /// where it holds because there is nowhere else to write. The definitions a
    /// later phase reads out of files are where an empty body stops being a
    /// mistake and becomes an ordinary case.
    ///
    /// Rewritten by [`crate::Runner::telling`] on the runner's own definition:
    /// part of what these say is about the session — which model is answering,
    /// how hard it was asked to think — and both of those move while it runs.
    ///
    /// [`told`]: AgentSpec::told
    instructions: Option<Box<str>>,

    /// Which model answers for this agent, and how.
    ///
    /// Spelled the way the resolved provider spells it. A vendor alias — the
    /// short word a person types — is resolved during wiring and never reaches
    /// here, because the same alias means different models at different
    /// vendors and this definition is meant to outlive one run's provider.
    ///
    /// Rewritten by [`crate::Runner::ask`] and [`crate::Runner::think`] on the
    /// runner's own definition, for the same reason `instructions` is: a
    /// session can be asked to change model or effort without being restarted.
    pub model: Model,
}

impl AgentSpec {
    /// A definition for one agent, with nothing yet said about what it is for.
    ///
    /// The id stands in as the name because an agent nobody has named is still
    /// referred to by the word it was selected under; the description and the
    /// instructions stay empty, because inventing either would be putting words
    /// in the caller's mouth.
    #[must_use]
    pub fn new(id: AgentId, model: Model) -> Self {
        Self {
            name: id.as_str().into(),
            description: "".into(),
            instructions: None,
            id,
            model,
        }
    }

    /// The sentence this agent is asked under, or `None` where nobody wrote
    /// one.
    #[must_use]
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    /// Says what this agent is asked under, reading nothing said as nothing
    /// said.
    ///
    /// The only way the field is written, so the difference between no
    /// instructions and empty ones cannot be lost by a caller that had no
    /// reason to know there was one. A struct literal would be the second way,
    /// and the field is private so that there is no second way:
    ///
    /// ```compile_fail,E0451
    /// use crucible_runner::AgentSpec;
    /// use crucible_core::AgentId;
    ///
    /// let spec = AgentSpec {
    ///     instructions: Some("".into()),
    ///     ..AgentSpec::new(AgentId::new("x"), unimplemented!())
    /// };
    /// ```
    pub fn told(&mut self, said: &str) {
        self.instructions = (!said.is_empty()).then(|| said.into());
    }
}
