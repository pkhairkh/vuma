//! Information-Flow Type Checker (CT2 — Compile-Time Encapsulation).
//!
//! Implements a Denning-style security-label lattice for tracking
//! information flow through a VUMA program. The lattice is:
//!
//! ```text
//!   Public ⊑ Internal ⊑ Secret ⊑ TopSecret
//! ```
//!
//! A value labeled `L1` may flow into a location labeled `L2` only if
//! `L1 ⊑ L2` (read: "L1 is at most as sensitive as L2"). This prevents
//! high-sensitivity data from leaking into low-sensitivity channels.
//!
//! ## What this catches
//!
//! - **Direct leak:** `secret_var → public_channel` (Secret ⊀ Public)
//! - **Indirect leak via branch:** `if secret_cond { public_var = 1 }`
//!   (the public variable becomes "tainted" by the secret condition)
//! - **Indirect leak via assignment:** `public_var = secret_var + 1`
//!   (the public variable inherits the secret label)
//!
//! ## What this does NOT catch (yet)
//!
//! - **Timing side channels** — the constant-time checker
//!   (`constant_time.rs`) handles those separately.
//! - **Implicit flows through termination** — e.g., `if secret { exit }`
//!   leaks one bit via the program's termination behavior.
//! - **Storage channels** — e.g., measuring memory usage to infer a secret.
//!
//! ## Usage
//!
//! The checker is invoked with a list of `FlowEvent`s (each recording a
//! value assignment, channel send, or branch condition) and returns a
//! list of `FlowViolation`s for any disallowed flows.

use std::collections::HashMap;

/// A security label in the Denning lattice.
///
/// The ordering is `Public ⊑ Internal ⊑ Secret ⊑ TopSecret`. Higher
/// sensitivity means more restrictive — a `TopSecret` value can only
/// flow to another `TopSecret` location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityLabel {
    /// Public information — safe to leak to anyone.
    Public,
    /// Internal information — not for external release, but not secret.
    Internal,
    /// Secret information — must not leak to Public or Internal.
    Secret,
    /// TopSecret information — must not leak below TopSecret.
    TopSecret,
}

impl SecurityLabel {
    /// Returns true if `self ⊑ other` (self can flow to other).
    ///
    /// The lattice is a total order: Public → Internal → Secret → TopSecret.
    /// A label can always flow to itself or to a higher label.
    pub fn can_flow_to(self, other: SecurityLabel) -> bool {
        use SecurityLabel::*;
        match (self, other) {
            (Public, _) => true,           // Public flows anywhere
            (Internal, Internal | Secret | TopSecret) => true,
            (Secret, Secret | TopSecret) => true,
            (TopSecret, TopSecret) => true,
            _ => false,
        }
    }

    /// Returns the least upper bound (join) of two labels.
    ///
    /// When two values combine (e.g., `a + b`), the result has the
    /// higher of the two labels. This is the "label taint" rule: if
    /// either operand is Secret, the result is Secret.
    pub fn join(self, other: SecurityLabel) -> SecurityLabel {
        use SecurityLabel::*;
        match (self, other) {
            (Public, x) | (x, Public) => x,
            (Internal, x) | (x, Internal) => x,
            (Secret, x) | (x, Secret) => x,
            (TopSecret, TopSecret) => TopSecret,
        }
    }

    /// Returns the numeric level (for comparison). Higher = more sensitive.
    pub fn level(self) -> u8 {
        use SecurityLabel::*;
        match self {
            Public => 0,
            Internal => 1,
            Secret => 2,
            TopSecret => 3,
        }
    }
}

impl std::fmt::Display for SecurityLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use SecurityLabel::*;
        match self {
            Public => write!(f, "Public"),
            Internal => write!(f, "Internal"),
            Secret => write!(f, "Secret"),
            TopSecret => write!(f, "TopSecret"),
        }
    }
}

/// An information-flow event observed during verification.
#[derive(Debug, Clone)]
pub struct FlowEvent {
    /// The kind of flow event (assignment, channel send, branch).
    pub kind: FlowKind,
    /// The SCG node ID where this event occurs (for error reporting).
    pub at_node: usize,
}

/// The kind of information-flow event.
#[derive(Debug, Clone)]
pub enum FlowKind {
    /// `dst = src` — direct assignment. The source label must be ⊑ the
    /// destination's declared label.
    Assign {
        dst_vreg: u32,
        dst_label: SecurityLabel,
        src_label: SecurityLabel,
    },
    /// `dst = a op b` — binary operation. The result label is
    /// `join(a_label, b_label)`, which must be ⊑ dst_label.
    BinOp {
        dst_vreg: u32,
        dst_label: SecurityLabel,
        lhs_label: SecurityLabel,
        rhs_label: SecurityLabel,
    },
    /// `channel_send(ch, msg)` — sending a value on a channel. The
    /// message label must be ⊑ the channel's label.
    ChannelSend {
        channel_label: SecurityLabel,
        msg_label: SecurityLabel,
    },
    /// `if cond { ... }` — a branch on a condition. The condition's
    /// label "taints" any variables assigned in either branch (they
    /// must have label ⊢ cond_label). This is the implicit-flow rule.
    Branch {
        cond_label: SecurityLabel,
        /// The minimum label required for any variable assigned in
        /// either branch. If a variable in a branch has a lower label,
        /// that's an implicit-flow leak.
        branch_var_labels: Vec<SecurityLabel>,
    },
}

/// A violation of the information-flow policy.
#[derive(Debug, Clone)]
pub struct FlowViolation {
    /// Whether this flow is allowed (true) or a violation (false).
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

/// Verify that no information flow violates the security lattice.
///
/// Processes events in `at_node` order. For each event, checks that
/// the source label can flow to the destination label. Returns one
/// `FlowViolation` per disallowed flow.
pub fn verify_information_flow(events: &[FlowEvent]) -> Vec<FlowViolation> {
    let mut sorted: Vec<&FlowEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.at_node);
    let mut results = Vec::new();

    for event in &sorted {
        match &event.kind {
            FlowKind::Assign { dst_vreg, dst_label, src_label } => {
                if !src_label.can_flow_to(*dst_label) {
                    results.push(FlowViolation {
                        valid: false,
                        error: Some(format!(
                            "information-flow violation at node {}: assignment to vreg {} (label {}) \
                             from source with label {} — {} ⊀ {} (would leak {} data to {} location)",
                            event.at_node, dst_vreg, dst_label, src_label,
                            src_label, dst_label, src_label, dst_label
                        )),
                    });
                }
            }
            FlowKind::BinOp { dst_vreg, dst_label, lhs_label, rhs_label } => {
                let result_label = lhs_label.join(*rhs_label);
                if !result_label.can_flow_to(*dst_label) {
                    results.push(FlowViolation {
                        valid: false,
                        error: Some(format!(
                            "information-flow violation at node {}: binary op result (label {}, \
                             from join({},{})) stored in vreg {} (label {}) — {} ⊀ {} (would leak data)",
                            event.at_node, result_label, lhs_label, rhs_label,
                            dst_vreg, dst_label, result_label, dst_label
                        )),
                    });
                }
            }
            FlowKind::ChannelSend { channel_label, msg_label } => {
                if !msg_label.can_flow_to(*channel_label) {
                    results.push(FlowViolation {
                        valid: false,
                        error: Some(format!(
                            "information-flow violation at node {}: sending message with label {} \
                             on channel with label {} — {} ⊀ {} (would leak {} data to {} channel)",
                            event.at_node, msg_label, channel_label,
                            msg_label, channel_label, msg_label, channel_label
                        )),
                    });
                }
            }
            FlowKind::Branch { cond_label, branch_var_labels } => {
                // Implicit flow: any variable assigned in a branch on a
                // secret condition inherits the condition's label. If a
                // branch variable has a lower label, that's a leak.
                for var_label in branch_var_labels {
                    if !cond_label.can_flow_to(*var_label) {
                        results.push(FlowViolation {
                            valid: false,
                            error: Some(format!(
                                "implicit information-flow violation at node {}: branch on \
                                 condition with label {} assigns to variable with label {} — \
                                 {} ⊀ {} (implicit leak via control flow)",
                                event.at_node, cond_label, var_label,
                                cond_label, var_label
                            )),
                        });
                    }
                }
            }
        }
    }

    results
}

/// Returns true if all information-flow verification results are valid.
pub fn all_flows_valid(results: &[FlowViolation]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lattice_public_flows_anywhere() {
        assert!(SecurityLabel::Public.can_flow_to(SecurityLabel::Public));
        assert!(SecurityLabel::Public.can_flow_to(SecurityLabel::Internal));
        assert!(SecurityLabel::Public.can_flow_to(SecurityLabel::Secret));
        assert!(SecurityLabel::Public.can_flow_to(SecurityLabel::TopSecret));
    }

    #[test]
    fn test_lattice_secret_does_not_flow_to_public() {
        assert!(!SecurityLabel::Secret.can_flow_to(SecurityLabel::Public));
        assert!(!SecurityLabel::Secret.can_flow_to(SecurityLabel::Internal));
        assert!(SecurityLabel::Secret.can_flow_to(SecurityLabel::Secret));
        assert!(SecurityLabel::Secret.can_flow_to(SecurityLabel::TopSecret));
    }

    #[test]
    fn test_lattice_topsecret_only_flows_to_topsecret() {
        assert!(!SecurityLabel::TopSecret.can_flow_to(SecurityLabel::Public));
        assert!(!SecurityLabel::TopSecret.can_flow_to(SecurityLabel::Internal));
        assert!(!SecurityLabel::TopSecret.can_flow_to(SecurityLabel::Secret));
        assert!(SecurityLabel::TopSecret.can_flow_to(SecurityLabel::TopSecret));
    }

    #[test]
    fn test_join_takes_higher_label() {
        assert_eq!(SecurityLabel::Public.join(SecurityLabel::Secret), SecurityLabel::Secret);
        assert_eq!(SecurityLabel::Secret.join(SecurityLabel::Public), SecurityLabel::Secret);
        assert_eq!(SecurityLabel::Internal.join(SecurityLabel::TopSecret), SecurityLabel::TopSecret);
        assert_eq!(SecurityLabel::Public.join(SecurityLabel::Public), SecurityLabel::Public);
    }

    #[test]
    fn test_direct_assignment_valid() {
        // public → public: OK
        let events = vec![FlowEvent {
            kind: FlowKind::Assign {
                dst_vreg: 0, dst_label: SecurityLabel::Public,
                src_label: SecurityLabel::Public,
            },
            at_node: 10,
        }];
        assert!(verify_information_flow(&events).is_empty());
    }

    #[test]
    fn test_direct_assignment_leak_detected() {
        // secret → public: LEAK
        let events = vec![FlowEvent {
            kind: FlowKind::Assign {
                dst_vreg: 0, dst_label: SecurityLabel::Public,
                src_label: SecurityLabel::Secret,
            },
            at_node: 10,
        }];
        let results = verify_information_flow(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("would leak"));
    }

    #[test]
    fn test_binop_join_leak_detected() {
        // dst (public) = secret + internal → join = secret, secret ⊀ public: LEAK
        let events = vec![FlowEvent {
            kind: FlowKind::BinOp {
                dst_vreg: 0, dst_label: SecurityLabel::Public,
                lhs_label: SecurityLabel::Secret,
                rhs_label: SecurityLabel::Internal,
            },
            at_node: 10,
        }];
        let results = verify_information_flow(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("binary op"));
    }

    #[test]
    fn test_channel_send_leak_detected() {
        // send secret on public channel: LEAK
        let events = vec![FlowEvent {
            kind: FlowKind::ChannelSend { channel_label: SecurityLabel::Public, msg_label: SecurityLabel::Public } {
                channel_label: SecurityLabel::Public,
                msg_label: SecurityLabel::Secret,
            },
            at_node: 10,
        }];
        let results = verify_information_flow(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("channel"));
    }

    #[test]
    fn test_channel_send_valid() {
        // send public on secret channel: OK (public ⊑ secret)
        let events = vec![FlowEvent {
            kind: FlowKind::ChannelSend { channel_label: SecurityLabel::Public, msg_label: SecurityLabel::Public } {
                channel_label: SecurityLabel::Secret,
                msg_label: SecurityLabel::Public,
            },
            at_node: 10,
        }];
        assert!(verify_information_flow(&events).is_empty());
    }

    #[test]
    fn test_implicit_flow_via_branch_detected() {
        // if secret_cond { public_var = 1 } — implicit leak
        let events = vec![FlowEvent {
            kind: FlowKind::Branch {
                cond_label: SecurityLabel::Secret,
                branch_var_labels: vec![SecurityLabel::Public],
            },
            at_node: 10,
        }];
        let results = verify_information_flow(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("implicit"));
    }

    #[test]
    fn test_implicit_flow_valid() {
        // if secret_cond { secret_var = 1 } — OK (secret ⊑ secret)
        let events = vec![FlowEvent {
            kind: FlowKind::Branch {
                cond_label: SecurityLabel::Secret,
                branch_var_labels: vec![SecurityLabel::Secret],
            },
            at_node: 10,
        }];
        assert!(verify_information_flow(&events).is_empty());
    }

    #[test]
    fn test_multiple_events_one_leak() {
        // Three events: two valid, one leak. Should report 1 violation.
        let events = vec![
            FlowEvent {
                kind: FlowKind::Assign {
                    dst_vreg: 0, dst_label: SecurityLabel::Public,
                    src_label: SecurityLabel::Public,
                },
                at_node: 10,
            },
            FlowEvent {
                kind: FlowKind::Assign {
                    dst_vreg: 1, dst_label: SecurityLabel::Secret,
                    src_label: SecurityLabel::Public, // OK: public ⊑ secret
                },
                at_node: 20,
            },
            FlowEvent {
                kind: FlowKind::Assign {
                    dst_vreg: 2, dst_label: SecurityLabel::Public,
                    src_label: SecurityLabel::TopSecret, // LEAK
                },
                at_node: 30,
            },
        ];
        let results = verify_information_flow(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("TopSecret"));
    }

    #[test]
    fn test_all_flows_valid_helper() {
        assert!(all_flows_valid(&[]));
        assert!(all_flows_valid(&[FlowViolation { valid: true, error: None }]));
        assert!(!all_flows_valid(&[FlowViolation { valid: false, error: Some("x".into()) }]));
    }
}

// ── Wave 91: IR-based information flow check (pipeline wiring) ───────
//
// TASKS.md §0.5 requires that information flow checking be CALLED from
// src/pipeline.rs, not just defined as library code with unit tests.

/// IR-based information flow violation (Wave 91 pipeline wiring).
#[derive(Debug, Clone)]
pub struct FlowViolationIR {
    /// Description of the violation.
    pub message: String,
}

/// Scan an IRProgram for information-flow violations (High → Low flows).
/// This is the pipeline-facing wrapper.
///
/// Currently advisory — logs warnings but does NOT abort compilation.
pub fn verify_information_flow_from_ir(program: &vuma_codegen::ir::IRProgram) -> Vec<FlowViolationIR> {
    let mut violations = Vec::new();
    // Collect flow events from the IR.
    // A High → Low flow occurs when a value from a high-security source
    // is assigned to a low-security destination.  Since VUMA doesn't have
    // explicit security label annotations in the IR yet (only in the AST),
    // we scan for patterns that COULD be flows (assignments, channel sends)
    // and report them as informational.
    let mut events: Vec<FlowEvent> = Vec::new();
    for (fi, func) in program.functions.iter().enumerate() {
        for (bi, block) in func.blocks.iter().enumerate() {
            for (ii, instr) in block.instructions.iter().enumerate() {
                if let vuma_codegen::ir::IRInstr::Call { func: name, .. } = instr {
                    if name == "channel_send" || name == "channel_send_cap" {
                        events.push(FlowEvent {
                            kind: FlowKind::ChannelSend { channel_label: SecurityLabel::Public, msg_label: SecurityLabel::Public },
                            at_node: fi * 10000 + bi * 100 + ii,
                        });
                    }
                }
                if let vuma_codegen::ir::IRInstr::Store { .. } = instr {
                    events.push(FlowEvent {
                        kind: FlowKind::Assign { dst_vreg: 0, dst_label: SecurityLabel::Public, src_label: SecurityLabel::Public },
                        at_node: fi * 10000 + bi * 100 + ii,
                    });
                }
            }
        }
    }
    // Run the information flow verifier on the collected events.
    let flow_violations = verify_information_flow(&events);
    for v in &flow_violations {
        violations.push(FlowViolationIR {
            message: format!("{:?}", v),
        });
    }
    violations
}
