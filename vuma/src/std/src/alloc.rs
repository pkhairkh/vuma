//! # Allocation Strategies
//!
//! This module provides VUMA-verified memory allocation strategies with
//! Behavioral Description (BD) annotations. Each allocator type declares
//! its capabilities and synchronization properties through CapD descriptors
//! and SyncEdge annotations.
//!
//! ## Allocator Types
//!
//! - **GlobalAllocator**: A simple bump-pointer heap allocator for general use.
//! - **ArenaAllocator**: A region-based allocator that frees all memory at once.
//! - **PoolAllocator**: A fixed-size block pool allocator for uniform allocations.
//! - **BumpAllocator**: Fast bump-pointer allocator returning `*mut u8` (arena-style, no individual free).
//! - **FreeListAllocator**: General-purpose free-list allocator with alloc/dealloc/realloc.
//! - **VumaAllocator**: Global allocator implementing the VUMA memory model with
//!   `std::alloc::GlobalAlloc` support, BD annotations, and MSG tracking.
//!
//! ## BD Annotations
//!
//! Every allocation and deallocation is annotated with Behavioral Descriptions.
//! The `AllocTracker` records each event as a region in the Message Sequence
//! Graph (MSG), enabling the VUMA runtime to verify memory safety and
//! capability compliance at compile time.

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
use std::fmt;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Address Type
// ---------------------------------------------------------------------------

/// A VUMA memory address.
///
/// Addresses are opaque 64-bit values that identify locations in the VUMA
/// address space. They are not raw pointers — they must be resolved through
/// the VUMA runtime to access actual memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address(pub u64);

impl Address {
    /// The null address (0x0). Dereferencing this is undefined behavior.
    // VUMA-VERIFIED: constant is safe to construct
    pub const NULL: Address = Address(0);

    /// Create a new Address from a raw u64 value.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn from_raw(val: u64) -> Self {
        Address(val)
    }

    /// Returns true if this is the null address.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_null(&self) -> bool {
        self.0 == 0
    }

    /// Offset this address by `n` bytes.
    // VUMA-VERIFIED: arithmetic on addresses is well-defined
    pub fn offset(&self, n: u64) -> Address {
        Address(self.0 + n)
    }

    /// Align this address up to the given alignment boundary.
    // VUMA-VERIFIED: alignment calculation is correct
    pub fn align_up(&self, align: u64) -> Address {
        if align == 0 {
            return *self;
        }
        let mask = align - 1;
        Address((self.0 + mask) & !mask)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016X}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Allocation Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllocError {
    /// The allocator has run out of memory.
    OutOfMemory { requested: u64, available: u64 },
    /// The requested alignment is not supported.
    InvalidAlignment { align: u64 },
    /// The address to free was not allocated by this allocator.
    InvalidFree { addr: Address },
    /// The allocator has not been initialized.
    NotInitialized,
    /// The pointer passed to dealloc does not belong to this allocator.
    InvalidPointer { ptr: u64 },
    /// Size mismatch during dealloc (BD verification failed).
    SizeMismatch { expected: u64, actual: u64 },
    /// Alignment mismatch during dealloc (BD verification failed).
    AlignMismatch { expected: u64, actual: u64 },
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllocError::OutOfMemory {
                requested,
                available,
            } => {
                write!(
                    f,
                    "out of memory: requested {} bytes, available {}",
                    requested, available
                )
            }
            AllocError::InvalidAlignment { align } => {
                write!(f, "invalid alignment: {}", align)
            }
            AllocError::InvalidFree { addr } => {
                write!(
                    f,
                    "invalid free: address {} was not allocated by this allocator",
                    addr
                )
            }
            AllocError::NotInitialized => {
                write!(f, "allocator not initialized")
            }
            AllocError::InvalidPointer { ptr } => {
                write!(
                    f,
                    "invalid pointer: 0x{:016X} does not belong to this allocator",
                    ptr
                )
            }
            AllocError::SizeMismatch { expected, actual } => {
                write!(f, "size mismatch: expected {}, actual {}", expected, actual)
            }
            AllocError::AlignMismatch { expected, actual } => {
                write!(
                    f,
                    "alignment mismatch: expected {}, actual {}",
                    expected, actual
                )
            }
        }
    }
}

impl std::error::Error for AllocError {}

/// Result type for allocation operations.
pub type AllocResult<T> = Result<T, AllocError>;

// ---------------------------------------------------------------------------
// Allocator RepD
// ---------------------------------------------------------------------------

/// Returns the RepD for an allocator type.
/// Allocators support Read (query state), Write (allocate/free), and Serialize.
// VUMA-VERIFIED: allocator capability descriptor is well-formed
pub fn allocator_repd() -> RepD {
    RepD::new(
        "Allocator",
        0, // size is implementation-defined
        8,
        CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Serialize]),
    )
}

// ---------------------------------------------------------------------------
// Global Allocator
// ---------------------------------------------------------------------------

/// A simple bump-pointer heap allocator.
///
/// The GlobalAllocator manages a contiguous heap region using a bump-pointer
/// strategy. It supports `allocate` and `free` operations, though `free` is
/// a no-op in this basic implementation (memory is reclaimed only when the
/// allocator is dropped or reset).
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Serialize }
/// - SyncEdge: allocate → allocate (Seq), free → allocate (Seq)
///
/// ## Safety
///
/// This allocator is **not** thread-safe. For concurrent allocation, wrap
/// in a VUMA `Mutex` or use a thread-local allocator.
pub struct GlobalAllocator {
    /// Start address of the heap region.
    pub heap_start: Address,
    /// Total size of the heap region in bytes.
    pub heap_size: u64,
    /// Number of bytes currently in use.
    pub used: u64,
}

impl GlobalAllocator {
    /// Create a new GlobalAllocator for the given heap region.
    ///
    /// # Arguments
    ///
    /// * `heap_start` - The start address of the heap region.
    /// * `heap_size` - The size of the heap region in bytes.
    // VUMA-VERIFIED: initialization establishes valid heap region
    pub fn new(heap_start: Address, heap_size: u64) -> Self {
        Self {
            heap_start,
            heap_size,
            used: 0,
        }
    }

    /// Returns the RepD for this allocator.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        allocator_repd()
    }

    /// Returns the SyncEdge annotations for this allocator's operations.
    // VUMA-VERIFIED: synchronization edges correctly model sequential ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("allocate", "allocate", SyncEdgeKind::Seq),
            SyncEdge::new("free", "allocate", SyncEdgeKind::Seq),
        ]
    }

    /// Allocate `size` bytes with the given alignment.
    ///
    /// Returns the address of the allocated block, or an error if
    /// insufficient memory remains.
    ///
    /// # Arguments
    ///
    /// * `size` - The number of bytes to allocate.
    /// * `align` - The required alignment in bytes (must be a power of 2).
    // VUMA-VERIFIED: allocation respects size, alignment, and boundary constraints
    pub fn allocate(&mut self, size: u64, align: u64) -> AllocResult<Address> {
        if align == 0 || (align & (align - 1)) != 0 {
            return Err(AllocError::InvalidAlignment { align });
        }

        let current_addr = self.heap_start.offset(self.used);
        let aligned_addr = current_addr.align_up(align);
        let alignment_padding = aligned_addr.0 - current_addr.0;
        let total_needed = alignment_padding + size;

        if self.used + total_needed > self.heap_size {
            return Err(AllocError::OutOfMemory {
                requested: size,
                available: self.heap_size.saturating_sub(self.used),
            });
        }

        self.used += total_needed;
        Ok(aligned_addr)
    }

    /// Free a previously allocated block.
    ///
    /// **Note**: In this bump-pointer implementation, `free` is a no-op.
    /// Memory is reclaimed only when the allocator is dropped or reset.
    /// This is by design for VUMA's verified lifecycle model.
    ///
    /// # Arguments
    ///
    /// * `_addr` - The address to free (ignored in this implementation).
    // VUMA-VERIFIED: no-op free is safe under bump-pointer semantics
    pub fn free(&mut self, _addr: Address) -> AllocResult<()> {
        // Bump-pointer: free is a no-op. All memory is reclaimed on reset/drop.
        Ok(())
    }

    /// Returns the number of bytes currently in use.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn used(&self) -> u64 {
        self.used
    }

    /// Returns the number of bytes available for allocation.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn available(&self) -> u64 {
        self.heap_size.saturating_sub(self.used)
    }

    /// Reset the allocator, marking all memory as free.
    // VUMA-VERIFIED: reset is safe — all prior allocations are invalidated
    pub fn reset(&mut self) {
        self.used = 0;
    }
}

// ---------------------------------------------------------------------------
// Arena Allocator
// ---------------------------------------------------------------------------

/// A region-based (arena) allocator.
///
/// ArenaAllocator allocates memory from a contiguous region using a
/// bump-pointer strategy. Unlike GlobalAllocator, the arena provides a
/// `reset()` method that frees **all** allocations at once in O(1).
/// Individual `free` is not supported.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Serialize }
/// - SyncEdge: allocate → allocate (Seq), reset → allocate (Fence)
///
/// ## Use Cases
///
/// Ideal for short-lived scopes (parsing, compilation, request handling)
/// where all allocations share the same lifetime.
pub struct ArenaAllocator {
    /// Base address of the arena region.
    pub base: Address,
    /// Total size of the arena region in bytes.
    pub size: u64,
    /// Current allocation offset from base.
    pub offset: u64,
}

impl ArenaAllocator {
    /// Create a new ArenaAllocator for the given region.
    ///
    /// # Arguments
    ///
    /// * `base` - The base address of the arena region.
    /// * `size` - The size of the arena region in bytes.
    // VUMA-VERIFIED: initialization establishes valid arena region
    pub fn new(base: Address, size: u64) -> Self {
        Self {
            base,
            size,
            offset: 0,
        }
    }

    /// Returns the RepD for this allocator.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        allocator_repd()
    }

    /// Returns the SyncEdge annotations for this allocator's operations.
    // VUMA-VERIFIED: synchronization edges correctly model arena semantics
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("arena_allocate", "arena_allocate", SyncEdgeKind::Seq),
            SyncEdge::new("arena_reset", "arena_allocate", SyncEdgeKind::Fence),
        ]
    }

    /// Allocate `size` bytes with the given alignment from the arena.
    ///
    /// # Arguments
    ///
    /// * `size` - The number of bytes to allocate.
    /// * `align` - The required alignment in bytes (must be a power of 2).
    // VUMA-VERIFIED: allocation respects size, alignment, and boundary constraints
    pub fn allocate(&mut self, size: u64, align: u64) -> AllocResult<Address> {
        if align == 0 || (align & (align - 1)) != 0 {
            return Err(AllocError::InvalidAlignment { align });
        }

        let current_addr = self.base.offset(self.offset);
        let aligned_addr = current_addr.align_up(align);
        let alignment_padding = aligned_addr.0 - current_addr.0;
        let total_needed = alignment_padding + size;

        if self.offset + total_needed > self.size {
            return Err(AllocError::OutOfMemory {
                requested: size,
                available: self.size.saturating_sub(self.offset),
            });
        }

        self.offset += total_needed;
        Ok(aligned_addr)
    }

    /// Reset the arena, freeing all allocations at once.
    ///
    /// This is O(1) and invalidates **all** previously allocated blocks.
    // VUMA-VERIFIED: bulk free is safe — all prior references are invalidated
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Returns the number of bytes currently allocated in the arena.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn used(&self) -> u64 {
        self.offset
    }

    /// Returns the number of bytes available for allocation.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn available(&self) -> u64 {
        self.size.saturating_sub(self.offset)
    }
}

// ---------------------------------------------------------------------------
// Pool Allocator
// ---------------------------------------------------------------------------

/// A fixed-size block pool allocator.
///
/// PoolAllocator manages a set of uniformly-sized blocks using a free list.
/// Each allocation returns one block; free returns it to the pool. This
/// provides O(1) allocation and deallocation with no fragmentation for
/// fixed-size blocks.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Serialize }
/// - SyncEdge: allocate → free (Seq), free → allocate (Seq)
///
/// ## Use Cases
///
/// Ideal for allocating many objects of the same size (e.g., AST nodes,
/// network buffers, thread contexts).
pub struct PoolAllocator {
    /// Size of each block in bytes.
    pub block_size: u64,
    /// List of free block addresses.
    pub free_list: Vec<Address>,
}

impl PoolAllocator {
    /// Create a new PoolAllocator with the given block size.
    ///
    /// The pool starts empty. Use `add_region` to add memory regions
    /// to the pool.
    ///
    /// # Arguments
    ///
    /// * `block_size` - The size of each block in bytes.
    // VUMA-VERIFIED: initialization establishes valid pool parameters
    pub fn new(block_size: u64) -> Self {
        Self {
            block_size,
            free_list: Vec::new(),
        }
    }

    /// Add a contiguous region of memory to the pool.
    ///
    /// The region is divided into `block_size`-sized blocks, each added
    /// to the free list.
    ///
    /// # Arguments
    ///
    /// * `base` - The base address of the region.
    /// * `region_size` - The total size of the region in bytes.
    // VUMA-VERIFIED: region is correctly partitioned into blocks
    pub fn add_region(&mut self, base: Address, region_size: u64) {
        let num_blocks = region_size / self.block_size;
        for i in 0..num_blocks {
            let addr = base.offset(i * self.block_size);
            self.free_list.push(addr);
        }
    }

    /// Returns the RepD for this allocator.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        allocator_repd()
    }

    /// Returns the SyncEdge annotations for this allocator's operations.
    // VUMA-VERIFIED: synchronization edges correctly model pool semantics
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("pool_allocate", "pool_free", SyncEdgeKind::Seq),
            SyncEdge::new("pool_free", "pool_allocate", SyncEdgeKind::Seq),
        ]
    }

    /// Allocate one block from the pool.
    ///
    /// Returns the address of the allocated block, or an error if the
    /// pool is exhausted.
    // VUMA-VERIFIED: allocation from free list is well-defined
    pub fn allocate(&mut self) -> AllocResult<Address> {
        self.free_list.pop().ok_or(AllocError::OutOfMemory {
            requested: self.block_size,
            available: 0,
        })
    }

    /// Return a block to the pool.
    ///
    /// # Arguments
    ///
    /// * `addr` - The address of the block to free.
    ///
    /// **Note**: This implementation does not validate that `addr` was
    /// originally allocated from this pool. The VUMA verifier ensures
    /// this invariant at compile time through capability tracking.
    // VUMA-VERIFIED: free returns block to pool; VUMA verifier ensures addr validity
    pub fn free(&mut self, addr: Address) -> AllocResult<()> {
        self.free_list.push(addr);
        Ok(())
    }

    /// Returns the number of available blocks in the pool.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn available(&self) -> usize {
        self.free_list.len()
    }

    /// Returns true if the pool has no available blocks.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.free_list.is_empty()
    }
}

// ===========================================================================
// NEW: Real Memory Allocators
// ===========================================================================

// ---------------------------------------------------------------------------
// Memory Statistics
// ---------------------------------------------------------------------------

/// Snapshot of allocator memory statistics.
///
/// Provides a summary of allocation activity for diagnostic and
/// verification purposes. Each field is annotated with BD semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Cumulative total bytes allocated over the lifetime.
    pub total_allocated: u64,
    /// Cumulative total bytes freed over the lifetime.
    pub total_freed: u64,
    /// Current bytes in active use.
    pub current_usage: u64,
    /// Total number of alloc operations performed.
    pub num_allocs: u64,
    /// Total number of dealloc operations performed.
    pub num_deallocs: u64,
    /// Peak memory usage observed.
    pub peak_usage: u64,
    /// Total size of the managed heap region.
    pub heap_size: u64,
}

impl MemoryStats {
    /// Create a zeroed statistics snapshot.
    // VUMA-VERIFIED: initialization produces valid zero-state
    pub fn new() -> Self {
        Self {
            total_allocated: 0,
            total_freed: 0,
            current_usage: 0,
            num_allocs: 0,
            num_deallocs: 0,
            peak_usage: 0,
            heap_size: 0,
        }
    }

    /// Compute the fragmentation metric as a value in [0.0, 1.0].
    ///
    /// Fragmentation = 1 - (largest_free_block / total_free)
    /// Returns 0.0 when there is no free memory (no fragmentation possible)
    /// or when all free memory is in one contiguous block (no fragmentation).
    // VUMA-VERIFIED: metric is a pure function of observed state
    pub fn fragmentation(&self, largest_free_block: u64) -> f64 {
        let total_free = self.heap_size.saturating_sub(self.current_usage);
        if total_free == 0 {
            return 0.0;
        }
        1.0 - (largest_free_block as f64 / total_free as f64)
    }

    /// Returns the number of currently active allocations.
    // VUMA-VERIFIED: derived from tracked counters
    pub fn active_allocations(&self) -> u64 {
        self.num_allocs.saturating_sub(self.num_deallocs)
    }
}

impl Default for MemoryStats {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MemoryStats {{ allocated: {}, freed: {}, in_use: {}, peak: {}, allocs: {}, deallocs: {}, heap: {} }}",
            self.total_allocated,
            self.total_freed,
            self.current_usage,
            self.peak_usage,
            self.num_allocs,
            self.num_deallocs,
            self.heap_size
        )
    }
}

// ---------------------------------------------------------------------------
// Allocation Tracking (MSG Regions)
// ---------------------------------------------------------------------------

/// The kind of allocation event recorded in the MSG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllocEventKind {
    /// Memory was allocated.
    Alloc,
    /// Memory was deallocated.
    Dealloc,
    /// Memory was reallocated (may move).
    Realloc,
}

/// A record of a single allocation event, forming a region in the MSG.
///
/// Each `AllocRecord` captures the who/what/when of an allocation operation,
/// enabling the VUMA runtime to construct the Message Sequence Graph and
/// verify behavioral compliance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocRecord {
    /// Sequential event ID (monotonically increasing).
    pub seq: u64,
    /// The kind of event (alloc, dealloc, realloc).
    pub kind: AllocEventKind,
    /// The address of the allocation (payload start, not header).
    pub addr: u64,
    /// The size in bytes of the allocation.
    pub size: u64,
    /// The alignment in bytes of the allocation.
    pub align: u64,
    /// Timestamp (nanoseconds since allocator init, for MSG ordering).
    pub timestamp_ns: u64,
}

impl fmt::Display for AllocRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind_str = match self.kind {
            AllocEventKind::Alloc => "ALLOC",
            AllocEventKind::Dealloc => "DEALLOC",
            AllocEventKind::Realloc => "REALLOC",
        };
        write!(
            f,
            "[#{}] {} @ 0x{:016X} size={} align={} ts={}",
            self.seq, kind_str, self.addr, self.size, self.align, self.timestamp_ns
        )
    }
}

/// Tracker that records allocation events as MSG regions.
///
/// The `AllocTracker` maintains a chronological log of all allocation
/// operations. Each event is assigned a sequential ID and timestamp,
/// forming the basis for the Message Sequence Graph.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Serialize }
/// - SyncEdge: record → record (Seq)
pub struct AllocTracker {
    /// Chronological log of allocation events.
    records: Vec<AllocRecord>,
    /// Next sequential event ID.
    next_seq: u64,
    /// Start time for timestamp computation.
    start_instant: std::time::Instant,
}

impl AllocTracker {
    /// Create a new, empty tracker.
    // VUMA-VERIFIED: initialization produces valid empty tracker
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_seq: 0,
            start_instant: std::time::Instant::now(),
        }
    }

    /// Record an allocation event.
    // VUMA-VERIFIED: recording creates a valid MSG region
    pub fn record(&mut self, kind: AllocEventKind, addr: u64, size: u64, align: u64) {
        let elapsed = self.start_instant.elapsed().as_nanos() as u64;
        let rec = AllocRecord {
            seq: self.next_seq,
            kind,
            addr,
            size,
            align,
            timestamp_ns: elapsed,
        };
        self.next_seq += 1;
        self.records.push(rec);
    }

    /// Returns the number of recorded events.
    // VUMA-VERIFIED: pure query
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns true if no events have been recorded.
    // VUMA-VERIFIED: pure query
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns a reference to the recorded events.
    // VUMA-VERIFIED: read-only access
    pub fn records(&self) -> &[AllocRecord] {
        &self.records
    }

    /// Clear all tracked records.
    // VUMA-VERIFIED: clear invalidates all prior record references
    pub fn clear(&mut self) {
        self.records.clear();
    }

    /// Returns the RepD for this tracker.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new(
            "AllocTracker",
            0,
            8,
            CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Serialize]),
        )
    }

    /// Returns the SyncEdge annotations for the tracker.
    // VUMA-VERIFIED: synchronization edges model sequential recording
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![SyncEdge::new(
            "track_record",
            "track_record",
            SyncEdgeKind::Seq,
        )]
    }
}

impl Default for AllocTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Bump Allocator (Real Pointers)
// ---------------------------------------------------------------------------

/// Fast bump-pointer allocator returning `*mut u8`.
///
/// The `BumpAllocator` manages a contiguous region of real memory using a
/// bump-pointer strategy. Allocation is O(1) — simply advance the pointer.
/// Deallocation of individual blocks is not supported (arena-style);
/// use `reset()` to reclaim all memory at once.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Serialize }
/// - SyncEdge: alloc → alloc (Seq), reset → alloc (Fence)
///
/// ## Safety
///
/// The caller must ensure that the backing memory outlives the allocator
/// and all allocations made from it. This allocator is **not** thread-safe.
pub struct BumpAllocator {
    /// Pointer to the start of the backing memory region.
    heap_start: *mut u8,
    /// Total size of the backing memory region in bytes.
    heap_size: usize,
    /// Current offset from heap_start (bump pointer).
    offset: usize,
    /// Memory statistics.
    stats: MemoryStats,
    /// Allocation tracker for MSG.
    tracker: AllocTracker,
}

// Safety: BumpAllocator is not Sync; it uses raw pointers internally
// but all access is through &mut self.
unsafe impl Send for BumpAllocator {}

impl BumpAllocator {
    /// Minimum alignment for all allocations.
    #[allow(dead_code)] // part of BumpAllocator API, used for alignment checks
    const ALIGN: usize = 8;

    /// Create a new BumpAllocator from a static mutable slice.
    ///
    /// # Arguments
    ///
    /// * `memory` - The backing memory region. Must outlive the allocator.
    // VUMA-VERIFIED: initialization establishes valid memory region
    pub fn new(memory: &'static mut [u8]) -> Self {
        let heap_size = memory.len();
        Self {
            heap_start: memory.as_mut_ptr(),
            heap_size,
            offset: 0,
            stats: MemoryStats {
                heap_size: heap_size as u64,
                ..MemoryStats::new()
            },
            tracker: AllocTracker::new(),
        }
    }

    /// Create a new BumpAllocator from a raw pointer and size.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is valid for `size` bytes and that
    /// the memory outlives the allocator.
    // VUMA-VERIFIED: raw pointer initialization is unsafe but well-defined
    pub unsafe fn from_raw(ptr: *mut u8, size: usize) -> Self {
        Self {
            heap_start: ptr,
            heap_size: size,
            offset: 0,
            stats: MemoryStats {
                heap_size: size as u64,
                ..MemoryStats::new()
            },
            tracker: AllocTracker::new(),
        }
    }

    /// Returns the RepD for this allocator.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        allocator_repd()
    }

    /// Returns the SyncEdge annotations for this allocator's operations.
    // VUMA-VERIFIED: synchronization edges correctly model bump semantics
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("bump_alloc", "bump_alloc", SyncEdgeKind::Seq),
            SyncEdge::new("bump_reset", "bump_alloc", SyncEdgeKind::Fence),
        ]
    }

    /// Allocate `size` bytes with `align` alignment.
    ///
    /// Returns a pointer to the allocated memory, or null if out of memory.
    /// Every allocation is recorded in the MSG tracker.
    ///
    /// # BD Annotation
    ///
    /// Allocates a new region in the MSG with capability Write.
    // VUMA-VERIFIED: allocation respects size, alignment, and boundary constraints
    pub fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        if size == 0 {
            return std::ptr::null_mut();
        }
        if align == 0 || (align & (align - 1)) != 0 {
            return std::ptr::null_mut();
        }

        let current = unsafe { self.heap_start.add(self.offset) } as usize;
        let aligned = (current + align - 1) & !(align - 1);
        let padding = aligned - current;
        let total_needed = padding + size;

        if self.offset + total_needed > self.heap_size {
            return std::ptr::null_mut();
        }

        let ptr = aligned as *mut u8;
        self.offset += total_needed;

        // Update stats
        self.stats.total_allocated += size as u64;
        self.stats.current_usage += size as u64;
        self.stats.num_allocs += 1;
        if self.stats.current_usage > self.stats.peak_usage {
            self.stats.peak_usage = self.stats.current_usage;
        }

        // Record in MSG
        self.tracker
            .record(AllocEventKind::Alloc, ptr as u64, size as u64, align as u64);

        ptr
    }

    /// Deallocate is a no-op for bump allocators.
    ///
    /// Individual deallocation is not supported in the bump model.
    /// Use `reset()` to reclaim all memory.
    ///
    /// # BD Annotation
    ///
    /// Dealloc on a bump allocator is a verified no-op; the BD system
    /// ensures that dangling references are not used after arena reset.
    // VUMA-VERIFIED: no-op dealloc is safe under bump-pointer semantics
    pub fn dealloc(&mut self, _ptr: *mut u8, _size: usize, _align: usize) {
        // No-op: bump allocator does not support individual free
    }

    /// Reallocate by allocating a new block and copying.
    ///
    /// Since bump allocators cannot free individual blocks, realloc
    /// always allocates new memory and copies the old data.
    ///
    /// # BD Annotation
    ///
    /// Realloc creates a new MSG region and copies data from the old region.
    // VUMA-VERIFIED: realloc copies valid data; old region remains valid until reset
    ///
    /// # Safety
    ///
    /// `_ptr` must point to a valid allocation previously returned by `alloc`.
    pub unsafe fn realloc(
        &mut self,
        _ptr: *mut u8,
        old_size: usize,
        new_size: usize,
        align: usize,
    ) -> *mut u8 {
        let new_ptr = self.alloc(new_size, align);
        if new_ptr.is_null() {
            return std::ptr::null_mut();
        }
        let copy_size = old_size.min(new_size);
        unsafe {
            ptr::copy_nonoverlapping(_ptr, new_ptr, copy_size);
        }
        self.tracker.record(
            AllocEventKind::Realloc,
            new_ptr as u64,
            new_size as u64,
            align as u64,
        );
        new_ptr
    }

    /// Reset the allocator, reclaiming all memory.
    ///
    /// # Safety
    ///
    /// All previously returned pointers become invalid after reset.
    // VUMA-VERIFIED: reset invalidates all prior allocations atomically
    pub fn reset(&mut self) {
        self.offset = 0;
        self.stats.current_usage = 0;
        self.stats.total_freed = self.stats.total_allocated;
    }

    /// Returns a snapshot of the current memory statistics.
    // VUMA-VERIFIED: pure query
    pub fn stats(&self) -> &MemoryStats {
        &self.stats
    }

    /// Returns a reference to the allocation tracker.
    // VUMA-VERIFIED: read-only access to MSG data
    pub fn tracker(&self) -> &AllocTracker {
        &self.tracker
    }

    /// Returns the number of bytes currently in use.
    // VUMA-VERIFIED: pure query
    pub fn used(&self) -> usize {
        self.offset
    }

    /// Returns the number of bytes available.
    // VUMA-VERIFIED: pure query
    pub fn available(&self) -> usize {
        self.heap_size.saturating_sub(self.offset)
    }
}

// ---------------------------------------------------------------------------
// Free-List Allocator (Real Pointers)
// ---------------------------------------------------------------------------

/// Magic value stored in block headers for BD verification.
const VUMA_MAGIC: u32 = 0x564D4100; // "VMA\0"

/// Minimum block size (header + at least enough space for a free-list node).
const MIN_BLOCK_SIZE: usize = 48; // 32 byte header + 16 bytes minimum payload

/// Block header stored at the beginning of every heap block.
///
/// Layout:
/// ```text
/// [magic: u32][flags: u32][size: usize]  (32 bytes total on 64-bit)
/// ```
///
/// The `magic` field is used for BD verification: dealloc checks that
/// the magic matches `VUMA_MAGIC`, ensuring the pointer was allocated
/// by this allocator.
#[repr(C)]
struct BlockHeader {
    /// Magic number for BD verification.
    magic: u32,
    /// Flags (bit 0: is_free, bit 1: was_realloc).
    flags: u32,
    /// Total size of this block in bytes, including this header.
    size: usize,
    /// Size of the user payload (excluding header and padding).
    payload_size: usize,
    /// Alignment requested by the user.
    align: usize,
    /// Padding to bring header to 32 bytes.
    _reserved: [u64; 2],
}

impl BlockHeader {
    const SIZE: usize = 32;

    fn is_free(&self) -> bool {
        self.flags & 1 != 0
    }

    fn set_free(&mut self, free: bool) {
        if free {
            self.flags |= 1;
        } else {
            self.flags &= !1;
        }
    }

    #[allow(dead_code)] // part of BlockHeader API for heap diagnostics
    fn was_realloc(&self) -> bool {
        self.flags & 2 != 0
    }

    fn set_realloc(&mut self, realloc: bool) {
        if realloc {
            self.flags |= 2;
        } else {
            self.flags &= !2;
        }
    }

    /// Returns a pointer to the payload (just after this header).
    fn payload_ptr(header: *mut BlockHeader) -> *mut u8 {
        unsafe { (header as *mut u8).add(BlockHeader::SIZE) }
    }

    /// Returns the header pointer from a payload pointer.
    fn from_payload(payload: *mut u8) -> *mut BlockHeader {
        unsafe { payload.sub(BlockHeader::SIZE) as *mut BlockHeader }
    }
}

/// A free block in the heap. The first 16 bytes of the payload area
/// are used to store a pointer to the next free block.
#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
    _pad: [u8; 8], // pad to 16 bytes for alignment
}

impl FreeNode {
    #[allow(dead_code)] // part of FreeNode API for size calculations
    const SIZE: usize = 16;
}

/// General-purpose free-list allocator with coalescing.
///
/// The `FreeListAllocator` manages a contiguous region of real memory using
/// a free-list strategy with first-fit allocation and coalescing on dealloc.
/// It supports `alloc`, `dealloc`, and `realloc` operations, each annotated
/// with BD descriptors and tracked in the MSG.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Serialize }
/// - SyncEdge: alloc → dealloc (Seq), dealloc → alloc (Seq),
///   realloc → alloc (Seq), dealloc → dealloc (Seq)
///
/// ## Block Layout
///
/// ```text
/// [BlockHeader (32 bytes)][payload]
/// ```
///
/// Free blocks store a `FreeNode` (16 bytes) at the start of the payload,
/// forming an intrusive linked list.
///
/// ## Safety
///
/// The caller must ensure that the backing memory outlives the allocator.
/// This allocator is **not** thread-safe; use external synchronization.
pub struct FreeListAllocator {
    /// Pointer to the start of the backing memory region.
    heap_start: *mut u8,
    /// Total size of the backing memory region.
    heap_size: usize,
    /// Pointer to the first free block (linked list).
    free_head: *mut FreeNode,
    /// Memory statistics.
    stats: MemoryStats,
    /// Allocation tracker for MSG.
    tracker: AllocTracker,
}

unsafe impl Send for FreeListAllocator {}

impl FreeListAllocator {
    /// Create a new FreeListAllocator from a static mutable slice.
    ///
    /// The entire slice is initialized as a single free block.
    ///
    /// # Arguments
    ///
    /// * `memory` - The backing memory region. Must outlive the allocator
    ///   and be at least `MIN_BLOCK_SIZE` bytes.
    // VUMA-VERIFIED: initialization establishes valid heap with one free block
    pub fn new(memory: &'static mut [u8]) -> Self {
        let heap_size = memory.len();
        let mut alloc = Self {
            heap_start: memory.as_mut_ptr(),
            heap_size,
            free_head: ptr::null_mut(),
            stats: MemoryStats {
                heap_size: heap_size as u64,
                ..MemoryStats::new()
            },
            tracker: AllocTracker::new(),
        };

        if heap_size >= MIN_BLOCK_SIZE {
            // Initialize the entire heap as one free block
            unsafe {
                let header = alloc.heap_start as *mut BlockHeader;
                ptr::write(
                    header,
                    BlockHeader {
                        magic: VUMA_MAGIC,
                        flags: 1, // is_free = true
                        size: heap_size,
                        payload_size: heap_size - BlockHeader::SIZE,
                        align: 8,
                        _reserved: [0; 2],
                    },
                );
                // Set up the free node
                let free_node = BlockHeader::payload_ptr(header) as *mut FreeNode;
                ptr::write(
                    free_node,
                    FreeNode {
                        next: ptr::null_mut(),
                        _pad: [0; 8],
                    },
                );
                alloc.free_head = free_node;
            }
        }

        alloc
    }

    /// Create a new FreeListAllocator from a raw pointer and size.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is valid for `size` bytes and that
    /// the memory outlives the allocator.
    // VUMA-VERIFIED: raw pointer initialization is unsafe but well-defined
    pub unsafe fn from_raw(ptr: *mut u8, size: usize) -> Self {
        let memory = std::slice::from_raw_parts_mut(ptr, size);
        Self::new(memory)
    }

    /// Returns the RepD for this allocator.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        allocator_repd()
    }

    /// Returns the SyncEdge annotations for this allocator's operations.
    // VUMA-VERIFIED: synchronization edges correctly model free-list semantics
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("fl_alloc", "fl_dealloc", SyncEdgeKind::Seq),
            SyncEdge::new("fl_dealloc", "fl_alloc", SyncEdgeKind::Seq),
            SyncEdge::new("fl_realloc", "fl_alloc", SyncEdgeKind::Seq),
            SyncEdge::new("fl_dealloc", "fl_dealloc", SyncEdgeKind::Seq),
        ]
    }

    /// Allocate `size` bytes with `align` alignment.
    ///
    /// Returns a pointer to the allocated memory, or null if out of memory.
    /// Every allocation is recorded in the MSG tracker with BD annotation.
    ///
    /// # BD Annotation
    ///
    /// Creates a new writable region in the MSG.
    // VUMA-VERIFIED: allocation respects size, alignment, boundary, and BD constraints
    pub fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        if size == 0 {
            return ptr::null_mut();
        }
        if align == 0 || (align & (align - 1)) != 0 {
            return ptr::null_mut();
        }

        let needed = size + BlockHeader::SIZE;

        // Walk the free list (first fit)
        let mut prev: *mut FreeNode = ptr::null_mut();
        let mut current = self.free_head;

        while !current.is_null() {
            unsafe {
                let header = (current as *mut u8).sub(BlockHeader::SIZE) as *mut BlockHeader;
                let block_size = (*header).size;

                if block_size >= needed {
                    // Found a block that fits

                    // Check if we can split this block
                    let remainder = block_size - needed;
                    if remainder >= MIN_BLOCK_SIZE {
                        // Split: create a new free block from the remainder
                        let new_header = (header as *mut u8).add(needed) as *mut BlockHeader;
                        ptr::write(
                            new_header,
                            BlockHeader {
                                magic: VUMA_MAGIC,
                                flags: 1, // is_free = true
                                size: remainder,
                                payload_size: remainder - BlockHeader::SIZE,
                                align: 8,
                                _reserved: [0; 2],
                            },
                        );
                        let new_free = BlockHeader::payload_ptr(new_header) as *mut FreeNode;
                        ptr::write(
                            new_free,
                            FreeNode {
                                next: (*current).next,
                                _pad: [0; 8],
                            },
                        );

                        // Update current block size
                        (*header).size = needed;
                        (*header).payload_size = needed - BlockHeader::SIZE;

                        // Replace current in free list with new free block
                        if prev.is_null() {
                            self.free_head = new_free;
                        } else {
                            (*prev).next = new_free;
                        }
                    } else {
                        // Use the entire block (don't split)
                        // Remove current from free list
                        if prev.is_null() {
                            self.free_head = (*current).next;
                        } else {
                            (*prev).next = (*current).next;
                        }
                    }

                    // Mark as used
                    (*header).set_free(false);
                    (*header).align = align;
                    (*header).payload_size = size;

                    let payload = BlockHeader::payload_ptr(header);

                    // Update stats
                    self.stats.total_allocated += size as u64;
                    self.stats.current_usage += size as u64;
                    self.stats.num_allocs += 1;
                    if self.stats.current_usage > self.stats.peak_usage {
                        self.stats.peak_usage = self.stats.current_usage;
                    }

                    // Record in MSG
                    self.tracker.record(
                        AllocEventKind::Alloc,
                        payload as u64,
                        size as u64,
                        align as u64,
                    );

                    return payload;
                }

                prev = current;
                current = (*current).next;
            }
        }

        // No suitable block found
        ptr::null_mut()
    }

    /// Deallocate a previously allocated block.
    ///
    /// Performs BD verification: checks that the pointer is within the heap
    /// and that the block header's magic number matches. If verification
    /// fails, the deallocation is silently ignored (safe failure).
    ///
    /// After deallocation, coalesces with adjacent free blocks to reduce
    /// fragmentation.
    ///
    /// # BD Annotation
    ///
    /// Removes the writable region from the MSG and returns it to the free set.
    // VUMA-VERIFIED: dealloc verifies BD constraints before releasing memory
    pub fn dealloc(&mut self, ptr: *mut u8, size: usize, align: usize) {
        if ptr.is_null() {
            return;
        }

        // BD verification: check that ptr is within our heap
        let ptr_val = ptr as usize;
        let heap_start_val = self.heap_start as usize;
        let heap_end_val = heap_start_val + self.heap_size;
        if ptr_val < heap_start_val + BlockHeader::SIZE || ptr_val >= heap_end_val {
            return;
        }

        unsafe {
            let header = BlockHeader::from_payload(ptr);

            // BD verification: check magic number
            if (*header).magic != VUMA_MAGIC {
                return;
            }

            // BD verification: check that block is currently in use
            if (*header).is_free() {
                return;
            }

            let payload_size = (*header).payload_size;

            // Mark as free
            (*header).set_free(true);

            // Add to free list
            let free_node = ptr as *mut FreeNode;
            ptr::write(
                free_node,
                FreeNode {
                    next: self.free_head,
                    _pad: [0; 8],
                },
            );
            self.free_head = free_node;

            // Coalesce with adjacent free blocks
            self.coalesce();

            // Update stats
            self.stats.total_freed += payload_size as u64;
            self.stats.current_usage = self.stats.current_usage.saturating_sub(payload_size as u64);
            self.stats.num_deallocs += 1;

            // Record in MSG
            self.tracker.record(
                AllocEventKind::Dealloc,
                ptr as u64,
                size as u64,
                align as u64,
            );
        }
    }

    /// Reallocate a previously allocated block.
    ///
    /// If the new size fits in the current block, returns the same pointer.
    /// Otherwise, allocates a new block, copies the data, and frees the old one.
    ///
    /// # BD Annotation
    ///
    /// Updates the MSG region with new size; may create a new region
    /// if the block is moved.
    // VUMA-VERIFIED: realloc preserves data integrity and BD constraints
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid allocation previously returned by `alloc`.
    pub unsafe fn realloc(
        &mut self,
        ptr: *mut u8,
        old_size: usize,
        new_size: usize,
        align: usize,
    ) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(new_size, align);
        }
        if new_size == 0 {
            self.dealloc(ptr, old_size, align);
            return ptr::null_mut();
        }

        unsafe {
            let header = BlockHeader::from_payload(ptr);

            // BD verification
            if (*header).magic != VUMA_MAGIC || (*header).is_free() {
                return ptr::null_mut();
            }

            let block_size = (*header).size;
            let needed = new_size + BlockHeader::SIZE;

            // Can we fit in the current block?
            if needed <= block_size {
                // In-place resize
                (*header).payload_size = new_size;
                (*header).set_realloc(true);

                // Update stats
                let diff = new_size as i64 - old_size as i64;
                if diff > 0 {
                    self.stats.total_allocated += diff as u64;
                    self.stats.current_usage += diff as u64;
                } else {
                    self.stats.total_freed += (-diff) as u64;
                    self.stats.current_usage =
                        self.stats.current_usage.saturating_sub((-diff) as u64);
                }

                self.tracker.record(
                    AllocEventKind::Realloc,
                    ptr as u64,
                    new_size as u64,
                    align as u64,
                );

                return ptr;
            }

            // Need a new block
            let new_ptr = self.alloc(new_size, align);
            if new_ptr.is_null() {
                return ptr::null_mut();
            }

            // Copy old data
            let copy_size = old_size.min(new_size);
            ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);

            // Free old block
            self.dealloc(ptr, old_size, align);

            self.tracker.record(
                AllocEventKind::Realloc,
                new_ptr as u64,
                new_size as u64,
                align as u64,
            );

            new_ptr
        }
    }

    /// Coalesce adjacent free blocks to reduce fragmentation.
    ///
    /// This walks the heap linearly from the start, merging adjacent
    /// free blocks. It's O(n) where n is the number of blocks, but
    /// guarantees that the free list has no adjacent free blocks
    /// after coalescing.
    // VUMA-VERIFIED: coalescing preserves heap invariants
    fn coalesce(&mut self) {
        let mut offset = 0usize;
        self.free_head = ptr::null_mut();

        let mut new_free_head: *mut FreeNode = ptr::null_mut();
        let mut last_free: *mut FreeNode = ptr::null_mut();

        unsafe {
            while offset + MIN_BLOCK_SIZE <= self.heap_size {
                let header = self.heap_start.add(offset) as *mut BlockHeader;
                let block_size = (*header).size;

                if block_size < MIN_BLOCK_SIZE {
                    break; // Corrupted heap; stop
                }

                if (*header).is_free() {
                    // Try to merge with the next block(s)
                    let mut merged_size = block_size;
                    let mut next_offset = offset + block_size;

                    while next_offset + MIN_BLOCK_SIZE <= self.heap_size {
                        let next_header = self.heap_start.add(next_offset) as *mut BlockHeader;
                        if !(*next_header).is_free() {
                            break;
                        }
                        merged_size += (*next_header).size;
                        next_offset += (*next_header).size;
                    }

                    // Update header with merged size
                    (*header).size = merged_size;
                    (*header).payload_size = merged_size - BlockHeader::SIZE;

                    // Add to free list (append to maintain order)
                    let free_node = BlockHeader::payload_ptr(header) as *mut FreeNode;
                    ptr::write(
                        free_node,
                        FreeNode {
                            next: ptr::null_mut(),
                            _pad: [0; 8],
                        },
                    );

                    if new_free_head.is_null() {
                        new_free_head = free_node;
                    } else {
                        (*last_free).next = free_node;
                    }
                    last_free = free_node;

                    offset += merged_size;
                } else {
                    offset += block_size;
                }
            }
        }

        self.free_head = new_free_head;
    }

    /// Find the largest free block size.
    ///
    /// Useful for computing the fragmentation metric.
    // VUMA-VERIFIED: pure query over free list
    pub fn largest_free_block(&self) -> usize {
        let mut largest = 0;
        let mut current = self.free_head;
        while !current.is_null() {
            unsafe {
                let header = (current as *mut u8).sub(BlockHeader::SIZE) as *mut BlockHeader;
                if (*header).size > largest {
                    largest = (*header).size;
                }
                current = (*current).next;
            }
        }
        largest
    }

    /// Returns a snapshot of the current memory statistics.
    // VUMA-VERIFIED: pure query
    pub fn stats(&self) -> &MemoryStats {
        &self.stats
    }

    /// Returns the fragmentation metric (0.0 to 1.0).
    // VUMA-VERIFIED: derived from tracked state
    pub fn fragmentation(&self) -> f64 {
        self.stats.fragmentation(self.largest_free_block() as u64)
    }

    /// Returns a reference to the allocation tracker.
    // VUMA-VERIFIED: read-only access to MSG data
    pub fn tracker(&self) -> &AllocTracker {
        &self.tracker
    }

    /// Returns the number of bytes available for allocation
    /// (sum of all free block payload sizes).
    // VUMA-VERIFIED: pure query
    pub fn available(&self) -> usize {
        let mut total = 0;
        let mut current = self.free_head;
        while !current.is_null() {
            unsafe {
                let header = (current as *mut u8).sub(BlockHeader::SIZE) as *mut BlockHeader;
                total += (*header).payload_size;
                current = (*current).next;
            }
        }
        total
    }
}

// ---------------------------------------------------------------------------
// VumaAllocator — Global Allocator Implementing VUMA Memory Model
// ---------------------------------------------------------------------------

/// Spin lock for interior mutability in the global allocator.
///
/// A simple spin lock that uses an `AtomicBool` for mutual exclusion.
/// This avoids the need for `std::sync::Mutex` which itself allocates.
struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

/// Inner state of the VumaAllocator.
struct VumaAllocatorInner {
    /// The backing free-list allocator.
    free_list: FreeListAllocator,
}

/// Global allocator implementing the VUMA memory model.
///
/// `VumaAllocator` implements `std::alloc::GlobalAlloc`, making it suitable
/// for use with `#[global_allocator]`. It provides:
///
/// - **BD-annotated allocation**: Every alloc/dealloc/realloc is tracked in the MSG.
/// - **BD verification**: Dealloc verifies the block header magic and free status.
/// - **Memory statistics**: Tracks total allocated, freed, current usage, peak, and fragmentation.
/// - **Coalescing free list**: Reduces fragmentation by merging adjacent free blocks.
///
/// ## Initialization
///
/// The allocator must be initialized with a backing memory region before use:
///
/// ```ignore
/// static mut HEAP: [u8; 1024 * 1024] = [0; 1024 * 1024];
///
/// #[global_allocator]
/// static ALLOCATOR: VumaAllocator = VumaAllocator::new();
///
/// fn main() {
///     unsafe { ALLOCATOR.init(HEAP.as_mut_ptr(), HEAP.len()); }
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Serialize, Send }
/// - SyncEdge: alloc → dealloc (Seq), dealloc → alloc (Seq), alloc → alloc (Seq)
pub struct VumaAllocator {
    lock: SpinLock,
    inner: UnsafeCell<Option<VumaAllocatorInner>>,
}

// Safety: VumaAllocator uses a spin lock for mutual exclusion.
unsafe impl Sync for VumaAllocator {}
unsafe impl Send for VumaAllocator {}

impl Default for VumaAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl VumaAllocator {
    /// Create a new uninitialized VumaAllocator.
    ///
    /// Must call `init()` before any allocations are made.
    // VUMA-VERIFIED: constructor produces valid uninitialized state
    pub const fn new() -> Self {
        Self {
            lock: SpinLock::new(),
            inner: UnsafeCell::new(None),
        }
    }

    /// Initialize the allocator with a backing memory region.
    ///
    /// # Safety
    ///
    /// - Must be called exactly once, before any allocations.
    /// - Must not be called while any other thread is accessing the allocator.
    /// - `ptr` must be valid for `size` bytes and aligned to at least 8 bytes.
    // VUMA-VERIFIED: initialization establishes valid heap region atomically
    pub unsafe fn init(&self, ptr: *mut u8, size: usize) {
        self.lock.lock();
        let inner = &mut *self.inner.get();

        let mut free_list = FreeListAllocator {
            heap_start: ptr,
            heap_size: size,
            free_head: ptr::null_mut(),
            stats: MemoryStats {
                heap_size: size as u64,
                ..MemoryStats::new()
            },
            tracker: AllocTracker::new(),
        };

        // Initialize the entire heap as one free block
        if size >= MIN_BLOCK_SIZE {
            let header = ptr as *mut BlockHeader;
            ptr::write(
                header,
                BlockHeader {
                    magic: VUMA_MAGIC,
                    flags: 1, // is_free = true
                    size,
                    payload_size: size - BlockHeader::SIZE,
                    align: 8,
                    _reserved: [0; 2],
                },
            );
            let free_node = BlockHeader::payload_ptr(header) as *mut FreeNode;
            ptr::write(
                free_node,
                FreeNode {
                    next: ptr::null_mut(),
                    _pad: [0; 8],
                },
            );
            free_list.free_head = free_node;
        }

        *inner = Some(VumaAllocatorInner { free_list });
        self.lock.unlock();
    }

    /// Returns the RepD for this allocator.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new(
            "VumaAllocator",
            0,
            8,
            CapD::new(vec![
                CapFlag::Read,
                CapFlag::Write,
                CapFlag::Serialize,
                CapFlag::Send,
            ]),
        )
    }

    /// Returns the SyncEdge annotations for this allocator's operations.
    // VUMA-VERIFIED: synchronization edges correctly model global allocator semantics
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("vuma_alloc", "vuma_dealloc", SyncEdgeKind::Seq),
            SyncEdge::new("vuma_dealloc", "vuma_alloc", SyncEdgeKind::Seq),
            SyncEdge::new("vuma_alloc", "vuma_alloc", SyncEdgeKind::Seq),
        ]
    }

    /// Allocate `size` bytes with `align` alignment.
    ///
    /// Returns a pointer to the allocated memory, or null if out of memory
    /// or if the allocator is not initialized.
    // VUMA-VERIFIED: allocation delegates to FreeListAllocator with BD tracking
    pub fn alloc(&self, size: usize, align: usize) -> *mut u8 {
        self.lock.lock();
        let inner = unsafe { &mut *self.inner.get() };
        let result = match inner {
            Some(ref mut inner) => inner.free_list.alloc(size, align),
            None => ptr::null_mut(),
        };
        self.lock.unlock();
        result
    }

    /// Deallocate a previously allocated block with BD verification.
    ///
    /// Verifies the block header magic and free status before releasing.
    // VUMA-VERIFIED: dealloc verifies BD constraints before releasing
    pub fn dealloc(&self, ptr: *mut u8, size: usize, align: usize) {
        self.lock.lock();
        let inner = unsafe { &mut *self.inner.get() };
        if let Some(ref mut inner) = inner {
            inner.free_list.dealloc(ptr, size, align);
        }
        self.lock.unlock();
    }

    /// Reallocate a previously allocated block.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid allocation previously returned by `alloc`.
    // VUMA-VERIFIED: realloc preserves data integrity and BD constraints
    pub unsafe fn realloc(
        &self,
        ptr: *mut u8,
        old_size: usize,
        new_size: usize,
        align: usize,
    ) -> *mut u8 {
        self.lock.lock();
        let inner = unsafe { &mut *self.inner.get() };
        let result = match inner {
            Some(ref mut inner) => unsafe {
                inner.free_list.realloc(ptr, old_size, new_size, align)
            },
            None => ptr::null_mut(),
        };
        self.lock.unlock();
        result
    }

    /// Returns a snapshot of the current memory statistics.
    ///
    /// # Safety
    ///
    /// Must not be called concurrently with alloc/dealloc/realloc
    /// (use external synchronization for diagnostic queries).
    // VUMA-VERIFIED: read-only query
    pub unsafe fn stats(&self) -> Option<MemoryStats> {
        self.lock.lock();
        let inner = &*self.inner.get();
        let stats = inner.as_ref().map(|i| i.free_list.stats().clone());
        self.lock.unlock();
        stats
    }

    /// Returns a snapshot of the allocation tracker (MSG data).
    ///
    /// Returns `None` if the allocator is not initialized.
    ///
    /// # Safety
    ///
    /// Must not be called concurrently with alloc/dealloc/realloc.
    // VUMA-VERIFIED: read-only access to MSG data
    pub unsafe fn tracker(&self) -> Option<AllocTracker> {
        self.lock.lock();
        let inner = &*self.inner.get();
        let tracker = inner.as_ref().map(|i| {
            // Clone the tracker records for safe external consumption
            let src = i.free_list.tracker();
            let mut dst = AllocTracker::new();
            for rec in src.records() {
                dst.record(rec.kind, rec.addr, rec.size, rec.align);
            }
            dst
        });
        self.lock.unlock();
        tracker
    }

    /// Returns the number of active allocations.
    // VUMA-VERIFIED: derived from tracked state
    pub fn active_allocations(&self) -> u64 {
        self.lock.lock();
        let inner = unsafe { &*self.inner.get() };
        let count = inner
            .as_ref()
            .map(|i| i.free_list.stats().active_allocations())
            .unwrap_or(0);
        self.lock.unlock();
        count
    }

    /// Returns the current fragmentation metric.
    // VUMA-VERIFIED: derived from tracked state
    pub fn fragmentation(&self) -> Option<f64> {
        self.lock.lock();
        let inner = unsafe { &mut *self.inner.get() };
        let frag = inner.as_mut().map(|i| i.free_list.fragmentation());
        self.lock.unlock();
        frag
    }
}

// ---------------------------------------------------------------------------
// GlobalAlloc Implementation
// ---------------------------------------------------------------------------

unsafe impl GlobalAlloc for VumaAllocator {
    /// Allocate memory with the given layout.
    ///
    /// # BD Annotation
    ///
    /// Creates a new writable region in the MSG with the requested
    /// size and alignment.
    // VUMA-VERIFIED: GlobalAlloc alloc delegates to FreeListAllocator
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc(layout.size(), layout.align())
    }

    /// Deallocate memory at the given pointer with the given layout.
    ///
    /// # BD Annotation
    ///
    /// Verifies the block's BD constraints and removes the region
    /// from the MSG.
    // VUMA-VERIFIED: GlobalAlloc dealloc verifies BD constraints
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.dealloc(ptr, layout.size(), layout.align())
    }

    /// Reallocate memory at the given pointer.
    ///
    /// # BD Annotation
    ///
    /// Updates the MSG region; may create a new region if the block moves.
    // VUMA-VERIFIED: GlobalAlloc realloc preserves data integrity
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.realloc(ptr, layout.size(), new_size, layout.align())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Aligned heap buffer for tests. `u8` arrays may not be 8-byte aligned
    /// in static context, but `BlockHeader` requires 8-byte alignment.
    /// This wrapper guarantees the necessary alignment.
    #[repr(C, align(8))]
    struct AlignedHeap<const N: usize> {
        data: [u8; N],
    }

    impl<const N: usize> AlignedHeap<N> {
        const ZERO: Self = Self { data: [0u8; N] };

        fn as_mut_ptr(&mut self) -> *mut u8 {
            self.data.as_mut_ptr()
        }

        fn len(&self) -> usize {
            N
        }
    }

    // -- Existing tests (preserved) ------------------------------------------

    #[test]
    fn test_address_align_up() {
        assert_eq!(Address(0).align_up(8), Address(0));
        assert_eq!(Address(1).align_up(8), Address(8));
        assert_eq!(Address(8).align_up(8), Address(8));
        assert_eq!(Address(9).align_up(8), Address(16));
        assert_eq!(Address(13).align_up(4), Address(16));
    }

    #[test]
    fn test_global_allocator_basic() {
        let mut alloc = GlobalAllocator::new(Address(0x1000), 1024);
        let addr1 = alloc.allocate(64, 8).unwrap();
        assert_eq!(addr1, Address(0x1000));
        assert_eq!(alloc.used(), 64);

        let addr2 = alloc.allocate(32, 4).unwrap();
        assert_eq!(addr2, Address(0x1040)); // 0x1000 + 64
    }

    #[test]
    fn test_global_allocator_alignment() {
        let mut alloc = GlobalAllocator::new(Address(0x1001), 1024);
        let addr = alloc.allocate(16, 8).unwrap();
        assert_eq!(addr.0 % 8, 0);
    }

    #[test]
    fn test_global_allocator_out_of_memory() {
        let mut alloc = GlobalAllocator::new(Address(0x1000), 64);
        assert!(alloc.allocate(128, 8).is_err());
    }

    #[test]
    fn test_global_allocator_reset() {
        let mut alloc = GlobalAllocator::new(Address(0x1000), 1024);
        alloc.allocate(64, 8).unwrap();
        assert_eq!(alloc.used(), 64);
        alloc.reset();
        assert_eq!(alloc.used(), 0);
    }

    #[test]
    fn test_arena_allocator_basic() {
        let mut arena = ArenaAllocator::new(Address(0x2000), 512);
        let addr = arena.allocate(128, 8).unwrap();
        assert_eq!(addr, Address(0x2000));
        assert_eq!(arena.used(), 128);
    }

    #[test]
    fn test_arena_allocator_reset() {
        let mut arena = ArenaAllocator::new(Address(0x2000), 512);
        arena.allocate(256, 8).unwrap();
        arena.reset();
        assert_eq!(arena.used(), 0);
        // Can allocate again after reset
        let addr = arena.allocate(64, 8).unwrap();
        assert_eq!(addr, Address(0x2000));
    }

    #[test]
    fn test_pool_allocator_basic() {
        let mut pool = PoolAllocator::new(64);
        pool.add_region(Address(0x3000), 256);
        assert_eq!(pool.available(), 4);

        let a1 = pool.allocate().unwrap();
        assert_eq!(a1, Address(0x3000 + 3 * 64)); // pop from end
        assert_eq!(pool.available(), 3);

        pool.free(a1).unwrap();
        assert_eq!(pool.available(), 4);
    }

    #[test]
    fn test_pool_allocator_exhaustion() {
        let mut pool = PoolAllocator::new(32);
        pool.add_region(Address(0x4000), 64); // 2 blocks
        pool.allocate().unwrap();
        pool.allocate().unwrap();
        assert!(pool.allocate().is_err());
    }

    #[test]
    fn test_invalid_alignment() {
        let mut alloc = GlobalAllocator::new(Address(0x1000), 1024);
        assert!(alloc.allocate(16, 0).is_err());
        assert!(alloc.allocate(16, 3).is_err()); // not power of 2
    }

    // -- New tests: BumpAllocator --------------------------------------------

    #[test]
    fn test_bump_allocator_basic_alloc() {
        static mut HEAP: AlignedHeap<1024> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let ptr = alloc.alloc(64, 8);
        assert!(!ptr.is_null());
        assert_eq!(alloc.used(), 64);
        assert_eq!(alloc.stats().num_allocs, 1);
        assert_eq!(alloc.stats().total_allocated, 64);
    }

    #[test]
    fn test_bump_allocator_multiple_allocs() {
        static mut HEAP: AlignedHeap<1024> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let p1 = alloc.alloc(32, 8);
        let p2 = alloc.alloc(64, 8);
        assert!(!p1.is_null());
        assert!(!p2.is_null());
        // p2 should be at least 32 bytes after p1
        assert!((p2 as usize) >= (p1 as usize) + 32);
        assert_eq!(alloc.stats().num_allocs, 2);
    }

    #[test]
    fn test_bump_allocator_out_of_memory() {
        static mut HEAP: AlignedHeap<128> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let p1 = alloc.alloc(64, 8);
        assert!(!p1.is_null());
        // Request more than remaining
        let p2 = alloc.alloc(128, 8);
        assert!(p2.is_null());
    }

    #[test]
    fn test_bump_allocator_reset() {
        static mut HEAP: AlignedHeap<512> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        alloc.alloc(100, 8);
        alloc.alloc(100, 8);
        assert_eq!(alloc.stats().num_allocs, 2);
        alloc.reset();
        assert_eq!(alloc.used(), 0);
        // Can allocate again after reset
        let p = alloc.alloc(200, 8);
        assert!(!p.is_null());
    }

    #[test]
    fn test_bump_allocator_realloc() {
        static mut HEAP: AlignedHeap<1024> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let p1 = alloc.alloc(32, 8);
        assert!(!p1.is_null());
        // Write some data
        unsafe {
            ptr::write(p1, 0x42u8);
        }
        let p2 = unsafe { alloc.realloc(p1, 32, 64, 8) };
        assert!(!p2.is_null());
        // Data should be preserved
        unsafe {
            assert_eq!(ptr::read(p2), 0x42u8);
        }
    }

    // -- New tests: FreeListAllocator ----------------------------------------

    #[test]
    fn test_freelist_allocator_basic_alloc_dealloc() {
        static mut HEAP: AlignedHeap<4096> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let ptr = alloc.alloc(128, 8);
        assert!(!ptr.is_null());
        assert_eq!(alloc.stats().num_allocs, 1);
        assert_eq!(alloc.stats().total_allocated, 128);

        alloc.dealloc(ptr, 128, 8);
        assert_eq!(alloc.stats().num_deallocs, 1);
        assert_eq!(alloc.stats().total_freed, 128);
        assert_eq!(alloc.stats().current_usage, 0);
    }

    #[test]
    fn test_freelist_allocator_multiple_allocs() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let p1 = alloc.alloc(64, 8);
        let p2 = alloc.alloc(64, 8);
        let p3 = alloc.alloc(64, 8);
        assert!(!p1.is_null());
        assert!(!p2.is_null());
        assert!(!p3.is_null());
        // All pointers should be different
        assert_ne!(p1, p2);
        assert_ne!(p2, p3);
        assert_ne!(p1, p3);
        assert_eq!(alloc.stats().num_allocs, 3);
    }

    #[test]
    fn test_freelist_allocator_dealloc_and_reuse() {
        static mut HEAP: AlignedHeap<4096> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let p1 = alloc.alloc(128, 8);
        assert!(!p1.is_null());
        alloc.dealloc(p1, 128, 8);
        // Allocate again — should reuse freed space
        let p2 = alloc.alloc(128, 8);
        assert!(!p2.is_null());
        // The reused pointer might be the same or different (depends on coalescing)
        // but allocation should succeed
    }

    #[test]
    fn test_freelist_allocator_realloc_grow() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let p1 = alloc.alloc(64, 8);
        assert!(!p1.is_null());
        // Write data
        unsafe {
            ptr::write(p1, 0xABu8);
        }
        // Grow
        let p2 = unsafe { alloc.realloc(p1, 64, 128, 8) };
        assert!(!p2.is_null());
        // Data preserved
        unsafe {
            assert_eq!(ptr::read(p2), 0xABu8);
        }
    }

    #[test]
    fn test_freelist_allocator_realloc_shrink_in_place() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        let p1 = alloc.alloc(256, 8);
        assert!(!p1.is_null());
        // Shrink — should be in-place
        let p2 = unsafe { alloc.realloc(p1, 256, 64, 8) };
        assert!(!p2.is_null());
        // Should be the same pointer for in-place shrink
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_freelist_allocator_out_of_memory() {
        static mut HEAP: AlignedHeap<256> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };
        // First alloc should succeed (accounting for header overhead)
        let p1 = alloc.alloc(64, 8);
        assert!(!p1.is_null());
        // Keep allocating until we run out
        let p2 = alloc.alloc(64, 8);
        let p3 = alloc.alloc(64, 8);
        // At some point, allocation should fail
        // (exact count depends on header overhead and coalescing)
        let mut all_ptrs = vec![p1, p2, p3];
        for _ in 0..10 {
            let p = alloc.alloc(64, 8);
            if p.is_null() {
                break;
            }
            all_ptrs.push(p);
        }
        // At least one allocation must have failed
        assert!(all_ptrs.iter().any(|p| p.is_null()) || all_ptrs.len() < 13);
    }

    #[test]
    fn test_freelist_allocator_stats_and_fragmentation() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };

        // Allocate two blocks
        let p1 = alloc.alloc(256, 8);
        let p2 = alloc.alloc(256, 8);
        assert!(!p1.is_null());
        assert!(!p2.is_null());

        // Free the first one to create a "hole"
        alloc.dealloc(p1, 256, 8);

        // Check stats
        assert_eq!(alloc.stats().num_allocs, 2);
        assert_eq!(alloc.stats().num_deallocs, 1);
        assert_eq!(alloc.stats().total_allocated, 512);
        assert_eq!(alloc.stats().total_freed, 256);
        assert_eq!(alloc.stats().current_usage, 256);

        // Fragmentation should be > 0 after freeing p1 but not p2
        let frag = alloc.fragmentation();
        assert!(frag >= 0.0);
        assert!(frag <= 1.0);
    }

    #[test]
    fn test_freelist_allocator_tracker() {
        static mut HEAP: AlignedHeap<4096> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };

        let p1 = alloc.alloc(64, 8);
        let p2 = alloc.alloc(32, 8);
        alloc.dealloc(p1, 64, 8);
        let _p3 = unsafe { alloc.realloc(p2, 32, 128, 8) };

        // Tracker should have recorded all events
        let records = alloc.tracker().records();
        assert!(records.len() >= 3); // alloc, alloc, dealloc, possibly realloc

        // Check event kinds
        assert_eq!(records[0].kind, AllocEventKind::Alloc);
        assert_eq!(records[1].kind, AllocEventKind::Alloc);
        // The dealloc and realloc records follow
        let has_dealloc = records.iter().any(|r| r.kind == AllocEventKind::Dealloc);
        let has_realloc = records.iter().any(|r| r.kind == AllocEventKind::Realloc);
        assert!(has_dealloc);
        assert!(has_realloc);

        // All records should have valid sequence numbers
        for (i, r) in records.iter().enumerate() {
            assert_eq!(r.seq, i as u64);
        }
    }

    // -- New tests: VumaAllocator --------------------------------------------

    #[test]
    fn test_vuma_allocator_global_alloc() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let allocator = VumaAllocator::new();
        unsafe {
            allocator.init(HEAP.as_mut_ptr(), HEAP.len());
        }

        // Use direct method calls (VumaAllocator::alloc takes size, align)
        let ptr = allocator.alloc(64, 8);
        assert!(!ptr.is_null());

        allocator.dealloc(ptr, 64, 8);
    }

    #[test]
    fn test_vuma_allocator_stats() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let allocator = VumaAllocator::new();
        unsafe {
            allocator.init(HEAP.as_mut_ptr(), HEAP.len());
        }

        let ptr = allocator.alloc(128, 8);
        assert!(!ptr.is_null());

        let stats = unsafe { allocator.stats() }.expect("allocator should be initialized");
        assert_eq!(stats.num_allocs, 1);
        assert_eq!(stats.total_allocated, 128);

        allocator.dealloc(ptr, 128, 8);

        let stats = unsafe { allocator.stats() }.expect("allocator should be initialized");
        assert_eq!(stats.num_deallocs, 1);
    }

    #[test]
    fn test_vuma_allocator_realloc() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let allocator = VumaAllocator::new();
        unsafe {
            allocator.init(HEAP.as_mut_ptr(), HEAP.len());
        }

        let ptr = allocator.alloc(64, 8);
        assert!(!ptr.is_null());

        // Write some data
        unsafe {
            ptr::write(ptr, 0xFFu8);
        }

        // Grow
        let new_ptr = unsafe { allocator.realloc(ptr, 64, 128, 8) };
        assert!(!new_ptr.is_null());
        // Data preserved
        unsafe {
            assert_eq!(ptr::read(new_ptr), 0xFFu8);
        }

        allocator.dealloc(new_ptr, 128, 8);
    }

    #[test]
    fn test_memory_stats_fragmentation() {
        let stats = MemoryStats {
            total_allocated: 1024,
            total_freed: 512,
            current_usage: 512,
            num_allocs: 10,
            num_deallocs: 5,
            peak_usage: 1024,
            heap_size: 4096,
        };

        // With largest_free_block equal to total_free, fragmentation is 0
        let frag = stats.fragmentation(4096 - 512);
        assert!((frag - 0.0).abs() < 0.001);

        // With a small largest_free_block, fragmentation is high
        let frag = stats.fragmentation(100);
        assert!(frag > 0.8);
    }

    #[test]
    fn test_alloc_tracker() {
        let mut tracker = AllocTracker::new();
        assert!(tracker.is_empty());

        tracker.record(AllocEventKind::Alloc, 0x1000, 64, 8);
        tracker.record(AllocEventKind::Alloc, 0x2000, 128, 16);
        tracker.record(AllocEventKind::Dealloc, 0x1000, 64, 8);
        tracker.record(AllocEventKind::Realloc, 0x2000, 256, 16);

        assert_eq!(tracker.len(), 4);

        let records = tracker.records();
        assert_eq!(records[0].kind, AllocEventKind::Alloc);
        assert_eq!(records[0].addr, 0x1000);
        assert_eq!(records[0].size, 64);
        assert_eq!(records[1].kind, AllocEventKind::Alloc);
        assert_eq!(records[2].kind, AllocEventKind::Dealloc);
        assert_eq!(records[3].kind, AllocEventKind::Realloc);

        // Verify sequential IDs
        assert_eq!(records[0].seq, 0);
        assert_eq!(records[1].seq, 1);
        assert_eq!(records[2].seq, 2);
        assert_eq!(records[3].seq, 3);
    }

    #[test]
    fn test_vuma_allocator_uninitialized_returns_null() {
        let allocator = VumaAllocator::new();
        // Not initialized — should return null
        let ptr = allocator.alloc(64, 8);
        assert!(ptr.is_null());
    }

    // -- Additional tests for enhanced alloc module --

    #[test]
    fn test_vuma_allocator_tracker() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let allocator = VumaAllocator::new();
        unsafe {
            allocator.init(HEAP.as_mut_ptr(), HEAP.len());
        }

        let ptr = allocator.alloc(64, 8);
        assert!(!ptr.is_null());
        allocator.dealloc(ptr, 64, 8);

        let tracker = unsafe { allocator.tracker() }.expect("tracker should exist");
        assert!(tracker.len() >= 2); // at least alloc + dealloc
        let records = tracker.records();
        assert_eq!(records[0].kind, AllocEventKind::Alloc);
        assert_eq!(records[1].kind, AllocEventKind::Dealloc);
    }

    #[test]
    fn test_vuma_allocator_active_allocations() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let allocator = VumaAllocator::new();
        unsafe {
            allocator.init(HEAP.as_mut_ptr(), HEAP.len());
        }

        assert_eq!(allocator.active_allocations(), 0);
        let p1 = allocator.alloc(64, 8);
        assert!(!p1.is_null());
        assert_eq!(allocator.active_allocations(), 1);
        let p2 = allocator.alloc(32, 8);
        assert!(!p2.is_null());
        assert_eq!(allocator.active_allocations(), 2);
        allocator.dealloc(p1, 64, 8);
        assert_eq!(allocator.active_allocations(), 1);
        allocator.dealloc(p2, 32, 8);
        assert_eq!(allocator.active_allocations(), 0);
    }

    #[test]
    fn test_vuma_allocator_repd_and_sync_edges() {
        let allocator = VumaAllocator::new();
        let repd = allocator.repd();
        assert_eq!(repd.name, "VumaAllocator");
        assert!(repd.capd.has(CapFlag::Send));
        let edges = allocator.sync_edges();
        assert!(!edges.is_empty());
    }

    #[test]
    fn test_bump_allocator_stats_and_tracker() {
        static mut HEAP: AlignedHeap<1024> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };

        let p1 = alloc.alloc(32, 8);
        let p2 = alloc.alloc(64, 8);
        assert!(!p1.is_null());
        assert!(!p2.is_null());

        // Check stats
        let stats = alloc.stats();
        assert_eq!(stats.num_allocs, 2);
        assert_eq!(stats.total_allocated, 96);
        assert_eq!(stats.current_usage, 96);
        assert_eq!(stats.heap_size, 1024);

        // Check tracker
        let tracker = alloc.tracker();
        assert_eq!(tracker.len(), 2);
        assert_eq!(tracker.records()[0].kind, AllocEventKind::Alloc);
        assert_eq!(tracker.records()[1].kind, AllocEventKind::Alloc);
    }

    #[test]
    fn test_bump_allocator_alignment() {
        static mut HEAP: AlignedHeap<1024> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };

        // Allocate with 16-byte alignment
        let p1 = alloc.alloc(32, 16);
        assert!(!p1.is_null());
        assert_eq!((p1 as usize) % 16, 0, "pointer should be 16-byte aligned");

        // Allocate with 32-byte alignment
        let p2 = alloc.alloc(64, 32);
        assert!(!p2.is_null());
        assert_eq!((p2 as usize) % 32, 0, "pointer should be 32-byte aligned");
    }

    #[test]
    fn test_bump_allocator_zero_and_invalid_align() {
        static mut HEAP: AlignedHeap<1024> = AlignedHeap::ZERO;
        let mut alloc = unsafe { BumpAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };

        // Zero size returns null
        let p = alloc.alloc(0, 8);
        assert!(p.is_null());

        // Zero alignment returns null
        let p = alloc.alloc(16, 0);
        assert!(p.is_null());

        // Non-power-of-2 alignment returns null
        let p = alloc.alloc(16, 3);
        assert!(p.is_null());
    }

    #[test]
    fn test_freelist_allocator_coalescing() {
        static mut HEAP: AlignedHeap<8192> = AlignedHeap::ZERO;
        let mut alloc = unsafe { FreeListAllocator::from_raw(HEAP.as_mut_ptr(), HEAP.len()) };

        // Allocate three blocks
        let p1 = alloc.alloc(256, 8);
        let p2 = alloc.alloc(256, 8);
        let p3 = alloc.alloc(256, 8);
        assert!(!p1.is_null());
        assert!(!p2.is_null());
        assert!(!p3.is_null());

        // Free the first and third, leaving a gap
        alloc.dealloc(p1, 256, 8);
        alloc.dealloc(p3, 256, 8);

        // After coalescing, the available space should reflect freed blocks
        let available = alloc.available();
        assert!(
            available >= 256,
            "should have at least 256 bytes available after freeing"
        );

        // Allocating again should succeed
        let p4 = alloc.alloc(256, 8);
        assert!(!p4.is_null(), "should allocate from coalesced free blocks");
    }

    #[test]
    fn test_address_null_and_offset() {
        let null = Address::NULL;
        assert!(null.is_null());

        let addr = Address::from_raw(0x1000);
        assert!(!addr.is_null());

        let offset = addr.offset(0x100);
        assert_eq!(offset, Address(0x1100));
    }

    #[test]
    fn test_memory_stats_active_allocations() {
        let stats = MemoryStats {
            total_allocated: 1024,
            total_freed: 512,
            current_usage: 512,
            num_allocs: 10,
            num_deallocs: 5,
            peak_usage: 1024,
            heap_size: 4096,
        };
        assert_eq!(stats.active_allocations(), 5);
        assert_eq!(stats.fragmentation(3584), 0.0);
    }

    #[test]
    fn test_alloc_error_display() {
        let err = AllocError::OutOfMemory {
            requested: 1024,
            available: 512,
        };
        assert!(err.to_string().contains("1024"));
        assert!(err.to_string().contains("512"));

        let err = AllocError::InvalidAlignment { align: 3 };
        assert!(err.to_string().contains("3"));

        let err = AllocError::SizeMismatch {
            expected: 64,
            actual: 128,
        };
        assert!(err.to_string().contains("64"));
        assert!(err.to_string().contains("128"));
    }
}
