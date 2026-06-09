//! Verification debt tracking for the IVE module.
//!
//! Verification debt represents properties that have not yet been formally
//! verified but should be. This module provides a priority-ordered queue
//! of outstanding verification obligations, with debt scoring, aging,
//! automatic resolution, and comprehensive reporting.

use crate::result::{ConfidenceLevel, VerificationResult, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

/// The priority of a verification debt item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    /// Must be resolved immediately — safety-critical property.
    Critical,
    /// Should be resolved soon — important for correctness.
    High,
    /// Can be deferred — nice-to-have verification.
    Medium,
    /// Low priority — defensive verification.
    Low,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "CRITICAL"),
            Self::High => write!(f, "HIGH"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::Low => write!(f, "LOW"),
        }
    }
}

impl Priority {
    /// Returns a numeric value for priority (higher = more urgent).
    pub fn weight(self) -> f64 {
        match self {
            Self::Critical => 1.0,
            Self::High => 0.75,
            Self::Medium => 0.5,
            Self::Low => 0.25,
        }
    }

    /// Elevate the priority by one level (if possible).
    pub fn elevate(self) -> Self {
        match self {
            Self::Critical => Self::Critical,
            Self::High => Self::Critical,
            Self::Medium => Self::High,
            Self::Low => Self::Medium,
        }
    }
}

// ---------------------------------------------------------------------------
// DebtStatus
// ---------------------------------------------------------------------------

/// The current status of a verification debt item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DebtStatus {
    /// Not yet started.
    Pending,
    /// Currently being worked on.
    InProgress,
    /// Verification completed / resolved.
    Resolved,
}

impl fmt::Display for DebtStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "PENDING"),
            Self::InProgress => write!(f, "IN_PROGRESS"),
            Self::Resolved => write!(f, "RESOLVED"),
        }
    }
}

// ---------------------------------------------------------------------------
// DebtItem
// ---------------------------------------------------------------------------

/// A single verification debt item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebtItem {
    /// The property or invariant that needs verification.
    pub property: String,
    /// The priority of this debt item.
    pub priority: Priority,
    /// The current resolution status.
    pub status: DebtStatus,
    /// Timestamp when this item was added (epoch seconds).
    pub added_at: i64,
}

impl DebtItem {
    /// Construct a new debt item.
    pub fn new(property: impl Into<String>, priority: Priority, added_at: i64) -> Self {
        Self {
            property: property.into(),
            priority,
            status: DebtStatus::Pending,
            added_at,
        }
    }

    /// Mark this item as in progress.
    pub fn start(&mut self) {
        self.status = DebtStatus::InProgress;
    }

    /// Mark this item as resolved.
    pub fn resolve(&mut self) {
        self.status = DebtStatus::Resolved;
    }

    /// Returns `true` if this item is still pending.
    pub fn is_pending(&self) -> bool {
        self.status == DebtStatus::Pending
    }

    /// Returns `true` if this item has been resolved.
    pub fn is_resolved(&self) -> bool {
        self.status == DebtStatus::Resolved
    }
}

// ---------------------------------------------------------------------------
// VerificationDebt
// ---------------------------------------------------------------------------

/// A collection of verification debt items, ordered by priority.
///
/// The debt tracker helps the VUMA system understand which properties still
/// need formal verification, allowing the compiler to emit appropriate
/// warnings and the developer to prioritize verification effort.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct VerificationDebt {
    /// The collection of debt items.
    items: Vec<DebtItem>,
}

impl VerificationDebt {
    /// Construct a new, empty verification debt tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new debt item and return its index.
    pub fn add(&mut self, item: DebtItem) -> usize {
        let idx = self.items.len();
        self.items.push(item);
        idx
    }

    /// Resolve the debt item at the given index.
    ///
    /// Returns `true` if the item existed and was resolved.
    pub fn resolve(&mut self, index: usize) -> bool {
        if let Some(item) = self.items.get_mut(index) {
            item.resolve();
            true
        } else {
            false
        }
    }

    /// Return the highest-priority unresolved critical debt item, if any.
    pub fn next_critical(&self) -> Option<&DebtItem> {
        self.items
            .iter()
            .filter(|i| i.status != DebtStatus::Resolved && i.priority == Priority::Critical)
            .min_by_key(|i| i.added_at)
    }

    /// Return the total number of unresolved debt items.
    pub fn total_debt(&self) -> usize {
        self.items.iter().filter(|i| !i.is_resolved()).count()
    }

    /// Return the total number of debt items (including resolved).
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if there are no debt items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Return an iterator over unresolved debt items, sorted by priority.
    pub fn outstanding(&self) -> impl Iterator<Item = &DebtItem> {
        let mut pending: Vec<&DebtItem> = self
            .items
            .iter()
            .filter(|i| !i.is_resolved())
            .collect();
        pending.sort_by_key(|i| i.priority);
        pending.into_iter()
    }

    /// Return the number of debt items at each priority level (unresolved only).
    pub fn debt_by_priority(&self) -> [(Priority, usize); 4] {
        let mut counts = [0usize; 4];
        for item in &self.items {
            if item.is_resolved() {
                continue;
            }
            let idx = match item.priority {
                Priority::Critical => 0,
                Priority::High => 1,
                Priority::Medium => 2,
                Priority::Low => 3,
            };
            counts[idx] += 1;
        }
        [
            (Priority::Critical, counts[0]),
            (Priority::High, counts[1]),
            (Priority::Medium, counts[2]),
            (Priority::Low, counts[3]),
        ]
    }
}

impl fmt::Display for VerificationDebt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Verification Debt ({} outstanding):", self.total_debt())?;
        for (priority, count) in self.debt_by_priority() {
            writeln!(f, "  {priority}: {count}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DebtContext
// ---------------------------------------------------------------------------

/// Additional context information used for debt scoring.
///
/// The context captures situational factors that affect the urgency and
/// severity of a verification debt item. For example, a debt in library
/// code that is accessed concurrently with security implications should
/// be scored much higher than the same debt in application code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebtContext {
    /// Whether the debt is in library/API code (affects downstream users).
    pub is_library_code: bool,
    /// Whether the code has concurrent access patterns (races possible).
    pub has_concurrent_access: bool,
    /// Whether the code is on a performance-critical path.
    pub is_performance_critical: bool,
    /// Whether the code has security implications (auth, crypto, etc.).
    pub has_security_implications: bool,
}

impl Default for DebtContext {
    fn default() -> Self {
        Self {
            is_library_code: false,
            has_concurrent_access: false,
            is_performance_critical: false,
            has_security_implications: false,
        }
    }
}

impl DebtContext {
    /// Construct a new debt context with all flags set to false.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set library code flag.
    pub fn with_library_code(mut self, val: bool) -> Self {
        self.is_library_code = val;
        self
    }

    /// Builder: set concurrent access flag.
    pub fn with_concurrent_access(mut self, val: bool) -> Self {
        self.has_concurrent_access = val;
        self
    }

    /// Builder: set performance critical flag.
    pub fn with_performance_critical(mut self, val: bool) -> Self {
        self.is_performance_critical = val;
        self
    }

    /// Builder: set security implications flag.
    pub fn with_security_implications(mut self, val: bool) -> Self {
        self.has_security_implications = val;
        self
    }
}

// ---------------------------------------------------------------------------
// DebtScore
// ---------------------------------------------------------------------------

/// A multi-factor scoring model for verification debt items.
///
/// The score combines severity (how bad the violation is), likelihood
/// (estimated probability of a real bug), and impact (estimated runtime
/// consequence) into a composite score that drives prioritization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebtScore {
    /// Severity of the violation: 0.0 (proven safe) to 1.0 (hard violation).
    pub severity: f64,
    /// Estimated probability that the debt corresponds to a real bug: 0.0-1.0.
    pub likelihood: f64,
    /// Estimated runtime impact if the bug manifests: 0.0-1.0.
    pub impact: f64,
    /// Weighted combination of the three factors.
    pub composite: f64,
}

impl DebtScore {
    /// Weights for the composite score.
    const SEVERITY_WEIGHT: f64 = 0.4;
    const LIKELIHOOD_WEIGHT: f64 = 0.3;
    const IMPACT_WEIGHT: f64 = 0.3;

    /// Compute a debt score from a verification result and context.
    ///
    /// The severity is derived from the verification status (violated is worst,
    /// proven is best). The likelihood and impact are estimated from the
    /// debt context flags (concurrent access, security implications, etc.).
    pub fn compute(violation: &VerificationResult, context: &DebtContext) -> Self {
        // Severity is based purely on the verification status.
        let severity = match &violation.status {
            VerificationStatus::Violated { .. } => 1.0,
            VerificationStatus::Unverified { .. } => 0.6,
            VerificationStatus::ProbablySafe { .. } => 0.3,
            VerificationStatus::Proven => 0.0,
        };

        // Likelihood: base rate modified by context flags.
        let mut likelihood: f64 = 0.3;
        if context.has_concurrent_access {
            likelihood += 0.35;
        }
        if context.has_security_implications {
            likelihood += 0.15;
        }
        if context.is_library_code {
            likelihood += 0.1;
        }
        if context.is_performance_critical {
            likelihood += 0.1;
        }
        let likelihood: f64 = likelihood.min(1.0_f64);

        // Impact: determined by what the code affects at runtime.
        let impact = if context.has_security_implications {
            0.95
        } else if context.has_concurrent_access {
            0.8
        } else if context.is_library_code {
            0.65
        } else if context.is_performance_critical {
            0.55
        } else {
            0.3
        };

        let composite = Self::SEVERITY_WEIGHT * severity
            + Self::LIKELIHOOD_WEIGHT * likelihood
            + Self::IMPACT_WEIGHT * impact;

        Self {
            severity,
            likelihood,
            impact,
            composite,
        }
    }

    /// Compute a debt score with default (empty) context.
    pub fn compute_default(violation: &VerificationResult) -> Self {
        Self::compute(violation, &DebtContext::default())
    }

    /// Derive a priority level from this score.
    pub fn to_priority(&self) -> Priority {
        if self.composite >= 0.8 {
            Priority::Critical
        } else if self.composite >= 0.6 {
            Priority::High
        } else if self.composite >= 0.35 {
            Priority::Medium
        } else {
            Priority::Low
        }
    }
}

impl fmt::Display for DebtScore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DebtScore(severity={:.2}, likelihood={:.2}, impact={:.2}, composite={:.2})",
            self.severity, self.likelihood, self.impact, self.composite
        )
    }
}

// ---------------------------------------------------------------------------
// AgedDebt
// ---------------------------------------------------------------------------

/// A debt item with aging information applied.
///
/// The longer a debt remains unresolved, the higher its effective priority.
/// The `age_factor` increases with age (capped at 2.0) and drives the
/// `adjusted_priority` which may be elevated above the original.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgedDebt {
    /// The underlying debt item.
    pub debt: DebtItem,
    /// How long the debt has been unresolved.
    pub age: Duration,
    /// Aging multiplier: increases with age, caps at 2.0.
    pub age_factor: f64,
    /// The priority adjusted for aging (may be elevated from the original).
    pub adjusted_priority: Priority,
}

impl AgedDebt {
    /// Compute the age factor from a duration.
    ///
    /// The formula increases the factor by 0.1 per day, starting from 1.0,
    /// with a hard cap at 2.0.
    pub fn compute_age_factor(age: Duration) -> f64 {
        let days = age.as_secs_f64() / 86400.0;
        (1.0 + days * 0.1).min(2.0)
    }

    /// Compute the adjusted priority given the original priority and age factor.
    ///
    /// If the age factor reaches 1.5, the priority is elevated by one level.
    /// If the age factor reaches 1.8, it is elevated by two levels.
    pub fn compute_adjusted_priority(original: Priority, age_factor: f64) -> Priority {
        if age_factor >= 1.8 {
            original.elevate().elevate()
        } else if age_factor >= 1.5 {
            original.elevate()
        } else {
            original
        }
    }

    /// Compute the effective composite score for sorting.
    ///
    /// Combines the debt score composite with the age factor to produce
    /// a single sortable value.
    pub fn effective_score(&self, score: &DebtScore) -> f64 {
        score.composite * self.age_factor
    }
}

impl fmt::Display for AgedDebt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = self.age.as_secs_f64() / 86400.0;
        write!(
            f,
            "AgedDebt({} [{}], age={:.1}d, factor={:.2}, adjusted={})",
            self.debt.property,
            self.debt.priority,
            days,
            self.age_factor,
            self.adjusted_priority
        )
    }
}

// ---------------------------------------------------------------------------
// AutoResolution
// ---------------------------------------------------------------------------

/// An automatic resolution applied to a debt item.
///
/// When a re-verification produces a stronger result than what originally
/// created the debt, the debt can be automatically resolved or adjusted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AutoResolution {
    /// The debt was resolved because the proof was strengthened.
    StrengthenedProof {
        /// The ID of the resolved debt.
        debt_id: u64,
        /// The new confidence level after strengthening.
        new_confidence: ConfidenceLevel,
    },
    /// The requirement was weakened, making the debt no longer relevant.
    WeakenedRequirement {
        /// The ID of the resolved debt.
        debt_id: u64,
        /// Why the requirement was weakened.
        reason: String,
    },
    /// The old debt was superseded by a new debt with a stronger result.
    SupersededByNewProof {
        /// The old debt that was resolved.
        old_debt: u64,
        /// The new debt that supersedes it.
        new_debt: u64,
    },
    /// The context changed, altering the severity of the debt.
    ContextChanged {
        /// The ID of the affected debt.
        debt_id: u64,
        /// The new severity score.
        new_severity: f64,
    },
}

impl fmt::Display for AutoResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StrengthenedProof {
                debt_id,
                new_confidence,
            } => write!(
                f,
                "StrengthenedProof(debt={}, confidence={})",
                debt_id, new_confidence
            ),
            Self::WeakenedRequirement { debt_id, reason } => {
                write!(f, "WeakenedRequirement(debt={}, reason={})", debt_id, reason)
            }
            Self::SupersededByNewProof { old_debt, new_debt } => {
                write!(f, "SupersededByNewProof(old={}, new={})", old_debt, new_debt)
            }
            Self::ContextChanged {
                debt_id,
                new_severity,
            } => write!(
                f,
                "ContextChanged(debt={}, new_severity={:.2})",
                debt_id, new_severity
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// DebtTrend
// ---------------------------------------------------------------------------

/// The trend direction for the verification debt count over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DebtTrend {
    /// Debt count is increasing (new debts faster than resolutions).
    Increasing,
    /// Debt count is roughly stable.
    Stable,
    /// Debt count is decreasing (resolutions faster than new debts).
    Decreasing,
}

impl fmt::Display for DebtTrend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Increasing => write!(f, "INCREASING"),
            Self::Stable => write!(f, "STABLE"),
            Self::Decreasing => write!(f, "DECREASING"),
        }
    }
}

// ---------------------------------------------------------------------------
// DebtReport
// ---------------------------------------------------------------------------

/// A comprehensive report on the current state of verification debt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebtReport {
    /// Total number of unresolved debt items.
    pub total_debt_items: usize,
    /// Debt count broken down by priority level.
    pub by_priority: HashMap<Priority, usize>,
    /// Debt count broken down by invariant/property name.
    pub by_invariant: HashMap<String, usize>,
    /// Age of the oldest unresolved debt item, if any.
    pub oldest_debt_age: Option<Duration>,
    /// Average age of unresolved debt items (zero if no debts).
    pub average_age: Duration,
    /// Number of debts that have been automatically resolved.
    pub auto_resolved_count: usize,
    /// The top 5 most critical aged debt items (by effective score).
    pub top_5_critical: Vec<AgedDebt>,
    /// The trend direction for the debt count.
    pub debt_trend: DebtTrend,
}

impl fmt::Display for DebtReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Verification Debt Report ===")?;
        writeln!(f, "Total debt items: {}", self.total_debt_items)?;
        writeln!(f, "Auto-resolved: {}", self.auto_resolved_count)?;
        writeln!(f, "Trend: {}", self.debt_trend)?;
        if let Some(oldest) = self.oldest_debt_age {
            let days = oldest.as_secs_f64() / 86400.0;
            writeln!(f, "Oldest debt: {:.1} days", days)?;
        }
        let avg_days = self.average_age.as_secs_f64() / 86400.0;
        writeln!(f, "Average age: {:.1} days", avg_days)?;
        writeln!(f, "By priority:")?;
        for pri in &[Priority::Critical, Priority::High, Priority::Medium, Priority::Low] {
            let count = self.by_priority.get(pri).copied().unwrap_or(0);
            writeln!(f, "  {}: {}", pri, count)?;
        }
        if !self.top_5_critical.is_empty() {
            writeln!(f, "Top critical debts:")?;
            for ad in &self.top_5_critical {
                writeln!(f, "  {}", ad)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TrackedDebt (internal)
// ---------------------------------------------------------------------------

/// Internal representation of a tracked debt item within the tracker.
///
/// Stores the original debt, its score, context, and creation time for
/// aging computation. Not serialized directly; the tracker reconstructs
/// `Instant` from epoch-relative timestamps on deserialization.
#[derive(Debug, Clone)]
struct TrackedDebt {
    /// Unique identifier for this tracked debt.
    id: u64,
    /// The debt item being tracked.
    item: DebtItem,
    /// The computed score for this debt.
    score: DebtScore,
    /// The context under which this debt was scored.
    context: DebtContext,
    /// When this debt was created (used for aging).
    created_at: Instant,
    /// The original priority before any aging adjustment.
    original_priority: Priority,
}

// ---------------------------------------------------------------------------
// VerificationDebtTracker
// ---------------------------------------------------------------------------

/// An advanced verification debt tracker with scoring, aging, and auto-resolution.
///
/// The tracker maintains a collection of debt items with their associated
/// scores and contexts, applies aging to increase the effective priority
/// of long-standing debts, and can automatically resolve debts when
/// re-verification produces stronger results.
#[derive(Debug)]
pub struct VerificationDebtTracker {
    /// Tracked debts.
    debts: Vec<TrackedDebt>,
    /// Next debt ID to allocate.
    next_id: u64,
    /// Count of debts that have been automatically resolved.
    auto_resolved_count: usize,
    /// Snapshots of the outstanding debt count over time (for trend detection).
    debt_count_history: Vec<usize>,
}

impl Default for VerificationDebtTracker {
    fn default() -> Self {
        Self {
            debts: Vec::new(),
            next_id: 0,
            auto_resolved_count: 0,
            debt_count_history: Vec::new(),
        }
    }
}

impl VerificationDebtTracker {
    /// Construct a new, empty verification debt tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new debt item to the tracker, returning its ID.
    ///
    /// The debt is scored using the provided verification result and context.
    /// The current outstanding count is recorded for trend detection.
    pub fn add_debt(
        &mut self,
        item: DebtItem,
        result: &VerificationResult,
        context: &DebtContext,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let score = DebtScore::compute(result, context);
        let original_priority = item.priority;
        let created_at = Instant::now();

        self.debts.push(TrackedDebt {
            id,
            item,
            score,
            context: context.clone(),
            created_at,
            original_priority,
        });

        // Record snapshot for trend detection.
        self.debt_count_history.push(self.outstanding_count());
        id
    }

    /// Resolve a debt by its ID.
    ///
    /// Returns `true` if the debt was found and resolved.
    pub fn resolve_debt(&mut self, debt_id: u64) -> bool {
        if let Some(tracked) = self.debts.iter_mut().find(|d| d.id == debt_id) {
            if !tracked.item.is_resolved() {
                tracked.item.resolve();
                self.debt_count_history.push(self.outstanding_count());
                return true;
            }
        }
        false
    }

    /// Return the number of outstanding (unresolved) debt items.
    pub fn outstanding_count(&self) -> usize {
        self.debts.iter().filter(|d| !d.item.is_resolved()).count()
    }

    /// Return the total number of tracked debt items (including resolved).
    pub fn total_count(&self) -> usize {
        self.debts.len()
    }

    /// Apply aging to all tracked debt items.
    ///
    /// Updates the priority of each debt based on how long it has been
    /// unresolved. The aging formula increases the effective priority
    /// by elevating the priority level when the age factor reaches
    /// certain thresholds (1.5 for one elevation, 1.8 for two).
    pub fn apply_aging(&mut self, now: Instant) {
        for tracked in &mut self.debts {
            if tracked.item.is_resolved() {
                continue;
            }
            let age = now.duration_since(tracked.created_at);
            let age_factor = AgedDebt::compute_age_factor(age);
            let adjusted = AgedDebt::compute_adjusted_priority(tracked.original_priority, age_factor);
            tracked.item.priority = adjusted;
        }
        // Record snapshot after aging.
        self.debt_count_history.push(self.outstanding_count());
    }

    /// Attempt to automatically resolve debts based on a new verification result.
    ///
    /// If the new result is stronger than the result that originally created
    /// a debt, the debt can be automatically resolved. The logic is:
    ///
    /// - **Proven** result → resolves any debt with the same invariant
    ///   (StrengthenedProof).
    /// - **ProbablySafe** result → resolves Unverified or Violated debts
    ///   for the same invariant (StrengthenedProof with Medium confidence).
    /// - If the new result has a lower severity than the existing debt's
    ///   score, the context has changed (ContextChanged).
    pub fn try_auto_resolve(&mut self, result: &VerificationResult) -> Vec<AutoResolution> {
        let mut resolutions = Vec::new();

        for tracked in &mut self.debts {
            if tracked.item.is_resolved() {
                continue;
            }
            if tracked.item.property != result.invariant {
                continue;
            }

            let new_score = DebtScore::compute(result, &tracked.context);

            match &result.status {
                VerificationStatus::Proven => {
                    // A proof resolves any outstanding debt for this invariant.
                    tracked.item.resolve();
                    self.auto_resolved_count += 1;
                    resolutions.push(AutoResolution::StrengthenedProof {
                        debt_id: tracked.id,
                        new_confidence: ConfidenceLevel::High,
                    });
                }
                VerificationStatus::ProbablySafe { .. } => {
                    // ProbablySafe resolves Unverified or Violated debts.
                    if tracked.score.severity > new_score.severity {
                        tracked.item.resolve();
                        self.auto_resolved_count += 1;
                        resolutions.push(AutoResolution::StrengthenedProof {
                            debt_id: tracked.id,
                            new_confidence: ConfidenceLevel::Medium,
                        });
                    }
                }
                VerificationStatus::Unverified { .. } => {
                    // Unverified can only resolve Violated debts if the
                    // context changed (e.g., the violation is now less severe).
                    if new_score.severity < tracked.score.severity {
                        tracked.score.severity = new_score.severity;
                        tracked.score.composite = DebtScore::SEVERITY_WEIGHT * new_score.severity
                            + DebtScore::LIKELIHOOD_WEIGHT * tracked.score.likelihood
                            + DebtScore::IMPACT_WEIGHT * tracked.score.impact;
                        resolutions.push(AutoResolution::ContextChanged {
                            debt_id: tracked.id,
                            new_severity: new_score.severity,
                        });
                    }
                }
                VerificationStatus::Violated { .. } => {
                    // A new violation with lower severity is a context change.
                    if new_score.severity < tracked.score.severity {
                        tracked.score.severity = new_score.severity;
                        tracked.score.composite = DebtScore::SEVERITY_WEIGHT * new_score.severity
                            + DebtScore::LIKELIHOOD_WEIGHT * tracked.score.likelihood
                            + DebtScore::IMPACT_WEIGHT * tracked.score.impact;
                        resolutions.push(AutoResolution::ContextChanged {
                            debt_id: tracked.id,
                            new_severity: new_score.severity,
                        });
                    }
                }
            }
        }

        if !resolutions.is_empty() {
            self.debt_count_history.push(self.outstanding_count());
        }
        resolutions
    }

    /// Generate a comprehensive debt report.
    ///
    /// The report includes counts by priority and invariant, age statistics,
    /// the top 5 most critical debts, auto-resolution count, and the
    /// current debt trend.
    pub fn generate_debt_report(&self) -> DebtReport {
        let now = Instant::now();

        let mut by_priority: HashMap<Priority, usize> = HashMap::new();
        let mut by_invariant: HashMap<String, usize> = HashMap::new();
        let mut ages: Vec<Duration> = Vec::new();
        let mut aged_debts: Vec<AgedDebt> = Vec::new();

        for tracked in &self.debts {
            if tracked.item.is_resolved() {
                continue;
            }

            // Count by priority.
            *by_priority.entry(tracked.item.priority).or_insert(0) += 1;

            // Count by invariant/property.
            by_invariant
                .entry(tracked.item.property.clone())
                .and_modify(|c| *c += 1)
                .or_insert(1);

            // Compute age.
            let age = now.duration_since(tracked.created_at);
            ages.push(age);

            let age_factor = AgedDebt::compute_age_factor(age);
            let adjusted_priority =
                AgedDebt::compute_adjusted_priority(tracked.original_priority, age_factor);

            aged_debts.push(AgedDebt {
                debt: tracked.item.clone(),
                age,
                age_factor,
                adjusted_priority,
            });
        }

        // Sort by effective score descending for top-5.
        aged_debts.sort_by(|a, b| {
            let score_a = a.age_factor * a.debt.priority.weight();
            let score_b = b.age_factor * b.debt.priority.weight();
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_5_critical: Vec<AgedDebt> = aged_debts.into_iter().take(5).collect();

        // Age statistics.
        let total_debt_items = ages.len();
        let oldest_debt_age = ages.iter().max().copied();
        let average_age = if ages.is_empty() {
            Duration::ZERO
        } else {
            let total_secs: f64 = ages.iter().map(|d| d.as_secs_f64()).sum();
            Duration::from_secs_f64(total_secs / ages.len() as f64)
        };

        // Compute trend.
        let debt_trend = self.compute_trend();

        DebtReport {
            total_debt_items,
            by_priority,
            by_invariant,
            oldest_debt_age,
            average_age,
            auto_resolved_count: self.auto_resolved_count,
            top_5_critical,
            debt_trend,
        }
    }

    /// Compute the current debt trend from the count history.
    ///
    /// Compares the last third of history to the first third:
    /// - Increasing: recent average > early average by >= 10%
    /// - Decreasing: recent average < early average by >= 10%
    /// - Stable: otherwise
    fn compute_trend(&self) -> DebtTrend {
        let n = self.debt_count_history.len();
        if n < 3 {
            return DebtTrend::Stable;
        }

        let third = n / 3;
        let early: f64 = self.debt_count_history[..third]
            .iter()
            .map(|&c| c as f64)
            .sum::<f64>()
            / third as f64;
        let recent_start = n - third;
        let recent: f64 = self.debt_count_history[recent_start..]
            .iter()
            .map(|&c| c as f64)
            .sum::<f64>()
            / third as f64;

        if early == 0.0 {
            if recent > 0.0 {
                return DebtTrend::Increasing;
            }
            return DebtTrend::Stable;
        }

        let ratio = recent / early;
        if ratio >= 1.1 {
            DebtTrend::Increasing
        } else if ratio <= 0.9 {
            DebtTrend::Decreasing
        } else {
            DebtTrend::Stable
        }
    }

    /// Get a reference to a tracked debt by ID, if it exists.
    pub fn get_debt(&self, debt_id: u64) -> Option<&DebtItem> {
        self.debts.iter().find(|d| d.id == debt_id).map(|d| &d.item)
    }

    /// Get the score for a tracked debt by ID.
    pub fn get_score(&self, debt_id: u64) -> Option<&DebtScore> {
        self.debts.iter().find(|d| d.id == debt_id).map(|d| &d.score)
    }
}

impl fmt::Display for VerificationDebtTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let report = self.generate_debt_report();
        write!(f, "{}", report)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::CounterExample;

    // -- Existing tests (preserved) --

    #[test]
    fn add_and_resolve_debt() {
        let mut debt = VerificationDebt::new();
        let idx = debt.add(DebtItem::new("liveness_check", Priority::Critical, 100));
        assert_eq!(debt.total_debt(), 1);
        assert!(debt.resolve(idx));
        assert_eq!(debt.total_debt(), 0);
    }

    #[test]
    fn next_critical_returns_highest_priority() {
        let mut debt = VerificationDebt::new();
        debt.add(DebtItem::new("low_item", Priority::Low, 50));
        debt.add(DebtItem::new("critical_item", Priority::Critical, 100));
        debt.add(DebtItem::new("high_item", Priority::High, 75));

        let crit = debt.next_critical().unwrap();
        assert_eq!(crit.property, "critical_item");
    }

    #[test]
    fn debt_by_priority_counts_correctly() {
        let mut debt = VerificationDebt::new();
        debt.add(DebtItem::new("c1", Priority::Critical, 1));
        debt.add(DebtItem::new("c2", Priority::Critical, 2));
        debt.add(DebtItem::new("h1", Priority::High, 3));

        let counts = debt.debt_by_priority();
        assert_eq!(counts[0], (Priority::Critical, 2));
        assert_eq!(counts[1], (Priority::High, 1));
        assert_eq!(counts[2], (Priority::Medium, 0));
        assert_eq!(counts[3], (Priority::Low, 0));
    }

    // -- New tests (8+ required) --

    #[test]
    fn debt_score_computation_for_various_violation_types() {
        let context = DebtContext::default();

        // Violated → severity 1.0
        let violated = VerificationResult::new(
            "exclusivity",
            VerificationStatus::Violated {
                counterexample: CounterExample::new(
                    vec!["entry".into()],
                    "entry".into(),
                    "double write".into(),
                ),
            },
            "exclusivity violation",
        );
        let score = DebtScore::compute(&violated, &context);
        assert_eq!(score.severity, 1.0);
        assert!(score.composite > 0.0);

        // ProbablySafe → severity 0.3
        let probably_safe = VerificationResult::new(
            "liveness",
            VerificationStatus::ProbablySafe {
                assumptions: vec!["no_overflow".into()],
            },
            "likely safe",
        );
        let score = DebtScore::compute(&probably_safe, &context);
        assert!((score.severity - 0.3).abs() < 1e-9);

        // Unverified → severity 0.6
        let unverified = VerificationResult::new(
            "origin",
            VerificationStatus::Unverified {
                reason: "not checked".into(),
            },
            "pending",
        );
        let score = DebtScore::compute(&unverified, &context);
        assert!((score.severity - 0.6).abs() < 1e-9);

        // Proven → severity 0.0
        let proven = VerificationResult::new("cleanup", VerificationStatus::Proven, "all good");
        let score = DebtScore::compute(&proven, &context);
        assert_eq!(score.severity, 0.0);
    }

    #[test]
    fn aging_increases_priority() {
        // Simulate 6 days of aging → age_factor = 1.0 + 6 * 0.1 = 1.6 ≥ 1.5
        let age = Duration::from_secs(6 * 86400);
        let age_factor = AgedDebt::compute_age_factor(age);
        assert!(age_factor >= 1.5);
        assert!(age_factor < 1.8); // Not enough for double elevation

        let original = Priority::Medium;
        let adjusted = AgedDebt::compute_adjusted_priority(original, age_factor);
        assert_eq!(adjusted, Priority::High); // Medium → High (one elevation)
    }

    #[test]
    fn aging_caps_at_2_0() {
        // Simulate 100 days of aging → 1.0 + 100 * 0.1 = 11.0, capped at 2.0
        let age = Duration::from_secs(100 * 86400);
        let age_factor = AgedDebt::compute_age_factor(age);
        assert!((age_factor - 2.0).abs() < 1e-9);
    }

    #[test]
    fn auto_resolution_when_reverification_strengthens() {
        let mut tracker = VerificationDebtTracker::new();

        // Add a debt from an Unverified result.
        let unverified_result = VerificationResult::new(
            "exclusivity",
            VerificationStatus::Unverified {
                reason: "not yet checked".into(),
            },
            "pending verification",
        );
        let context = DebtContext::new();
        let debt_id = tracker.add_debt(
            DebtItem::new("exclusivity", Priority::High, 100),
            &unverified_result,
            &context,
        );

        assert_eq!(tracker.outstanding_count(), 1);

        // Re-verify and get a Proven result → should auto-resolve.
        let proven_result = VerificationResult::new(
            "exclusivity",
            VerificationStatus::Proven,
            "formally proven safe",
        );
        let resolutions = tracker.try_auto_resolve(&proven_result);

        assert_eq!(resolutions.len(), 1);
        assert_eq!(
            resolutions[0],
            AutoResolution::StrengthenedProof {
                debt_id,
                new_confidence: ConfidenceLevel::High,
            }
        );
        assert_eq!(tracker.outstanding_count(), 0);
        assert_eq!(tracker.auto_resolved_count, 1);
    }

    #[test]
    fn debt_report_generation() {
        let mut tracker = VerificationDebtTracker::new();

        let result1 = VerificationResult::new(
            "liveness",
            VerificationStatus::Unverified {
                reason: "pending".into(),
            },
            "unverified",
        );
        let result2 = VerificationResult::new(
            "exclusivity",
            VerificationStatus::Violated {
                counterexample: CounterExample::new(
                    vec!["a".into()],
                    "a".into(),
                    "violation".into(),
                ),
            },
            "violated",
        );
        let ctx = DebtContext::new()
            .with_library_code(true)
            .with_security_implications(true);

        tracker.add_debt(DebtItem::new("liveness", Priority::High, 1), &result1, &ctx);
        tracker.add_debt(
            DebtItem::new("exclusivity", Priority::Critical, 2),
            &result2,
            &ctx,
        );

        let report = tracker.generate_debt_report();
        assert_eq!(report.total_debt_items, 2);
        assert_eq!(*report.by_priority.get(&Priority::Critical).unwrap_or(&0), 1);
        assert_eq!(*report.by_priority.get(&Priority::High).unwrap_or(&0), 1);
        assert!(report.by_invariant.contains_key("liveness"));
        assert!(report.by_invariant.contains_key("exclusivity"));
        assert!(report.oldest_debt_age.is_some());
        assert_eq!(report.auto_resolved_count, 0);
        assert!(report.top_5_critical.len() <= 5);
    }

    #[test]
    fn debt_trend_detection() {
        let mut tracker = VerificationDebtTracker::new();

        // Simulate increasing trend by adding history entries manually.
        // The tracker records history on each add_debt, but for this test
        // we need enough data points. We'll add and resolve debts to
        // create the trend.

        let result = VerificationResult::new(
            "test",
            VerificationStatus::Unverified {
                reason: "test".into(),
            },
            "test",
        );
        let ctx = DebtContext::new();

        // Add several debts (increasing count).
        tracker.add_debt(DebtItem::new("a", Priority::Low, 1), &result, &ctx);
        tracker.add_debt(DebtItem::new("b", Priority::Low, 2), &result, &ctx);
        tracker.add_debt(DebtItem::new("c", Priority::Low, 3), &result, &ctx);
        tracker.add_debt(DebtItem::new("d", Priority::Low, 4), &result, &ctx);
        tracker.add_debt(DebtItem::new("e", Priority::Low, 5), &result, &ctx);
        tracker.add_debt(DebtItem::new("f", Priority::Low, 6), &result, &ctx);

        // The trend should be increasing (we only added debts, never resolved).
        let trend = tracker.compute_trend();
        assert_eq!(trend, DebtTrend::Increasing);

        // Now resolve all debts to create a decreasing trend.
        for i in 0..6 {
            tracker.resolve_debt(i);
        }

        let trend = tracker.compute_trend();
        // After resolving everything, the count history should show decrease.
        assert!(matches!(trend, DebtTrend::Decreasing | DebtTrend::Stable));
    }

    #[test]
    fn context_affects_severity_score() {
        let violated = VerificationResult::new(
            "exclusivity",
            VerificationStatus::Violated {
                counterexample: CounterExample::new(
                    vec!["x".into()],
                    "x".into(),
                    "race".into(),
                ),
            },
            "data race",
        );

        // Minimal context.
        let minimal_ctx = DebtContext::new();
        let minimal_score = DebtScore::compute(&violated, &minimal_ctx);

        // High-risk context: library + concurrent + security.
        let risky_ctx = DebtContext::new()
            .with_library_code(true)
            .with_concurrent_access(true)
            .with_security_implications(true);
        let risky_score = DebtScore::compute(&violated, &risky_ctx);

        // Severity should be the same (it's based on status, not context).
        assert_eq!(minimal_score.severity, risky_score.severity);

        // But likelihood and impact should be higher for the risky context.
        assert!(risky_score.likelihood > minimal_score.likelihood);
        assert!(risky_score.impact > minimal_score.impact);
        assert!(risky_score.composite > minimal_score.composite);
    }

    #[test]
    fn top_5_critical_debt_ordering() {
        let mut tracker = VerificationDebtTracker::new();

        let violated = VerificationResult::new(
            "test",
            VerificationStatus::Violated {
                counterexample: CounterExample::new(
                    vec!["x".into()],
                    "x".into(),
                    "violation".into(),
                ),
            },
            "violated",
        );
        let ctx = DebtContext::new().with_security_implications(true);

        // Add debts with different priorities.
        tracker.add_debt(
            DebtItem::new("low_prop", Priority::Low, 1),
            &violated,
            &ctx,
        );
        tracker.add_debt(
            DebtItem::new("crit_prop", Priority::Critical, 2),
            &violated,
            &ctx,
        );
        tracker.add_debt(
            DebtItem::new("med_prop", Priority::Medium, 3),
            &violated,
            &ctx,
        );
        tracker.add_debt(
            DebtItem::new("high_prop", Priority::High, 4),
            &violated,
            &ctx,
        );
        tracker.add_debt(
            DebtItem::new("another_crit", Priority::Critical, 5),
            &violated,
            &ctx,
        );
        tracker.add_debt(
            DebtItem::new("another_low", Priority::Low, 6),
            &violated,
            &ctx,
        );

        let report = tracker.generate_debt_report();
        assert_eq!(report.top_5_critical.len(), 5);

        // Critical items should come before non-critical.
        // The first items should have Critical or High adjusted priority.
        let first = &report.top_5_critical[0];
        assert!(matches!(
            first.adjusted_priority,
            Priority::Critical
        ));
    }

    // -- Additional tests --

    #[test]
    fn debt_score_to_priority_mapping() {
        // High composite → Critical
        let high_score = DebtScore {
            severity: 1.0,
            likelihood: 1.0,
            impact: 1.0,
            composite: 0.9,
        };
        assert_eq!(high_score.to_priority(), Priority::Critical);

        // Medium-high → High
        let med_high = DebtScore {
            severity: 0.8,
            likelihood: 0.5,
            impact: 0.5,
            composite: 0.62,
        };
        assert_eq!(med_high.to_priority(), Priority::High);

        // Medium → Medium
        let medium = DebtScore {
            severity: 0.5,
            likelihood: 0.3,
            impact: 0.3,
            composite: 0.38,
        };
        assert_eq!(medium.to_priority(), Priority::Medium);

        // Low → Low
        let low = DebtScore {
            severity: 0.1,
            likelihood: 0.1,
            impact: 0.1,
            composite: 0.1,
        };
        assert_eq!(low.to_priority(), Priority::Low);
    }

    #[test]
    fn probably_safe_auto_resolves_violated_debt() {
        let mut tracker = VerificationDebtTracker::new();

        // Add a debt from a Violated result.
        let violated_result = VerificationResult::new(
            "liveness",
            VerificationStatus::Violated {
                counterexample: CounterExample::new(
                    vec!["a".into()],
                    "a".into(),
                    "uaf".into(),
                ),
            },
            "use-after-free",
        );
        let ctx = DebtContext::new();
        let debt_id = tracker.add_debt(
            DebtItem::new("liveness", Priority::Critical, 1),
            &violated_result,
            &ctx,
        );

        // Re-verify with ProbablySafe → severity goes from 1.0 to 0.3,
        // which is lower, so the debt can be auto-resolved.
        let probably_safe = VerificationResult::new(
            "liveness",
            VerificationStatus::ProbablySafe {
                assumptions: vec!["safe_alloc".into()],
            },
            "probably safe under assumption",
        );
        let resolutions = tracker.try_auto_resolve(&probably_safe);

        assert_eq!(resolutions.len(), 1);
        assert_eq!(
            resolutions[0],
            AutoResolution::StrengthenedProof {
                debt_id,
                new_confidence: ConfidenceLevel::Medium,
            }
        );
    }

    #[test]
    fn context_changed_auto_resolution() {
        let mut tracker = VerificationDebtTracker::new();

        // Add a debt from a Violated result (severity = 1.0).
        let violated_result = VerificationResult::new(
            "origin",
            VerificationStatus::Violated {
                counterexample: CounterExample::new(
                    vec!["b".into()],
                    "b".into(),
                    "taint".into(),
                ),
            },
            "taint violation",
        );
        let ctx = DebtContext::new();
        let debt_id = tracker.add_debt(
            DebtItem::new("origin", Priority::Critical, 1),
            &violated_result,
            &ctx,
        );

        // Re-verify with another Violated but same severity → no change.
        let another_violated = VerificationResult::new(
            "origin",
            VerificationStatus::Violated {
                counterexample: CounterExample::new(
                    vec!["c".into()],
                    "c".into(),
                    "different taint".into(),
                ),
            },
            "another taint",
        );
        let resolutions = tracker.try_auto_resolve(&another_violated);
        // Same severity, so no context change (new_severity is not less than old).
        assert!(resolutions.is_empty());

        // Now with an Unverified result (severity 0.6 < 1.0) → ContextChanged.
        let unverified = VerificationResult::new(
            "origin",
            VerificationStatus::Unverified {
                reason: "context shifted".into(),
            },
            "no longer violating",
        );
        let resolutions = tracker.try_auto_resolve(&unverified);
        assert_eq!(resolutions.len(), 1);
        assert!(matches!(
            &resolutions[0],
            AutoResolution::ContextChanged {
                debt_id: id,
                new_severity: 0.6,
            } if *id == debt_id
        ));
    }

    #[test]
    fn aging_double_elevation() {
        // 20 days → factor = 1.0 + 20 * 0.1 = 2.0, capped at 2.0.
        // With factor >= 1.8, priority is elevated twice.
        let age = Duration::from_secs(20 * 86400);
        let age_factor = AgedDebt::compute_age_factor(age);
        assert!(age_factor >= 1.8);

        let original = Priority::Low;
        let adjusted = AgedDebt::compute_adjusted_priority(original, age_factor);
        assert_eq!(adjusted, Priority::High); // Low → Medium → High
    }

    #[test]
    fn priority_elevation_boundaries() {
        // Critical stays Critical.
        assert_eq!(Priority::Critical.elevate(), Priority::Critical);

        // High → Critical.
        assert_eq!(Priority::High.elevate(), Priority::Critical);

        // Medium → High.
        assert_eq!(Priority::Medium.elevate(), Priority::High);

        // Low → Medium.
        assert_eq!(Priority::Low.elevate(), Priority::Medium);
    }

    #[test]
    fn debt_context_builder_pattern() {
        let ctx = DebtContext::new()
            .with_library_code(true)
            .with_concurrent_access(true)
            .with_performance_critical(false)
            .with_security_implications(true);

        assert!(ctx.is_library_code);
        assert!(ctx.has_concurrent_access);
        assert!(!ctx.is_performance_critical);
        assert!(ctx.has_security_implications);
    }

    #[test]
    fn debt_report_display_format() {
        let report = DebtReport {
            total_debt_items: 3,
            by_priority: {
                let mut m = HashMap::new();
                m.insert(Priority::Critical, 1);
                m.insert(Priority::High, 2);
                m
            },
            by_invariant: {
                let mut m = HashMap::new();
                m.insert("liveness".to_string(), 1);
                m.insert("exclusivity".to_string(), 2);
                m
            },
            oldest_debt_age: Some(Duration::from_secs(86400)),
            average_age: Duration::from_secs(43200),
            auto_resolved_count: 5,
            top_5_critical: vec![],
            debt_trend: DebtTrend::Increasing,
        };

        let display = format!("{}", report);
        assert!(display.contains("Verification Debt Report"));
        assert!(display.contains("3"));
        assert!(display.contains("INCREASING"));
    }

    #[test]
    fn auto_resolution_display_format() {
        let r1 = AutoResolution::StrengthenedProof {
            debt_id: 42,
            new_confidence: ConfidenceLevel::High,
        };
        assert!(format!("{}", r1).contains("StrengthenedProof"));

        let r2 = AutoResolution::WeakenedRequirement {
            debt_id: 7,
            reason: "scope narrowed".into(),
        };
        assert!(format!("{}", r2).contains("WeakenedRequirement"));

        let r3 = AutoResolution::SupersededByNewProof {
            old_debt: 1,
            new_debt: 2,
        };
        assert!(format!("{}", r3).contains("SupersededByNewProof"));

        let r4 = AutoResolution::ContextChanged {
            debt_id: 3,
            new_severity: 0.5,
        };
        assert!(format!("{}", r4).contains("ContextChanged"));
    }

    #[test]
    fn stable_trend_when_no_change() {
        let tracker = VerificationDebtTracker::new();
        // With < 3 data points, trend is Stable.
        assert_eq!(tracker.compute_trend(), DebtTrend::Stable);
    }

    #[test]
    fn aged_debt_display_format() {
        let aged = AgedDebt {
            debt: DebtItem::new("test_prop", Priority::High, 1),
            age: Duration::from_secs(5 * 86400),
            age_factor: 1.5,
            adjusted_priority: Priority::Critical,
        };
        let display = format!("{}", aged);
        assert!(display.contains("test_prop"));
        assert!(display.contains("5.0d"));
        assert!(display.contains("1.50"));
    }

    #[test]
    fn debt_score_display_format() {
        let score = DebtScore {
            severity: 0.8,
            likelihood: 0.6,
            impact: 0.4,
            composite: 0.62,
        };
        let display = format!("{}", score);
        assert!(display.contains("0.80"));
        assert!(display.contains("0.62"));
    }
}
