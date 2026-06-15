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
//!   Also provides system-call-level heap allocation (`heap_alloc`, `heap_free`,
//!   `heap_realloc`) that underpins the `.vuma` `allocate`/`free` builtins.
//! - **collections**: Verified data structures (DoublyLinkedList, Vec, HashMap, RingBuffer)
//!   with BD-annotated methods and capability tracking.
//! - **crypto**: Cryptographic primitive declarations (SHA-256 constants, logical
//!   functions, byte-access helpers) and documentation of available VUMA crypto idioms.
//! - **io**: I/O bindings for file, standard stream, and network operations with
//!   capability-based access control. Also provides low-level syscall wrappers
//!   (`read_bytes`, `write_bytes`) and little-endian byte access (`read_u32_le`,
//!   `write_u32_le`).
//! - **string**: String and memory operations (`strlen`, `strcmp`, `memcpy`, `memset`)
//!   that operate on VUMA `Address` pointers.
//! - **math**: Mathematical utility functions (`abs`, `min`, `max`, `clamp`) commonly
//!   needed by LLMs writing real programs.
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
pub mod crypto;
pub mod env;
pub mod error;
pub mod fs;
pub mod io;
pub mod math;
pub mod net;
pub mod path;
pub mod primitives;
pub mod process;
pub mod string;
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
    BumpAllocator, FreeListAllocator, GlobalAllocator, heap_alloc, heap_free, heap_realloc,
    MemoryStats, PoolAllocator, VumaAllocator,
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
    // Low-level I/O syscalls and byte access
    read_bytes,
    write_bytes,
    read_u32_le,
    write_u32_le,
};

// Re-export sync types
pub use sync::{
    Barrier, BarrierCapD, Channel, ChannelCapD, Mutex, MutexCapD, MutexGuard, RwLock, RwLockCapD,
    RwLockReadGuard, RwLockWriteGuard,
};

// Re-export crypto types
pub use crypto::{
    SHA256_K, SHA256_H, crypto_capd, sha256_ch, sha256_maj, sha256_big_sigma0, sha256_big_sigma1,
    sha256_small_sigma0, sha256_small_sigma1, sha256_read_u32_be, sha256_write_u32_be,
    ct_select_u32, ct_eq_u32, ct_ne_u32, ct_lt_u32, ct_gte_u32,
};

// Re-export string/memory operations
pub use string::{strlen, strcmp, memcpy, memset};

// Re-export math utility functions
pub use math::{abs, min, max, clamp};

/// VUMA Standard Library version
pub const VERSION: &str = "0.1.0";

/// Returns the library version string.
// VUMA-VERIFIED: pure function, no side effects
pub fn version() -> &'static str {
    VERSION
}
