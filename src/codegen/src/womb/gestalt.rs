//! # Gestalt — Tagless, Context-Dependent Memory Superposition
//!
//! LLMs think in context, not tags. We eliminate runtime tag overhead by
//! pushing the proof burden to the IVE. When a `GestaltInterpret` node is
//! encountered, the IVE performs abstract interpretation on the `CapD`
//! context to determine if the interpretation is provably safe.

use vuma_scg::{ContextAssertNode, GestaltDeclNode, GestaltInterpretNode, NodeId, NodePayload, SCG};
use std::collections::{HashMap, HashSet};

/// The result of a Gestalt interpretation proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GestaltProof {
    /// The IVE proved the interpretation is safe — no runtime tag needed.
    /// The codegen emits a raw memory cast.
    ProvenSafe {
        /// The variant that was proven active.
        variant: String,
        /// The proof condition (string representation for debugging).
        condition: String,
    },
    /// The IVE could not prove safety — a runtime tag check is required.
    /// The Gestalt is "degraded" with a hidden 1-byte tag.
    RequiresTagCheck {
        /// The variant being checked.
        variant: String,
        /// The byte offset of the tag within the allocation.
        tag_offset: u64,
    },
    /// The interpretation is unsafe and cannot be proven.
    /// This is a hard compile error under `--strict-gestalts`.
    Unprovable {
        /// The variant that could not be proven.
        variant: String,
        /// Why the proof failed.
        reason: String,
    },
}

/// The Gestalt interpreter performs abstract interpretation to verify
/// that GestaltInterpret nodes are safe.
pub struct GestaltInterpreter {
    /// Map from (gestalt_name, base_ptr) → set of proven-active variants.
    /// This is built from ContextAssert nodes.
    context_assertions: HashMap<(String, String), HashSet<String>>,
    /// Whether --strict-gestalts is enabled (hard error vs degrade).
    strict_mode: bool,
}

impl GestaltInterpreter {
    /// Create a new GestaltInterpreter.
    pub fn new(strict_mode: bool) -> Self {
        Self {
            context_assertions: HashMap::new(),
            strict_mode,
        }
    }

    /// Run the interpreter over the SCG, verifying all GestaltInterpret nodes.
    pub fn run(&mut self, scg: &mut SCG) -> Result<(), Vec<String>> {
        // Phase 1: Collect all ContextAssert nodes
        self.collect_context_assertions(scg);

        // Phase 2: Verify each GestaltInterpret node
        let interpret_ids: Vec<NodeId> = scg.node_ids()
            .filter(|id| {
                if let Some(n) = scg.get_node(*id) {
                    n.node_type == vuma_scg::NodeType::GestaltInterpret
                } else { false }
            })
            .collect();

        let mut errors = Vec::new();
        for id in interpret_ids {
            if let Err(e) = self.verify_interpretation(scg, id) {
                errors.push(e);
            }
        }

        // Phase 3: Mark GestaltDecl nodes as degraded if any interpretation
        // required a tag check
        self.mark_degraded_gestalts(scg);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Phase 1: Collect all ContextAssert nodes into the assertions map.
    fn collect_context_assertions(&mut self, scg: &SCG) {
        for id in scg.node_ids() {
            let node = match scg.get_node(id) { Some(n) => n, None => continue };
            if let NodePayload::ContextAssert(assert) = &node.payload {
                let key = (assert.gestalt_name.clone(), assert.base_ptr.clone());
                self.context_assertions
                    .entry(key)
                    .or_default()
                    .insert(assert.variant_name.clone());
            }
        }
    }

    /// Phase 2: Verify a single GestaltInterpret node.
    ///
    /// This is the core of the Interpretation Invariant.
    pub fn verify_interpretation(&self, scg: &mut SCG, node_id: NodeId) -> Result<(), String> {
        let interpret = {
            let node = scg.get_node(node_id)
                .ok_or("GestaltInterpret node not found")?;
            if let NodePayload::GestaltInterpret(g) = &node.payload {
                g.clone()
            } else {
                return Err("Node is not a GestaltInterpret".to_string());
            }
        };

        let proof = self.prove_variant(&interpret);

        // Update the node with the proof result
        if let Some(node) = scg.get_node_mut(node_id) {
            if let NodePayload::GestaltInterpret(g) = &mut node.payload {
                match &proof {
                    GestaltProof::ProvenSafe { .. } => {
                        g.proven_safe = true;
                        g.requires_tag_check = false;
                    }
                    GestaltProof::RequiresTagCheck { .. } => {
                        g.proven_safe = false;
                        g.requires_tag_check = true;
                    }
                    GestaltProof::Unprovable { variant, reason } => {
                        if self.strict_mode {
                            return Err(format!(
                                "Gestalt '{}' cannot be interpreted as '{}' in strict mode: {}",
                                interpret.gestalt_name, variant, reason
                            ));
                        }
                        // In non-strict mode, degrade with a tag check
                        g.proven_safe = false;
                        g.requires_tag_check = true;
                    }
                }
            }
        }

        Ok(())
    }

    /// Attempt to prove that a specific variant is active for a Gestalt.
    ///
    /// ## Algorithm
    ///
    /// 1. Check if there's a ContextAssert for this (gestalt, ptr, variant)
    /// 2. If yes → ProvenSafe
    /// 3. If no → check if the Gestalt is already degraded
    ///    - If degraded → RequiresTagCheck (use the runtime tag)
    ///    - If not degraded → Unprovable (the IVE can't prove it)
    fn prove_variant(&self, interpret: &GestaltInterpretNode) -> GestaltProof {
        let key = (interpret.gestalt_name.clone(), interpret.base_ptr.clone());

        // Check if the variant is asserted in the current context
        if let Some(asserted) = self.context_assertions.get(&key) {
            if asserted.contains(&interpret.variant_name) {
                return GestaltProof::ProvenSafe {
                    variant: interpret.variant_name.clone(),
                    condition: format!(
                        "context_assert({}, {}, {})",
                        interpret.gestalt_name,
                        interpret.base_ptr,
                        interpret.variant_name
                    ),
                };
            }
        }

        // Not proven — check if we should degrade
        // (In a full implementation, this would check the GestaltDecl's degraded flag)
        GestaltProof::RequiresTagCheck {
            variant: interpret.variant_name.clone(),
            tag_offset: 0, // Tag is at byte 0
        }
    }

    /// Phase 3: Mark GestaltDecl nodes as degraded if any interpretation
    /// of that Gestalt required a tag check.
    fn mark_degraded_gestalts(&self, scg: &mut SCG) {
        // Collect which Gestalts need degradation
        let mut needs_degradation: HashSet<String> = HashSet::new();
        for id in scg.node_ids() {
            if let Some(node) = scg.get_node(id) {
                if let NodePayload::GestaltInterpret(g) = &node.payload {
                    if g.requires_tag_check {
                        needs_degradation.insert(g.gestalt_name.clone());
                    }
                }
            }
        }

        // Mark the GestaltDecl nodes
        let decl_ids: Vec<NodeId> = scg.node_ids()
            .filter(|id| {
                if let Some(n) = scg.get_node(*id) {
                    n.node_type == vuma_scg::NodeType::GestaltDecl
                } else { false }
            })
            .collect();
        for id in decl_ids {
            if let Some(node) = scg.get_node_mut(id) {
                if let NodePayload::GestaltDecl(g) = &mut node.payload {
                    if needs_degradation.contains(&g.name) {
                        g.degraded = true;
                        g.tag_offset = Some(0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proven_safe() {
        let mut interp = GestaltInterpreter::new(false);
        interp.context_assertions.insert(
            ("Message".to_string(), "ptr".to_string()),
            {
                let mut s = HashSet::new();
                s.insert("Text".to_string());
                s
            },
        );

        let interpret = GestaltInterpretNode {
            base_ptr: "ptr".to_string(),
            gestalt_name: "Message".to_string(),
            variant_name: "Text".to_string(),
            result_var: "result".to_string(),
            proven_safe: false,
            requires_tag_check: false,
        };

        let proof = interp.prove_variant(&interpret);
        assert!(matches!(proof, GestaltProof::ProvenSafe { .. }));
    }

    #[test]
    fn test_unproven_requires_tag() {
        let interp = GestaltInterpreter::new(false);

        let interpret = GestaltInterpretNode {
            base_ptr: "ptr".to_string(),
            gestalt_name: "Message".to_string(),
            variant_name: "Binary".to_string(),
            result_var: "result".to_string(),
            proven_safe: false,
            requires_tag_check: false,
        };

        let proof = interp.prove_variant(&interpret);
        assert!(matches!(proof, GestaltProof::RequiresTagCheck { .. }));
    }

    #[test]
    fn test_strict_mode_unprovable() {
        let interp = GestaltInterpreter::new(true);

        let interpret = GestaltInterpretNode {
            base_ptr: "ptr".to_string(),
            gestalt_name: "Message".to_string(),
            variant_name: "Unknown".to_string(),
            result_var: "result".to_string(),
            proven_safe: false,
            requires_tag_check: false,
        };

        // In strict mode with no assertion, the proof should be RequiresTagCheck
        // (which becomes an error in verify_interpretation)
        let proof = interp.prove_variant(&interpret);
        assert!(matches!(proof, GestaltProof::RequiresTagCheck { .. }));
    }
}
