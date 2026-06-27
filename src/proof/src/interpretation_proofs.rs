//! # Gestalt Interpretation Verification
//!
//! Moved from `src/codegen/src/womb/gestalt.rs` to the IVE proof engine
//! where it belongs. This module implements the Interpretation Invariant
//! for Gestalt (tagless memory superposition) nodes.
//!
//! When a `GestaltInterpret` node is encountered, the IVE performs abstract
//! interpretation on the `CapD` context to determine if the interpretation
//! is provably safe. If it is, no runtime tag is needed. If not, the Gestalt
//! is "degraded" with a hidden 1-byte runtime tag.

use std::collections::{HashMap, HashSet};
use vuma_scg::graph::SCG;
use vuma_scg::node::{
    ContextAssertNode, GestaltDeclNode, GestaltInterpretNode, NodeId, NodePayload, NodeType,
};

// ---------------------------------------------------------------------------
// Original InterpretationProof (preserved for backward compatibility)
// ---------------------------------------------------------------------------

/// A proof that an interpretation (type cast / view change) is valid.
///
/// This struct is used by the composition system and serialization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterpretationProof {
    /// Proofs that BD representations are compatible across the interpretation.
    pub bd_compatibility_proofs: Vec<BDCompatibilityProof>,
    /// Proofs that reinterpretation is safe (no aliasing violations).
    pub reinterpretation_safety_proofs: Vec<ReinterpretationSafetyProof>,
    /// The underlying formal proof.
    pub proof: crate::proof::Proof,
}

/// A proof that two BD representations are compatible.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BDCompatibilityProof {
    /// The formal proof.
    pub proof: crate::proof::Proof,
}

/// A proof that reinterpretation is safe (no aliasing violations).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReinterpretationSafetyProof {
    /// The formal proof.
    pub proof: crate::proof::Proof,
}

// ---------------------------------------------------------------------------
// Gestalt-specific verification (moved from codegen/womb)
// ---------------------------------------------------------------------------

/// The result of a Gestalt interpretation proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GestaltProof {
    /// The IVE proved the interpretation is safe — no runtime tag needed.
    ProvenSafe {
        variant: String,
        condition: String,
    },
    /// The IVE could not prove safety — a runtime tag check is required.
    RequiresTagCheck {
        variant: String,
        tag_offset: u64,
    },
    /// The interpretation is unsafe and cannot be proven.
    Unprovable {
        variant: String,
        reason: String,
    },
}

/// The Gestalt verifier performs abstract interpretation to verify
/// that GestaltInterpret nodes are safe.
///
/// This is wired into the main verification loop via `verify_interpretation`.
pub struct GestaltVerifier {
    /// Map from (gestalt_name, base_ptr) → set of proven-active variants.
    context_assertions: HashMap<(String, String), HashSet<String>>,
    /// Whether --strict-gestalts is enabled (hard error vs degrade).
    strict_mode: bool,
}

impl GestaltVerifier {
    /// Create a new GestaltVerifier.
    pub fn new(strict_mode: bool) -> Self {
        Self {
            context_assertions: HashMap::new(),
            strict_mode,
        }
    }

    /// Run the verifier over the SCG, verifying all GestaltInterpret nodes.
    ///
    /// This is the entry point called by the main IVE verification loop.
    pub fn run(&mut self, scg: &mut SCG) -> Result<(), Vec<String>> {
        self.collect_context_assertions(scg);

        let interpret_ids: Vec<NodeId> = scg
            .node_ids()
            .filter(|id| {
                if let Some(n) = scg.get_node(*id) {
                    n.node_type == NodeType::GestaltInterpret
                } else {
                    false
                }
            })
            .collect();

        let mut errors = Vec::new();
        for id in interpret_ids {
            if let Err(e) = self.verify_interpretation(scg, id) {
                errors.push(e);
            }
        }

        self.mark_degraded_gestalts(scg);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Collect all ContextAssert nodes into the assertions map.
    fn collect_context_assertions(&mut self, scg: &SCG) {
        for id in scg.node_ids() {
            if let Some(node) = scg.get_node(id) {
                if let NodePayload::ContextAssert(assert) = &node.payload {
                    let key = (assert.gestalt_name.clone(), assert.base_ptr.clone());
                    self.context_assertions
                        .entry(key)
                        .or_default()
                        .insert(assert.variant_name.clone());
                }
            }
        }
    }

    /// Verify a single GestaltInterpret node.
    ///
    /// This is the core of the Interpretation Invariant for Gestalts.
    /// It determines whether the interpretation can be proven safe
    /// or requires a runtime tag check (degradation).
    pub fn verify_interpretation(
        &self,
        scg: &mut SCG,
        node_id: NodeId,
    ) -> Result<(), String> {
        let interpret = {
            let node = scg
                .get_node(node_id)
                .ok_or("GestaltInterpret node not found")?;
            if let NodePayload::GestaltInterpret(g) = &node.payload {
                g.clone()
            } else {
                return Err("Node is not a GestaltInterpret".to_string());
            }
        };

        let proof = self.prove_variant(&interpret);

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
    /// 2. If yes → ProvenSafe (zero-cost cast, no runtime tag)
    /// 3. If no → RequiresTagCheck (degrade with hidden 1-byte tag)
    fn prove_variant(&self, interpret: &GestaltInterpretNode) -> GestaltProof {
        let key = (interpret.gestalt_name.clone(), interpret.base_ptr.clone());

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

        GestaltProof::RequiresTagCheck {
            variant: interpret.variant_name.clone(),
            tag_offset: 0,
        }
    }

    /// Mark GestaltDecl nodes as degraded if any interpretation required a tag check.
    fn mark_degraded_gestalts(&self, scg: &mut SCG) {
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

        let decl_ids: Vec<NodeId> = scg
            .node_ids()
            .filter(|id| {
                if let Some(n) = scg.get_node(*id) {
                    n.node_type == NodeType::GestaltDecl
                } else {
                    false
                }
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
    fn test_proven_safe_when_asserted() {
        let mut verifier = GestaltVerifier::new(false);
        verifier.context_assertions.insert(
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

        let proof = verifier.prove_variant(&interpret);
        assert!(matches!(proof, GestaltProof::ProvenSafe { .. }));
    }

    #[test]
    fn test_requires_tag_when_unproven() {
        let verifier = GestaltVerifier::new(false);

        let interpret = GestaltInterpretNode {
            base_ptr: "ptr".to_string(),
            gestalt_name: "Message".to_string(),
            variant_name: "Binary".to_string(),
            result_var: "result".to_string(),
            proven_safe: false,
            requires_tag_check: false,
        };

        let proof = verifier.prove_variant(&interpret);
        assert!(matches!(proof, GestaltProof::RequiresTagCheck { .. }));
    }
}
