//! Sandbox outcome evaluation: diff, metrics, and recommendation.

use crate::{
    config::BranchConfig,
    diff::extractor::DiffExtractor,
    error::BranchResult,
    sandbox::environment::SimulationEnvironment,
    types::{Branch, BranchMetrics, DiffResult},
};
use std::sync::Arc;

/// Evaluates a completed sandbox run by comparing the sandbox branch against its parent.
///
/// # Example
/// ```rust,ignore
/// let report = SandboxEvaluator.evaluate(&env, &parent).await?;
/// match report.recommendation {
///     Recommendation::Commit => engine.commit_to_trunk(env.branch.id).await?,
///     Recommendation::Discard => engine.discard(env.branch.id).await?,
///     Recommendation::NeedsReview(notes) => println!("Review: {:?}", notes),
/// }
/// ```
pub struct SandboxEvaluator;

/// The decision produced by the evaluator after reviewing a sandbox run.
#[derive(Debug, Clone)]
pub enum Recommendation {
    /// The sandbox changes are safe to promote to the parent or trunk.
    Commit,
    /// The sandbox changes should be discarded without promotion.
    Discard,
    /// Manual review is needed before a decision can be made.
    NeedsReview(Vec<String>),
}

/// Aggregates diff results, branch metrics, and a promotion recommendation.
#[derive(Debug)]
pub struct EvaluationReport {
    /// The diff between the sandbox branch and its parent.
    pub diff: DiffResult,
    /// The runtime metrics recorded for the sandbox branch.
    pub metrics: BranchMetrics,
    /// The evaluator's promotion recommendation.
    pub recommendation: Recommendation,
}

impl SandboxEvaluator {
    /// Evaluates the sandbox environment against its parent branch.
    ///
    /// Diffs `env.branch` vs `parent`, then calls [`recommend`] with the diff and metrics
    /// to produce an [`EvaluationReport`].
    pub async fn evaluate(
        &self,
        env: &SimulationEnvironment,
        parent: &Branch,
        config: Arc<BranchConfig>,
    ) -> BranchResult<EvaluationReport> {
        let extractor = DiffExtractor::new(Arc::clone(&config));
        let diff = extractor.diff(&env.branch, parent, None).await?;
        let metrics = env.branch.metrics.clone();
        let threshold = config.divergence_threshold;
        let recommendation = recommend(&diff, &metrics, threshold);

        Ok(EvaluationReport {
            diff,
            metrics,
            recommendation,
        })
    }
}

/// Produces a [`Recommendation`] from diff statistics and branch metrics.
///
/// Rules:
/// - `divergence_score >= threshold` → [`Recommendation::NeedsReview`]
/// - No changes detected → [`Recommendation::Discard`]
/// - Otherwise → [`Recommendation::Commit`]
pub fn recommend(diff: &DiffResult, metrics: &BranchMetrics, threshold: f64) -> Recommendation {
    let mut notes: Vec<String> = Vec::new();

    if diff.divergence_score >= threshold {
        notes.push(format!(
            "divergence score {:.3} meets or exceeds threshold {:.3}",
            diff.divergence_score, threshold
        ));
    }

    if metrics.op_count == 0
        && diff.stats.added == 0
        && diff.stats.modified == 0
        && diff.stats.removed == 0
    {
        return Recommendation::Discard;
    }

    if !notes.is_empty() {
        return Recommendation::NeedsReview(notes);
    }

    Recommendation::Commit
}
