//! Branch registry, lifecycle management, and naming helpers.

/// Branch lifecycle operations.
pub mod lifecycle;
/// Branch model aliases and metadata helpers.
pub mod model;
/// Branch name validation and slug generation.
pub mod naming;
/// SQLite-backed branch registry storage.
pub mod store;

pub use lifecycle::BranchLifecycle;
pub use model::BranchMetadata;
pub use naming::NamingValidator;
pub use store::BranchStore;