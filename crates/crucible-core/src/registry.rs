//! Source-aware registries: what was contributed, by whom, and which record
//! answers to a name.
//!
//! Tools, commands, providers and every later contribution kind share one
//! shape: a bounded set of records, each with a stable identifier and a
//! [`Provenance`] saying where it came from. What differs is the record type,
//! so the registry is generic over anything [`Registered`], and the policy for
//! two records claiming one identifier is declared once per registry as a
//! [`Collision`].
//!
//! A registry is read through immutable snapshots and written through staged
//! transactions. A [`Staged`] set is built up from the current generation, or
//! from nothing for a reload, validated record by record, and then committed as
//! one swap — so a reader never sees half of a batch, and a batch that fails
//! part-way leaves no trace. Every snapshot is its own [`RegistryGeneration`];
//! a [`RegistryHandle`] minted from one generation resolves only through that
//! generation and is refused as stale by every later one.
//!
//! Registration is not availability or activation. A record in a registry is
//! a validated contribution and nothing more: whether a run may see it, and
//! whether it is running, are decided by the layers above this one.

use std::fmt;
use std::sync::{Arc, Mutex};

/// The most bytes in a stable, receipt-safe source identifier.
pub const SOURCE_ID_BYTES: usize = 256;

/// The most bytes in the diagnostic spelling of a source.
pub const SOURCE_LABEL_BYTES: usize = 4 * 1024;

/// The most records one registry generation retains.
pub const REGISTRY_ENTRIES: usize = 4096;

/// The aggregate retained bytes one registry generation may hold.
pub const REGISTRY_BYTES: usize = 16 * 1024 * 1024;

/// Where a registration came from.
///
/// This is deliberately a closed, coarse set. A receipt may name the kind
/// without disclosing the path or package text kept for collision diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Compiled into this Crucible binary.
    Builtin,
    /// Supplied directly by the user.
    User,
    /// Supplied by a trusted project.
    Project,
    /// Contributed by an extension.
    Extension,
    /// Materialized from an MCP server.
    Mcp,
    /// Contributed by a skill.
    Skill,
    /// Local to an agent definition.
    Agent,
    /// A bounded source kind not covered above.
    Other,
}

impl SourceKind {
    /// The receipt spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
            Self::Extension => "extension",
            Self::Mcp => "mcp",
            Self::Skill => "skill",
            Self::Agent => "agent",
            Self::Other => "other",
        }
    }
}

/// Why a source could not be described.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProvenanceError {
    /// A retained string was empty.
    #[error("{field} must not be empty")]
    Empty {
        /// Which field.
        field: &'static str,
    },
    /// A retained string crossed its boundary.
    #[error("{field} is {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Which field.
        field: &'static str,
        /// Its boundary.
        maximum: usize,
        /// What was supplied.
        actual: usize,
    },
}

/// A registration's bounded source identity and its diagnostic spelling.
#[derive(Clone, PartialEq, Eq)]
pub struct Provenance {
    kind: SourceKind,
    id: Box<str>,
    label: Box<str>,
}

impl Provenance {
    /// Builds one source record.
    ///
    /// `id` is the non-sensitive stable spelling allowed into audit receipts;
    /// `label` is the fuller text shown only in an explicit wiring diagnostic.
    ///
    /// # Errors
    ///
    /// [`ProvenanceError`] when either spelling is empty or over its retained
    /// boundary.
    pub fn new(
        kind: SourceKind,
        id: impl Into<Box<str>>,
        label: impl Into<Box<str>>,
    ) -> Result<Self, ProvenanceError> {
        let id = id.into();
        bounded("source id", &id, SOURCE_ID_BYTES)?;
        let label = label.into();
        bounded("source label", &label, SOURCE_LABEL_BYTES)?;

        Ok(Self { kind, id, label })
    }

    /// A tool compiled into this binary.
    ///
    /// # Errors
    ///
    /// [`ProvenanceError`] if `name` cannot fit in a source identity or
    /// diagnostic. Built-in names are constants, so that is a wiring defect.
    pub fn builtin(name: &str) -> Result<Self, ProvenanceError> {
        Self::new(
            SourceKind::Builtin,
            format!("crucible:{name}"),
            format!("built-in {name} tool"),
        )
    }

    /// The coarse source kind safe to retain in a receipt.
    #[must_use]
    pub const fn kind(&self) -> SourceKind {
        self.kind
    }

    /// The bounded non-sensitive identifier safe to retain in a receipt.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The full diagnostic spelling.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The part of this source safe to retain with an invocation event.
    #[must_use]
    pub fn receipt(&self) -> SourceReceipt {
        SourceReceipt {
            kind: self.kind,
            id: self.id.clone(),
        }
    }

    /// The bytes this record keeps alive.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.id.len().saturating_add(self.label.len())
    }
}

impl fmt::Debug for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provenance")
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("label", &"[redacted]")
            .finish()
    }
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}:{}]", self.label, self.kind.as_str(), self.id)
    }
}

/// The receipt-safe projection of a registration source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReceipt {
    kind: SourceKind,
    id: Box<str>,
}

impl SourceReceipt {
    /// The coarse source family.
    #[must_use]
    pub const fn kind(&self) -> SourceKind {
        self.kind
    }

    /// The bounded, non-sensitive source identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

fn bounded(field: &'static str, value: &str, maximum: usize) -> Result<(), ProvenanceError> {
    if value.is_empty() {
        return Err(ProvenanceError::Empty { field });
    }
    if value.len() > maximum {
        return Err(ProvenanceError::TooLong {
            field,
            maximum,
            actual: value.len(),
        });
    }
    Ok(())
}

/// What a registry can hold.
///
/// The identifier is the collision key: two records with one identifier are
/// one slot, and the registry's [`Collision`] policy decides which, if either,
/// keeps it. The provenance is what the decision is made from and what its
/// diagnostic names.
pub trait Registered: Send + Sync + 'static {
    /// The stable identifier two contributions would collide on.
    fn id(&self) -> &str;

    /// Where this record came from.
    fn provenance(&self) -> &Provenance;

    /// The bytes this record keeps alive, for the generation's ceiling.
    fn retained_bytes(&self) -> usize;
}

/// What a registry does when two records claim one identifier.
///
/// Declared once per registry, because an identifier that is refused in one
/// place and quietly replaced in another is a rule nobody can predict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collision {
    /// The second registration is refused, naming both.
    ///
    /// The rule for tools, skills and every kind whose identifier a model or a
    /// reader acts on: a contribution cannot take a name over by arriving
    /// later or from a nearer source.
    Refuse,
    /// The record from the earlier-listed source kind keeps the identifier and
    /// the other is recorded as shadowed; two records of one kind, or of kinds
    /// the list leaves out, are refused.
    ///
    /// Deterministic because the order is written here rather than derived
    /// from registration order, and complete because the shadowed record is
    /// kept in the diagnostic rather than dropped.
    Prefer(&'static [SourceKind]),
}

impl Collision {
    /// The rank of `kind` under this policy: lower wins, `None` never does.
    fn rank(self, kind: SourceKind) -> Option<usize> {
        match self {
            Self::Refuse => None,
            Self::Prefer(order) => order.iter().position(|listed| *listed == kind),
        }
    }
}

/// One identifier claimed by two records, and which of them kept it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shadow {
    id: Box<str>,
    winner: Provenance,
    loser: Provenance,
}

impl Shadow {
    /// The contested identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The registration that answers to the identifier.
    #[must_use]
    pub const fn winner(&self) -> &Provenance {
        &self.winner
    }

    /// The registration that was kept out of the generation.
    #[must_use]
    pub const fn loser(&self) -> &Provenance {
        &self.loser
    }
}

impl fmt::Display for Shadow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{id}: {winner} shadows {loser}",
            id = self.id,
            winner = self.winner,
            loser = self.loser
        )
    }
}

/// Why a registration or a lookup was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The record's source could not be described.
    #[error(transparent)]
    Provenance(#[from] ProvenanceError),
    /// Two distinct registrations answer to one identifier and the policy
    /// prefers neither.
    #[error("{id} is registered by both {first} and {second}")]
    Duplicate {
        /// The contested identifier.
        id: Box<str>,
        /// The registration already present.
        first: Provenance,
        /// The registration that was refused.
        second: Provenance,
    },
    /// A generation would hold too many records.
    #[error("a registry generation has {actual} entries; the maximum is {maximum}")]
    Entries {
        /// The ceiling.
        maximum: usize,
        /// The requested count.
        actual: usize,
    },
    /// Aggregate retained data crossed its boundary.
    #[error("a registry generation retains {actual} bytes; the maximum is {maximum}")]
    Bytes {
        /// The ceiling.
        maximum: usize,
        /// The requested bytes.
        actual: usize,
    },
    /// No record answers to the identifier.
    #[error("{id} is not registered")]
    Unknown {
        /// The identifier looked for.
        id: Box<str>,
    },
    /// The staged set was built from a generation that is no longer current.
    #[error("the registry changed while a transaction was staged; stage it again")]
    Superseded,
    /// A handle from another generation was presented.
    #[error("the handle belongs to an earlier registry generation")]
    Stale,
}

/// Equality identity for one committed generation.
///
/// Pointer-based, so two generations with identical records are still two:
/// a handle minted from one says nothing about the other. The label is
/// descriptive evidence for a diagnostic, never a second way to resolve a
/// handle.
#[derive(Clone)]
pub struct RegistryGeneration(Arc<GenerationLabel>);

struct GenerationLabel {
    label: Box<str>,
}

impl RegistryGeneration {
    fn new() -> Self {
        Self(Arc::new(GenerationLabel {
            label: uuid::Uuid::now_v7().to_string().into(),
        }))
    }

    /// The opaque label a diagnostic may quote.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.0.label
    }
}

impl PartialEq for RegistryGeneration {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RegistryGeneration {}

impl fmt::Debug for RegistryGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RegistryGeneration([opaque])")
    }
}

/// A record found in one exact generation.
///
/// Not a reference to the record: resolving it through a snapshot rechecks the
/// generation first, so a handle captured before a reload fails clearly rather
/// than reaching a record the replacement no longer holds.
#[derive(Debug, Clone)]
pub struct RegistryHandle {
    generation: RegistryGeneration,
    index: usize,
}

impl RegistryHandle {
    /// The generation this handle resolves through.
    #[must_use]
    pub const fn generation(&self) -> &RegistryGeneration {
        &self.generation
    }
}

/// One immutable, registration-ordered generation of a registry.
pub struct RegistrySnapshot<T> {
    generation: RegistryGeneration,
    entries: Arc<[Arc<T>]>,
    shadows: Arc<[Shadow]>,
}

impl<T> Clone for RegistrySnapshot<T> {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation.clone(),
            entries: Arc::clone(&self.entries),
            shadows: Arc::clone(&self.shadows),
        }
    }
}

impl<T: Registered> fmt::Debug for RegistrySnapshot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistrySnapshot")
            .field("generation", &self.generation)
            .field(
                "entries",
                &self
                    .entries
                    .iter()
                    .map(|entry| entry.id())
                    .collect::<Vec<_>>(),
            )
            .field("shadows", &self.shadows)
            .finish()
    }
}

impl<T: Registered> RegistrySnapshot<T> {
    /// A generation with nothing in it.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            generation: RegistryGeneration::new(),
            entries: Arc::new([]),
            shadows: Arc::new([]),
        }
    }

    /// Which exact generation this is.
    #[must_use]
    pub const fn generation(&self) -> &RegistryGeneration {
        &self.generation
    }

    /// Every record, in the order it was registered.
    #[must_use]
    pub fn entries(&self) -> &[Arc<T>] {
        &self.entries
    }

    /// Every identifier a second record claimed, and who kept it.
    #[must_use]
    pub fn shadows(&self) -> &[Shadow] {
        &self.shadows
    }

    /// The record answering to `id`.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&Arc<T>> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    /// A handle to the record answering to `id`, bound to this generation.
    #[must_use]
    pub fn handle(&self, id: &str) -> Option<RegistryHandle> {
        self.entries
            .iter()
            .position(|entry| entry.id() == id)
            .map(|index| RegistryHandle {
                generation: self.generation.clone(),
                index,
            })
    }

    /// Resolves a handle only through the generation that minted it.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Stale`] for a handle from another generation, or one
    /// this generation cannot account for.
    pub fn resolve(&self, handle: &RegistryHandle) -> Result<&Arc<T>, RegistryError> {
        if handle.generation != self.generation {
            return Err(RegistryError::Stale);
        }
        self.entries.get(handle.index).ok_or(RegistryError::Stale)
    }

    /// What this generation holds, in receipt-safe terms.
    #[must_use]
    pub fn inspect(&self) -> RegistryReport {
        RegistryReport {
            generation: self.generation.label().into(),
            registered: self
                .entries
                .iter()
                .map(|entry| RegistryRow {
                    id: entry.id().into(),
                    source: entry.provenance().receipt(),
                })
                .collect(),
            shadowed: self
                .shadows
                .iter()
                .map(|shadow| RegistryRow {
                    id: shadow.id.clone(),
                    source: shadow.loser.receipt(),
                })
                .collect(),
        }
    }

    fn stage(&self, policy: Collision) -> Staged<T> {
        Staged {
            based_on: Some(self.generation.clone()),
            policy,
            entries: self.entries.to_vec(),
            shadows: self.shadows.to_vec(),
            retained: self.entries.iter().fold(0_usize, |total, entry| {
                total.saturating_add(entry.retained_bytes())
            }),
        }
    }
}

/// One registered or shadowed record, as capability inspection reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRow {
    id: Box<str>,
    source: SourceReceipt,
}

impl RegistryRow {
    /// The record's identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The receipt-safe source.
    #[must_use]
    pub const fn source(&self) -> &SourceReceipt {
        &self.source
    }
}

/// A receipt-safe account of one generation: what answers, and what was
/// contributed but kept out.
///
/// Neither list carries a label or a path; the fuller diagnostic is the
/// [`Shadow`] on the snapshot, for the wiring that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryReport {
    generation: Box<str>,
    registered: Vec<RegistryRow>,
    shadowed: Vec<RegistryRow>,
}

impl RegistryReport {
    /// The generation's opaque label.
    #[must_use]
    pub fn generation(&self) -> &str {
        &self.generation
    }

    /// Every record that answers to its identifier.
    #[must_use]
    pub fn registered(&self) -> &[RegistryRow] {
        &self.registered
    }

    /// Every record another one kept out.
    #[must_use]
    pub fn shadowed(&self) -> &[RegistryRow] {
        &self.shadowed
    }
}

/// A set of registrations being assembled for one atomic commit.
///
/// Validation happens as each record is staged, so a refusal names the exact
/// record; nothing here is visible to a reader until [`Registry::commit`]
/// swaps the whole set in. Dropping a staged set discards it.
pub struct Staged<T> {
    based_on: Option<RegistryGeneration>,
    policy: Collision,
    entries: Vec<Arc<T>>,
    shadows: Vec<Shadow>,
    retained: usize,
}

impl<T: Registered> Staged<T> {
    /// Adds one record.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Duplicate`] when a record already answers to the
    /// identifier and the policy prefers neither; [`RegistryError::Entries`]
    /// or [`RegistryError::Bytes`] when the set would cross a ceiling.
    pub fn register(&mut self, record: T) -> Result<(), RegistryError> {
        let record = Arc::new(record);
        let Some(index) = self
            .entries
            .iter()
            .position(|present| present.id() == record.id())
        else {
            return self.append(record);
        };

        let present = Arc::clone(self.entries.get(index).ok_or(RegistryError::Stale)?);
        let kept = self.policy.rank(present.provenance().kind());
        let offered = self.policy.rank(record.provenance().kind());
        match (kept, offered) {
            (Some(kept), Some(offered)) if offered < kept => {
                self.shadows.push(Shadow {
                    id: record.id().into(),
                    winner: record.provenance().clone(),
                    loser: present.provenance().clone(),
                });
                self.retained = self.retained.saturating_sub(present.retained_bytes());
                self.check_bytes(record.retained_bytes())?;
                self.retained = self.retained.saturating_add(record.retained_bytes());
                if let Some(slot) = self.entries.get_mut(index) {
                    *slot = record;
                }
                Ok(())
            }
            (Some(kept), Some(offered)) if offered > kept => {
                self.shadows.push(Shadow {
                    id: record.id().into(),
                    winner: present.provenance().clone(),
                    loser: record.provenance().clone(),
                });
                Ok(())
            }
            _ => Err(RegistryError::Duplicate {
                id: record.id().into(),
                first: present.provenance().clone(),
                second: record.provenance().clone(),
            }),
        }
    }

    /// Removes the record answering to `id`, and every shadow it cast or
    /// suffered.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Unknown`] when nothing answers to the identifier.
    pub fn deregister(&mut self, id: &str) -> Result<Arc<T>, RegistryError> {
        let index = self
            .entries
            .iter()
            .position(|present| present.id() == id)
            .ok_or_else(|| RegistryError::Unknown { id: id.into() })?;
        let removed = self.entries.remove(index);
        self.shadows.retain(|shadow| &*shadow.id != id);
        self.retained = self.retained.saturating_sub(removed.retained_bytes());
        Ok(removed)
    }

    /// The identifiers staged so far, in registration order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.id())
    }

    fn append(&mut self, record: Arc<T>) -> Result<(), RegistryError> {
        if self.entries.len() >= REGISTRY_ENTRIES {
            return Err(RegistryError::Entries {
                maximum: REGISTRY_ENTRIES,
                actual: self.entries.len().saturating_add(1),
            });
        }
        self.check_bytes(record.retained_bytes())?;
        self.retained = self.retained.saturating_add(record.retained_bytes());
        self.entries.push(record);
        Ok(())
    }

    fn check_bytes(&self, adding: usize) -> Result<(), RegistryError> {
        let retained = self.retained.saturating_add(adding);
        if retained > REGISTRY_BYTES {
            return Err(RegistryError::Bytes {
                maximum: REGISTRY_BYTES,
                actual: retained,
            });
        }
        Ok(())
    }
}

impl<T: Registered> fmt::Debug for Staged<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Staged")
            .field("based_on", &self.based_on)
            .field("policy", &self.policy)
            .field("entries", &self.ids().collect::<Vec<_>>())
            .field("shadows", &self.shadows)
            .finish_non_exhaustive()
    }
}

/// A live registry: one current generation, replaced whole.
pub struct Registry<T> {
    policy: Collision,
    current: Mutex<RegistrySnapshot<T>>,
}

impl<T: Registered> Registry<T> {
    /// An empty registry under `policy`.
    #[must_use]
    pub fn new(policy: Collision) -> Self {
        Self {
            policy,
            current: Mutex::new(RegistrySnapshot::empty()),
        }
    }

    /// The collision policy every commit is checked under.
    #[must_use]
    pub const fn policy(&self) -> Collision {
        self.policy
    }

    /// The current generation.
    #[must_use]
    pub fn snapshot(&self) -> RegistrySnapshot<T> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Starts a transaction from the current generation.
    ///
    /// The commit is refused if another transaction lands first, so two
    /// writers cannot each build on a generation the other has replaced.
    #[must_use]
    pub fn stage(&self) -> Staged<T> {
        self.snapshot().stage(self.policy)
    }

    /// Starts a transaction from nothing: a reload, whose commit replaces
    /// every record whatever the current generation holds.
    #[must_use]
    pub fn replacing(&self) -> Staged<T> {
        Staged {
            based_on: None,
            policy: self.policy,
            entries: Vec::new(),
            shadows: Vec::new(),
            retained: 0,
        }
    }

    /// Makes a staged set the current generation, all at once.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Superseded`] when the set was staged from a generation
    /// that is no longer current. Nothing changes; stage again from the new one.
    pub fn commit(&self, staged: Staged<T>) -> Result<RegistrySnapshot<T>, RegistryError> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(based_on) = &staged.based_on
            && *based_on != current.generation
        {
            return Err(RegistryError::Superseded);
        }
        let next = RegistrySnapshot {
            generation: RegistryGeneration::new(),
            entries: staged.entries.into(),
            shadows: staged.shadows.into(),
        };
        *current = next.clone();
        Ok(next)
    }
}

impl<T> fmt::Debug for Registry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Record {
        id: &'static str,
        provenance: Provenance,
        bytes: usize,
    }

    impl Record {
        fn from(kind: SourceKind, id: &'static str, source: &str) -> Self {
            Self {
                id,
                provenance: Provenance::new(kind, source, format!("{source} ({id})")).unwrap(),
                bytes: 8,
            }
        }

        fn weighing(mut self, bytes: usize) -> Self {
            self.bytes = bytes;
            self
        }
    }

    impl Registered for Record {
        fn id(&self) -> &str {
            self.id
        }

        fn provenance(&self) -> &Provenance {
            &self.provenance
        }

        fn retained_bytes(&self) -> usize {
            self.bytes
        }
    }

    const USER_OVER_PROJECT: Collision =
        Collision::Prefer(&[SourceKind::User, SourceKind::Project]);

    #[test]
    fn a_staged_set_is_invisible_until_it_is_committed() {
        let registry = Registry::new(Collision::Refuse);
        let mut staged = registry.stage();
        staged
            .register(Record::from(SourceKind::Builtin, "read", "crucible:read"))
            .unwrap();

        assert!(registry.snapshot().entries().is_empty());

        let committed = registry.commit(staged).unwrap();
        assert_eq!(committed.entries().len(), 1);
        assert_eq!(registry.snapshot().generation(), committed.generation());
        assert!(registry.snapshot().find("read").is_some());
    }

    #[test]
    fn a_refused_record_leaves_no_trace_of_the_batch() {
        let registry = Registry::new(Collision::Refuse);
        let mut first = registry.stage();
        first
            .register(Record::from(SourceKind::Builtin, "read", "crucible:read"))
            .unwrap();
        registry.commit(first).unwrap();

        let mut batch = registry.stage();
        batch
            .register(Record::from(SourceKind::Extension, "grep", "ext:search"))
            .unwrap();
        let refused = batch
            .register(Record::from(SourceKind::Extension, "read", "ext:search"))
            .unwrap_err();
        assert!(
            matches!(&refused, RegistryError::Duplicate { id, first, second }
                if &**id == "read" && first.id() == "crucible:read" && second.id() == "ext:search"),
            "{refused:?}"
        );
        drop(batch);

        let current = registry.snapshot();
        assert_eq!(
            current.entries().len(),
            1,
            "the failed batch left a record behind"
        );
        assert!(current.find("grep").is_none());
    }

    #[test]
    fn a_transaction_staged_from_a_replaced_generation_is_refused() {
        let registry = Registry::new(Collision::Refuse);
        let mut first = registry.stage();
        let mut second = registry.stage();
        first
            .register(Record::from(SourceKind::User, "one", "user:a"))
            .unwrap();
        second
            .register(Record::from(SourceKind::User, "two", "user:b"))
            .unwrap();

        registry.commit(first).unwrap();
        assert_eq!(
            registry.commit(second).unwrap_err(),
            RegistryError::Superseded
        );
        assert!(registry.snapshot().find("two").is_none());
    }

    #[test]
    fn precedence_is_declared_and_names_both_sides() {
        let registry = Registry::new(USER_OVER_PROJECT);
        let mut staged = registry.stage();
        staged
            .register(Record::from(
                SourceKind::Project,
                "fmt",
                "project:.crucible",
            ))
            .unwrap();
        staged
            .register(Record::from(SourceKind::User, "fmt", "user:~/.crucible"))
            .unwrap();
        let committed = registry.commit(staged).unwrap();

        let winner = committed.find("fmt").unwrap();
        assert_eq!(winner.provenance().kind(), SourceKind::User);
        let [shadow] = committed.shadows() else {
            panic!(
                "one shadow diagnostic was expected: {:?}",
                committed.shadows()
            );
        };
        assert_eq!(shadow.id(), "fmt");
        assert_eq!(shadow.winner().id(), "user:~/.crucible");
        assert_eq!(shadow.loser().id(), "project:.crucible");
        assert_eq!(
            shadow.to_string(),
            "fmt: user:~/.crucible (fmt) [user:user:~/.crucible] shadows project:.crucible (fmt) [project:project:.crucible]"
        );
    }

    #[test]
    fn the_order_records_arrive_in_does_not_decide_precedence() {
        let registry = Registry::new(USER_OVER_PROJECT);
        let mut staged = registry.stage();
        staged
            .register(Record::from(SourceKind::User, "fmt", "user:~/.crucible"))
            .unwrap();
        staged
            .register(Record::from(
                SourceKind::Project,
                "fmt",
                "project:.crucible",
            ))
            .unwrap();
        let committed = registry.commit(staged).unwrap();

        assert_eq!(
            committed.find("fmt").unwrap().provenance().kind(),
            SourceKind::User
        );
        assert_eq!(committed.entries().len(), 1);
        let [shadow] = committed.shadows() else {
            panic!(
                "one shadow diagnostic was expected: {:?}",
                committed.shadows()
            );
        };
        assert_eq!(shadow.loser().kind(), SourceKind::Project);
    }

    #[test]
    fn two_records_of_one_rank_or_an_unlisted_kind_are_refused_even_under_precedence() {
        let registry = Registry::new(USER_OVER_PROJECT);
        let mut staged = registry.stage();
        staged
            .register(Record::from(SourceKind::User, "fmt", "user:a"))
            .unwrap();
        assert!(matches!(
            staged.register(Record::from(SourceKind::User, "fmt", "user:b")),
            Err(RegistryError::Duplicate { .. })
        ));
        assert!(matches!(
            staged.register(Record::from(SourceKind::Extension, "fmt", "ext:c")),
            Err(RegistryError::Duplicate { .. })
        ));
    }

    #[test]
    fn a_handle_resolves_only_through_the_generation_that_minted_it() {
        let registry = Registry::new(Collision::Refuse);
        let mut staged = registry.stage();
        staged
            .register(Record::from(SourceKind::Builtin, "read", "crucible:read"))
            .unwrap();
        let first = registry.commit(staged).unwrap();
        let handle = first.handle("read").unwrap();
        assert_eq!(first.resolve(&handle).unwrap().id(), "read");

        let mut reload = registry.replacing();
        reload
            .register(Record::from(SourceKind::Builtin, "read", "crucible:read"))
            .unwrap();
        let second = registry.commit(reload).unwrap();

        assert_eq!(second.resolve(&handle).unwrap_err(), RegistryError::Stale);
        assert!(second.handle("read").is_some());
        assert_ne!(first.generation(), second.generation());
    }

    #[test]
    fn a_reload_replaces_every_record_whatever_the_current_generation_holds() {
        let registry = Registry::new(Collision::Refuse);
        let mut staged = registry.stage();
        staged
            .register(Record::from(SourceKind::Extension, "old", "ext:v1"))
            .unwrap();
        registry.commit(staged).unwrap();

        let mut reload = registry.replacing();
        reload
            .register(Record::from(SourceKind::Extension, "new", "ext:v2"))
            .unwrap();
        // Another writer lands in between: a reload is not built on a
        // generation, so it still commits.
        let mut aside = registry.stage();
        aside
            .register(Record::from(SourceKind::User, "aside", "user:x"))
            .unwrap();
        registry.commit(aside).unwrap();

        let committed = registry.commit(reload).unwrap();
        assert!(committed.find("old").is_none());
        assert!(committed.find("aside").is_none());
        assert!(committed.find("new").is_some());
    }

    #[test]
    fn deregistering_removes_the_record_and_its_shadows() {
        let registry = Registry::new(USER_OVER_PROJECT);
        let mut staged = registry.stage();
        staged
            .register(Record::from(
                SourceKind::Project,
                "fmt",
                "project:.crucible",
            ))
            .unwrap();
        staged
            .register(Record::from(SourceKind::User, "fmt", "user:~/.crucible"))
            .unwrap();
        staged
            .register(Record::from(SourceKind::User, "lint", "user:~/.crucible"))
            .unwrap();
        let removed = staged.deregister("fmt").unwrap();
        assert_eq!(removed.provenance().kind(), SourceKind::User);
        assert_eq!(
            staged.deregister("fmt").unwrap_err(),
            RegistryError::Unknown { id: "fmt".into() }
        );

        let committed = registry.commit(staged).unwrap();
        assert!(committed.shadows().is_empty());
        assert_eq!(committed.ids(), ["lint"]);
    }

    #[test]
    fn the_ceilings_hold_across_a_transaction() {
        let registry = Registry::new(Collision::Refuse);
        let mut staged = registry.stage();
        staged
            .register(Record::from(SourceKind::User, "big", "user:a").weighing(REGISTRY_BYTES))
            .unwrap();
        let over = staged
            .register(Record::from(SourceKind::User, "more", "user:a").weighing(1))
            .unwrap_err();
        assert!(matches!(over, RegistryError::Bytes { .. }), "{over:?}");

        // Precedence replacement is charged for what it adds, not stacked on
        // what it removes.
        let registry = Registry::new(USER_OVER_PROJECT);
        let mut staged = registry.stage();
        staged
            .register(
                Record::from(SourceKind::Project, "fmt", "project:a").weighing(REGISTRY_BYTES),
            )
            .unwrap();
        staged
            .register(Record::from(SourceKind::User, "fmt", "user:a").weighing(REGISTRY_BYTES))
            .unwrap();
        assert_eq!(registry.commit(staged).unwrap().entries().len(), 1);
    }

    #[test]
    fn inspection_reports_receipts_and_never_labels() {
        let registry = Registry::new(USER_OVER_PROJECT);
        let mut staged = registry.stage();
        staged
            .register(Record::from(
                SourceKind::Project,
                "fmt",
                "project:/secret/path",
            ))
            .unwrap();
        staged
            .register(Record::from(SourceKind::User, "fmt", "user:home"))
            .unwrap();
        let report = registry.commit(staged).unwrap().inspect();

        let [registered] = report.registered() else {
            panic!("one registered row was expected: {report:?}");
        };
        assert_eq!(registered.id(), "fmt");
        assert_eq!(registered.source().kind(), SourceKind::User);
        let [shadowed] = report.shadowed() else {
            panic!("one shadowed row was expected: {report:?}");
        };
        assert_eq!(shadowed.source().id(), "project:/secret/path");
        assert!(
            !format!("{report:?}").contains("(fmt)"),
            "a label reached the report: {report:?}"
        );
        assert!(!report.generation().is_empty());
    }

    #[test]
    fn a_provenance_is_bounded_and_redacts_its_label_from_debug() {
        assert!(matches!(
            Provenance::new(SourceKind::User, "", "label"),
            Err(ProvenanceError::Empty { field: "source id" })
        ));
        assert!(matches!(
            Provenance::new(SourceKind::User, "x".repeat(SOURCE_ID_BYTES + 1), "label"),
            Err(ProvenanceError::TooLong {
                field: "source id",
                ..
            })
        ));
        let provenance = Provenance::new(SourceKind::User, "user:a", "/home/me/.crucible").unwrap();
        assert!(!format!("{provenance:?}").contains("/home/me"));
        assert_eq!(provenance.to_string(), "/home/me/.crucible [user:user:a]");
        assert_eq!(provenance.receipt().id(), "user:a");
    }

    impl<T: Registered> RegistrySnapshot<T> {
        fn ids(&self) -> Vec<&str> {
            self.entries.iter().map(|entry| entry.id()).collect()
        }
    }
}
