//! Snapshot creation, verification, and cleanup utilities.

/// SQLite file snapshot copier.
pub mod copier;
/// Snapshot garbage collector.
pub mod gc;
/// Snapshot manifest persistence.
pub mod manifest;
/// Snapshot integrity verification.
pub mod verifier;
