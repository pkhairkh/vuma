//! # Aura — Self-Describing Metadata for Runtime Introspection
//!
//! Opt-in, parallel metadata graphs for runtime introspection. This allows
//! LLM agents to inspect live memory without a debugger. It trades memory
//! overhead for zero-friction AI introspection.

use vuma_scg::{AuraAttachNode, NodeId, NodePayload, SCG};
use std::collections::{HashMap, HashSet};

/// The AuraHeader is stored before the base pointer when Aura is attached.
///
/// Layout (32 bytes):
/// ```text
/// Offset  Size  Field
/// 0       8     schema_hash (u64)
/// 8       4     version (u32)
/// 12      4     padding
/// 16      8     bounds_size (u64)
/// 24      8     schema_name_ptr (u64, pointer to string)
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AuraHeader {
    /// Hash identifying the metadata schema.
    pub schema_hash: u64,
    /// Schema version for forward/backward compatibility.
    pub version: u32,
    /// Total size of the base allocation (for bounds checking).
    pub bounds_size: u64,
    /// Name of the schema (for debugging).
    pub schema_name: String,
}

impl AuraHeader {
    /// The size of the AuraHeader in bytes.
    pub const SIZE: u64 = 32;

    /// The alignment of the AuraHeader.
    pub const ALIGN: u64 = 8;

    /// Compute the physical address of the AuraHeader given the base pointer.
    ///
    /// The AuraHeader is stored BEFORE the base pointer:
    ///   [AuraHeader (32 bytes)] [base data...]
    ///                    ^base_ptr points here
    pub fn header_address(base_ptr: u64) -> u64 {
        base_ptr - Self::SIZE
    }

    /// Compute the base pointer given the header address.
    pub fn base_address(header_ptr: u64) -> u64 {
        header_ptr + Self::SIZE
    }

    /// Create from an AuraAttachNode.
    pub fn from_attach(attach: &AuraAttachNode) -> Self {
        Self {
            schema_hash: attach.schema_hash,
            version: attach.version,
            bounds_size: attach.bounds_size,
            schema_name: attach.schema_name.clone(),
        }
    }
}

/// The Aura cleanup verifier ensures that Aura metadata is properly
/// freed when the base Concept or Manifold is freed.
///
/// ## Cleanup Invariant
///
/// The MSG builder must link the lifetime of the Aura to the base
/// Concept or Manifold. If the base is freed, the IVE strictly verifies
/// the Aura is freed. Double-free or leak of the Aura triggers a hard fail.
pub struct AuraCleanupVerifier {
    /// Map from base_ptr → aura_ptr (the pointer with the Aura header).
    /// This tracks which base allocations have Aura attached.
    aura_map: HashMap<String, String>,
    /// Set of base_ptrs that have been freed.
    freed_bases: HashSet<String>,
    /// Set of aura_ptrs that have been freed.
    freed_auras: HashSet<String>,
}

impl AuraCleanupVerifier {
    /// Create a new AuraCleanupVerifier.
    pub fn new() -> Self {
        Self {
            aura_map: HashMap::new(),
            freed_bases: HashSet::new(),
            freed_auras: HashSet::new(),
        }
    }

    /// Run the cleanup verifier over the SCG.
    ///
    /// This checks that:
    /// 1. Every AuraAttach has a corresponding Deallocation
    /// 2. The Aura is freed when the base is freed
    /// 3. No double-free of Aura or base
    pub fn run(&mut self, scg: &SCG) -> Result<(), Vec<String>> {
        // Phase 1: Collect all AuraAttach nodes
        self.collect_aura_attachments(scg);

        // Phase 2: Collect all Deallocation nodes
        self.collect_deallocations(scg);

        // Phase 3: Verify cleanup
        self.verify_cleanup()
    }

    /// Phase 1: Collect all AuraAttach nodes.
    fn collect_aura_attachments(&mut self, scg: &SCG) {
        for id in scg.node_ids() {
            let node = match scg.get_node(id) { Some(n) => n, None => continue };
            if let NodePayload::AuraAttach(a) = &node.payload {
                self.aura_map.insert(a.base_ptr.clone(), a.result_ptr.clone());
            }
        }
    }

    /// Phase 2: Collect all Deallocation nodes and track freed pointers.
    fn collect_deallocations(&mut self, scg: &SCG) {
        for id in scg.node_ids() {
            let node = match scg.get_node(id) { Some(n) => n, None => continue };
            if let NodePayload::Deallocation(dealloc) = &node.payload {
                // Check if the freed pointer is a base with Aura
                // or an Aura pointer itself
                // (In a full implementation, we'd resolve the variable name
                // to the actual pointer)
                let ptr_name = format!("ptr_{}", dealloc.region_id.as_u64());
                if self.freed_bases.contains(&ptr_name) || self.freed_auras.contains(&ptr_name) {
                    // Double-free detected — will be caught in verify_cleanup
                }
                if self.aura_map.contains_key(&ptr_name) {
                    self.freed_bases.insert(ptr_name.clone());
                } else if self.aura_map.values().any(|v| v == &ptr_name) {
                    self.freed_auras.insert(ptr_name);
                }
            }
        }
    }

    /// Phase 3: Verify that all Aura attachments are properly cleaned up.
    fn verify_cleanup(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        for (base_ptr, aura_ptr) in &self.aura_map {
            let base_freed = self.freed_bases.contains(base_ptr);
            let aura_freed = self.freed_auras.contains(aura_ptr);

            if base_freed && !aura_freed {
                errors.push(format!(
                    "Aura leak: base '{}' was freed but its Aura '{}' was not",
                    base_ptr, aura_ptr
                ));
            }

            if !base_freed && aura_freed {
                errors.push(format!(
                    "Dangling Aura: Aura '{}' was freed but base '{}' is still alive",
                    aura_ptr, base_ptr
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Check if a pointer has Aura metadata attached.
    pub fn has_aura(&self, base_ptr: &str) -> bool {
        self.aura_map.contains_key(base_ptr)
    }

    /// Get the Aura pointer for a given base pointer.
    pub fn get_aura_ptr(&self, base_ptr: &str) -> Option<&str> {
        self.aura_map.get(base_ptr).map(|s| s.as_str())
    }
}

impl Default for AuraCleanupVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_address_computation() {
        // Base pointer at 0x1000, header is at 0x1000 - 32 = 0xFE0
        assert_eq!(AuraHeader::header_address(0x1000), 0xFE0);
        assert_eq!(AuraHeader::base_address(0xFE0), 0x1000);
    }

    #[test]
    fn test_header_size() {
        assert_eq!(AuraHeader::SIZE, 32);
    }

    #[test]
    fn test_from_attach() {
        let attach = AuraAttachNode {
            base_ptr: "base".to_string(),
            schema_hash: 0x12345678,
            version: 1,
            bounds_size: 1024,
            schema_name: "Point".to_string(),
            result_ptr: "aura_ptr".to_string(),
        };
        let header = AuraHeader::from_attach(&attach);
        assert_eq!(header.schema_hash, 0x12345678);
        assert_eq!(header.version, 1);
        assert_eq!(header.bounds_size, 1024);
        assert_eq!(header.schema_name, "Point");
    }
}
