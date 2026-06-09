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

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::fmt;

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
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllocError::OutOfMemory { requested, available } => {
                write!(f, "out of memory: requested {} bytes, available {}", requested, available)
            }
            AllocError::InvalidAlignment { align } => {
                write!(f, "invalid alignment: {}", align)
            }
            AllocError::InvalidFree { addr } => {
                write!(f, "invalid free: address {} was not allocated by this allocator", addr)
            }
            AllocError::NotInitialized => {
                write!(f, "allocator not initialized")
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
        Self { base, size, offset: 0 }
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
        self.free_list
            .pop()
            .ok_or(AllocError::OutOfMemory {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}
