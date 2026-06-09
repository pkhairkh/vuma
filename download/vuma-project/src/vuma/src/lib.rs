//! # VUMA Core — Verified-Unsafe Memory Access
//!
//! This crate provides the foundational data types and the Memory State Graph
//! (MSG) for the VUMA project. VUMA's goal is to make `unsafe` memory access
//! in systems programs *verifiable* by tracking every allocation, pointer
//! derivation, access, and synchronisation event.
//!
//! ## Architecture
//!
//! The core model is organised around four interconnected concepts:
//!
//! | Concept        | Module              | Description                                    |
//! |----------------|----------------------|------------------------------------------------|
//! | **Region**     | [`region`]          | Contiguous memory span (heap, stack, mmap, …)  |
//! | **Derivation** | [`derivation`]      | How a pointer was derived from a region        |
//! | **Access**     | [`access`]          | A read or write at a program point             |
//! | **Sync Edge**  | [`sync`]            | Ordering between accesses (hb, atomic, mutex)  |
//! | **MSG**        | [`msg`]             | The graph that ties everything together         |
//!
//! Supporting types:
//!
//! - [`address`] — the `Address` newtype with hex display and arithmetic.
//! - [`program_point`] — source location tracking (`file:line:col`).
//!
//! ## Quick start
//!
//! ```rust
//! use vuma_core::msg::MSG;
//! use vuma_core::region::{Region, RegionId, RegionStatus};
//! use vuma_core::address::Address;
//! use vuma_core::program_point::ProgramPoint;
//!
//! let mut msg = MSG::new();
//!
//! let region = Region {
//!     id: RegionId(1),
//!     base: Address::from(0x1000_u64),
//!     size: 0x200,
//!     status: RegionStatus::Allocated,
//!     alloc_point: ProgramPoint::new("main.vu", 10, 5),
//!     free_point: None,
//!     owner_context: None,
//! };
//! msg.add_region(region);
//!
//! assert_eq!(msg.region_count(), 1);
//! assert_eq!(msg.region_of(Address::from(0x1050_u64)), Some(RegionId(1)));
//! ```

pub mod access;
// pub mod access_analysis; // compile errors from other agent
pub mod address;
pub mod derivation;
pub mod invariant_exclusivity;
pub mod invariant_cleanup;
pub mod invariant_interpretation;
pub mod invariant_liveness;
pub mod invariant_origin;
pub mod msg_incremental;
pub mod msg;
pub mod msg_builder;
pub mod program_point;
pub mod region;
pub mod repl;
pub mod scg_to_msg;
pub mod security;
pub mod sync;

// Re-export the most commonly used types at the crate root for convenience.
pub use access::{Access, AccessId, AccessKind};
pub use address::Address;
pub use derivation::{Derivation, DerivationId, DerivationKind, DerivationSource};
pub use msg::MSG;
pub use program_point::{NodeId, ProgramPoint};
pub use region::{Region, RegionId, RegionStatus};
pub use sync::{LockId, Ordering, SyncEdge, SyncEdgeId};
pub use msg_incremental::{
    MSGDelta, DeltaResult, DeltaError, VerificationStatus,
    apply_delta, compute_delta, compute_scg_delta,
    SCGSnapshot, SCGNode, EntityDelta,
};
pub use repl::{VumaRepl, ReplError, ReplResult, ReplProfile};
pub use security::{
    SecurityLevel, FlowPolicy, TaintSource, TaintLabel, TaintStatus, TaintTracker,
    SecurityRel, SecurityCapability, SecurityBoundary, BoundaryId,
    DeclassificationRecord, DeclassificationProof, GateFunctionId,
    SecurityViolation, Arm64SecurityMapping, Arm64Feature, PacKey, MteMode, BtiType,
    SecNode, SecEdge, SecurityVerifier, VerificationResult,
    propagate_taint_through_chain,
};
