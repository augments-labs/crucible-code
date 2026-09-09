//! Bounded, source-aware registries and the snapshots they publish.
//!
//! One registry owns a name space: it decides which source wins a collision,
//! records why, and publishes an immutable generation that consumers read
//! without holding a lock. Nothing here knows what is being registered, which
//! is why tools, extension contributions and providers can share it.

mod registry;

pub use registry::{
    Collision, Provenance, ProvenanceError, REGISTRY_BYTES, REGISTRY_ENTRIES, Registered, Registry,
    RegistryError, RegistryGeneration, RegistryHandle, RegistryReport, RegistryRow,
    RegistrySnapshot, SOURCE_ID_BYTES, SOURCE_LABEL_BYTES, Shadow, SourceKind, SourceReceipt,
    Staged,
};
