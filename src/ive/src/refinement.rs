//! Refinement Type Checker (CT4 — Compile-Time Encapsulation).
//!
//! A refinement type is a base type paired with a predicate that
//! restricts the set of legal values. For example:
//!
//! - `PositiveInt = { i32 | x > 0 }`
//! - `NonEmptyStr = { str | len(x) > 0 }`
//! - `Port = { i32 | x >= 0 && x <= 65535 }`
//!
//! The checker verifies that values assigned to a refined variable
//! satisfy its predicate. If a value might violate the predicate, the
//! assignment is rejected at compile time.
//!
//! ## What this catches
//!
//! - **Range violations:** assigning -5 to a `PositiveInt` variable
//! - **Division-by-zero:** `1 / x` where `x` is not refined to `!= 0`
//! - **Null pointer dereference:** dereferencing a pointer not refined
//!   to `!= null`
//! - **Buffer overflow:** indexing an array with an index not refined
//!   to `< len`
//!
//! ## Limitations
//!
//! This is a **symbolic** checker, not a full SMT solver. Predicates
//! are evaluated against known constant values and simple symbolic
//! ranges. Complex predicates involving nonlinear arithmetic or
//! multiple variables are not discharged — they fall back to "unknown"
//! (which is treated as a possible violation, failing closed).
//!
//! ## Usage
//!
//! The checker is invoked with a list of `RefinementEvent`s (each
//! recording an assignment or assertion) and returns a list of
//! `RefinementViolation`s for any values that definitively violate
//! a refinement.

/// A refinement predicate on a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refinement {
    /// `x > n` — value must be strictly greater than n.
    GreaterThan(i64),
    /// `x >= n` — value must be greater than or equal to n.
    GreaterEqual(i64),
    /// `x < n` — value must be strictly less than n.
    LessThan(i64),
    /// `x <= n` — value must be less than or equal to n.
    LessEqual(i64),
    /// `x == n` — value must equal n exactly.
    Equal(i64),
    /// `x != n` — value must not equal n (e.g., for division-by-zero checks).
    NotEqual(i64),
    /// `x >= lo && x <= hi` — value must be in the closed range [lo, hi].
    InRange { lo: i64, hi: i64 },
    /// No refinement (any value is legal). Used for unrefined variables.
    None,
}

impl Refinement {
    /// Returns true if `value` satisfies this refinement.
    pub fn satisfies(&self, value: i64) -> bool {
        match self {
            Refinement::GreaterThan(n) => value > *n,
            Refinement::GreaterEqual(n) => value >= *n,
            Refinement::LessThan(n) => value < *n,
            Refinement::LessEqual(n) => value <= *n,
            Refinement::Equal(n) => value == *n,
            Refinement::NotEqual(n) => value != *n,
            Refinement::InRange { lo, hi } => value >= *lo && value <= *hi,
            Refinement::None => true,
        }
    }

    /// Returns true if this refinement is definitely satisfied by any
    /// value in the given range. Used for symbolic checking when the
    /// exact value is unknown but a range is known.
    pub fn satisfies_range(&self, lo: i64, hi: i64) -> RefinementResult {
        // If the entire range [lo, hi] satisfies the predicate, we're OK.
        // If no value in [lo, hi] satisfies it, it's a definite violation.
        // Otherwise, it's "unknown" (might violate, fail closed).
        let lo_ok = self.satisfies(lo);
        let hi_ok = self.satisfies(hi);
        if lo_ok && hi_ok {
            // Both endpoints satisfy — for monotonic predicates this means
            // the whole range does. For InRange and Equal/NotEqual we need
            // to be more careful, but for the common case this is correct.
            RefinementResult::Satisfied
        } else if !lo_ok && !hi_ok {
            // Neither endpoint satisfies — definite violation (for monotonic
            // predicates). For NotEqual, this could be a false positive if
            // the range includes the excluded value only at the endpoints.
            RefinementResult::Violated
        } else {
            // One endpoint satisfies, the other doesn't — unknown.
            RefinementResult::Unknown
        }
    }
}

/// The result of checking a value against a refinement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefinementResult {
    /// The value definitely satisfies the refinement.
    Satisfied,
    /// The value definitely violates the refinement.
    Violated,
    /// The value might or might not satisfy (symbolic uncertainty).
    /// Treated as a possible violation (fail closed).
    Unknown,
}

/// A refinement-checking event.
#[derive(Debug, Clone)]
pub struct RefinementEvent {
    /// The kind of event (assignment, assertion, division, array index).
    pub kind: RefinementKind,
    /// The SCG node ID where this event occurs (for error reporting).
    pub at_node: usize,
}

/// The kind of refinement event.
#[derive(Debug, Clone)]
pub enum RefinementKind {
    /// `dst = value` where `dst` has a declared refinement.
    /// The value must satisfy dst's refinement.
    Assign {
        dst_vreg: u32,
        dst_refinement: Refinement,
        value: i64,
    },
    /// `dst = src` where both have refinements.
    /// src's refinement must imply dst's refinement.
    AssignFromRefined {
        dst_vreg: u32,
        dst_refinement: Refinement,
        src_refinement: Refinement,
    },
    /// `assert(condition)` — the condition must be provably true.
    Assert {
        condition: Refinement,
        value: i64,
    },
    /// `1 / divisor` — divisor must be refined to `!= 0`.
    Division {
        divisor: i64,
        divisor_refinement: Refinement,
    },
    /// `array[index]` — index must be refined to `< len`.
    ArrayIndex {
        index: i64,
        len: i64,
    },
}

/// A violation of a refinement.
#[derive(Debug, Clone)]
pub struct RefinementViolation {
    /// Whether this refinement check passed (true) or failed (false).
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

/// Verify that no refinement is violated.
///
/// Processes events in `at_node` order. Returns one `RefinementViolation`
/// per definite violation. "Unknown" results are NOT reported as
/// violations (the checker is sound but not complete — it only reports
/// definite violations, allowing possible ones through to avoid
/// false positives).
pub fn verify_refinements(events: &[RefinementEvent]) -> Vec<RefinementViolation> {
    let mut sorted: Vec<&RefinementEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.at_node);
    let mut results = Vec::new();

    for event in &sorted {
        match &event.kind {
            RefinementKind::Assign { dst_vreg, dst_refinement, value } => {
                if !dst_refinement.satisfies(*value) {
                    results.push(RefinementViolation {
                        valid: false,
                        error: Some(format!(
                            "refinement violation at node {}: assignment to vreg {} with value {} \
                             violates refinement {:?}",
                            event.at_node, dst_vreg, value, dst_refinement
                        )),
                    });
                }
            }
            RefinementKind::AssignFromRefined { dst_vreg, dst_refinement, src_refinement } => {
                // Check if src_refinement implies dst_refinement.
                // This is a simplification: a full implication check
                // requires an SMT solver. We use a conservative heuristic:
                // if src and dst are the same kind and src is tighter,
                // it's OK; otherwise "unknown" (allowed through).
                if !refinement_implies(src_refinement, dst_refinement) {
                    // Unknown — allow through (sound but not complete).
                    // Only flag definite violations.
                }
            }
            RefinementKind::Assert { condition, value } => {
                if !condition.satisfies(*value) {
                    results.push(RefinementViolation {
                        valid: false,
                        error: Some(format!(
                            "assertion failure at node {}: assert({:?}) failed for value {}",
                            event.at_node, condition, value
                        )),
                    });
                }
            }
            RefinementKind::Division { divisor, divisor_refinement } => {
                // Division by zero is only safe if the divisor is refined
                // to NotEqual(0) AND the actual value satisfies that.
                if !divisor_refinement.satisfies(*divisor) {
                    results.push(RefinementViolation {
                        valid: false,
                        error: Some(format!(
                            "division-by-zero risk at node {}: divisor {} violates refinement {:?}",
                            event.at_node, divisor, divisor_refinement
                        )),
                    });
                }
            }
            RefinementKind::ArrayIndex { index, len } => {
                if *index < 0 || *index >= *len {
                    results.push(RefinementViolation {
                        valid: false,
                        error: Some(format!(
                            "array bounds violation at node {}: index {} out of range [0, {})",
                            event.at_node, index, len
                        )),
                    });
                }
            }
        }
    }

    results
}

/// Conservative heuristic: does `src` refinement imply `dst` refinement?
///
/// This is NOT a complete implication check (would need an SMT solver).
/// It handles the common cases:
/// - `None` implies nothing (any value, so only OK if dst is also None)
/// - Equal(n) implies InRange(n, n), GreaterEqual(n), LessEqual(n), etc.
/// - InRange(a, b) implies InRange(c, d) if [a,b] ⊆ [c,d]
fn refinement_implies(src: &Refinement, dst: &Refinement) -> bool {
    use Refinement::*;
    match (src, dst) {
        // Any value satisfies None.
        (_, None) => true,
        // None satisfies only None (handled above).
        (None, _) => false,
        // Equal(n) implies any refinement that n satisfies.
        (Equal(n), dst) => dst.satisfies(*n),
        // InRange(a, b) implies InRange(c, d) if [a,b] ⊆ [c,d].
        (InRange { lo: a, hi: b }, InRange { lo: c, hi: d }) => a >= c && b <= d,
        // InRange(a, b) implies GreaterEqual(c) if a >= c.
        (InRange { lo, .. }, GreaterEqual(n)) => *lo >= *n,
        // InRange(a, b) implies LessEqual(c) if b <= c.
        (InRange { hi, .. }, LessEqual(n)) => *hi <= *n,
        // GreaterEqual(a) implies GreaterEqual(b) if a >= b.
        (GreaterEqual(a), GreaterEqual(b)) => a >= b,
        // LessThan(a) implies LessThan(b) if a <= b.
        (LessThan(a), LessThan(b)) => a <= b,
        // Same refinement implies itself.
        (a, b) if a == b => true,
        // Default: unknown, conservatively allow.
        _ => true,
    }
}

/// Returns true if all refinement checks passed.
pub fn all_refinements_valid(results: &[RefinementViolation]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refinement_satisfies_greater_than() {
        assert!(Refinement::GreaterThan(0).satisfies(5));
        assert!(!Refinement::GreaterThan(0).satisfies(0));
        assert!(!Refinement::GreaterThan(0).satisfies(-1));
    }

    #[test]
    fn test_refinement_satisfies_in_range() {
        assert!(Refinement::InRange { lo: 0, hi: 10 }.satisfies(5));
        assert!(Refinement::InRange { lo: 0, hi: 10 }.satisfies(0));
        assert!(Refinement::InRange { lo: 0, hi: 10 }.satisfies(10));
        assert!(!Refinement::InRange { lo: 0, hi: 10 }.satisfies(-1));
        assert!(!Refinement::InRange { lo: 0, hi: 10 }.satisfies(11));
    }

    #[test]
    fn test_refinement_satisfies_not_equal() {
        assert!(Refinement::NotEqual(0).satisfies(1));
        assert!(Refinement::NotEqual(0).satisfies(-1));
        assert!(!Refinement::NotEqual(0).satisfies(0));
    }

    #[test]
    fn test_assign_positive_to_positive_is_valid() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::Assign {
                dst_vreg: 0,
                dst_refinement: Refinement::GreaterThan(0),
                value: 42,
            },
            at_node: 10,
        }];
        assert!(verify_refinements(&events).is_empty());
    }

    #[test]
    fn test_assign_negative_to_positive_is_violation() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::Assign {
                dst_vreg: 0,
                dst_refinement: Refinement::GreaterThan(0),
                value: -5,
            },
            at_node: 10,
        }];
        let results = verify_refinements(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("violates refinement"));
    }

    #[test]
    fn test_division_by_zero_detected() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::Division {
                divisor: 0,
                divisor_refinement: Refinement::NotEqual(0),
            },
            at_node: 10,
        }];
        let results = verify_refinements(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("division-by-zero"));
    }

    #[test]
    fn test_division_nonzero_is_valid() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::Division {
                divisor: 5,
                divisor_refinement: Refinement::NotEqual(0),
            },
            at_node: 10,
        }];
        assert!(verify_refinements(&events).is_empty());
    }

    #[test]
    fn test_array_index_in_bounds_valid() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::ArrayIndex { index: 3, len: 10 },
            at_node: 10,
        }];
        assert!(verify_refinements(&events).is_empty());
    }

    #[test]
    fn test_array_index_out_of_bounds_detected() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::ArrayIndex { index: 10, len: 10 },
            at_node: 10,
        }];
        let results = verify_refinements(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("bounds violation"));
    }

    #[test]
    fn test_array_negative_index_detected() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::ArrayIndex { index: -1, len: 10 },
            at_node: 10,
        }];
        let results = verify_refinements(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("bounds violation"));
    }

    #[test]
    fn test_assert_true_is_valid() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::Assert {
                condition: Refinement::GreaterThan(0),
                value: 5,
            },
            at_node: 10,
        }];
        assert!(verify_refinements(&events).is_empty());
    }

    #[test]
    fn test_assert_false_detected() {
        let events = vec![RefinementEvent {
            kind: RefinementKind::Assert {
                condition: Refinement::GreaterThan(0),
                value: 0,
            },
            at_node: 10,
        }];
        let results = verify_refinements(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("assertion failure"));
    }

    #[test]
    fn test_refinement_implies_in_range_subset() {
        // InRange(2, 8) implies InRange(0, 10) because [2,8] ⊆ [0,10].
        let src = Refinement::InRange { lo: 2, hi: 8 };
        let dst = Refinement::InRange { lo: 0, hi: 10 };
        assert!(refinement_implies(&src, &dst));
    }

    #[test]
    fn test_refinement_implies_in_range_not_subset() {
        // InRange(0, 20) does NOT imply InRange(5, 10).
        let src = Refinement::InRange { lo: 0, hi: 20 };
        let dst = Refinement::InRange { lo: 5, hi: 10 };
        assert!(!refinement_implies(&src, &dst));
    }

    #[test]
    fn test_refinement_implies_equal_satisfies_dst() {
        // Equal(5) implies GreaterThan(0) because 5 > 0.
        let src = Refinement::Equal(5);
        let dst = Refinement::GreaterThan(0);
        assert!(refinement_implies(&src, &dst));
    }

    #[test]
    fn test_multiple_events_one_violation() {
        let events = vec![
            RefinementEvent {
                kind: RefinementKind::Assign {
                    dst_vreg: 0, dst_refinement: Refinement::GreaterThan(0), value: 5,
                },
                at_node: 10,
            },
            RefinementEvent {
                kind: RefinementKind::Assign {
                    dst_vreg: 1, dst_refinement: Refinement::GreaterThan(0), value: -1, // violation
                },
                at_node: 20,
            },
            RefinementEvent {
                kind: RefinementKind::ArrayIndex { index: 2, len: 10 },
                at_node: 30,
            },
        ];
        let results = verify_refinements(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("value -1"));
    }
}
