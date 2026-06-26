//! # Concept — Relational Data with Lazy Layout Inference
//!
//! A Concept is not a rigid memory block; it is a set of relational edges.
//! The physical memory layout is lazily inferred by the compiler based on
//! access patterns. If fields are accessed together, they are packed (AoS).
//! If accessed independently in loops, they are separated (SoA).

use vuma_scg::{
    ConceptDeclNode, ConceptFieldNode, ConceptLayoutHint, NodeId, NodePayload, SCG,
};
use std::collections::HashMap;

/// The resolved physical layout for a Concept.
#[derive(Debug, Clone, PartialEq)]
pub struct ConceptLayout {
    /// The layout strategy chosen by the resolver.
    pub strategy: LayoutStrategy,
    /// Byte offset of each field within the allocation.
    pub field_offsets: HashMap<String, u64>,
    /// Total size in bytes.
    pub total_size: u64,
    /// Alignment requirement.
    pub align: u64,
}

/// The physical layout strategy chosen by the resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutStrategy {
    /// Array of Structs — fields packed together contiguously.
    /// Best when fields are accessed together.
    AoS,
    /// Struct of Arrays — each field in a separate buffer.
    /// Best when fields are accessed independently in loops.
    SoA,
}

/// The Layout Resolution Pass analyzes Concept access patterns and resolves
/// the physical memory layout.
///
/// ## Algorithm
///
/// 1. Walk all ConceptAccess nodes in the SCG
/// 2. Count how many times each field is accessed
/// 3. Analyze co-access patterns (which fields are accessed together)
/// 4. Decide AoS vs SoA based on the hint and access patterns:
///    - If `layout_hint` is `AoS`, always use AoS
///    - If `layout_hint` is `SoA`, always use SoA
///    - If `layout_hint` is `Auto`:
///      - Use SoA if any field has `independent_access = true` AND
///        `access_count > threshold`
///      - Otherwise use AoS
/// 5. Compute byte offsets and total size
pub struct LayoutResolutionPass {
    /// Access counts per (concept_name, field_name)
    access_counts: HashMap<(String, String), u64>,
    /// Co-access matrix: which fields are accessed together
    co_access: HashMap<(String, String, String), u64>,
}

impl LayoutResolutionPass {
    /// Create a new LayoutResolutionPass.
    pub fn new() -> Self {
        Self {
            access_counts: HashMap::new(),
            co_access: HashMap::new(),
        }
    }

    /// Run the layout resolution pass over the entire SCG.
    ///
    /// This analyzes all ConceptAccess nodes, collects access patterns,
    /// and resolves the physical layout for each ConceptDecl.
    pub fn run(&mut self, scg: &mut SCG) -> Result<(), String> {
        // Phase 1: Collect access patterns
        self.collect_access_patterns(scg);

        // Phase 2: Resolve layout for each ConceptDecl
        let concept_ids: Vec<NodeId> = scg.node_ids()
            .filter(|id| {
                if let Some(n) = scg.get_node(*id) {
                    n.node_type == vuma_scg::NodeType::ConceptDecl
                } else { false }
            })
            .collect();

        for concept_id in concept_ids {
            self.resolve_concept_layout(scg, concept_id)?;
        }

        Ok(())
    }

    /// Phase 1: Walk all ConceptAccess nodes and count access patterns.
    fn collect_access_patterns(&mut self, scg: &SCG) {
        let access_nodes: Vec<(NodeId, String, String)> = scg.node_ids()
            .filter_map(|id| {
                if let Some(n) = scg.get_node(id) {
                    if let NodePayload::ConceptAccess(a) = &n.payload {
                        return Some((id, a.concept_name.clone(), a.field_name.clone()));
                    }
                }
                None
            })
            .collect();

        // Count individual field accesses
        for (_, concept, field) in &access_nodes {
            *self.access_counts.entry((concept.clone(), field.clone())).or_insert(0) += 1;
        }

        // Analyze co-access: fields accessed in the same function
        // are considered co-accessed. A more sophisticated analysis
        // would look at basic block co-occurrence.
        let mut by_concept: HashMap<String, Vec<String>> = HashMap::new();
        for (_, concept, field) in &access_nodes {
            by_concept.entry(concept.clone()).or_default().push(field.clone());
        }
        for (concept, fields) in &by_concept {
            for i in 0..fields.len() {
                for j in (i + 1)..fields.len() {
                    let key = (concept.clone(), fields[i].clone(), fields[j].clone());
                    *self.co_access.entry(key).or_insert(0) += 1;
                }
            }
        }
    }

    /// Phase 2: Resolve the physical layout for a single ConceptDecl.
    fn resolve_concept_layout(&self, scg: &mut SCG, concept_id: NodeId) -> Result<(), String> {
        // Get a copy of the ConceptDeclNode
        let concept = {
            let node = scg.get_node(concept_id)
                .ok_or("ConceptDecl node not found")?;
            if let NodePayload::ConceptDecl(c) = &node.payload {
                c.clone()
            } else {
                return Err("Node is not a ConceptDecl".to_string());
            }
        };

        // Decide layout strategy
        let strategy = self.decide_strategy(&concept);

        // Compute field offsets and sizes
        let layout = self.compute_offsets(&concept, strategy);

        // Update the ConceptDeclNode with resolved layout
        let node = scg.get_node_mut(concept_id)
            .ok_or("ConceptDecl node not found for mutation")?;
        if let NodePayload::ConceptDecl(c) = &mut node.payload {
            c.layout_resolved = true;
            c.resolved_offsets = layout.field_offsets.iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            c.resolved_size = layout.total_size;
            c.resolved_align = layout.align;
        }

        // Also update ConceptAccess nodes with resolved offsets
        let access_ids: Vec<NodeId> = scg.node_ids()
            .filter(|id| {
                if let Some(n) = scg.get_node(*id) {
                    if let NodePayload::ConceptAccess(a) = &n.payload {
                        return a.concept_name == concept.name;
                    }
                }
                false
            })
            .collect();

        for aid in access_ids {
            if let Some(node) = scg.get_node_mut(aid) {
                if let NodePayload::ConceptAccess(a) = &mut node.payload {
                    if let Some(&off) = layout.field_offsets.get(&a.field_name) {
                        a.resolved_offset = Some(off);
                    }
                }
            }
        }

        Ok(())
    }

    /// Decide AoS vs SoA based on the hint and access patterns.
    fn decide_strategy(&self, concept: &ConceptDeclNode) -> LayoutStrategy {
        match concept.layout_hint {
            ConceptLayoutHint::AoS => LayoutStrategy::AoS,
            ConceptLayoutHint::SoA => LayoutStrategy::SoA,
            ConceptLayoutHint::Auto => {
                // Check if any field has high independent access
                let threshold = 10; // fields accessed >10 times independently → SoA
                for field_name in &concept.field_names {
                    let count = self.access_counts
                        .get(&(concept.name.clone(), field_name.clone()))
                        .copied()
                        .unwrap_or(0);
                    if count > threshold {
                        // Check if this field is NOT co-accessed with others
                        let co_count: u64 = concept.field_names.iter()
                            .filter(|f| *f != field_name)
                            .map(|f| {
                                self.co_access.get(&(
                                    concept.name.clone(),
                                    field_name.clone(),
                                    f.clone(),
                                )).copied().unwrap_or(0)
                            })
                            .sum();
                        if co_count < count / 2 {
                            // More independent than co-accessed → SoA
                            return LayoutStrategy::SoA;
                        }
                    }
                }
                LayoutStrategy::AoS
            }
        }
    }

    /// Compute byte offsets for each field based on the chosen strategy.
    fn compute_offsets(&self, concept: &ConceptDeclNode, strategy: LayoutStrategy) -> ConceptLayout {
        let mut field_offsets = HashMap::new();
        let mut offset = 0u64;
        let mut max_align = 1u64;

        // Default field sizes (would come from BD inference in a full impl)
        let field_size = 8u64; // All fields are 8 bytes (u64/pointer) by default
        let field_align = 8u64;

        match strategy {
            LayoutStrategy::AoS => {
                // Pack fields contiguously
                for field_name in &concept.field_names {
                    // Align offset
                    offset = (offset + field_align - 1) & !(field_align - 1);
                    field_offsets.insert(field_name.clone(), offset);
                    offset += field_size;
                    max_align = max_align.max(field_align);
                }
                // Round up total size to alignment
                let total_size = (offset + max_align - 1) & !(max_align - 1);
                ConceptLayout {
                    strategy,
                    field_offsets,
                    total_size,
                    align: max_align,
                }
            }
            LayoutStrategy::SoA => {
                // Each field gets its own array. For a single instance,
                // the "offset" is the field index * element_size.
                // For N instances, each field would be a separate buffer
                // of N * element_size bytes.
                for (i, field_name) in concept.field_names.iter().enumerate() {
                    field_offsets.insert(field_name.clone(), (i as u64) * field_size);
                }
                let total_size = concept.field_names.len() as u64 * field_size;
                ConceptLayout {
                    strategy,
                    field_offsets,
                    total_size,
                    align: field_align,
                }
            }
        }
    }
}

impl Default for LayoutResolutionPass {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aos_layout() {
        let concept = ConceptDeclNode {
            name: "Point".to_string(),
            field_names: vec!["x".to_string(), "y".to_string()],
            region_id: vuma_scg::RegionId::new(0),
            layout_hint: ConceptLayoutHint::AoS,
            layout_resolved: false,
            resolved_offsets: vec![],
            resolved_size: 0,
            resolved_align: 0,
        };
        let pass = LayoutResolutionPass::new();
        let layout = pass.compute_offsets(&concept, LayoutStrategy::AoS);
        assert_eq!(layout.strategy, LayoutStrategy::AoS);
        assert_eq!(layout.field_offsets.get("x"), Some(&0));
        assert_eq!(layout.field_offsets.get("y"), Some(&8));
        assert_eq!(layout.total_size, 16);
    }

    #[test]
    fn test_soa_layout() {
        let concept = ConceptDeclNode {
            name: "Point".to_string(),
            field_names: vec!["x".to_string(), "y".to_string(), "z".to_string()],
            region_id: vuma_scg::RegionId::new(0),
            layout_hint: ConceptLayoutHint::SoA,
            layout_resolved: false,
            resolved_offsets: vec![],
            resolved_size: 0,
            resolved_align: 0,
        };
        let pass = LayoutResolutionPass::new();
        let layout = pass.compute_offsets(&concept, LayoutStrategy::SoA);
        assert_eq!(layout.strategy, LayoutStrategy::SoA);
        assert_eq!(layout.field_offsets.get("x"), Some(&0));
        assert_eq!(layout.field_offsets.get("y"), Some(&8));
        assert_eq!(layout.field_offsets.get("z"), Some(&16));
    }
}
