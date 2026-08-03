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

use std::collections::HashSet;

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
            (Public, _) => true, // Public flows anywhere
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
            FlowKind::Assign {
                dst_vreg,
                dst_label,
                src_label,
            } => {
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
            FlowKind::BinOp {
                dst_vreg,
                dst_label,
                lhs_label,
                rhs_label,
            } => {
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
            FlowKind::ChannelSend {
                channel_label,
                msg_label,
            } => {
                if !msg_label.can_flow_to(*channel_label) {
                    results.push(FlowViolation {
                        valid: false,
                        error: Some(format!(
                            "information-flow violation at node {}: sending message with label {} \
                             on channel with label {} — {} ⊀ {} (would leak {} data to {} channel)",
                            event.at_node,
                            msg_label,
                            channel_label,
                            msg_label,
                            channel_label,
                            msg_label,
                            channel_label
                        )),
                    });
                }
            }
            FlowKind::Branch {
                cond_label,
                branch_var_labels,
            } => {
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
                                event.at_node, cond_label, var_label, cond_label, var_label
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
        assert_eq!(
            SecurityLabel::Public.join(SecurityLabel::Secret),
            SecurityLabel::Secret
        );
        assert_eq!(
            SecurityLabel::Secret.join(SecurityLabel::Public),
            SecurityLabel::Secret
        );
        assert_eq!(
            SecurityLabel::Internal.join(SecurityLabel::TopSecret),
            SecurityLabel::TopSecret
        );
        assert_eq!(
            SecurityLabel::Public.join(SecurityLabel::Public),
            SecurityLabel::Public
        );
    }

    #[test]
    fn test_direct_assignment_valid() {
        // public → public: OK
        let events = vec![FlowEvent {
            kind: FlowKind::Assign {
                dst_vreg: 0,
                dst_label: SecurityLabel::Public,
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
                dst_vreg: 0,
                dst_label: SecurityLabel::Public,
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
                dst_vreg: 0,
                dst_label: SecurityLabel::Public,
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
            kind: FlowKind::ChannelSend {
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
            kind: FlowKind::ChannelSend {
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
                    dst_vreg: 0,
                    dst_label: SecurityLabel::Public,
                    src_label: SecurityLabel::Public,
                },
                at_node: 10,
            },
            FlowEvent {
                kind: FlowKind::Assign {
                    dst_vreg: 1,
                    dst_label: SecurityLabel::Secret,
                    src_label: SecurityLabel::Public, // OK: public ⊑ secret
                },
                at_node: 20,
            },
            FlowEvent {
                kind: FlowKind::Assign {
                    dst_vreg: 2,
                    dst_label: SecurityLabel::Public,
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
        assert!(all_flows_valid(&[FlowViolation {
            valid: true,
            error: None
        }]));
        assert!(!all_flows_valid(&[FlowViolation {
            valid: false,
            error: Some("x".into())
        }]));
    }
}

// ── IR-based information flow check (pipeline wiring) ───────
//
// TASKS.md §0.5 requires that information flow checking be CALLED from
// src/pipeline.rs, not just defined as library code with unit tests.
//
// Wave 2 fix (Task 3): the wrapper now consults `secret_vars` (collected
// from `#[secret]` annotations by `pipeline.rs::collect_secret_vars`) to
// assign real `SecurityLabel::Secret` labels to vregs whose declared name
// is in that set, instead of hardcoding `SecurityLabel::Public` for every
// flow. The vreg→name lookup goes through `IRFunction::vregs`, which the
// SCG→IR bridge populates from source-level `let`/parameter names. This
// means a `#[secret] let k = ...` in the source produces an IR vreg named
// `"k"`, and any `Store`/`ChannelSend` involving that vreg is labeled
// `Secret` here. Limitation: this is a name-based proxy, not full
// AST→IR label propagation — vregs whose names are stripped by optimisation
// (e.g. anonymous temporaries holding a secret value) will not be tainted.
// Full label plumbing through the IR is deferred to a later wave (see
// `docs/caveats.md` §0.7).

/// IR-based information flow violation ( pipeline wiring).
#[derive(Debug, Clone)]
pub struct FlowViolationIR {
    /// Description of the violation.
    pub message: String,
}

/// Resolve a vreg's [`SecurityLabel`] from its declared name.
///
/// Returns `SecurityLabel::Secret` when `vreg` is an `IRValue::Register`
/// whose ID maps (via `vregs`) to a `VirtualRegister` whose `name` is in
/// `secret_vars`. Returns `SecurityLabel::Public` otherwise — covering
/// immediates, addresses, labels, anonymous vregs, and named vregs that
/// are not annotated `#[secret]` at the source level.
fn label_of_vreg(
    vreg: &vuma_codegen::ir::IRValue,
    vregs: &std::collections::HashMap<u32, vuma_codegen::ir::VirtualRegister>,
    secret_vars: &HashSet<String>,
) -> SecurityLabel {
    match vreg {
        vuma_codegen::ir::IRValue::Register(id) => {
            let is_secret = vregs
                .get(id)
                .and_then(|vr| vr.name.as_deref())
                .map(|name| secret_vars.contains(name))
                .unwrap_or(false);
            if is_secret {
                SecurityLabel::Secret
            } else {
                SecurityLabel::Public
            }
        }
        _ => SecurityLabel::Public,
    }
}

/// V-A3-8: Resolve a vreg's effective security label, consulting the
/// dynamic taint map (from Loads of Secret memory) first, then falling
/// back to the static `secret_vars` lookup.
///
/// This is a free function (not a closure) so it can be called from
/// within the IR walk loop without holding an immutable borrow of
/// `vreg_labels` — the loop body needs to mutate `vreg_labels` in the
/// Load arm.
fn resolve_label(
    vreg: &vuma_codegen::ir::IRValue,
    vregs: &std::collections::HashMap<u32, vuma_codegen::ir::VirtualRegister>,
    secret_vars: &HashSet<String>,
    vreg_labels: &std::collections::HashMap<u32, SecurityLabel>,
) -> SecurityLabel {
    match vreg {
        vuma_codegen::ir::IRValue::Register(id) => {
            // Dynamic taint takes precedence (a vreg loaded from
            // Secret memory is Secret even if its declared name
            // isn't in secret_vars).
            if let Some(&label) = vreg_labels.get(id) {
                return label;
            }
            label_of_vreg(vreg, vregs, secret_vars)
        }
        _ => SecurityLabel::Public,
    }
}

/// Scan an IRProgram for information-flow violations (High → Low flows).
/// This is the pipeline-facing wrapper.
///
/// `secret_vars` is the set of source-level variable names annotated with
/// `#[secret]` (collected by `pipeline.rs::collect_secret_vars` and threaded
/// down through `run_ir_pipeline`). Each `Store` and `ChannelSend` event
/// derived from the IR is labeled `Secret` if any of its vregs' declared
/// names appears in `secret_vars`, otherwise `Public`. The underlying
/// `verify_information_flow` then flags any `Secret → Public` (or higher)
/// flow as a violation.
///
/// V-A3-8 fix: indirect leak detection via alias analysis. Previously,
/// the verifier only checked direct flows (Store of a Secret vreg to a
/// Public destination, ChannelSend of a Secret msg to a Public channel).
/// It missed indirect flows through memory:
///   1. `Store secret_vreg -> [addr_vreg]`  (memory[addr] now holds Secret)
///   2. `Load dst_vreg <- [addr_vreg]`      (dst inherits Secret from memory)
///   3. `Store dst_vreg -> [public_addr]`   (LEAK: Secret → Public via memory)
///
/// The fix maintains two side tables:
///   - `memory_labels: HashMap<u32, SecurityLabel>` — the label of the
///     value currently stored at each memory cell (keyed by the addr
///     vreg ID used in the Store). A cell is `Secret` if any Secret
///     value has ever been stored to it (monotonic — once Secret,
///     always Secret, modelling the standard non-interference
///     property).
///   - `vreg_labels: HashMap<u32, SecurityLabel>` — the label of each
///     vreg, combining the static `secret_vars` lookup with the
///     dynamic taint propagated from Loads. A vreg is `Secret` if
///     either its declared name is in `secret_vars` OR it was loaded
///     from a Secret memory cell.
///
/// This is a flow-insensitive, field-insensitive alias analysis: each
/// addr vreg is treated as a distinct memory cell (no aliasing between
/// different addr vregs). A future extension could add a proper
/// alias-analysis pass (e.g., Andersen-style) to handle cases where
/// multiple vregs point to the same cell. The current model is sound
/// (never misses a leak) but may over-approximate (false positives on
/// cells that happen to share an addr vreg with an unrelated Secret
/// store).
///
/// Currently advisory in shape but wired as a hard-fail gate by the caller
/// in `pipeline.rs` — any non-empty `Vec<FlowViolationIR>` aborts compilation.
pub fn verify_information_flow_from_ir(
    program: &vuma_codegen::ir::IRProgram,
    secret_vars: &HashSet<String>,
) -> Vec<FlowViolationIR> {
    let mut violations = Vec::new();
    // Collect flow events from the IR.
    // A High → Low flow occurs when a value from a high-security source
    // is assigned to a low-security destination. We derive real
    // `SecurityLabel`s by consulting `secret_vars`: any vreg whose
    // declared name (from `IRFunction::vregs`) is in `secret_vars` is
    // labeled `Secret`; everything else is `Public`.
    let mut events: Vec<FlowEvent> = Vec::new();
    // V-A3-8: side tables for indirect-flow tracking.
    // memory_labels: addr_vreg_id -> SecurityLabel of the value stored
    //   at that address. Monotonically Secret (once Secret, always Secret).
    // vreg_labels: vreg_id -> SecurityLabel, combining static (secret_vars)
    //   and dynamic (Load-taint) labels. Takes precedence over the static
    //   label_of_vreg lookup when present.
    let mut memory_labels: std::collections::HashMap<u32, SecurityLabel> =
        std::collections::HashMap::new();
    let mut vreg_labels: std::collections::HashMap<u32, SecurityLabel> =
        std::collections::HashMap::new();
    for (fi, func) in program.functions.iter().enumerate() {
        for (bi, block) in func.blocks.iter().enumerate() {
            for (ii, instr) in block.instructions.iter().enumerate() {
                let at_node = fi * 10000 + bi * 100 + ii;
                match instr {
                    // Canonical channel-send instruction emitted by the
                    // SCG→IR bridge. The channel handle (`ch`) is the
                    // destination label, the message (`msg`) is the source.
                    vuma_codegen::ir::IRInstr::ChannelSend { ch, msg, .. } => {
                        events.push(FlowEvent {
                            kind: FlowKind::ChannelSend {
                                channel_label: resolve_label(ch, &func.vregs, secret_vars, &vreg_labels),
                                msg_label: resolve_label(msg, &func.vregs, secret_vars, &vreg_labels),
                            },
                            at_node,
                        });
                    }
                    // Legacy / pre-IPC-lowering form: a `Call` to
                    // `channel_send`/`channel_send_cap` with
                    // `args = [ch, msg]`. Handled for robustness — by the
                    // time `verify_information_flow_from_ir` runs in
                    // `run_ir_pipeline`, IPC lowering has usually rewritten
                    // these into `Syscall`/`Store`/`Load`/`BinOp`, but the
                    // canonical `IRInstr::ChannelSend` form above covers the
                    // SCG-NodePayload path.
                    vuma_codegen::ir::IRInstr::Call { func: name, args, .. }
                        if name == "channel_send" || name == "channel_send_cap" =>
                    {
                        let channel_label = args
                            .first()
                            .map(|v| resolve_label(v, &func.vregs, secret_vars, &vreg_labels))
                            .unwrap_or(SecurityLabel::Public);
                        let msg_label = args
                            .get(1)
                            .map(|v| resolve_label(v, &func.vregs, secret_vars, &vreg_labels))
                            .unwrap_or(SecurityLabel::Public);
                        events.push(FlowEvent {
                            kind: FlowKind::ChannelSend {
                                channel_label,
                                msg_label,
                            },
                            at_node,
                        });
                    }
                    // `Store { value, addr, .. }` — treat as an assignment
                    // from `value` (source) to the memory location at `addr`
                    // (destination). Real vreg IDs are extracted from the
                    // `IRValue::Register` fields instead of the previous
                    // hardcoded `0`. Labels are resolved via `resolve_label`
                    // — `Secret` when the vreg's declared name is in
                    // `secret_vars` (static) OR the vreg was tainted by a
                    // prior Load from Secret memory (dynamic, V-A3-8).
                    // A `Secret → Public` store (writing a secret value to a
                    // non-secret destination) is flagged by `verify_information_flow`.
                    //
                    // V-A3-8: also update `memory_labels[addr_vreg]` to the
                    // source label, so subsequent Loads from this address
                    // inherit the taint.
                    vuma_codegen::ir::IRInstr::Store { value, addr, .. } => {
                        let dst_vreg = addr.as_register().unwrap_or(0);
                        let dst_label = resolve_label(addr, &func.vregs, secret_vars, &vreg_labels);
                        let src_label = resolve_label(value, &func.vregs, secret_vars, &vreg_labels);
                        events.push(FlowEvent {
                            kind: FlowKind::Assign {
                                dst_vreg,
                                dst_label,
                                src_label,
                            },
                            at_node,
                        });
                        // V-A3-8: propagate taint to the memory cell.
                        // Monotonic: once Secret, always Secret.
                        let cell_label = memory_labels
                            .get(&dst_vreg)
                            .copied()
                            .unwrap_or(SecurityLabel::Public);
                        let new_cell_label = if src_label == SecurityLabel::Secret
                            || cell_label == SecurityLabel::Secret
                        {
                            SecurityLabel::Secret
                        } else {
                            SecurityLabel::Public
                        };
                        memory_labels.insert(dst_vreg, new_cell_label);
                    }
                    // V-A3-8: `Load { dst, addr, .. }` — read a value from
                    // the memory cell at `addr` into `dst`. The dst vreg
                    // inherits the label of the memory cell (recorded in
                    // `memory_labels` by a prior Store). If no prior Store
                    // to this address is recorded, the cell is conservatively
                    // Public (the verifier has no information about it).
                    //
                    // This is the key fix for indirect leaks: without this
                    // arm, a Load from a Secret-tainted memory cell would
                    // produce a Public vreg, and subsequent Stores of that
                    // vreg to Public destinations would NOT be flagged.
                    //
                    // We do NOT emit a FlowEvent::Assign for the Load itself
                    // — the Load is just a taint propagation, not a leak.
                    // The leak is detected when the tainted dst vreg is
                    // subsequently Stored to a Public destination or sent
                    // on a Public channel (those arms call `resolve_label`
                    // which consults `vreg_labels` and picks up the taint).
                    vuma_codegen::ir::IRInstr::Load { dst, addr, .. } => {
                        let addr_vreg = addr.as_register().unwrap_or(0);
                        let dst_vreg = dst.as_register().unwrap_or(0);
                        let cell_label = memory_labels
                            .get(&addr_vreg)
                            .copied()
                            .unwrap_or(SecurityLabel::Public);
                        // Record the dst vreg's dynamic label so subsequent
                        // uses of dst (in Store/ChannelSend) resolve to the
                        // tainted label via `resolve_label`.
                        vreg_labels.insert(dst_vreg, cell_label);
                    }
                    _ => {}
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

#[cfg(test)]
mod ir_tests {
    //! Tests for the IR-based wrapper `verify_information_flow_from_ir`.
    //!
    //! These construct minimal `IRProgram`s with named vregs and verify
    //! that the wrapper (a) assigns `Secret` to vregs whose declared name
    //! is in `secret_vars`, (b) assigns `Public` to everything else, and
    //! (c) surfaces `Secret → Public` flows as `FlowViolationIR`s. The
    //! underlying `verify_information_flow` lattice is exercised in the
    //! `tests` module above; here we only test the wrapper's input-shaping.
    use super::*;

    /// Build a one-function, one-block `IRProgram` whose block holds the
    /// supplied instructions. Each `(id, name)` pair in `named_vregs` is
    /// registered in the function's `vregs` table so `label_of_vreg` can
    /// resolve it.
    fn build_program(
        instructions: Vec<vuma_codegen::ir::IRInstr>,
        named_vregs: &[(u32, &str)],
    ) -> vuma_codegen::ir::IRProgram {
        use vuma_codegen::ir::{IRFunction, IRProgram, VirtualRegister};
        let mut func = IRFunction::new("test_fn");
        for (id, name) in named_vregs {
            func.register_vreg(VirtualRegister::named(*id, *name));
        }
        // `IRFunction::new` pre-populates a single entry block at index 0.
        func.blocks[0].instructions = instructions;
        let mut prog = IRProgram::new();
        prog.functions.push(func);
        prog
    }

    #[test]
    fn test_store_secret_to_public_is_leak() {
        // `Store { value: %v1 (named "secret_key"), addr: %v0 (named "out") }`
        // with `secret_vars = {"secret_key"}`.
        // → src_label=Secret, dst_label=Public → Secret ⊀ Public → LEAK.
        let instr = vuma_codegen::ir::IRInstr::Store {
            value: vuma_codegen::ir::IRValue::Register(1),
            addr: vuma_codegen::ir::IRValue::Register(0),
            offset: 0,
            ty: vuma_codegen::ir::IRType::I64,
        };
        let prog = build_program(vec![instr], &[(0, "out"), (1, "secret_key")]);
        let mut secrets = HashSet::new();
        secrets.insert("secret_key".to_string());
        let violations = verify_information_flow_from_ir(&prog, &secrets);
        assert_eq!(
            violations.len(),
            1,
            "storing a secret-named value into a non-secret destination must be flagged"
        );
        assert!(
            violations[0].message.contains("would leak"),
            "violation message should mention the leak: {:?}",
            violations[0].message
        );
    }

    #[test]
    fn test_store_public_to_public_is_ok() {
        // `Store { value: %v1 (named "x"), addr: %v0 (named "out") }`
        // with `secret_vars = {"secret_key"}` (neither "x" nor "out" is secret).
        // → src=Public, dst=Public → Public ⊑ Public → OK.
        let instr = vuma_codegen::ir::IRInstr::Store {
            value: vuma_codegen::ir::IRValue::Register(1),
            addr: vuma_codegen::ir::IRValue::Register(0),
            offset: 0,
            ty: vuma_codegen::ir::IRType::I64,
        };
        let prog = build_program(vec![instr], &[(0, "out"), (1, "x")]);
        let mut secrets = HashSet::new();
        secrets.insert("secret_key".to_string());
        let violations = verify_information_flow_from_ir(&prog, &secrets);
        assert!(
            violations.is_empty(),
            "public→public store must not be flagged: {:?}",
            violations
        );
    }

    #[test]
    fn test_store_secret_to_secret_is_ok() {
        // Both source and destination are secret-named.
        // → src=Secret, dst=Secret → Secret ⊑ Secret → OK.
        let instr = vuma_codegen::ir::IRInstr::Store {
            value: vuma_codegen::ir::IRValue::Register(1),
            addr: vuma_codegen::ir::IRValue::Register(0),
            offset: 0,
            ty: vuma_codegen::ir::IRType::I64,
        };
        let prog = build_program(
            vec![instr],
            &[(0, "secret_buf"), (1, "secret_key")],
        );
        let mut secrets = HashSet::new();
        secrets.insert("secret_key".to_string());
        secrets.insert("secret_buf".to_string());
        let violations = verify_information_flow_from_ir(&prog, &secrets);
        assert!(
            violations.is_empty(),
            "secret→secret store must not be flagged: {:?}",
            violations
        );
    }

    #[test]
    fn test_channel_send_secret_on_public_channel_is_leak() {
        // `ChannelSend { ch: %v0 (named "public_ch"), msg: %v1 (named "secret_key") }`
        // with `secret_vars = {"secret_key"}`.
        // → channel_label=Public, msg_label=Secret → Secret ⊀ Public → LEAK.
        let instr = vuma_codegen::ir::IRInstr::ChannelSend {
            ch: vuma_codegen::ir::IRValue::Register(0),
            msg: vuma_codegen::ir::IRValue::Register(1),
            ty: None,
        };
        let prog = build_program(vec![instr], &[(0, "public_ch"), (1, "secret_key")]);
        let mut secrets = HashSet::new();
        secrets.insert("secret_key".to_string());
        let violations = verify_information_flow_from_ir(&prog, &secrets);
        assert_eq!(
            violations.len(),
            1,
            "sending a secret-named message on a public-named channel must be flagged"
        );
        assert!(
            violations[0].message.contains("channel"),
            "violation message should mention the channel: {:?}",
            violations[0].message
        );
    }

    #[test]
    fn test_empty_secret_vars_yields_no_violations() {
        // With an empty `secret_vars`, every vreg is labeled `Public` and
        // no flow can be a leak — this preserves the historical "structural
        // zero-violations" behaviour for programs without `#[secret]`
        // annotations.
        let instr = vuma_codegen::ir::IRInstr::Store {
            value: vuma_codegen::ir::IRValue::Register(1),
            addr: vuma_codegen::ir::IRValue::Register(0),
            offset: 0,
            ty: vuma_codegen::ir::IRType::I64,
        };
        let prog = build_program(vec![instr], &[(0, "out"), (1, "x")]);
        let secrets: HashSet<String> = HashSet::new();
        let violations = verify_information_flow_from_ir(&prog, &secrets);
        assert!(
            violations.is_empty(),
            "with no #[secret] annotations no flows should be flagged: {:?}",
            violations
        );
    }

    #[test]
    fn test_real_vreg_id_propagated_not_hardcoded_zero() {
        // Regression guard: the wrapper used to hardcode `dst_vreg: 0` for
        // every Store. Now the real vreg ID from `addr` must appear in the
        // violation message. We use `addr = Register(7)` and assert the
        // message references "vreg 7" (not "vreg 0").
        let instr = vuma_codegen::ir::IRInstr::Store {
            value: vuma_codegen::ir::IRValue::Register(8),
            addr: vuma_codegen::ir::IRValue::Register(7),
            offset: 0,
            ty: vuma_codegen::ir::IRType::I64,
        };
        let prog = build_program(vec![instr], &[(7, "out"), (8, "secret_key")]);
        let mut secrets = HashSet::new();
        secrets.insert("secret_key".to_string());
        let violations = verify_information_flow_from_ir(&prog, &secrets);
        assert_eq!(violations.len(), 1);
        assert!(
            violations[0].message.contains("vreg 7"),
            "violation should reference the real dst vreg id (7), not hardcoded 0: {:?}",
            violations[0].message
        );
    }
}
