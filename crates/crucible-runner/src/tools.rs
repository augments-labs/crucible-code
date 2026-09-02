//! The static toolset a session offers, by name.
//!
//! Registration is persistent; advertisement and reachability are snapshots.
//! A deferred entry is absent from both until its shared [`Revealed`] record
//! says otherwise. Materializing once gives the provider request and every call
//! returned by that request the same immutable answer even if the live reveal
//! state moves while the provider is answering.

use std::sync::{Arc, Mutex};

use crucible_core::{
    Collision, DescribeTool, Provenance, Registered, Registry, RegistryGeneration,
    RegistrySnapshot, Revealed, Tool, ToolDescriptor, ToolEntry, ToolHooks, ToolProvenance,
    ToolSchema, ToolSnapshot, Toolset, ToolsetContext, ToolsetError,
};

/// Every tool the model may call.
///
/// A registry generation rather than a map. A session offers a handful of
/// tools, and walking six names costs less than hashing one; the generation
/// also keeps the order they were added, which is the order they are advertised
/// in. Two registrations of one name are refused outright: a tool name is
/// something the model acts on, so no source may take it over by arriving later
/// or from nearer.
///
/// Registered and advertised are two different things. A **deferred** tool is
/// here and callable, and is left out of what the model is shown until it looks
/// the name up — because a schema the model can see is one it pays for on every
/// request of every turn, and most sessions never touch most tools.
#[derive(Debug)]
pub struct Tools {
    registry: Registry<Offered>,
    roster: RegistrySnapshot<Offered>,
    revealed: Revealed,
    cached: Mutex<Option<Cached>>,
}

/// One tool, with what is said about it and whether it is said up front.
///
/// One record rather than parallel lists: three vectors that must stay the same
/// length and the same order are three chances to get one of them wrong, and
/// nothing about them would say so.
#[derive(Debug)]
struct Offered {
    entry: ToolEntry,
    deferred: bool,
}

impl Registered for Offered {
    fn id(&self) -> &str {
        self.entry.descriptor().name()
    }

    fn provenance(&self) -> &Provenance {
        self.entry.descriptor().provenance()
    }

    fn retained_bytes(&self) -> usize {
        self.entry.descriptor().retained_bytes()
    }
}

/// The last materialization, reusable while neither registration nor reveal
/// state has changed.
#[derive(Debug)]
struct Cached {
    roster: RegistryGeneration,
    revealed: u64,
    snapshot: ToolSnapshot,
}

impl Default for Tools {
    fn default() -> Self {
        let registry = Registry::new(Collision::Refuse);
        let roster = registry.snapshot();
        Self {
            registry,
            roster,
            revealed: Revealed::new(),
            cached: Mutex::new(None),
        }
    }
}

impl Tools {
    /// A session with nothing to call yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, reading what has been looked up from `revealed`.
    ///
    /// The caller keeps a clone and hands it to whatever does the revealing, so
    /// the roster and the tool that grows it cannot hold different answers.
    #[must_use]
    pub fn looking_up(revealed: Revealed) -> Self {
        Self {
            revealed,
            ..Self::default()
        }
    }

    /// Offers one more tool.
    ///
    /// A duplicate name is refused, naming both registrations.
    ///
    /// # Errors
    ///
    /// [`ToolsetError::Duplicate`] when an entry already answers to the name.
    pub fn add(
        &mut self,
        descriptor: ToolDescriptor,
        tool: Arc<dyn Tool>,
    ) -> Result<(), ToolsetError> {
        self.add_with_hooks(descriptor, tool, ToolHooks::new())
    }

    /// Offers one more tool with its exact invocation middleware.
    ///
    /// # Errors
    ///
    /// [`ToolsetError::Duplicate`] when an entry already answers to the name.
    pub fn add_with_hooks(
        &mut self,
        descriptor: ToolDescriptor,
        tool: Arc<dyn Tool>,
        hooks: ToolHooks,
    ) -> Result<(), ToolsetError> {
        self.offer(ToolEntry::with_hooks(descriptor, tool, hooks), false)
    }

    /// Offers one more tool, held back until the model asks for it by name.
    ///
    /// Which tools are worth deferring is the wiring's decision and not a
    /// property of a tool: the same `bash` is indispensable to one session and
    /// untouched by the next, and only the thing assembling a session knows
    /// which it is looking at.
    ///
    /// # Errors
    ///
    /// [`ToolsetError::Duplicate`] when an entry already answers to the name.
    pub fn defer(
        &mut self,
        descriptor: ToolDescriptor,
        tool: Arc<dyn Tool>,
    ) -> Result<(), ToolsetError> {
        self.defer_with_hooks(descriptor, tool, ToolHooks::new())
    }

    /// Defers one tool with its exact invocation middleware.
    ///
    /// # Errors
    ///
    /// [`ToolsetError::Duplicate`] when an entry already answers to the name.
    pub fn defer_with_hooks(
        &mut self,
        descriptor: ToolDescriptor,
        tool: Arc<dyn Tool>,
        hooks: ToolHooks,
    ) -> Result<(), ToolsetError> {
        self.offer(ToolEntry::with_hooks(descriptor, tool, hooks), true)
    }

    /// Registers one compiled tool with built-in provenance.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] when its descriptor is invalid or its name collides.
    pub fn add_builtin<T>(&mut self, tool: T) -> Result<(), ToolsetError>
    where
        T: DescribeTool + Tool + 'static,
    {
        let provenance = ToolProvenance::builtin(tool.name())?;
        let descriptor = tool.descriptor(provenance)?;
        self.add(descriptor, Arc::new(tool))
    }

    /// Registers one compiled tool deferred, with built-in provenance.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] when its descriptor is invalid or its name collides.
    pub fn defer_builtin<T>(&mut self, tool: T) -> Result<(), ToolsetError>
    where
        T: DescribeTool + Tool + 'static,
    {
        let provenance = ToolProvenance::builtin(tool.name())?;
        let descriptor = tool.descriptor(provenance)?;
        self.defer(descriptor, Arc::new(tool))
    }

    fn offer(&mut self, entry: ToolEntry, deferred: bool) -> Result<(), ToolsetError> {
        let mut staged = self.registry.stage();
        staged.register(Offered { entry, deferred })?;
        self.roster = self.registry.commit(staged)?;
        *self
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }

    fn offered(&self) -> impl Iterator<Item = &Offered> {
        self.roster.entries().iter().map(Arc::as_ref)
    }

    /// The tool the model named, if there is one it may call.
    ///
    /// A deferred tool it has not looked up is **not** one, and that is a
    /// separate decision from leaving the schema out. Hiding a name is not
    /// withholding it: a model can call any name it has seen, and one of the
    /// things it sees is a web page crucible fetched for it. So a page naming
    /// `web_fetch` must not be a page that reaches it, and the gate is here
    /// rather than only in what is advertised.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&ToolEntry> {
        self.offered()
            .find(|offered| offered.entry.descriptor().name() == name)
            .filter(|offered| !offered.deferred || self.revealed.holds(name))
            .map(|offered| &offered.entry)
    }

    /// What the model is shown: every tool that is not deferred, and every
    /// deferred one it has looked up.
    ///
    /// Built per call rather than kept, because what belongs in it changes when
    /// the model looks a name up mid-turn. It is read once per request, against
    /// a roster of under a dozen.
    #[must_use]
    pub fn advertised(&self) -> Vec<ToolSchema<'_>> {
        self.offered()
            .filter(|offered| {
                !offered.deferred || self.revealed.holds(offered.entry.descriptor().name())
            })
            .map(|offered| offered.entry.descriptor().advertised())
            .collect()
    }

    /// One immutable roster for a provider request and the calls it returns.
    ///
    /// An unchanged material roster reuses its opaque generation. A reveal or
    /// registration change produces a new one; the earlier snapshot stays
    /// usable by the response already admitted against it.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] if the visible roster crosses its aggregate bounds.
    pub fn snapshot(&self) -> Result<ToolSnapshot, ToolsetError> {
        let revealed = self.revealed.revision();
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(present) = cached.as_ref()
            && present.roster == *self.roster.generation()
            && present.revealed == revealed
        {
            return Ok(present.snapshot.clone());
        }

        let snapshot = ToolSnapshot::new(
            self.offered()
                .filter(|offered| {
                    !offered.deferred || self.revealed.holds(offered.entry.descriptor().name())
                })
                .map(|offered| offered.entry.clone()),
        )?;
        *cached = Some(Cached {
            roster: self.roster.generation().clone(),
            revealed,
            snapshot: snapshot.clone(),
        });
        Ok(snapshot)
    }

    /// The same, by name and nothing else.
    ///
    /// For the sentence the model is asked under, which says which tools it has
    /// and leaves what each does to the schema beside it. Off this registry so
    /// that a prompt cannot advertise a tool the session does not offer.
    #[must_use]
    pub fn offering(&self) -> Vec<String> {
        self.advertised()
            .into_iter()
            .map(|schema| schema.name.to_owned())
            .collect()
    }

    /// Every tool that is held back, with the name and description a search
    /// matches against.
    ///
    /// Includes the ones already looked up. A model that searches twice should
    /// see the same answer both times, and a tool vanishing from a catalogue
    /// because it is now offered reads as the tool having gone away.
    #[must_use]
    pub fn deferred(&self) -> Vec<&ToolDescriptor> {
        self.offered()
            .filter(|offered| offered.deferred)
            .map(|offered| offered.entry.descriptor())
            .collect()
    }
}

impl Toolset for Tools {
    fn prepare(&self, _context: &ToolsetContext) -> Result<(), ToolsetError> {
        Ok(())
    }

    fn snapshot(&self, _context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError> {
        Self::snapshot(self)
    }

    fn refresh(&self, _context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError> {
        Self::snapshot(self)
    }

    fn dispose(&self, _context: &ToolsetContext) -> Result<(), ToolsetError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crucible_core::{
        Ancestry, Cancel, Permission, Settled, ToolArgs, ToolCall, ToolContext, ToolDescriptor,
        ToolId, ToolProvenance, ToolSourceKind, Toolset, ToolsetContext, Unwatched, Verdict,
    };

    use super::*;
    use crate::fake::{Fixed, Says, changing};

    #[test]
    fn the_static_roster_adapts_to_the_live_toolset_lifecycle() {
        let mut tools = Tools::new();
        tools.add_builtin(Fixed::new("read")).unwrap();
        tools.add_builtin(Fixed::new("grep")).unwrap();
        let expected: Vec<(String, String)> = tools
            .advertised()
            .iter()
            .map(|tool| (tool.name.to_owned(), tool.schema.to_owned()))
            .collect();
        let context = ToolsetContext::new(Ancestry::new(), Cancel::new(), None);

        Toolset::prepare(&tools, &context).unwrap();
        let before = Toolset::snapshot(&tools, &context).unwrap();
        let refreshed = Toolset::refresh(&tools, &context).unwrap();
        Toolset::dispose(&tools, &context).unwrap();
        Toolset::dispose(&tools, &context).unwrap();

        let advertised = |snapshot: &ToolSnapshot| {
            snapshot
                .advertised()
                .iter()
                .map(|tool| (tool.name.to_owned(), tool.schema.to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(advertised(&before), expected);
        assert_eq!(advertised(&refreshed), expected);
    }

    #[test]
    fn owned_descriptor_data_can_be_advertised_and_invoked_from_its_snapshot() {
        fn descriptor() -> ToolDescriptor {
            let name = String::from("runtime_owned");
            let schema =
                String::from(r#"{"description":"Runtime owned.","type":"object","properties":{}}"#);
            let source_id = String::from("test:runtime-owned");
            let source_label = String::from("runtime-owned test fixture");
            let provenance =
                ToolProvenance::new(ToolSourceKind::User, source_id, source_label).unwrap();

            ToolDescriptor::new(name, schema, provenance).unwrap()
        }

        let mut tools = Tools::new();
        tools
            .add(descriptor(), Arc::new(Fixed::new("runtime_owned")))
            .unwrap();
        let snapshot = tools.snapshot().unwrap();
        let advertised = snapshot.advertised();

        assert_eq!(advertised.len(), 1);
        assert_eq!(
            advertised.first().map(|tool| (tool.name, tool.schema)),
            Some((
                "runtime_owned",
                r#"{"description":"Runtime owned.","type":"object","properties":{}}"#
            ))
        );

        let call = ToolCall {
            id: ToolId::new("runtime-call"),
            name: "runtime_owned".into(),
            args: ToolArgs::new("{}"),
        };
        let entry = snapshot
            .find(&call.name)
            .expect("the snapshot that advertised the tool must reach it");
        let sensitivity = entry.tool().sensitivity(&call.args);
        let mut permission = Permission::new();
        let mut ask = Says::new(Verdict::Allow);
        let Settled::Approved(approved) = permission.decide(&call, &sensitivity, &mut ask) else {
            panic!("the read-only fixture was not approved");
        };

        let cancel = Cancel::new();
        let context = ToolContext::new(Ancestry::new(), call.id.clone(), &cancel, None, &Unwatched);
        let output = entry.tool().run(approved, &context).unwrap();
        assert_eq!(output.text(), "done");
    }

    #[test]
    fn a_tool_is_found_by_the_name_the_model_used() {
        let mut tools = Tools::new();
        tools.add_builtin(Fixed::new("read")).unwrap();
        tools.add_builtin(Fixed::new("write")).unwrap();

        assert_eq!(
            tools.find("write").map(|entry| entry.descriptor().name()),
            Some("write")
        );
    }

    #[test]
    fn a_name_no_tool_answers_to_finds_nothing() {
        let mut tools = Tools::new();
        tools.add_builtin(Fixed::new("read")).unwrap();

        assert!(tools.find("frobnicate").is_none());
    }

    #[test]
    fn a_duplicate_name_is_rejected_and_names_both_sources() {
        let mut tools = Tools::new();
        let descriptor = |id: &str, label: &str| {
            ToolDescriptor::new(
                "read",
                r#"{"type":"object","properties":{}}"#,
                ToolProvenance::new(ToolSourceKind::User, id, label).unwrap(),
            )
            .unwrap()
        };
        tools
            .add(
                descriptor("package:one", "the first package"),
                Arc::new(Fixed::new("read")),
            )
            .unwrap();

        let problem = tools
            .add(
                descriptor("package:two", "the second package"),
                Arc::new(Fixed::new("read").risking(changing())),
            )
            .unwrap_err();

        let said = problem.to_string();
        assert!(said.contains("the first package"), "{said}");
        assert!(said.contains("the second package"), "{said}");
        assert_eq!(tools.advertised().len(), 1);
    }

    #[test]
    fn a_deferred_tool_is_not_advertised_until_it_is_looked_up() {
        let revealed = Revealed::new();
        let mut tools = Tools::looking_up(revealed.clone());
        tools.add_builtin(Fixed::new("read")).unwrap();
        tools.defer_builtin(Fixed::new("web_search")).unwrap();

        let named = |tools: &Tools| -> Vec<String> {
            tools
                .advertised()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        };

        assert_eq!(named(&tools), ["read"]);

        revealed.reveal("web_search");
        assert_eq!(named(&tools), ["read", "web_search"]);
    }

    #[test]
    fn a_deferred_tool_cannot_be_called_before_it_is_looked_up() {
        // Not advertised is not the same as not reachable, and only one of
        // those is a gate. A model can name any tool it has seen, and what it
        // sees includes a web page crucible fetched for it — so a page naming
        // `web_fetch` must not be a page that reaches it.
        let revealed = Revealed::new();
        let mut tools = Tools::looking_up(revealed.clone());
        tools.defer_builtin(Fixed::new("web_fetch")).unwrap();

        assert!(
            tools.find("web_fetch").is_none(),
            "a deferred tool ran without being looked up",
        );

        revealed.reveal("web_fetch");
        assert_eq!(
            tools
                .find("web_fetch")
                .map(|entry| entry.descriptor().name()),
            Some("web_fetch")
        );
    }

    #[test]
    fn an_admission_is_reachable_only_through_the_generation_that_minted_it() {
        let revealed = Revealed::new();
        let mut tools = Tools::looking_up(revealed.clone());
        tools.defer_builtin(Fixed::new("web_fetch")).unwrap();
        let call = ToolCall {
            id: ToolId::new("fetch-one"),
            name: "web_fetch".into(),
            args: ToolArgs::new("{}"),
        };

        assert!(tools.snapshot().unwrap().admit(&call).is_err());
        revealed.reveal("web_fetch");
        let admitted_from = tools.snapshot().unwrap();
        let admission = admitted_from.admit(&call).unwrap();
        let entry = admitted_from.resolve(&admission).unwrap();
        let sensitivity = entry.tool().sensitivity(&call.args);
        let mut permission = Permission::new();
        let mut ask = Says::new(Verdict::Allow);
        let Settled::Approved(approved) =
            permission.decide_admitted(&admission, &sensitivity, &mut ask)
        else {
            panic!("the admitted read-only fixture was not approved");
        };
        revealed.forget();
        let current = tools.snapshot().unwrap();

        assert!(admitted_from.resolve(&admission).is_ok());
        assert!(admitted_from.resolve_approved(&approved).is_ok());
        assert!(current.resolve(&admission).is_err());
        assert!(current.resolve_approved(&approved).is_err());
        assert!(current.admit(&call).is_err());
    }

    #[test]
    fn what_is_held_back_is_listed_whether_or_not_it_was_looked_up() {
        // A model that searches twice should see the same answer both times. A
        // tool vanishing from the catalogue because it is now offered reads as
        // the tool having gone away.
        let revealed = Revealed::new();
        let mut tools = Tools::looking_up(revealed.clone());
        tools.add_builtin(Fixed::new("read")).unwrap();
        tools.defer_builtin(Fixed::new("web_search")).unwrap();

        revealed.reveal("web_search");

        let held: Vec<&str> = tools.deferred().iter().map(|tool| tool.name()).collect();
        assert_eq!(held, ["web_search"]);
    }

    #[test]
    fn tools_are_advertised_in_the_order_they_were_added() {
        let mut tools = Tools::new();
        tools.add_builtin(Fixed::new("read")).unwrap();
        tools.add_builtin(Fixed::new("grep")).unwrap();

        let advertised: Vec<&str> = tools.advertised().iter().map(|tool| tool.name).collect();

        assert_eq!(advertised, ["read", "grep"]);
    }
}
