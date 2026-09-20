//! A definition, and the one way to build one.

use std::sync::Arc;

use crucible_types::AgentId;

use crate::availability::Availability;
use crate::guardrails::{Declared, InputGuardrail, NameTaken, OutputGuardrail};
use crate::instructions::Instructions;
use crate::model::Model;

/// A reusable agent definition.
///
/// Settled when it is built and never afterwards: there is no way to write to
/// one of these, which is what makes a single definition safe to hand to two
/// runs at the same time. A session that changes what it is asked under builds
/// another definition and selects it; the request already out keeps the one it
/// started with, because it is holding its own reference to a value nobody can
/// rewrite.
///
/// Every field is private, with an accessor apiece. A reader of this type gets
/// to ask what the agent is; nothing gets to make it something else.
///
/// Clonable, because that is how a session changes what it is asked under: the
/// definition in force is replaced whole by another built from it, rather than
/// written to. Cloning one copies two words, a model selection and two lists of
/// shared handles with the names they were declared under; the guardrails
/// themselves are never duplicated.
#[derive(Debug, Clone)]
pub struct Agent {
    /// What this agent is called where one is selected: a configuration
    /// document, a command line, or — later — another agent delegating to it.
    ///
    /// An address rather than a label: it is what a definition is looked up by,
    /// and it is fixed for the life of the value like every other field here.
    /// [`Agent::aimed`] and [`Agent::telling`] build another definition under
    /// the same one, so what selected an agent goes on selecting it.
    id: AgentId,

    /// The name a reader sees.
    ///
    /// Apart from the id because an id is an address and a name is for people:
    /// renaming an agent must not silently repoint everything that selected it.
    ///
    /// Private for the reason `instructions` is, read from the other end:
    /// [`Agent::new`] gives this the id's own word, and a field a caller can
    /// assign is a field that assignment can blank. [`AgentBuilder::named`] is
    /// where it is written.
    name: Box<str>,

    /// What this agent is for, in one sentence.
    ///
    /// Not decoration. Where an agent becomes something another agent can hand
    /// work to, this is what that decision is made on — so it says what the
    /// agent is good for rather than restating its name.
    description: Box<str>,

    /// The sentence the model is asked under, where the definition has one.
    ///
    /// The difference between nothing said and an empty thing said belongs to
    /// [`Instructions`], which is the only way text reaches this field.
    instructions: Instructions,

    /// Which model answers for this agent, and how.
    ///
    /// Spelled the way the resolved provider spells it. A vendor alias — the
    /// short word a person types — is resolved during wiring and never reaches
    /// here, because the same alias means different models at different
    /// vendors and this definition is meant to outlive one run's provider.
    model: Model,

    /// Which of the run's tools this agent is offered.
    availability: Availability,

    /// What the caller's words are held to before any of them are sent.
    input: Box<[Declared<dyn InputGuardrail>]>,

    /// What the model's final answer is held to before it is accepted.
    output: Box<[Declared<dyn OutputGuardrail>]>,
}

impl Agent {
    /// A definition for one agent, with nothing yet said about what it is for.
    ///
    /// The id stands in as the name because an agent nobody has named is still
    /// referred to by the word it was selected under; the description and the
    /// instructions stay empty, because inventing either would be putting words
    /// in the caller's mouth. No tool is declared and no guardrail is asked,
    /// which is a session exactly as it ran before either existed.
    #[must_use]
    pub fn new(id: AgentId, model: Model) -> Self {
        AgentBuilder::new(id, model).build()
    }

    /// What this agent is called where one is selected.
    #[must_use]
    pub const fn id(&self) -> &AgentId {
        &self.id
    }

    /// The sentence this agent is asked under, or `None` where nobody wrote
    /// one.
    #[must_use]
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.text()
    }

    /// The name a reader sees, which is the id where nobody chose another.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What this agent is for, or empty where nobody said.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Which model answers for this agent, and how.
    #[must_use]
    pub const fn model(&self) -> &Model {
        &self.model
    }

    /// Which of the run's tools this agent is offered.
    #[must_use]
    pub const fn availability(&self) -> &Availability {
        &self.availability
    }

    /// The same agent, answered for by a different model.
    ///
    /// A whole definition rather than a written field, and the same identity:
    /// a session asked to change model is still the agent it was, so everything
    /// that selected it still selects it and everything it was told still
    /// holds. What a caller does with this is replace the one definition it
    /// holds; a definition anybody else is holding — a request already out, a
    /// run beside this one — is untouched, because nothing here writes to the
    /// value it was built from.
    #[must_use]
    pub fn aimed(&self, model: Model) -> Self {
        Self {
            model,
            ..self.clone()
        }
    }

    /// The same agent, under different operator instructions.
    ///
    /// Built the way [`Agent::aimed`] is and for the same reasons, through the
    /// one call that keeps the difference between nothing said and an empty
    /// thing said.
    #[must_use]
    pub fn telling(&self, said: &str) -> Self {
        Self {
            instructions: Instructions::said(said),
            ..self.clone()
        }
    }

    /// The checks the caller's words are held to, in the order they were
    /// declared.
    #[must_use]
    pub fn input_guardrails(&self) -> &[Declared<dyn InputGuardrail>] {
        &self.input
    }

    /// The checks the final answer is held to, in the order they were declared.
    #[must_use]
    pub fn output_guardrails(&self) -> &[Declared<dyn OutputGuardrail>] {
        &self.output
    }
}

/// The one way to build an [`Agent`] that says more than its id and its model.
///
/// Every field it writes is private on the value it produces, so this is not a
/// convenience over a struct literal: it is the only route, and a rule about a
/// field holds here because there is nowhere else to write.
#[derive(Debug)]
pub struct AgentBuilder {
    agent: Agent,
}

impl AgentBuilder {
    /// A builder for the agent `id`, answered for by `model`.
    #[must_use]
    pub fn new(id: AgentId, model: Model) -> Self {
        Self {
            agent: Agent {
                name: id.as_str().into(),
                description: "".into(),
                instructions: Instructions::none(),
                id,
                model,
                availability: Availability::Everything,
                input: Box::new([]),
                output: Box::new([]),
            },
        }
    }

    /// Calls this agent something other than the word it is selected under.
    ///
    /// Blank is not a name: a definition called nothing is the case
    /// [`AgentBuilder::new`] already answers by standing the id in, and letting
    /// a caller take that back would leave a reader an empty line where the
    /// agent's word belongs.
    #[must_use]
    pub fn named(mut self, called: &str) -> Self {
        if !called.is_empty() {
            self.agent.name = called.into();
        }
        self
    }

    /// Says what this agent is for.
    #[must_use]
    pub fn describing(mut self, what: &str) -> Self {
        self.agent.description = what.into();
        self
    }

    /// Says what this agent is asked under, reading nothing said as nothing
    /// said.
    ///
    /// The only way a caller's text reaches the field, so the difference
    /// between no instructions and empty ones cannot be lost by a caller that
    /// had no reason to know there was one. A struct literal would be the
    /// second way, and every field is private so that there is no second way.
    ///
    /// The error code below is what this fails with today, not something the
    /// harness checks: `compile_fail` accepts any compile error, so a rename
    /// anywhere in the snippet would keep it green for the wrong reason. Both
    /// snippets in this crate are kept to the one call that must not compile.
    ///
    /// ```compile_fail,E0451
    /// use crucible_agents::{Agent, AgentBuilder};
    /// use crucible_types::AgentId;
    ///
    /// let agent = Agent {
    ///     instructions: Some("".into()),
    ///     ..AgentBuilder::new(AgentId::new("x"), unimplemented!()).build()
    /// };
    /// ```
    #[must_use]
    pub fn telling(mut self, said: &str) -> Self {
        self.agent.instructions = Instructions::said(said);
        self
    }

    /// Offers this agent only part of what the run can reach.
    #[must_use]
    pub fn offering(mut self, availability: Availability) -> Self {
        self.agent.availability = availability;
        self
    }

    /// Holds what the caller asks to one more check, after the ones already
    /// declared.
    ///
    /// The check's name is read here, once, and is what anything it decides is
    /// written under from then on.
    ///
    /// # Errors
    ///
    /// [`NameTaken`] where a check on either end of this definition was already
    /// declared under that name.
    pub fn checking_input(mut self, guardrail: Arc<dyn InputGuardrail>) -> Result<Self, NameTaken> {
        let declared = Declared::under(self.free(guardrail.name())?, guardrail);
        self.agent.input = appended(std::mem::take(&mut self.agent.input), declared);
        Ok(self)
    }

    /// Holds the final answer to one more check, after the ones already
    /// declared.
    ///
    /// The check's name is read here, once, as an input check's is.
    ///
    /// # Errors
    ///
    /// [`NameTaken`] where a check on either end of this definition was already
    /// declared under that name.
    pub fn checking_output(
        mut self,
        guardrail: Arc<dyn OutputGuardrail>,
    ) -> Result<Self, NameTaken> {
        let declared = Declared::under(self.free(guardrail.name())?, guardrail);
        self.agent.output = appended(std::mem::take(&mut self.agent.output), declared);
        Ok(self)
    }

    /// `name`, kept, where no check on this definition has it yet.
    fn free(&self, name: &str) -> Result<Box<str>, NameTaken> {
        let input = self.agent.input.iter().map(Declared::name);
        let output = self.agent.output.iter().map(Declared::name);
        if input.chain(output).any(|taken| taken == name) {
            return Err(NameTaken::of(name));
        }
        Ok(name.into())
    }

    /// The definition, settled.
    #[must_use]
    pub fn build(self) -> Agent {
        self.agent
    }
}

/// One more of the same, keeping the order they were declared in.
fn appended<T>(declared: Box<[T]>, one: T) -> Box<[T]> {
    let mut all = declared.into_vec();
    all.push(one);
    all.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_models::Effort;
    use crucible_types::{Modalities, Modality};

    /// A definition with everything said about it that can be said.
    fn described() -> Agent {
        AgentBuilder::new(
            AgentId::new("coding"),
            Model {
                name: "claude-test".into(),
                max_tokens: 1024,
                window: None,
                accepts: Some(Modalities::empty().insert(Modality::Text)),
                effort: None,
            },
        )
        .named("Coding")
        .describing("Edits this repository and runs its checks.")
        .telling("You are an expert in coding.")
        .build()
    }

    #[test]
    fn a_definition_given_only_an_id_answers_to_it_and_claims_nothing_else() {
        let agent = Agent::new(AgentId::new("coding"), described().model().clone());

        assert_eq!(agent.id().as_str(), "coding");
        assert_eq!(
            agent.name(),
            "coding",
            "an unnamed agent lost the word it is selected under"
        );
        assert_eq!(
            agent.description(),
            "",
            "a description nobody wrote was invented"
        );
        assert!(
            agent.instructions().is_none(),
            "instructions nobody wrote were invented"
        );
        assert!(
            agent.input_guardrails().is_empty() && agent.output_guardrails().is_empty(),
            "a check nobody declared was invented"
        );
    }

    #[test]
    fn re_aiming_a_definition_leaves_the_one_it_was_built_from_alone() {
        // The whole reason a definition is replaced rather than written to: a
        // request already out, and a run beside this one, are holding the value
        // this was built from. If changing model reached them, two agents would
        // share one model selection and a session's `/model` would silently
        // re-aim its sibling mid-request.
        let before = described();

        let after = before.aimed(Model {
            name: "other".into(),
            effort: Some(Effort::Low),
            ..before.model().clone()
        });

        assert_eq!(&*before.model().name, "claude-test");
        assert_eq!(before.model().effort, None);
        assert_eq!(&*after.model().name, "other");
        assert_eq!(after.model().effort, Some(Effort::Low));
        assert_eq!(
            after.id(),
            before.id(),
            "re-aiming a session made it a different agent"
        );
        assert_eq!(
            after.instructions(),
            before.instructions(),
            "re-aiming a session lost what it was told"
        );
    }

    #[test]
    fn telling_a_definition_something_else_leaves_the_one_it_was_built_from_alone() {
        let before = described();

        let after = before.telling("Answer only in French.");

        assert_eq!(before.instructions(), Some("You are an expert in coding."));
        assert_eq!(after.instructions(), Some("Answer only in French."));
        assert_eq!(
            &*after.model().name,
            &*before.model().name,
            "being told something else changed which model answers"
        );
    }

    /// A check that allows everything, under whatever name it is given.
    #[derive(Debug)]
    struct Called(&'static str);

    impl InputGuardrail for Called {
        fn name(&self) -> &str {
            self.0
        }

        fn checking(
            &self,
            _context: &crate::AgentContext<'_>,
        ) -> Result<crate::Decision, crate::Undecided> {
            Ok(crate::Decision::Allowed)
        }
    }

    /// An output check that allows everything, under whatever name it is given.
    #[derive(Debug)]
    struct Vouching(&'static str);

    impl OutputGuardrail for Vouching {
        fn name(&self) -> &str {
            self.0
        }

        fn checking(
            &self,
            _context: &crate::AgentContext<'_>,
            _candidate: &str,
        ) -> Result<crate::Decision, crate::Undecided> {
            Ok(crate::Decision::Allowed)
        }
    }

    #[test]
    fn a_name_one_check_was_declared_under_is_not_given_to_a_second() {
        let declaring = || {
            AgentBuilder::new(AgentId::new("coding"), described().model().clone())
                .checking_input(Arc::new(Called("no-secrets")))
                .expect("the first check under a name is taken")
        };

        // Two checks answering to one name would each read as the other's
        // refusal, at either end of the invocation.
        let again = declaring().checking_input(Arc::new(Called("no-secrets")));
        assert_eq!(again.map(|_| ()), Err(NameTaken::of("no-secrets")));
        let across = declaring().checking_output(Arc::new(Vouching("no-secrets")));
        assert_eq!(across.map(|_| ()), Err(NameTaken::of("no-secrets")));

        let agent = declaring()
            .checking_output(Arc::new(Vouching("no-leaks")))
            .expect("another name is another check")
            .build();
        let input: Vec<&str> = agent
            .input_guardrails()
            .iter()
            .map(Declared::name)
            .collect();
        let output: Vec<&str> = agent
            .output_guardrails()
            .iter()
            .map(Declared::name)
            .collect();
        assert_eq!((input, output), (vec!["no-secrets"], vec!["no-leaks"]));
    }
}
