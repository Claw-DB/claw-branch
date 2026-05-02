//! Sandbox teardown and resource cleanup helpers.

use crate::error::BranchResult;

/// Performs sandbox cleanup with a no-op default implementation.
pub async fn cleanup_environment() -> BranchResult<()> {
    Ok(())
}