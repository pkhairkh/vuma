//! Memory State Graph (MSG).
//!
//! The [`MSG`] is the central data structure of VUMA. It is a directed graph
//! whose nodes are memory regions, pointer derivations, and access events,
//! and whose edges are derivation chains and synchronisation constraints.
//!
//! The MSG supports two fundamental queries:
//!
//! 1. **Provenance** — given an access, trace back to the originating region.
//! 2. **Data-race detection** — find pairs of conflicting accesses that are
//!    not ordered by any synchronisation edge.

use crate::access::{Access, AccessId};
use crate::address::Address;
use crate::derivation::{Derivation, DerivationId};
use crate::region::{Region, RegionId};
use crate::sync::{SyncEdge, SyncEdgeId};
use hashbrown::HashMap;
use std::fmt;

/// The Memory State Graph.
///
/// This structure owns all regions, derivations, accesses, and synchronisation
/// edges that have been observed during program analysis. It provides methods
/// for incremental construction and various query patterns.
#[derive(Debug, Clone, Default)]
pub struct MSG {
    /// Regions indexed by [`RegionId`].
    regions: HashMap<RegionId, Region>,
    /// Derivations indexed by [`DerivationId`].
    derivations: HashMap<DerivationId, Derivation>,
    /// Accesses indexed by [`AccessId`].
    accesses: HashMap<AccessId, Access>,
    /// Synchronisation edges indexed by [`SyncEdgeId`].
    sync_edges: HashMap<SyncEdgeId, SyncEdge>,

    // Counters for auto-incrementing IDs.
    next_region_id: u64,
    next_derivation_id: u64,
    next_access_id: u64,
    next_sync_edge_id: u64,
}

impl MSG {
    /// Create an empty Memory State Graph.
    pub fn new() -> Self {
        Self::default()
    }

    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Add a region to the graph and return its ID.
    ///
    /// The [`RegionId`] is assigned automatically.
    pub fn add_region(&mut self, region: Region) -> RegionId {
        let id = region.id;
        self.next_region_id = self.next_region_id.max(id.0 + 1);
        self.regions.insert(id, region);
        id
    }

    /// Add a derivation to the graph and return its ID.
    pub fn add_derivation(&mut self, derivation: Derivation) -> DerivationId {
        let id = derivation.id;
        self.next_derivation_id = self.next_derivation_id.max(id.0 + 1);
        self.derivations.insert(id, derivation);
        id
    }

    /// Add an access to the graph and return its ID.
    pub fn add_access(&mut self, access: Access) -> AccessId {
        let id = access.id;
        self.next_access_id = self.next_access_id.max(id.0 + 1);
        self.accesses.insert(id, access);
        id
    }

    /// Add a synchronisation edge to the graph and return its ID.
    pub fn add_sync_edge(&mut self, edge: SyncEdge) -> SyncEdgeId {
        let id = edge.id;
        self.next_sync_edge_id = self.next_sync_edge_id.max(id.0 + 1);
        self.sync_edges.insert(id, edge);
        id
    }

    // -----------------------------------------------------------------------
    // Simple lookups
    // -----------------------------------------------------------------------

    /// Look up a region by its ID.
    pub fn region(&self, id: RegionId) -> Option<&Region> {
        self.regions.get(&id)
    }

    /// Look up a derivation by its ID.
    pub fn derivation(&self, id: DerivationId) -> Option<&Derivation> {
        self.derivations.get(&id)
    }

    /// Look up an access by its ID.
    pub fn access(&self, id: AccessId) -> Option<&Access> {
        self.accesses.get(&id)
    }

    /// Look up a synchronisation edge by its ID.
    pub fn sync_edge(&self, id: SyncEdgeId) -> Option<&SyncEdge> {
        self.sync_edges.get(&id)
    }

    // -----------------------------------------------------------------------
    // Query methods
    // -----------------------------------------------------------------------

    /// Find the region that contains the given address.
    ///
    /// If multiple regions contain the address (overlap), the first one found
    /// is returned. In practice the front-end should ensure non-overlapping
    /// regions for unambiguous results.
    pub fn region_of(&self, addr: Address) -> Option<RegionId> {
        for (id, region) in &self.regions {
            if region.contains(addr) {
                return Some(*id);
            }
        }
        None
    }

    /// Trace the full derivation chain from `id` back to the originating
    /// region, returning the chain `[root, ..., parent, self]`.
    pub fn derivation_chain(&self, id: DerivationId) -> Vec<Derivation> {
        match self.derivation(id) {
            Some(d) => d.trace(|did| self.derivation(did).cloned()),
            None => Vec::new(),
        }
    }

    /// Find all accesses that are *concurrent* with the given access — i.e.
    /// not ordered by any synchronisation edge.
    ///
    /// This is a basic O(n × m) implementation where n is the number of
    /// accesses and m is the number of sync edges. A production
    /// implementation would use a reachability index.
    pub fn concurrent_accesses(&self, id: AccessId) -> Vec<AccessId> {
        let mut ordered: HashMap<AccessId, bool> = HashMap::new();

        for edge in self.sync_edges.values() {
            // If `id` is access1, then access2 is ordered after it.
            if edge.access1 == id {
                ordered.insert(edge.access2, true);
            }
            // If `id` is access2, then access1 is ordered before it.
            if edge.access2 == id {
                ordered.insert(edge.access1, true);
            }
        }

        self.accesses
            .keys()
            .filter(|&&aid| aid != id && !ordered.contains_key(&aid))
            .copied()
            .collect()
    }

    /// Find all accesses whose byte ranges overlap with the given access.
    ///
    /// `base` is the resolved base address of the target derivation for the
    /// query access. The caller must provide resolved base addresses for
    /// all other accesses via the `resolve_base` closure.
    pub fn overlapping_accesses<F>(
        &self,
        id: AccessId,
        base: Address,
        resolve_base: F,
    ) -> Vec<AccessId>
    where
        F: Fn(AccessId) -> Option<Address>,
    {
        let query = match self.access(id) {
            Some(a) => a,
            None => return Vec::new(),
        };

        let (q_start, q_end) = query.byte_range_at(base);

        self.accesses
            .iter()
            .filter(|(&aid, _)| aid != id)
            .filter_map(|(&aid, other)| {
                let other_base = resolve_base(aid)?;
                let (o_start, o_end) = other.byte_range_at(other_base);
                if q_start < o_end && o_start < q_end {
                    Some(aid)
                } else {
                    None
                }
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Counts
    // -----------------------------------------------------------------------

    /// Number of regions in the graph.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Number of derivations in the graph.
    pub fn derivation_count(&self) -> usize {
        self.derivations.len()
    }

    /// Number of accesses in the graph.
    pub fn access_count(&self) -> usize {
        self.accesses.len()
    }

    /// Number of synchronisation edges in the graph.
    pub fn sync_edge_count(&self) -> usize {
        self.sync_edges.len()
    }

    // -----------------------------------------------------------------------
    // Iterators
    // -----------------------------------------------------------------------

    /// Iterate over all regions in the graph.
    pub fn regions(&self) -> impl Iterator<Item = &Region> {
        self.regions.values()
    }

    /// Iterate over all derivations in the graph.
    pub fn derivations(&self) -> impl Iterator<Item = &Derivation> {
        self.derivations.values()
    }

    /// Iterate over all accesses in the graph.
    pub fn accesses(&self) -> impl Iterator<Item = &Access> {
        self.accesses.values()
    }

    /// Iterate over all synchronisation edges in the graph.
    pub fn sync_edges(&self) -> impl Iterator<Item = &SyncEdge> {
        self.sync_edges.values()
    }

    /// Iterate over all region IDs in the graph.
    pub fn region_ids(&self) -> impl Iterator<Item = RegionId> + '_ {
        self.regions.keys().copied()
    }

    /// Iterate over all derivation IDs in the graph.
    pub fn derivation_ids(&self) -> impl Iterator<Item = DerivationId> + '_ {
        self.derivations.keys().copied()
    }

    /// Iterate over all access IDs in the graph.
    pub fn access_ids(&self) -> impl Iterator<Item = AccessId> + '_ {
        self.accesses.keys().copied()
    }

    /// Iterate over all sync edge IDs in the graph.
    pub fn sync_edge_ids(&self) -> impl Iterator<Item = SyncEdgeId> + '_ {
        self.sync_edges.keys().copied()
    }

    // -----------------------------------------------------------------------
    // Removal
    // -----------------------------------------------------------------------

    /// Remove a region from the graph, returning it if it existed.
    pub fn remove_region(&mut self, id: RegionId) -> Option<Region> {
        self.regions.remove(&id)
    }

    /// Remove a derivation from the graph, returning it if it existed.
    pub fn remove_derivation(&mut self, id: DerivationId) -> Option<Derivation> {
        self.derivations.remove(&id)
    }

    /// Remove an access from the graph, returning it if it existed.
    pub fn remove_access(&mut self, id: AccessId) -> Option<Access> {
        self.accesses.remove(&id)
    }

    /// Remove a synchronisation edge from the graph, returning it if it existed.
    pub fn remove_sync_edge(&mut self, id: SyncEdgeId) -> Option<SyncEdge> {
        self.sync_edges.remove(&id)
    }
}

impl fmt::Display for MSG {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MSG {{ regions: {}, derivations: {}, accesses: {}, sync_edges: {} }}",
            self.region_count(),
            self.derivation_count(),
            self.access_count(),
            self.sync_edge_count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::AccessKind;
    use crate::derivation::{DerivationKind, DerivationSource};
    use crate::program_point::ProgramPoint;
    use crate::region::RegionStatus;
    use crate::sync::Ordering;

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    #[test]
    fn add_and_lookup_region() {
        let mut g = MSG::new();
        let r = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        };
        g.add_region(r);
        assert!(g.region(RegionId(1)).is_some());
        assert_eq!(g.region_count(), 1);
    }

    #[test]
    fn region_of_finds_containing() {
        let mut g = MSG::new();
        let r = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        };
        g.add_region(r);
        assert_eq!(g.region_of(Address::from(0x1050_u64)), Some(RegionId(1)));
        assert_eq!(g.region_of(Address::from(0xFFFF_u64)), None);
    }

    #[test]
    fn derivation_chain() {
        let mut g = MSG::new();
        g.add_derivation(Derivation {
            id: DerivationId(1),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x2000_u64)),
        });
        g.add_derivation(Derivation {
            id: DerivationId(2),
            source: DerivationSource::AnotherDerivation(DerivationId(1)),
            kind: DerivationKind::Offset { by: 0x40 },
            proven_range: (Address::from(0x1040_u64), Address::from(0x1080_u64)),
        });

        let chain = g.derivation_chain(DerivationId(2));
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].id, DerivationId(1));
        assert_eq!(chain[1].id, DerivationId(2));
    }

    #[test]
    fn concurrent_accesses_excludes_ordered() {
        let mut g = MSG::new();

        g.add_access(Access::new(
            AccessId(1),
            DerivationId(10),
            AccessKind::Write,
            4,
            dummy_pp(1),
        ));
        g.add_access(Access::new(
            AccessId(2),
            DerivationId(10),
            AccessKind::Read,
            4,
            dummy_pp(2),
        ));
        g.add_access(Access::new(
            AccessId(3),
            DerivationId(10),
            AccessKind::Read,
            4,
            dummy_pp(3),
        ));

        // A1 ─[hb]─▶ A2
        g.add_sync_edge(SyncEdge::new(
            SyncEdgeId(1),
            AccessId(1),
            AccessId(2),
            Ordering::HappensBefore,
        ));

        let concurrent = g.concurrent_accesses(AccessId(1));
        // A1 is ordered before A2, so only A3 is concurrent with A1.
        assert!(concurrent.contains(&AccessId(3)));
        assert!(!concurrent.contains(&AccessId(2)));
    }

    #[test]
    fn display_msg() {
        let g = MSG::new();
        assert_eq!(
            format!("{}", g),
            "MSG { regions: 0, derivations: 0, accesses: 0, sync_edges: 0 }"
        );
    }
}
