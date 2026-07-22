//! Arena allocator — Rust-level runtime for testing and callback path.
//!
//! This module provides a Rust-level arena allocator that mirrors the
//! codegen-lowered arena builtins (arena_new/arena_alloc/arena_grow/arena_free).
//! It's used for:
//!   - Unit testing the arena model
//!   - The vuma_context callback path (Wave 7)
//!
//! The arena is a bump allocator backed by mmap. No per-object malloc/free.
//! arena_alloc bumps an offset; arena_free unmaps the whole region.

use std::alloc::{self, Layout};

/// An arena allocator backed by a single mmap'd region.
pub struct Arena {
    /// Base pointer of the mmap'd region.
    base: *mut u8,
    /// Current bump-alloc offset.
    offset: usize,
    /// Total capacity in bytes.
    capacity: usize,
}

// SAFETY: Arena is only used from the thread that created it. The bump
// pointer is not shared across threads.
unsafe impl Send for Arena {}

impl Arena {
    /// Create a new arena with the given initial capacity (bytes).
    /// Uses the system allocator (mmap under the hood on most platforms).
    pub fn create(capacity: usize) -> Self {
        let layout = Layout::from_size_align(capacity, 8).expect("invalid arena layout");
        let base = unsafe { alloc::alloc(layout) };
        if base.is_null() {
            panic!("arena_create: allocation failed for {} bytes", capacity);
        }
        Arena {
            base,
            offset: 0,
            capacity,
        }
    }

    /// Bump-allocate `size` bytes within the arena. Returns a pointer to
    /// the allocated region. Panics if the arena is full (overflow).
    pub fn alloc_raw(&mut self, size: usize) -> *mut u8 {
        let aligned_size = (size + 7) & !7; // align to 8 bytes
        let new_offset = self.offset + aligned_size;
        if new_offset > self.capacity {
            panic!(
                "arena_alloc: overflow (offset={}, size={}, capacity={})",
                self.offset, aligned_size, self.capacity
            );
        }
        let ptr = unsafe { self.base.add(self.offset) };
        self.offset = new_offset;
        ptr
    }

    /// Bump-allocate a typed value within the arena. Returns a typed pointer.
    /// The caller is responsible for initializing the memory.
    pub fn alloc<T>(&mut self) -> *mut T {
        self.alloc_raw(std::mem::size_of::<T>()) as *mut T
    }

    /// Grow the arena to at least `min_capacity` bytes. Uses realloc.
    /// Existing allocations remain valid (realloc preserves contents).
    pub fn grow(&mut self, min_capacity: usize) {
        if min_capacity <= self.capacity {
            return;
        }
        let old_layout = Layout::from_size_align(self.capacity, 8).expect("invalid old layout");
        let new_base = unsafe { alloc::realloc(self.base, old_layout, min_capacity) };
        if new_base.is_null() {
            panic!("arena_grow: realloc failed for {} bytes", min_capacity);
        }
        self.base = new_base;
        self.capacity = min_capacity;
    }

    /// Destroy the arena, unmapping all memory. No per-object free.
    /// Consumes self to prevent double-free with Drop.
    pub fn destroy(self) {
        let layout = Layout::from_size_align(self.capacity, 8).expect("invalid layout");
        unsafe { alloc::dealloc(self.base, layout) }
        // Prevent Drop from running (which would double-free)
        std::mem::forget(self);
    }

    /// Returns the current offset (bytes used).
    pub fn used(&self) -> usize {
        self.offset
    }

    /// Returns the total capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the base pointer.
    pub fn base(&self) -> *mut u8 {
        self.base
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.capacity, 8).expect("invalid layout");
        unsafe { alloc::dealloc(self.base, layout) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_create() {
        let arena = Arena::create(4096);
        assert_eq!(arena.capacity(), 4096);
        assert_eq!(arena.used(), 0);
    }

    #[test]
    fn test_arena_alloc() {
        let mut arena = Arena::create(4096);
        let ptr: *mut u64 = arena.alloc::<u64>();
        assert!(!ptr.is_null());
        unsafe { *ptr = 42; }
        assert_eq!(unsafe { *ptr }, 42);
        assert_eq!(arena.used(), 8);
    }

    #[test]
    fn test_arena_multiple_alloc() {
        let mut arena = Arena::create(4096);
        let p1: *mut u64 = arena.alloc::<u64>();
        let p2: *mut u64 = arena.alloc::<u64>();
        let p3: *mut u64 = arena.alloc::<u64>();
        unsafe {
            *p1 = 10;
            *p2 = 20;
            *p3 = 30;
            assert_eq!(*p1, 10);
            assert_eq!(*p2, 20);
            assert_eq!(*p3, 30);
        }
        assert_eq!(arena.used(), 24); // 3 × 8 bytes
    }

    #[test]
    fn test_arena_grow() {
        let mut arena = Arena::create(64);
        // Fill the arena
        for _ in 0..7 {
            arena.alloc::<u64>();
        }
        assert_eq!(arena.used(), 56);
        // Grow to 4096
        arena.grow(4096);
        assert_eq!(arena.capacity(), 4096);
        // Allocate more
        let p: *mut u64 = arena.alloc::<u64>();
        unsafe { *p = 99; }
        assert_eq!(unsafe { *p }, 99);
    }

    #[test]
    #[should_panic(expected = "overflow")]
    fn test_arena_overflow() {
        let mut arena = Arena::create(8);
        // Allocate 16 bytes — should panic (overflow)
        arena.alloc_raw(16);
    }

    #[test]
    fn test_arena_destroy() {
        let arena = Arena::create(4096);
        arena.destroy(); // Should not panic
    }
}
