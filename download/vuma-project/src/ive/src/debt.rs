//! Verification debt tracking for the IVE module.
//!
//! Verification debt represents properties that have not yet been formally
//! verified but should be. This module provides a priority-ordered queue
//! of outstanding verification obligations.

use serde::{Deserialize, Serialize};
use std::fmt;

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}
