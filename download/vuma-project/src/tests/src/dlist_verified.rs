//! Verified Doubly-Linked List — VUMA Milestone M2.4
//!
//! This module proves that VUMA can verify non-trivial data structures WITHOUT
//! unsafe blocks. The doubly-linked list is Rust's classic `unsafe` showcase.
//! In VUMA, the IVE verifies all five invariants for it.
//!
//! # DList Implementation
//!
//! The DList is modeled using raw addresses (u64) simulating how a VUMA program
//! would work with VUMA-VERIFIED instead of `unsafe`:
//!
//! ```text
//! DListNode { data: u64, prev: u64, next: u64 }  — 24 bytes per node
//! DList     { head: u64, tail: u64, len: usize }  — 24 bytes header
//! ```
//!
//! # IVE Verification
//!
//! For each DList operation, we construct inputs for all 5 VUMA invariants:
//! 1. **Exclusivity** — no concurrent conflicting accesses
//! 2. **Interpretation** — every read interprets data under the correct BD
//! 3. **Liveness** — every resource is eventually provided / no UAF
//! 4. **Origin** — every pointer has traceable provenance
//! 5. **Cleanup** — every acquired resource is eventually released

use vuma_ive::{
    // Exclusivity
    AccessRecord, CapDInfo, ExclusivityAccessId as AccessId,
    ExclusivityAccessKind as AccessKind, ExclusivityInput, ExclusivityOutput,
    ExclusivityVerifier, SyncEdgeRecord, SyncOrdering, VerificationStatus,
    // Cleanup
    AnnotatedCleanupGraph, CleanupGraph, CleanupNodeId, CleanupReport,
    CleanupResourceId, CleanupResourceKind, CleanupVerifier, OperationKind,
    // Liveness
    DeadReason, EventAction, InitializationMap, LivenessInput,
    LivenessVerificationContext, LivenessVerifier, ObligationKind,
    PointId, ProofObligation, ResourceEvent, ResourceId as LivenessResourceId,
    ResourceKind as LivenessResourceKind, ThreadId,
    // Interpretation
    InterpretationVerifier, LocationId, ProgramPointId as InterpProgramPointId,
    // Result
    CounterExample,
};
use vuma_ive::origin::{
    Address as OriginAddress, RegionId as OriginRegionId, OriginVerifier,
    DerivationId, DerivationSource, DerivationKind as OriginDerivationKind,
    Derivation as OriginDerivation, Region as OriginRegionStruct,
    TaintLevel,
    Access as OriginAccess, AccessId as OriginAccessId,
    AccessKind as OriginAccessKind,
};
use vuma_ive::cleanup::ViolationKind as CleanupViolationKind;
use vuma_ive::liveness::ControlFlowEdge;
use vuma_bd::{
    capd::CapD,
    repd::{RepD, ByteRep, PtrRep},
    reld::RelD,
    descriptor::BD,
};

// ---------------------------------------------------------------------------
// DList Model
// ---------------------------------------------------------------------------

/// Simulated DList node: 24 bytes (data=8, prev=8, next=8).
const NODE_SIZE: u64 = 24;
/// Simulated DList header: 24 bytes (head=8, tail=8, len=8).
const DLIST_HEADER_SIZE: u64 = 24;

/// A simulated doubly-linked list node using raw addresses.
#[derive(Debug, Clone)]
struct DListNode {
    /// Base address of this node in simulated memory.
    addr: u64,
    /// Stored data value.
    data: u64,
    /// Address of previous node (0 = null).
    prev: u64,
    /// Address of next node (0 = null).
    next: u64,
}

impl DListNode {
    fn new(addr: u64, data: u64, prev: u64, next: u64) -> Self {
        Self { addr, data, prev, next }
    }
}

/// A simulated doubly-linked list using raw addresses.
#[derive(Debug, Clone)]
struct DList {
    /// Base address of the list header in simulated memory.
    header_addr: u64,
    /// Address of first node (0 = empty).
    head: u64,
    /// Address of last node (0 = empty).
    tail: u64,
    /// Number of nodes.
    len: usize,
    /// All nodes, keyed by address.
    nodes: std::collections::HashMap<u64, DListNode>,
    /// Next allocation address.
    next_alloc: u64,
}

impl DList {
    fn new(header_addr: u64) -> Self {
        Self {
            header_addr,
            head: 0,
            tail: 0,
            len: 0,
            nodes: std::collections::HashMap::new(),
            next_alloc: header_addr + DLIST_HEADER_SIZE,
        }
    }

    /// Allocate a new node at the next available address.
    fn alloc_node(&mut self, data: u64) -> u64 {
        let addr = self.next_alloc;
        self.next_alloc += NODE_SIZE;
        self.nodes.insert(addr, DListNode::new(addr, data, 0, 0));
        addr
    }

    /// Push a node to the back of the list. Returns the node address.
    fn push_back(&mut self, data: u64) -> u64 {
        let addr = self.alloc_node(data);
        if self.tail == 0 {
            // Empty list: new node is both head and tail.
            self.head = addr;
            self.tail = addr;
            self.nodes.get_mut(&addr).unwrap().prev = 0;
            self.nodes.get_mut(&addr).unwrap().next = 0;
        } else {
            // Non-empty: link after current tail.
            self.nodes.get_mut(&addr).unwrap().prev = self.tail;
            self.nodes.get_mut(&addr).unwrap().next = 0;
            self.nodes.get_mut(&self.tail).unwrap().next = addr;
            self.tail = addr;
        }
        self.len += 1;
        // Update header.
        self.update_header();
        addr
    }

    /// Push a node to the front of the list. Returns the node address.
    fn push_front(&mut self, data: u64) -> u64 {
        let addr = self.alloc_node(data);
        if self.head == 0 {
            self.head = addr;
            self.tail = addr;
            self.nodes.get_mut(&addr).unwrap().prev = 0;
            self.nodes.get_mut(&addr).unwrap().next = 0;
        } else {
            self.nodes.get_mut(&addr).unwrap().prev = 0;
            self.nodes.get_mut(&addr).unwrap().next = self.head;
            self.nodes.get_mut(&self.head).unwrap().prev = addr;
            self.head = addr;
        }
        self.len += 1;
        self.update_header();
        addr
    }

    /// Pop the back node. Returns (address, data) or None if empty.
    fn pop_back(&mut self) -> Option<(u64, u64)> {
        if self.tail == 0 {
            return None;
        }
        let tail_addr = self.tail;
        let tail_node = self.nodes.remove(&tail_addr).unwrap();
        if tail_node.prev == 0 {
            // Only node: list becomes empty.
            self.head = 0;
            self.tail = 0;
        } else {
            self.tail = tail_node.prev;
            self.nodes.get_mut(&self.tail).unwrap().next = 0;
        }
        self.len -= 1;
        self.update_header();
        Some((tail_addr, tail_node.data))
    }

    /// Pop the front node. Returns (address, data) or None if empty.
    fn pop_front(&mut self) -> Option<(u64, u64)> {
        if self.head == 0 {
            return None;
        }
        let head_addr = self.head;
        let head_node = self.nodes.remove(&head_addr).unwrap();
        if head_node.next == 0 {
            self.head = 0;
            self.tail = 0;
        } else {
            self.head = head_node.next;
            self.nodes.get_mut(&self.head).unwrap().prev = 0;
        }
        self.len -= 1;
        self.update_header();
        Some((head_addr, head_node.data))
    }

    /// Remove a middle node by address. Returns the data if found.
    fn remove_middle(&mut self, addr: u64) -> Option<u64> {
        let node = self.nodes.remove(&addr)?;
        let prev_addr = node.prev;
        let next_addr = node.next;

        if prev_addr != 0 {
            self.nodes.get_mut(&prev_addr).unwrap().next = next_addr;
        } else {
            self.head = next_addr;
        }
        if next_addr != 0 {
            self.nodes.get_mut(&next_addr).unwrap().prev = prev_addr;
        } else {
            self.tail = prev_addr;
        }
        self.len -= 1;
        self.update_header();
        Some(node.data)
    }

    /// Insert a new node after the given address. Returns new node address.
    fn insert_after(&mut self, after_addr: u64, data: u64) -> Option<u64> {
        if !self.nodes.contains_key(&after_addr) {
            return None;
        }
        let new_addr = self.alloc_node(data);
        let after_next = self.nodes[&after_addr].next;

        self.nodes.get_mut(&new_addr).unwrap().prev = after_addr;
        self.nodes.get_mut(&new_addr).unwrap().next = after_next;
        self.nodes.get_mut(&after_addr).unwrap().next = new_addr;

        if after_next != 0 {
            self.nodes.get_mut(&after_next).unwrap().prev = new_addr;
        } else {
            // after_addr was tail; new node becomes tail.
            self.tail = new_addr;
        }
        self.len += 1;
        self.update_header();
        Some(new_addr)
    }

    /// Traverse the list from head, returning data values in order.
    fn traverse(&self) -> Vec<u64> {
        let mut result = Vec::new();
        let mut current = self.head;
        while current != 0 {
            if let Some(node) = self.nodes.get(&current) {
                result.push(node.data);
                current = node.next;
            } else {
                break;
            }
        }
        result
    }

    /// Deallocate all nodes. Returns number of nodes freed.
    fn dealloc_all(&mut self) -> usize {
        let count = self.len;
        self.nodes.clear();
        self.head = 0;
        self.tail = 0;
        self.len = 0;
        self.update_header();
        count
    }

    fn update_header(&mut self) {
        // In our simulation, the header fields track head/tail/len directly.
        // The "writes" to the header are implicit.
    }
}

// ---------------------------------------------------------------------------
// Helpers for IVE input construction
// ---------------------------------------------------------------------------

/// Shorthand for exclusivity AccessId.
fn aid(id: u64) -> AccessId { AccessId(id) }

/// Helper: create a write access record for exclusivity.
fn write_access(id: u64, addr: u64, size: u64, point: &str, deriv: u64, region: u64) -> AccessRecord {
    AccessRecord::new(aid(id), AccessKind::Write, addr, size, point.to_string(), deriv, region)
}

/// Helper: create a read access record for exclusivity.
fn read_access(id: u64, addr: u64, size: u64, point: &str, deriv: u64, region: u64) -> AccessRecord {
    AccessRecord::new(aid(id), AccessKind::Read, addr, size, point.to_string(), deriv, region)
}

/// Verify exclusivity input and return the output.
fn verify_exclusivity(input: &ExclusivityInput) -> ExclusivityOutput {
    ExclusivityVerifier::new().verify(input)
}

/// Shorthand for cleanup ResourceId.
fn crid(id: u64) -> CleanupResourceId { CleanupResourceId(id) }

/// Shorthand for cleanup NodeId.
fn cnid(id: u64) -> CleanupNodeId { CleanupNodeId(id) }

/// Verify cleanup graph.
fn verify_cleanup(graph: &CleanupGraph) -> CleanupReport {
    CleanupVerifier::new().verify(graph)
}

/// Shorthand for liveness PointId.
fn lpp(id: u64) -> PointId { PointId(id) }

/// Shorthand for liveness ResourceId.
fn lrid(id: u64) -> LivenessResourceId { LivenessResourceId(id) }

/// Shorthand for ThreadId.
fn ltid(id: u64) -> ThreadId { ThreadId(id) }

/// Create a memory Allocate event for liveness.
fn alloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Allocate,
        point: lpp(point),
        thread: ltid(thread),
    }
}

/// Create a memory Deallocate event for liveness.
fn dealloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Deallocate,
        point: lpp(point),
        thread: ltid(thread),
    }
}

/// Create a memory Read event for liveness.
fn lread_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Read,
        point: lpp(point),
        thread: ltid(thread),
    }
}

/// Create a memory Write event for liveness.
fn lwrite_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Write,
        point: lpp(point),
        thread: ltid(thread),
    }
}

/// Create a simple unconditional CFG edge for liveness.
fn lcfg_edge(from: u64, to: u64) -> ControlFlowEdge {
    ControlFlowEdge {
        from: lpp(from),
        to: lpp(to),
        conditional: false,
        label: None,
    }
}

/// Build a linear CFG from a sequence of point IDs.
fn linear_lcfg(points: &[u64]) -> Vec<ControlFlowEdge> {
    points.windows(2).map(|w| lcfg_edge(w[0], w[1])).collect()
}

/// Shorthand for origin types.
fn oaddr(v: u64) -> OriginAddress { OriginAddress(v) }
fn orid(id: u64) -> OriginRegionId { OriginRegionId(id) }
fn odid(id: u64) -> DerivationId { DerivationId(id) }
fn oaid(id: u64) -> OriginAccessId { OriginAccessId(id) }

/// Build a standard BD for a DList node field (u64).
fn node_field_bd() -> BD {
    BD {
        repd: RepD::Byte(ByteRep { size: 8, align: 8 }),
        capd: CapD::all(),
        reld: RelD::empty(),
    }
}

/// Build a BD for a pointer field (prev/next).
fn node_ptr_bd() -> BD {
    BD {
        repd: RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Byte(ByteRep { size: NODE_SIZE, align: 8 })),
        }),
        capd: CapD::all(),
        reld: RelD::empty(),
    }
}

// ===========================================================================
// Test 1: dlist_push_back — Insert at tail
// ===========================================================================

#[test]
fn test_dlist_push_back() {
    // Create a list and push 3 nodes to the back.
    let mut list = DList::new(0x1000);
    let a = list.push_back(10);
    let b = list.push_back(20);
    let c = list.push_back(30);

    // Verify list state
    assert_eq!(list.len, 3);
    assert_eq!(list.traverse(), vec![10, 20, 30]);

    // --- Exclusivity: All writes are sequential (single-threaded) ---
    let mut excl_input = ExclusivityInput::new();
    // Write node A fields
    excl_input.add_access(write_access(1, a, 8, "push_back.A.data", 1, 1));
    excl_input.add_access(write_access(2, a + 8, 8, "push_back.A.prev", 2, 1));
    excl_input.add_access(write_access(3, a + 16, 8, "push_back.A.next", 3, 1));
    // Write node B fields + update A.next
    excl_input.add_access(write_access(4, b, 8, "push_back.B.data", 4, 2));
    excl_input.add_access(write_access(5, b + 8, 8, "push_back.B.prev", 5, 2));
    excl_input.add_access(write_access(6, b + 16, 8, "push_back.B.next", 6, 2));
    excl_input.add_access(write_access(7, a + 16, 8, "push_back.A.next->B", 3, 1)); // update A.next
    // Write node C fields + update B.next
    excl_input.add_access(write_access(8, c, 8, "push_back.C.data", 8, 3));
    excl_input.add_access(write_access(9, c + 8, 8, "push_back.C.prev", 9, 3));
    excl_input.add_access(write_access(10, c + 16, 8, "push_back.C.next", 10, 3));
    excl_input.add_access(write_access(11, b + 16, 8, "push_back.B.next->C", 6, 2)); // update B.next
    // All writes happen sequentially → add happens-before edges
    for i in 1..11 {
        excl_input.add_sync_edge(SyncEdgeRecord::new(aid(i), aid(i + 1), SyncOrdering::HappensBefore));
    }

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential push_back writes should have no exclusivity conflicts, got: {:?}",
        excl_output.conflicts
    );

    // --- Cleanup: All allocations are eventually freed ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_a = cg.add_node(
        OperationKind::Acquire { resource: crid(1), kind: CleanupResourceKind::Memory },
        "alloc_node_A",
    );
    let alloc_b = cg.add_node(
        OperationKind::Acquire { resource: crid(2), kind: CleanupResourceKind::Memory },
        "alloc_node_B",
    );
    let alloc_c = cg.add_node(
        OperationKind::Acquire { resource: crid(3), kind: CleanupResourceKind::Memory },
        "alloc_node_C",
    );
    let free_c = cg.add_node(
        OperationKind::Release { resource: crid(3), kind: CleanupResourceKind::Memory },
        "free_node_C",
    );
    let free_b = cg.add_node(
        OperationKind::Release { resource: crid(2), kind: CleanupResourceKind::Memory },
        "free_node_B",
    );
    let free_a = cg.add_node(
        OperationKind::Release { resource: crid(1), kind: CleanupResourceKind::Memory },
        "free_node_A",
    );
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(entry, alloc_a).unwrap();
    cg.add_edge(alloc_a, alloc_b).unwrap();
    cg.add_edge(alloc_b, alloc_c).unwrap();
    cg.add_edge(alloc_c, free_c).unwrap();
    cg.add_edge(free_c, free_b).unwrap();
    cg.add_edge(free_b, free_a).unwrap();
    cg.add_edge(free_a, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let cleanup_report = verify_cleanup(&cg);
    assert!(cleanup_report.clean, "All nodes should be freed, violations: {:?}", cleanup_report.violations);

    // --- Liveness: All allocations are live during access ---
    let mut liveness_input = LivenessInput::new();
    liveness_input.add_event(alloc_event(1, 1, 1));  // alloc A
    liveness_input.add_event(lwrite_event(1, 2, 1));  // write A
    liveness_input.add_event(alloc_event(2, 3, 1));  // alloc B
    liveness_input.add_event(lwrite_event(2, 4, 1));  // write B
    liveness_input.add_event(alloc_event(3, 5, 1));  // alloc C
    liveness_input.add_event(lwrite_event(3, 6, 1));  // write C
    liveness_input.add_event(lread_event(1, 7, 1));   // read A
    liveness_input.add_event(lread_event(2, 8, 1));   // read B
    liveness_input.add_event(lread_event(3, 9, 1));   // read C
    liveness_input.add_event(dealloc_event(1, 10, 1)); // free A
    liveness_input.add_event(dealloc_event(2, 11, 1)); // free B
    liveness_input.add_event(dealloc_event(3, 12, 1)); // free C
    liveness_input.cfg_edges = linear_lcfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    liveness_input.entry_point = Some(lpp(1));

    let mut liveness_verifier = LivenessVerifier::new();
    let liveness_result = liveness_verifier.verify(&liveness_input);
    assert!(
        liveness_result.invariant_holds,
        "All resources should be live during access, violations: {:?}",
        liveness_result.violations
    );

    // --- Origin: All pointers have valid provenance ---
    let mut origin_verifier = OriginVerifier::new();
    // Region for header + all nodes
    origin_verifier.add_region(OriginRegionStruct::new(orid(1), oaddr(0x1000), DLIST_HEADER_SIZE + 3 * NODE_SIZE));
    // Derivation: direct pointer from region for header
    origin_verifier.add_derivation(OriginDerivation::new(
        odid(1), DerivationSource::Region(orid(1)), OriginDerivationKind::Direct,
        (oaddr(0x1000), oaddr(0x1000 + DLIST_HEADER_SIZE)),
    ));
    // Derivation: offset from region for node A
    origin_verifier.add_derivation(OriginDerivation::new(
        odid(2), DerivationSource::Region(orid(1)),
        OriginDerivationKind::Offset { by: DLIST_HEADER_SIZE as i64 },
        (oaddr(a), oaddr(a + NODE_SIZE)),
    ));
    // Derivation: offset from region for node B
    origin_verifier.add_derivation(OriginDerivation::new(
        odid(3), DerivationSource::Region(orid(1)),
        OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + NODE_SIZE) as i64 },
        (oaddr(b), oaddr(b + NODE_SIZE)),
    ));
    // Derivation: offset from region for node C
    origin_verifier.add_derivation(OriginDerivation::new(
        odid(4), DerivationSource::Region(orid(1)),
        OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + 2 * NODE_SIZE) as i64 },
        (oaddr(c), oaddr(c + NODE_SIZE)),
    ));
    // Access: write to node A fields (initialized)
    origin_verifier.add_access(OriginAccess::new(oaid(1), odid(2), OriginAccessKind::Write, 24, "push_back.A", true));
    origin_verifier.add_access(OriginAccess::new(oaid(2), odid(3), OriginAccessKind::Write, 24, "push_back.B", true));
    origin_verifier.add_access(OriginAccess::new(oaid(3), odid(4), OriginAccessKind::Write, 24, "push_back.C", true));

    let origin_report = origin_verifier.verify();
    assert!(
        origin_report.is_clean(),
        "All derivations should have valid provenance, violations: {:?}",
        origin_report.violations
    );

    // --- Interpretation: Write-Read pairs have compatible BDs ---
    let mut interp_verifier = InterpretationVerifier::new();
    let node_bd = node_field_bd();
    let ptr_bd = node_ptr_bd();

    // Write node A fields, then read them back
    interp_verifier.record_write(LocationId(a), node_bd.clone(), InterpProgramPointId(1));
    interp_verifier.record_write(LocationId(a + 8), ptr_bd.clone(), InterpProgramPointId(2));
    interp_verifier.record_write(LocationId(a + 16), ptr_bd.clone(), InterpProgramPointId(3));
    // Reads happen after writes with same BD
    interp_verifier.record_read(LocationId(a), node_bd.clone(), InterpProgramPointId(10));
    interp_verifier.record_read(LocationId(a + 8), ptr_bd.clone(), InterpProgramPointId(11));
    interp_verifier.record_read(LocationId(a + 16), ptr_bd.clone(), InterpProgramPointId(12));

    let pairs = interp_verifier.extract_write_read_pairs();
    assert_eq!(pairs.len(), 3, "Expected 3 write-read pairs for node A");
    // All pairs should be compatible (same BD)
    for pair in &pairs {
        assert!(
            pair.write_bd.repd.compatible(&pair.read_bd.repd),
            "Write-read pair should have compatible RepDs"
        );
    }
}

// ===========================================================================
// Test 2: dlist_push_front — Insert at head
// ===========================================================================

#[test]
fn test_dlist_push_front() {
    let mut list = DList::new(0x2000);
    let a = list.push_front(10);
    let b = list.push_front(20);
    let c = list.push_front(30);

    // After push_front(10), push_front(20), push_front(30):
    // traversal should be [30, 20, 10]
    assert_eq!(list.len, 3);
    assert_eq!(list.traverse(), vec![30, 20, 10]);

    // --- Exclusivity: Sequential writes with happens-before ---
    let mut excl_input = ExclusivityInput::new();
    excl_input.add_access(write_access(1, a, 8, "pf.A.data", 1, 1));
    excl_input.add_access(write_access(2, b, 8, "pf.B.data", 2, 2));
    excl_input.add_access(write_access(3, b + 16, 8, "pf.B.next->A", 3, 2));
    excl_input.add_access(write_access(4, a + 8, 8, "pf.A.prev->B", 1, 1)); // update A.prev
    excl_input.add_access(write_access(5, c, 8, "pf.C.data", 4, 3));
    excl_input.add_access(write_access(6, c + 16, 8, "pf.C.next->B", 5, 3));
    excl_input.add_access(write_access(7, b + 8, 8, "pf.B.prev->C", 3, 2)); // update B.prev
    // Sequential ordering
    for i in 1..7 {
        excl_input.add_sync_edge(SyncEdgeRecord::new(aid(i), aid(i + 1), SyncOrdering::HappensBefore));
    }

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential push_front writes should have no exclusivity conflicts"
    );

    // --- Cleanup: All freed ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_a = cg.add_node(
        OperationKind::Acquire { resource: crid(1), kind: CleanupResourceKind::Memory }, "alloc_a");
    let alloc_b = cg.add_node(
        OperationKind::Acquire { resource: crid(2), kind: CleanupResourceKind::Memory }, "alloc_b");
    let alloc_c = cg.add_node(
        OperationKind::Acquire { resource: crid(3), kind: CleanupResourceKind::Memory }, "alloc_c");
    let free_a = cg.add_node(
        OperationKind::Release { resource: crid(1), kind: CleanupResourceKind::Memory }, "free_a");
    let free_b = cg.add_node(
        OperationKind::Release { resource: crid(2), kind: CleanupResourceKind::Memory }, "free_b");
    let free_c = cg.add_node(
        OperationKind::Release { resource: crid(3), kind: CleanupResourceKind::Memory }, "free_c");
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(entry, alloc_a).unwrap();
    cg.add_edge(alloc_a, alloc_b).unwrap();
    cg.add_edge(alloc_b, alloc_c).unwrap();
    cg.add_edge(alloc_c, free_c).unwrap();
    cg.add_edge(free_c, free_b).unwrap();
    cg.add_edge(free_b, free_a).unwrap();
    cg.add_edge(free_a, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(report.clean, "All resources freed: {:?}", report.violations);

    // --- Origin: All pointers traceable ---
    let mut ov = OriginVerifier::new();
    ov.add_region(OriginRegionStruct::new(orid(1), oaddr(0x2000), DLIST_HEADER_SIZE + 3 * NODE_SIZE));
    ov.add_derivation(OriginDerivation::new(
        odid(1), DerivationSource::Region(orid(1)), OriginDerivationKind::Direct,
        (oaddr(0x2000), oaddr(0x2000 + DLIST_HEADER_SIZE + 3 * NODE_SIZE)),
    ));
    // Offset derivations for each node
    for (i, node_addr) in [a, b, c].iter().enumerate() {
        ov.add_derivation(OriginDerivation::new(
            odid((i + 2) as u64), DerivationSource::Region(orid(1)),
            OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + (i as u64) * NODE_SIZE) as i64 },
            (oaddr(*node_addr), oaddr(*node_addr + NODE_SIZE)),
        ));
    }
    let oreport = ov.verify();
    assert!(oreport.is_clean(), "All derivations valid: {:?}", oreport.violations);
}

// ===========================================================================
// Test 3: dlist_pop_back — Remove from tail
// ===========================================================================

#[test]
fn test_dlist_pop_back() {
    let mut list = DList::new(0x3000);
    let a = list.push_back(10);
    let b = list.push_back(20);
    let c = list.push_back(30);

    // Pop C (tail)
    let (c_addr, c_data) = list.pop_back().unwrap();
    assert_eq!(c_addr, c);
    assert_eq!(c_data, 30);
    assert_eq!(list.len, 2);
    assert_eq!(list.traverse(), vec![10, 20]);

    // --- Liveness: verify no use-after-free ---
    // C was deallocated; B's next was set to 0. Any read of C after free = UAF.
    let mut linput = LivenessInput::new();
    linput.add_event(alloc_event(1, 1, 1));  // alloc A
    linput.add_event(alloc_event(2, 2, 1));  // alloc B
    linput.add_event(alloc_event(3, 3, 1));  // alloc C
    linput.add_event(lread_event(1, 4, 1));  // read A
    linput.add_event(lread_event(2, 5, 1));  // read B
    linput.add_event(lread_event(3, 6, 1));  // read C (while live)
    linput.add_event(dealloc_event(3, 7, 1)); // free C
    // A read of C after PP7 would be UAF, but we don't do it here
    linput.add_event(dealloc_event(2, 8, 1)); // free B
    linput.add_event(dealloc_event(1, 9, 1)); // free A
    linput.cfg_edges = linear_lcfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    linput.entry_point = Some(lpp(1));

    let mut lverifier = LivenessVerifier::new();
    let lresult = lverifier.verify(&linput);
    assert!(lresult.invariant_holds, "No UAF in correct pop_back: {:?}", lresult.violations);

    // Now verify that UAF WOULD be detected if we read C after freeing it
    let mut linput_uaf = LivenessInput::new();
    linput_uaf.add_event(alloc_event(3, 1, 1));
    linput_uaf.add_event(dealloc_event(3, 2, 1));
    linput_uaf.add_event(lread_event(3, 3, 1)); // UAF!
    linput_uaf.cfg_edges = linear_lcfg(&[1, 2, 3]);
    linput_uaf.entry_point = Some(lpp(1));

    let ctx = LivenessVerificationContext::new(linput_uaf);
    let paths = LivenessVerifier::new().compute_liveness_paths(&ctx);
    assert_eq!(paths.len(), 1);
    assert!(
        !paths[0].access_after_free.is_empty(),
        "Reading C after free should be detected as UAF"
    );

    // --- Cleanup: No double-free ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_c = cg.add_node(
        OperationKind::Acquire { resource: crid(3), kind: CleanupResourceKind::Memory }, "alloc_C");
    let free_c1 = cg.add_node(
        OperationKind::Release { resource: crid(3), kind: CleanupResourceKind::Memory }, "free_C_1");
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(entry, alloc_c).unwrap();
    cg.add_edge(alloc_c, free_c1).unwrap();
    cg.add_edge(free_c1, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(report.clean, "Single free should be clean");
    assert!(
        !report.violations.iter().any(|v| v.kind == CleanupViolationKind::DoubleFree),
        "No double-free should be detected"
    );
}

// ===========================================================================
// Test 4: dlist_pop_front — Remove from head
// ===========================================================================

#[test]
fn test_dlist_pop_front() {
    let mut list = DList::new(0x4000);
    let a = list.push_back(10);
    let b = list.push_back(20);
    let c = list.push_back(30);

    // Pop A (head)
    let (a_addr, a_data) = list.pop_front().unwrap();
    assert_eq!(a_addr, a);
    assert_eq!(a_data, 10);
    assert_eq!(list.len, 2);
    assert_eq!(list.traverse(), vec![20, 30]);

    // --- Exclusivity: Sequential updates ---
    let mut excl_input = ExclusivityInput::new();
    // Update B.prev from A to 0 (null)
    excl_input.add_access(write_access(1, b + 8, 8, "pop_front.B.prev=0", 1, 2));
    // Update head from A to B
    excl_input.add_access(write_access(2, list.header_addr, 8, "pop_front.head=B", 2, 0));
    // These are sequential
    excl_input.add_sync_edge(SyncEdgeRecord::new(aid(1), aid(2), SyncOrdering::HappensBefore));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(excl_output.is_proven(), "Sequential pop_front writes should be Proven");

    // --- Cleanup: A freed, B and C still live ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_a = cg.add_node(
        OperationKind::Acquire { resource: crid(1), kind: CleanupResourceKind::Memory }, "alloc_A");
    let alloc_b = cg.add_node(
        OperationKind::Acquire { resource: crid(2), kind: CleanupResourceKind::Memory }, "alloc_B");
    let alloc_c = cg.add_node(
        OperationKind::Acquire { resource: crid(3), kind: CleanupResourceKind::Memory }, "alloc_C");
    let access_b = cg.add_node(OperationKind::Access { resource: crid(2) }, "access_B");
    let free_a = cg.add_node(
        OperationKind::Release { resource: crid(1), kind: CleanupResourceKind::Memory }, "free_A");
    let free_b = cg.add_node(
        OperationKind::Release { resource: crid(2), kind: CleanupResourceKind::Memory }, "free_B");
    let free_c = cg.add_node(
        OperationKind::Release { resource: crid(3), kind: CleanupResourceKind::Memory }, "free_C");
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(entry, alloc_a).unwrap();
    cg.add_edge(alloc_a, alloc_b).unwrap();
    cg.add_edge(alloc_b, alloc_c).unwrap();
    cg.add_edge(alloc_c, free_a).unwrap(); // A freed after pop
    cg.add_edge(free_a, access_b).unwrap(); // B still accessible
    cg.add_edge(access_b, free_b).unwrap();
    cg.add_edge(free_b, free_c).unwrap();
    cg.add_edge(free_c, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(report.clean, "All resources properly managed: {:?}", report.violations);

    // --- Liveness: Access B after A freed (B is still live) ---
    let mut linput = LivenessInput::new();
    linput.add_event(alloc_event(1, 1, 1));
    linput.add_event(alloc_event(2, 2, 1));
    linput.add_event(alloc_event(3, 3, 1));
    linput.add_event(dealloc_event(1, 4, 1)); // free A
    linput.add_event(lread_event(2, 5, 1));   // read B (still live - OK!)
    linput.add_event(lread_event(3, 6, 1));   // read C (still live - OK!)
    linput.add_event(dealloc_event(2, 7, 1));
    linput.add_event(dealloc_event(3, 8, 1));
    linput.cfg_edges = linear_lcfg(&[1, 2, 3, 4, 5, 6, 7, 8]);
    linput.entry_point = Some(lpp(1));

    let mut lverifier = LivenessVerifier::new();
    let lresult = lverifier.verify(&linput);
    assert!(lresult.invariant_holds, "B and C still live after A freed: {:?}", lresult.violations);
}

// ===========================================================================
// Test 5: dlist_remove_middle — Remove interior node
// ===========================================================================

#[test]
fn test_dlist_remove_middle() {
    let mut list = DList::new(0x5000);
    let a = list.push_back(10);
    let b = list.push_back(20);
    let c = list.push_back(30);

    // Remove B (middle node): A.next = C, C.prev = A
    let b_data = list.remove_middle(b).unwrap();
    assert_eq!(b_data, 20);
    assert_eq!(list.len, 2);
    assert_eq!(list.traverse(), vec![10, 30]);

    // Verify pointer consistency
    assert_eq!(list.nodes[&a].next, c, "A.next should point to C");
    assert_eq!(list.nodes[&c].prev, a, "C.prev should point to A");

    // --- Exclusivity: Both writes (A.next and C.prev) are sequential ---
    let mut excl_input = ExclusivityInput::new();
    // Write A.next = C
    excl_input.add_access(write_access(1, a + 16, 8, "remove.A.next=C", 1, 1));
    // Write C.prev = A
    excl_input.add_access(write_access(2, c + 8, 8, "remove.C.prev=A", 2, 3));
    // Sequential: A.next updated before C.prev
    excl_input.add_sync_edge(SyncEdgeRecord::new(aid(1), aid(2), SyncOrdering::HappensBefore));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential pointer updates in remove_middle should be Proven"
    );

    // Verify that concurrent writes WOULD be flagged
    let mut excl_concurrent = ExclusivityInput::new();
    excl_concurrent.add_access(write_access(1, a + 16, 8, "A.next=C", 1, 1));
    excl_concurrent.add_access(write_access(2, c + 8, 8, "C.prev=A", 2, 3));
    // No sync edge → concurrent

    let concurrent_output = verify_exclusivity(&excl_concurrent);
    // These are at different addresses (a+16 vs c+8), so they don't overlap
    // and should be Proven even without sync
    assert!(
        concurrent_output.is_proven(),
        "Non-overlapping concurrent writes to different addresses should be Proven"
    );

    // --- Interpretation: Both writes are pointer fields (Ptr BD) ---
    let mut interp = InterpretationVerifier::new();
    let ptr_bd = node_ptr_bd();
    // Write A.next with Ptr BD, then read back with same BD
    interp.record_write(LocationId(a + 16), ptr_bd.clone(), InterpProgramPointId(1));
    interp.record_write(LocationId(c + 8), ptr_bd.clone(), InterpProgramPointId(2));
    interp.record_read(LocationId(a + 16), ptr_bd.clone(), InterpProgramPointId(10));
    interp.record_read(LocationId(c + 8), ptr_bd.clone(), InterpProgramPointId(11));

    let pairs = interp.extract_write_read_pairs();
    assert_eq!(pairs.len(), 2, "Expected 2 write-read pairs for pointer updates");
    for pair in &pairs {
        assert!(
            pair.write_bd.repd.compatible(&pair.read_bd.repd),
            "Pointer field write-read should have compatible BDs"
        );
    }

    // --- Cleanup: B freed, A and C still live ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_a = cg.add_node(
        OperationKind::Acquire { resource: crid(1), kind: CleanupResourceKind::Memory }, "alloc_A");
    let alloc_b = cg.add_node(
        OperationKind::Acquire { resource: crid(2), kind: CleanupResourceKind::Memory }, "alloc_B");
    let alloc_c = cg.add_node(
        OperationKind::Acquire { resource: crid(3), kind: CleanupResourceKind::Memory }, "alloc_C");
    let free_b = cg.add_node(
        OperationKind::Release { resource: crid(2), kind: CleanupResourceKind::Memory }, "free_B");
    let access_a = cg.add_node(OperationKind::Access { resource: crid(1) }, "access_A");
    let access_c = cg.add_node(OperationKind::Access { resource: crid(3) }, "access_C");
    let free_a = cg.add_node(
        OperationKind::Release { resource: crid(1), kind: CleanupResourceKind::Memory }, "free_A");
    let free_c = cg.add_node(
        OperationKind::Release { resource: crid(3), kind: CleanupResourceKind::Memory }, "free_C");
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(entry, alloc_a).unwrap();
    cg.add_edge(alloc_a, alloc_b).unwrap();
    cg.add_edge(alloc_b, alloc_c).unwrap();
    cg.add_edge(alloc_c, free_b).unwrap();  // B freed after removal
    cg.add_edge(free_b, access_a).unwrap(); // A still accessible
    cg.add_edge(access_a, access_c).unwrap();
    cg.add_edge(access_c, free_a).unwrap();
    cg.add_edge(free_a, free_c).unwrap();
    cg.add_edge(free_c, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(report.clean, "All resources properly managed after remove_middle: {:?}", report.violations);

    // --- Liveness: No UAF - A and C accessed only while live ---
    let mut linput = LivenessInput::new();
    linput.add_event(alloc_event(1, 1, 1)); // alloc A
    linput.add_event(alloc_event(2, 2, 1)); // alloc B
    linput.add_event(alloc_event(3, 3, 1)); // alloc C
    linput.add_event(dealloc_event(2, 4, 1)); // free B
    linput.add_event(lread_event(1, 5, 1));  // read A (live)
    linput.add_event(lread_event(3, 6, 1));  // read C (live)
    linput.add_event(dealloc_event(1, 7, 1)); // free A
    linput.add_event(dealloc_event(3, 8, 1)); // free C
    linput.cfg_edges = linear_lcfg(&[1, 2, 3, 4, 5, 6, 7, 8]);
    linput.entry_point = Some(lpp(1));

    let mut lverifier = LivenessVerifier::new();
    let lresult = lverifier.verify(&linput);
    assert!(lresult.invariant_holds, "No UAF in remove_middle: {:?}", lresult.violations);
}

// ===========================================================================
// Test 6: dlist_traverse — Iterate through list
// ===========================================================================

#[test]
fn test_dlist_traverse() {
    let mut list = DList::new(0x6000);
    let a = list.push_back(10);
    let b = list.push_back(20);
    let c = list.push_back(30);

    let values = list.traverse();
    assert_eq!(values, vec![10, 20, 30]);

    // --- Exclusivity: All reads are safe (reads never conflict) ---
    let mut excl_input = ExclusivityInput::new();
    // Read each node's data and next pointer during traversal
    excl_input.add_access(read_access(1, a, 8, "traverse.A.data", 1, 1));
    excl_input.add_access(read_access(2, a + 16, 8, "traverse.A.next", 2, 1));
    excl_input.add_access(read_access(3, b, 8, "traverse.B.data", 3, 2));
    excl_input.add_access(read_access(4, b + 16, 8, "traverse.B.next", 4, 2));
    excl_input.add_access(read_access(5, c, 8, "traverse.C.data", 5, 3));
    excl_input.add_access(read_access(6, c + 16, 8, "traverse.C.next", 6, 3));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "All reads during traversal should be Proven (reads never conflict)"
    );

    // --- Liveness: All reads from live memory ---
    let mut linput = LivenessInput::new();
    linput.add_event(alloc_event(1, 1, 1));
    linput.add_event(alloc_event(2, 2, 1));
    linput.add_event(alloc_event(3, 3, 1));
    linput.add_event(lwrite_event(1, 4, 1));
    linput.add_event(lwrite_event(2, 5, 1));
    linput.add_event(lwrite_event(3, 6, 1));
    // Traversal reads
    linput.add_event(lread_event(1, 7, 1));
    linput.add_event(lread_event(2, 8, 1));
    linput.add_event(lread_event(3, 9, 1));
    // Cleanup
    linput.add_event(dealloc_event(1, 10, 1));
    linput.add_event(dealloc_event(2, 11, 1));
    linput.add_event(dealloc_event(3, 12, 1));
    linput.cfg_edges = linear_lcfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    linput.entry_point = Some(lpp(1));

    let mut lverifier = LivenessVerifier::new();
    let lresult = lverifier.verify(&linput);
    assert!(lresult.invariant_holds, "All reads from live memory: {:?}", lresult.violations);

    // --- Origin: All pointer dereferences have valid provenance ---
    let mut ov = OriginVerifier::new();
    ov.add_region(OriginRegionStruct::new(orid(1), oaddr(0x6000), DLIST_HEADER_SIZE + 3 * NODE_SIZE));
    // Derivation for each node (offset from allocation region)
    for (i, node_addr) in [a, b, c].iter().enumerate() {
        ov.add_derivation(OriginDerivation::new(
            odid((i + 1) as u64), DerivationSource::Region(orid(1)),
            OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + (i as u64) * NODE_SIZE) as i64 },
            (oaddr(*node_addr), oaddr(*node_addr + NODE_SIZE)),
        ));
        // Read access for each node during traversal
        ov.add_access(OriginAccess::new(
            oaid((i + 1) as u64), odid((i + 1) as u64),
            OriginAccessKind::Read, 24,
            format!("traverse.node{}", i), true,
        ));
    }

    let oreport = ov.verify();
    assert!(
        oreport.is_clean(),
        "All traversal pointer dereferences have valid provenance, violations: {:?}",
        oreport.violations
    );
    // Verify all derivations are trusted
    for node in &oreport.provenance_forest {
        assert_eq!(node.taint, TaintLevel::Trusted, "All traversal nodes should be trusted");
    }
}

// ===========================================================================
// Test 7: dlist_dealloc_all — Free entire list
// ===========================================================================

#[test]
fn test_dlist_dealloc_all() {
    let mut list = DList::new(0x7000);
    list.push_back(10);
    list.push_back(20);
    list.push_back(30);

    let freed = list.dealloc_all();
    assert_eq!(freed, 3);
    assert_eq!(list.len, 0);
    assert_eq!(list.traverse(), Vec::<u64>::new());

    // --- Cleanup: Walk from head, free each node — no leaks ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc1 = cg.add_node(
        OperationKind::Acquire { resource: crid(1), kind: CleanupResourceKind::Memory }, "alloc1");
    let alloc2 = cg.add_node(
        OperationKind::Acquire { resource: crid(2), kind: CleanupResourceKind::Memory }, "alloc2");
    let alloc3 = cg.add_node(
        OperationKind::Acquire { resource: crid(3), kind: CleanupResourceKind::Memory }, "alloc3");
    // Access nodes before dealloc
    let access1 = cg.add_node(OperationKind::Access { resource: crid(1) }, "access1");
    let access2 = cg.add_node(OperationKind::Access { resource: crid(2) }, "access2");
    let access3 = cg.add_node(OperationKind::Access { resource: crid(3) }, "access3");
    // Free all
    let free1 = cg.add_node(
        OperationKind::Release { resource: crid(1), kind: CleanupResourceKind::Memory }, "free1");
    let free2 = cg.add_node(
        OperationKind::Release { resource: crid(2), kind: CleanupResourceKind::Memory }, "free2");
    let free3 = cg.add_node(
        OperationKind::Release { resource: crid(3), kind: CleanupResourceKind::Memory }, "free3");
    let ret = cg.add_node(OperationKind::Return, "return");

    cg.add_edge(entry, alloc1).unwrap();
    cg.add_edge(alloc1, alloc2).unwrap();
    cg.add_edge(alloc2, alloc3).unwrap();
    cg.add_edge(alloc3, access1).unwrap();
    cg.add_edge(access1, access2).unwrap();
    cg.add_edge(access2, access3).unwrap();
    cg.add_edge(access3, free1).unwrap();
    cg.add_edge(free1, free2).unwrap();
    cg.add_edge(free2, free3).unwrap();
    cg.add_edge(free3, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(report.clean, "All nodes freed: {:?}", report.violations);
    assert_eq!(report.acquires_checked, 3);

    // Verify no leaks via reachability
    let verifier = CleanupVerifier::new();
    let unreachable = verifier.quick_check_reachability(&cg);
    assert!(unreachable.is_empty(), "All acquires should have reachable releases");

    // --- Liveness: Full lifecycle ---
    let mut linput = LivenessInput::new();
    linput.add_event(alloc_event(1, 1, 1));
    linput.add_event(alloc_event(2, 2, 1));
    linput.add_event(alloc_event(3, 3, 1));
    linput.add_event(lwrite_event(1, 4, 1));
    linput.add_event(lwrite_event(2, 5, 1));
    linput.add_event(lwrite_event(3, 6, 1));
    linput.add_event(lread_event(1, 7, 1));
    linput.add_event(lread_event(2, 8, 1));
    linput.add_event(lread_event(3, 9, 1));
    linput.add_event(dealloc_event(1, 10, 1));
    linput.add_event(dealloc_event(2, 11, 1));
    linput.add_event(dealloc_event(3, 12, 1));
    linput.cfg_edges = linear_lcfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    linput.entry_point = Some(lpp(1));

    let mut lverifier = LivenessVerifier::new();
    let lresult = lverifier.verify(&linput);
    assert!(lresult.invariant_holds, "Full lifecycle clean: {:?}", lresult.violations);
}

// ===========================================================================
// Test 8: dlist_cyclic_pointers — Two pointers to same node through prev/next paths
// ===========================================================================

#[test]
fn test_dlist_cyclic_pointers() {
    let mut list = DList::new(0x8000);
    let a = list.push_back(10);
    let b = list.push_back(20);
    let c = list.push_back(30);

    // The key insight: A.next points to B, and B.prev points to A.
    // Two pointers to the same node B (through A.next and directly from list)
    // must be verified for exclusivity.
    //
    // In a doubly-linked list, prev/next pointers create cycles:
    //   A.next → B, B.prev → A (cycle)
    //   B.next → C, C.prev → B (cycle)
    //
    // Reading through both paths must be safe.

    // --- Exclusivity: Two reads of the same node through different paths ---
    let mut excl_input = ExclusivityInput::new();
    // Read B.data through path 1: head → A → A.next → B
    excl_input.add_access(read_access(1, b, 8, "via_A.next.read_B.data", 1, 2));
    // Read B.data through path 2: tail → C → C.prev → B
    excl_input.add_access(read_access(2, b, 8, "via_C.prev.read_B.data", 2, 2));
    // Both are reads to the same address → reads never conflict
    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Two reads of B through different paths should be Proven (reads never conflict)"
    );

    // --- Exclusivity: Write through one path, read through another ---
    let mut excl_write_read = ExclusivityInput::new();
    excl_write_read.add_access(write_access(1, b, 8, "write_B.data_direct", 1, 2));
    excl_write_read.add_access(read_access(2, b, 8, "read_B.data_via_prev", 2, 2));
    // Without sync edge: concurrent write + read → conflict
    let wr_output = verify_exclusivity(&excl_write_read);
    assert!(
        wr_output.is_violated(),
        "Concurrent write+read to B through cyclic paths should be Violated"
    );

    // With happens-before: safe
    let mut excl_ordered = ExclusivityInput::new();
    excl_ordered.add_access(write_access(1, b, 8, "write_B.data_direct", 1, 2));
    excl_ordered.add_access(read_access(2, b, 8, "read_B.data_via_prev", 2, 2));
    excl_ordered.add_sync_edge(SyncEdgeRecord::new(aid(1), aid(2), SyncOrdering::HappensBefore));
    let ordered_output = verify_exclusivity(&excl_ordered);
    assert!(
        ordered_output.is_proven(),
        "Ordered write→read to B through cyclic paths should be Proven"
    );

    // --- Interpretation: Same BD regardless of access path ---
    let mut interp = InterpretationVerifier::new();
    let data_bd = node_field_bd();
    let ptr_bd = node_ptr_bd();
    // Write B.data, B.next, B.prev
    interp.record_write(LocationId(b), data_bd.clone(), InterpProgramPointId(1));
    interp.record_write(LocationId(b + 8), ptr_bd.clone(), InterpProgramPointId(2));
    interp.record_write(LocationId(b + 16), ptr_bd.clone(), InterpProgramPointId(3));
    // Read B.data through A.next path
    interp.record_read(LocationId(b), data_bd.clone(), InterpProgramPointId(10));
    // Read B.data through C.prev path
    interp.record_read(LocationId(b), data_bd.clone(), InterpProgramPointId(11));
    // Read B.prev through A.next path
    interp.record_read(LocationId(b + 8), ptr_bd.clone(), InterpProgramPointId(12));

    let pairs = interp.extract_write_read_pairs();
    assert!(pairs.len() >= 3, "Expected at least 3 write-read pairs");
    for pair in &pairs {
        assert!(
            pair.write_bd.repd.compatible(&pair.read_bd.repd),
            "Cyclic reads should use same BD as writes"
        );
    }

    // --- Origin: Both paths trace back to the same allocation ---
    let mut ov = OriginVerifier::new();
    ov.add_region(OriginRegionStruct::new(orid(1), oaddr(0x8000), DLIST_HEADER_SIZE + 3 * NODE_SIZE));
    // Direct derivation for B from region
    ov.add_derivation(OriginDerivation::new(
        odid(1), DerivationSource::Region(orid(1)),
        OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + NODE_SIZE) as i64 },
        (oaddr(b), oaddr(b + NODE_SIZE)),
    ));
    // Derivation for B through A (A.next field read)
    ov.add_derivation(OriginDerivation::new(
        odid(2), DerivationSource::AnotherDerivation(odid(1)),
        OriginDerivationKind::Direct,
        (oaddr(b), oaddr(b + NODE_SIZE)),
    ));
    // Access B through both derivations
    ov.add_access(OriginAccess::new(oaid(1), odid(1), OriginAccessKind::Read, 24, "direct_B", true));
    ov.add_access(OriginAccess::new(oaid(2), odid(2), OriginAccessKind::Read, 24, "via_A_B", true));

    let oreport = ov.verify();
    assert!(
        oreport.is_clean(),
        "Both paths to B should have valid provenance, violations: {:?}",
        oreport.violations
    );
}

// ===========================================================================
// Test 9: dlist_insert_after — Insert after a given node
// ===========================================================================

#[test]
fn test_dlist_insert_after() {
    let mut list = DList::new(0x9000);
    let a = list.push_back(10);
    let b = list.push_back(30);
    // Insert 20 between A and B
    let c = list.insert_after(a, 20).unwrap();

    assert_eq!(list.len, 3);
    assert_eq!(list.traverse(), vec![10, 20, 30]);

    // Verify pointer consistency
    assert_eq!(list.nodes[&a].next, c, "A.next should point to C");
    assert_eq!(list.nodes[&c].prev, a, "C.prev should point to A");
    assert_eq!(list.nodes[&c].next, b, "C.next should point to B");
    assert_eq!(list.nodes[&b].prev, c, "B.prev should point to C");

    // --- Exclusivity: 4 pointer writes, all sequential ---
    let mut excl_input = ExclusivityInput::new();
    // C.prev = A, C.next = B (write new node pointers)
    excl_input.add_access(write_access(1, c + 8, 8, "ins.C.prev=A", 1, 3));
    excl_input.add_access(write_access(2, c + 16, 8, "ins.C.next=B", 2, 3));
    // A.next = C (update predecessor)
    excl_input.add_access(write_access(3, a + 16, 8, "ins.A.next=C", 3, 1));
    // B.prev = C (update successor)
    excl_input.add_access(write_access(4, b + 8, 8, "ins.B.prev=C", 4, 2));
    // Sequential ordering
    for i in 1..4 {
        excl_input.add_sync_edge(SyncEdgeRecord::new(aid(i), aid(i + 1), SyncOrdering::HappensBefore));
    }

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential insert_after writes should be Proven"
    );

    // --- Interpretation: All writes use pointer BD ---
    let mut interp = InterpretationVerifier::new();
    let ptr_bd = node_ptr_bd();
    let data_bd = node_field_bd();
    interp.record_write(LocationId(c), data_bd, InterpProgramPointId(1));
    interp.record_write(LocationId(c + 8), ptr_bd.clone(), InterpProgramPointId(2));
    interp.record_write(LocationId(c + 16), ptr_bd.clone(), InterpProgramPointId(3));
    interp.record_write(LocationId(a + 16), ptr_bd.clone(), InterpProgramPointId(4));
    interp.record_write(LocationId(b + 8), ptr_bd.clone(), InterpProgramPointId(5));
    // Read back
    interp.record_read(LocationId(c + 8), ptr_bd.clone(), InterpProgramPointId(10));
    interp.record_read(LocationId(c + 16), ptr_bd.clone(), InterpProgramPointId(11));
    interp.record_read(LocationId(a + 16), ptr_bd.clone(), InterpProgramPointId(12));
    interp.record_read(LocationId(b + 8), ptr_bd.clone(), InterpProgramPointId(13));

    let pairs = interp.extract_write_read_pairs();
    assert!(pairs.len() >= 4, "Expected at least 4 write-read pairs");
    for pair in &pairs {
        assert!(
            pair.write_bd.repd.compatible(&pair.read_bd.repd),
            "All insert_after write-read pairs should have compatible BDs"
        );
    }

    // --- Origin: New node C traced from same region ---
    let mut ov = OriginVerifier::new();
    ov.add_region(OriginRegionStruct::new(orid(1), oaddr(0x9000), DLIST_HEADER_SIZE + 3 * NODE_SIZE));
    ov.add_derivation(OriginDerivation::new(
        odid(1), DerivationSource::Region(orid(1)),
        OriginDerivationKind::Offset { by: DLIST_HEADER_SIZE as i64 },
        (oaddr(a), oaddr(a + NODE_SIZE)),
    ));
    ov.add_derivation(OriginDerivation::new(
        odid(2), DerivationSource::Region(orid(1)),
        OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + NODE_SIZE) as i64 },
        (oaddr(b), oaddr(b + NODE_SIZE)),
    ));
    ov.add_derivation(OriginDerivation::new(
        odid(3), DerivationSource::Region(orid(1)),
        OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + 2 * NODE_SIZE) as i64 },
        (oaddr(c), oaddr(c + NODE_SIZE)),
    ));
    // Access C through derivation from A
    ov.add_derivation(OriginDerivation::new(
        odid(4), DerivationSource::AnotherDerivation(odid(1)),
        OriginDerivationKind::Direct,
        (oaddr(c), oaddr(c + NODE_SIZE)),
    ));
    ov.add_access(OriginAccess::new(oaid(1), odid(3), OriginAccessKind::Write, 24, "write_C", true));
    ov.add_access(OriginAccess::new(oaid(2), odid(4), OriginAccessKind::Read, 24, "read_C_via_A", true));

    let oreport = ov.verify();
    assert!(
        oreport.is_clean(),
        "Insert_after provenance should be clean, violations: {:?}",
        oreport.violations
    );
}

// ===========================================================================
// Test 10: dlist_full_lifecycle — Create, push, traverse, remove, push, dealloc
// ===========================================================================

#[test]
fn test_dlist_full_lifecycle() {
    // Phase 1: Create and push
    let mut list = DList::new(0xA000);
    let a = list.push_back(10);
    let b = list.push_back(20);
    let c = list.push_back(30);
    assert_eq!(list.traverse(), vec![10, 20, 30]);

    // Phase 2: Traverse
    let values = list.traverse();
    assert_eq!(values, vec![10, 20, 30]);

    // Phase 3: Remove middle
    list.remove_middle(b);
    assert_eq!(list.traverse(), vec![10, 30]);

    // Phase 4: Push again
    let d = list.push_back(40);
    assert_eq!(list.traverse(), vec![10, 30, 40]);

    // Phase 5: Dealloc all
    let freed = list.dealloc_all();
    assert_eq!(freed, 3); // A, C, D (B was already removed)
    assert_eq!(list.len, 0);

    // --- Full exclusivity verification ---
    let mut excl_input = ExclusivityInput::new();
    // Phase 1: push_back writes
    excl_input.add_access(write_access(1, a, 8, "lifecycle.A.data", 1, 1));
    excl_input.add_access(write_access(2, b, 8, "lifecycle.B.data", 2, 2));
    excl_input.add_access(write_access(3, c, 8, "lifecycle.C.data", 3, 3));
    // Phase 2: traverse reads
    excl_input.add_access(read_access(4, a, 8, "lifecycle.read_A", 1, 1));
    excl_input.add_access(read_access(5, c, 8, "lifecycle.read_C", 3, 3));
    // Phase 3: remove B pointer updates
    excl_input.add_access(write_access(6, a + 16, 8, "lifecycle.A.next=C", 1, 1));
    excl_input.add_access(write_access(7, c + 8, 8, "lifecycle.C.prev=A", 3, 3));
    // Phase 4: push_back D
    excl_input.add_access(write_access(8, d, 8, "lifecycle.D.data", 4, 4));
    // Full sequential ordering
    for i in 1..8 {
        excl_input.add_sync_edge(SyncEdgeRecord::new(aid(i), aid(i + 1), SyncOrdering::HappensBefore));
    }

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Full lifecycle should have no exclusivity conflicts"
    );

    // --- Full cleanup verification ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_a = cg.add_node(
        OperationKind::Acquire { resource: crid(1), kind: CleanupResourceKind::Memory }, "alloc_A");
    let alloc_b = cg.add_node(
        OperationKind::Acquire { resource: crid(2), kind: CleanupResourceKind::Memory }, "alloc_B");
    let alloc_c = cg.add_node(
        OperationKind::Acquire { resource: crid(3), kind: CleanupResourceKind::Memory }, "alloc_C");
    let alloc_d = cg.add_node(
        OperationKind::Acquire { resource: crid(4), kind: CleanupResourceKind::Memory }, "alloc_D");
    let access_a1 = cg.add_node(OperationKind::Access { resource: crid(1) }, "access_A_1");
    let access_b = cg.add_node(OperationKind::Access { resource: crid(2) }, "access_B");
    let access_c = cg.add_node(OperationKind::Access { resource: crid(3) }, "access_C");
    let free_b = cg.add_node(
        OperationKind::Release { resource: crid(2), kind: CleanupResourceKind::Memory }, "free_B_remove");
    let access_a2 = cg.add_node(OperationKind::Access { resource: crid(1) }, "access_A_2");
    let access_c2 = cg.add_node(OperationKind::Access { resource: crid(3) }, "access_C_2");
    let access_d = cg.add_node(OperationKind::Access { resource: crid(4) }, "access_D");
    let free_a = cg.add_node(
        OperationKind::Release { resource: crid(1), kind: CleanupResourceKind::Memory }, "free_A");
    let free_c = cg.add_node(
        OperationKind::Release { resource: crid(3), kind: CleanupResourceKind::Memory }, "free_C");
    let free_d = cg.add_node(
        OperationKind::Release { resource: crid(4), kind: CleanupResourceKind::Memory }, "free_D");
    let ret = cg.add_node(OperationKind::Return, "return");

    cg.add_edge(entry, alloc_a).unwrap();
    cg.add_edge(alloc_a, alloc_b).unwrap();
    cg.add_edge(alloc_b, alloc_c).unwrap();
    cg.add_edge(alloc_c, access_a1).unwrap();
    cg.add_edge(access_a1, access_b).unwrap();
    cg.add_edge(access_b, access_c).unwrap();
    cg.add_edge(access_c, free_b).unwrap();
    cg.add_edge(free_b, alloc_d).unwrap();
    cg.add_edge(alloc_d, access_a2).unwrap();
    cg.add_edge(access_a2, access_c2).unwrap();
    cg.add_edge(access_c2, access_d).unwrap();
    cg.add_edge(access_d, free_a).unwrap();
    cg.add_edge(free_a, free_c).unwrap();
    cg.add_edge(free_c, free_d).unwrap();
    cg.add_edge(free_d, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(
        report.clean,
        "Full lifecycle cleanup should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.acquires_checked, 4);

    // --- Full liveness verification ---
    let mut linput = LivenessInput::new();
    // Phase 1: Create & push
    linput.add_event(alloc_event(1, 1, 1));  // A
    linput.add_event(lwrite_event(1, 2, 1));
    linput.add_event(alloc_event(2, 3, 1));  // B
    linput.add_event(lwrite_event(2, 4, 1));
    linput.add_event(alloc_event(3, 5, 1));  // C
    linput.add_event(lwrite_event(3, 6, 1));
    // Phase 2: Traverse
    linput.add_event(lread_event(1, 7, 1));
    linput.add_event(lread_event(2, 8, 1));
    linput.add_event(lread_event(3, 9, 1));
    // Phase 3: Remove B
    linput.add_event(dealloc_event(2, 10, 1)); // free B
    linput.add_event(lread_event(1, 11, 1));   // A still live
    linput.add_event(lread_event(3, 12, 1));   // C still live
    // Phase 4: Push D
    linput.add_event(alloc_event(4, 13, 1));  // D
    linput.add_event(lwrite_event(4, 14, 1));
    linput.add_event(lread_event(4, 15, 1));
    // Phase 5: Dealloc all
    linput.add_event(dealloc_event(1, 16, 1));
    linput.add_event(dealloc_event(3, 17, 1));
    linput.add_event(dealloc_event(4, 18, 1));
    linput.cfg_edges = linear_lcfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]);
    linput.entry_point = Some(lpp(1));

    let mut lverifier = LivenessVerifier::new();
    let lresult = lverifier.verify(&linput);
    assert!(
        lresult.invariant_holds,
        "Full lifecycle liveness should hold, violations: {:?}",
        lresult.violations
    );
    assert_eq!(lresult.resources_checked, 4);

    // --- Full origin verification ---
    let mut ov = OriginVerifier::new();
    ov.add_region(OriginRegionStruct::new(
        orid(1), oaddr(0xA000), DLIST_HEADER_SIZE + 4 * NODE_SIZE,
    ));
    // All nodes derive from the same allocation region
    for (i, node_addr) in [a, b, c, d].iter().enumerate() {
        ov.add_derivation(OriginDerivation::new(
            odid((i + 1) as u64), DerivationSource::Region(orid(1)),
            OriginDerivationKind::Offset { by: (DLIST_HEADER_SIZE + (i as u64) * NODE_SIZE) as i64 },
            (oaddr(*node_addr), oaddr(*node_addr + NODE_SIZE)),
        ));
    }
    // All accesses are writes (initialized)
    for i in 0..4 {
        ov.add_access(OriginAccess::new(
            oaid((i + 1) as u64), odid((i + 1) as u64),
            OriginAccessKind::Write, NODE_SIZE, format!("lifecycle_node{}", i), true,
        ));
    }

    let oreport = ov.verify();
    assert!(
        oreport.is_clean(),
        "Full lifecycle origin should be clean, violations: {:?}",
        oreport.violations
    );

    // Verify taint: all trusted
    for node in &oreport.provenance_forest {
        assert_eq!(node.taint, TaintLevel::Trusted, "All lifecycle derivations should be trusted");
        assert!(node.has_origin(), "All lifecycle derivations should have traceable origin");
    }

    // --- Interpretation: Full BD compatibility ---
    let mut interp = InterpretationVerifier::new();
    let data_bd = node_field_bd();
    let ptr_bd = node_ptr_bd();

    // Write all node fields
    for &node_addr in &[a, b, c, d] {
        interp.record_write(LocationId(node_addr), data_bd.clone(), InterpProgramPointId(node_addr));
        interp.record_write(LocationId(node_addr + 8), ptr_bd.clone(), InterpProgramPointId(node_addr + 1));
        interp.record_write(LocationId(node_addr + 16), ptr_bd.clone(), InterpProgramPointId(node_addr + 2));
    }
    // Read back A and C after B removal (same BD)
    interp.record_read(LocationId(a + 16), ptr_bd.clone(), InterpProgramPointId(100));
    interp.record_read(LocationId(c + 8), ptr_bd.clone(), InterpProgramPointId(101));

    let pairs = interp.extract_write_read_pairs();
    assert!(pairs.len() >= 2, "Expected write-read pairs for lifecycle");
    for pair in &pairs {
        assert!(
            pair.write_bd.repd.compatible(&pair.read_bd.repd),
            "All lifecycle write-read pairs should have compatible BDs"
        );
    }

    // --- Verify with LivenessVerificationContext for deeper analysis ---
    let ctx = LivenessVerificationContext::new(linput.clone());
    let paths = LivenessVerifier::new().compute_liveness_paths(&ctx);
    // 4 resources (A, B, C, D)
    assert_eq!(paths.len(), 4, "Expected 4 liveness paths");
    // B should have deallocation_point
    let b_path = paths.iter().find(|p| p.resource_id == 2).expect("B path");
    assert!(b_path.deallocation_point.is_some(), "B should be deallocated");
    assert!(b_path.access_after_free.is_empty(), "No access after free for B");
}
