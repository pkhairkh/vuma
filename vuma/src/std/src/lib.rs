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

pub mod alloc;
pub mod collections;
pub mod env;
pub mod error;
pub mod fs;
pub mod io;
pub mod net;
pub mod path;
pub mod primitives;
pub mod process;
pub mod sync;
pub mod thread;
pub mod time;

// Re-export core BD types for convenience
pub use primitives::{
    bool_repd, byte_repd, float32_repd, float64_repd, int16_repd, int32_repd, int64_repd,
    int8_repd, numeric_capd, numeric_reld, option_reld, ptr_reld, ptr_repd, region_ptr_reld,
    result_reld, slice_reld, string_capd, uint16_repd, uint32_repd, uint64_repd, uint8_repd, CapD,
    CapFlag, HasBD, RelD, RelKind, RepD, SyncEdge, BD,
};

// Re-export VUMA primitive types
pub use primitives::{Ptr, Range, RegionPtr, Slice, VumaOption, VumaResult};

// Re-export allocation types
pub use alloc::{
    Address, AllocError, AllocEventKind, AllocRecord, AllocResult, AllocTracker, ArenaAllocator,
    BumpAllocator, FreeListAllocator, GlobalAllocator, MemoryStats, PoolAllocator, VumaAllocator,
};

// Re-export collection types
pub use collections::{
    siphash_key, BdHashMapStats, BdVecStats, DoublyLinkedList, HashMap as VumaHashMap, HashMapIter,
    HashMapKeys, HashMapValues, RingBuffer, SipHasher13, Vec as VumaVec, VecIntoIter, VecIter,
    VecIterMut, VumaString, VumaStringChars,
};

// Re-export I/O types
pub use io::{
    // Legacy types (backward compatible)
    File,
    FileCapD,
    FileMode,
    NetworkCapD,
    Stderr,
    Stdin,
    Stdout,
    TcpListener,
    TcpStream,
    UdpSocket,
    // Buffered I/O
    VumaBufReader,
    VumaBufWriter,
    // VUMA file I/O
    VumaFile,
    // VUMA error types
    VumaIoError,
    VumaIoErrorKind,
    VumaIoResult,
    // Core I/O traits
    VumaReader,
    VumaStderr,
    // VUMA standard streams
    VumaStdin,
    VumaStdout,
    VumaWriter,
};

// Re-export sync types
pub use sync::{
    Barrier, BarrierCapD, Channel, ChannelCapD, Mutex, MutexCapD, MutexGuard, RwLock, RwLockCapD,
    RwLockReadGuard, RwLockWriteGuard,
};

/// VUMA Standard Library version
pub const VERSION: &str = "0.1.0";

/// Returns the library version string.
// VUMA-VERIFIED: pure function, no side effects
pub fn version() -> &'static str {
    VERSION
}
