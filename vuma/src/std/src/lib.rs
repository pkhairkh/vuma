//! # VUMA Standard Library (`vuma-std`)
//!
//! The VUMA standard library provides verified, BD-annotated foundational types,
//! allocation strategies, data structures, I/O bindings, and synchronization
//! primitives for the VUMA AI-native programming language framework.
//!
//! ## Module Overview
//!
//! - **primitives**: Behavioral Description (BD) definitions for primitive types
//!   (integers, floats, bool, byte, pointers) with Capability Descriptors (CapDs).
//! - **alloc**: Memory allocation strategies (Global, Arena, Pool, Bump, FreeList,
//!   VumaAllocator) with VUMA-compatible BD annotations for verified memory management.
//! - **collections**: Verified data structures (DoublyLinkedList, Vec, HashMap, RingBuffer)
//!   with BD-annotated methods and capability tracking.
//! - **io**: I/O bindings for file, standard stream, and network operations with
//!   capability-based access control.
//! - **sync**: Synchronization primitives (Mutex, RwLock, Channel, Barrier) with
//!   BD CapD annotations ensuring exclusive access patterns and SyncEdge annotations
//!   for the Message Sequence Graph (MSG).
//!
//! ## VUMA Verification
//!
//! All public methods marked with `// VUMA-VERIFIED` have been verified against
//! the VUMA Behavioral Description system, ensuring memory safety, capability
//! compliance, and data-race freedom within the VUMA runtime.

// VUMA-VERIFIED: module-level re-exports are BD-transparent

pub mod primitives;
pub mod alloc;
pub mod collections;
pub mod io;
pub mod sync;
pub mod net;
pub mod time;
pub mod process;

// Re-export core BD types for convenience
pub use primitives::{
    BD, RelD, RelKind, HasBD,
    RepD, CapD, CapFlag, SyncEdge,
    uint8_repd, uint16_repd, uint32_repd, uint64_repd,
    int8_repd, int16_repd, int32_repd, int64_repd,
    float32_repd, float64_repd,
    bool_repd, byte_repd, ptr_repd,
    numeric_capd, string_capd,
    ptr_reld, region_ptr_reld, slice_reld, result_reld, option_reld, numeric_reld,
};

// Re-export VUMA primitive types
pub use primitives::{
    Ptr, RegionPtr, Slice, VumaResult, VumaOption, Range,
};

// Re-export allocation types
pub use alloc::{
    Address, GlobalAllocator, ArenaAllocator, PoolAllocator,
    BumpAllocator, FreeListAllocator, VumaAllocator,
    AllocError, AllocResult,
    MemoryStats, AllocRecord, AllocEventKind, AllocTracker,
};

// Re-export collection types
pub use collections::{
    DoublyLinkedList, Vec as VumaVec, HashMap as VumaHashMap, RingBuffer,
    VumaString, SipHasher13, siphash_key,
    BdVecStats, BdHashMapStats,
    VecIter, VecIterMut, VecIntoIter,
    VumaStringChars, HashMapIter, HashMapKeys, HashMapValues,
};

// Re-export I/O types
pub use io::{
    // Core I/O traits
    VumaReader, VumaWriter,
    // Buffered I/O
    VumaBufReader, VumaBufWriter,
    // VUMA standard streams
    VumaStdin, VumaStdout, VumaStderr,
    // VUMA file I/O
    VumaFile,
    // VUMA error types
    VumaIoError, VumaIoErrorKind, VumaIoResult,
    // Legacy types (backward compatible)
    File, FileMode, FileCapD,
    Stdin, Stdout, Stderr,
    TcpStream, TcpListener, UdpSocket,
    NetworkCapD,
};

// Re-export sync types
pub use sync::{
    Mutex, RwLock, Channel, Barrier,
    MutexGuard, RwLockReadGuard, RwLockWriteGuard,
    MutexCapD, RwLockCapD, ChannelCapD, BarrierCapD,
};

/// VUMA Standard Library version
pub const VERSION: &str = "0.1.0";

/// Returns the library version string.
// VUMA-VERIFIED: pure function, no side effects
pub fn version() -> &'static str {
    VERSION
}
