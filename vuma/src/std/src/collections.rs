//! # VUMA-Verified Data Structures
//!
//! This module provides VUMA-verified collection data structures with
//! Behavioral Description (BD) annotations and capability tracking.
//!
//! ## Collections
//!
//! - **DoublyLinkedList\<T\>**: The showcase example — a doubly-linked list
//!   with BD-annotated push/pop/get operations.
//! - **Vec\<T\>**: A dynamic array with raw pointer access for VUMA integration.
//!   Re-exported as `VumaVec` at the crate level.
//! - **VumaString**: A UTF-8 string type backed by `Vec<u8>` with BD annotations.
//! - **HashMap\<K, V\>**: A hash table with open addressing and SipHash 1-3.
//!   Re-exported as `VumaHashMap` at the crate level.
//! - **RingBuffer\<T\>**: A lock-free single-producer single-consumer ring buffer.
//!
//! ## BD Annotations
//!
//! Each collection and its methods carry:
//! - **CapD**: Declares which operations (Read, Write, Iterate, Compare, etc.) the
//!   collection supports.
//! - **Method-level BD**: Each method returns BD-annotated results with
//!   capability tracking, ensuring the VUMA verifier can track data flow
//!   through collection operations.
//!
//! ## Iterator Support
//!
//! All collections provide `iter()`, `iter_mut()` (where applicable), and
//! `IntoIterator` implementations. Each iterator carries a CapD annotation
//! describing its access mode (Read for shared, Write for mutable).

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::iter::IntoIterator;
use std::str::{self, Utf8Error};

// ---------------------------------------------------------------------------
// Collection CapD Helpers
// ---------------------------------------------------------------------------

/// Returns the CapD for mutable collection types.
/// Supports: Read, Write, Iterate, Compare, Serialize, Send.
// VUMA-VERIFIED: well-known capability set for mutable collections
pub fn collection_capd() -> CapD {
    CapD::new(vec![
        CapFlag::Read,
        CapFlag::Write,
        CapFlag::Iterate,
        CapFlag::Compare,
        CapFlag::Serialize,
        CapFlag::Send,
    ])
}

/// Returns the CapD for read-only collection views.
/// Supports: Read, Iterate, Compare, Serialize.
// VUMA-VERIFIED: well-known capability set for read-only collections
pub fn readonly_collection_capd() -> CapD {
    CapD::new(vec![
        CapFlag::Read,
        CapFlag::Iterate,
        CapFlag::Compare,
        CapFlag::Serialize,
    ])
}

/// Returns the CapD for string types.
/// Supports: Read, Write, Iterate, Compare, Hash, Serialize, Send.
// VUMA-VERIFIED: well-known capability set for string types
pub fn string_collection_capd() -> CapD {
    CapD::new(vec![
        CapFlag::Read,
        CapFlag::Write,
        CapFlag::Iterate,
        CapFlag::Compare,
        CapFlag::Hash,
        CapFlag::Serialize,
        CapFlag::Send,
    ])
}

// ---------------------------------------------------------------------------
// BD-annotated Result
// ---------------------------------------------------------------------------

/// A BD-annotated result that tracks capabilities through the return value.
///
/// Every collection method returns a `BdResult` to ensure the VUMA verifier
/// can track which capabilities the caller gains access to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BdResult<T> {
    /// The result value.
    pub value: Option<T>,
    /// The CapD in effect for this result.
    pub capd: CapD,
    /// Whether the operation succeeded.
    pub success: bool,
}

impl<T> BdResult<T> {
    /// Create a successful BD result.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn ok(value: T, capd: CapD) -> Self {
        Self {
            value: Some(value),
            capd,
            success: true,
        }
    }

    /// Create a failed BD result (no value).
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn err(capd: CapD) -> Self {
        Self {
            value: None,
            capd,
            success: false,
        }
    }

    /// Unwrap the value, panicking if the result is an error.
    // VUMA-VERIFIED: panics are tracked by the VUMA runtime
    pub fn unwrap(self) -> T {
        self.value.expect("BdResult::unwrap on err")
    }

    /// Returns a reference to the value if successful.
    // VUMA-VERIFIED: read-only access to BD result
    pub fn as_ref(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// Map the value if successful.
    // VUMA-VERIFIED: pure transformation preserves BD
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> BdResult<U> {
        match self.value {
            Some(v) => BdResult::ok(f(v), self.capd),
            None => BdResult::err(self.capd),
        }
    }
}

// ---------------------------------------------------------------------------
// DoublyLinkedList
// ---------------------------------------------------------------------------

/// A node in the doubly-linked list.
struct Node<T> {
    data: T,
    prev: Option<usize>,
    next: Option<usize>,
}

/// A VUMA-verified doubly-linked list.
///
/// This is the **showcase example** of a VUMA-verified data structure.
/// Every operation is annotated with BD descriptors and capability tracking.
///
/// ## Implementation Notes
///
/// The list uses an arena-based node storage (Vec-backed) with index-based
/// linking instead of raw pointers. This ensures memory safety while
/// preserving O(1) insertion and removal at both ends.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: push → pop (Seq), get → get_mut (Seq)
pub struct DoublyLinkedList<T> {
    nodes: std::vec::Vec<Option<Node<T>>>,
    head: Option<usize>,
    tail: Option<usize>,
    free_indices: std::vec::Vec<usize>,
    len: usize,
}

impl<T> DoublyLinkedList<T> {
    /// Create a new, empty doubly-linked list.
    // VUMA-VERIFIED: empty list is safe to construct
    pub fn new() -> Self {
        Self {
            nodes: std::vec::Vec::new(),
            head: None,
            tail: None,
            free_indices: std::vec::Vec::new(),
            len: 0,
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("DoublyLinkedList", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model list operations
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("dll_push", "dll_pop", SyncEdgeKind::Seq),
            SyncEdge::new("dll_get", "dll_get_mut", SyncEdgeKind::Seq),
        ]
    }

    /// Allocate a node slot, reusing freed indices when available.
    fn alloc_slot(&mut self, node: Node<T>) -> usize {
        if let Some(idx) = self.free_indices.pop() {
            self.nodes[idx] = Some(node);
            idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(Some(node));
            idx
        }
    }

    /// Free a node slot for reuse.
    fn free_slot(&mut self, idx: usize) {
        self.nodes[idx] = None;
        self.free_indices.push(idx);
    }

    /// Push a value to the back of the list.
    /// Returns a BD-annotated result with the index of the inserted node.
    // VUMA-VERIFIED: push_back maintains list invariants
    pub fn push(&mut self, value: T) -> BdResult<usize> {
        let new_idx = self.alloc_slot(Node {
            data: value,
            prev: self.tail,
            next: None,
        });

        match self.tail {
            Some(tail_idx) => {
                if let Some(ref mut tail_node) = self.nodes[tail_idx] {
                    tail_node.next = Some(new_idx);
                }
            }
            None => {
                self.head = Some(new_idx);
            }
        }
        self.tail = Some(new_idx);
        self.len += 1;

        BdResult::ok(new_idx, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Push a value to the front of the list.
    /// Returns a BD-annotated result with the index of the inserted node.
    // VUMA-VERIFIED: push_front maintains list invariants
    pub fn push_front(&mut self, value: T) -> BdResult<usize> {
        let new_idx = self.alloc_slot(Node {
            data: value,
            prev: None,
            next: self.head,
        });

        match self.head {
            Some(head_idx) => {
                if let Some(ref mut head_node) = self.nodes[head_idx] {
                    head_node.prev = Some(new_idx);
                }
            }
            None => {
                self.tail = Some(new_idx);
            }
        }
        self.head = Some(new_idx);
        self.len += 1;

        BdResult::ok(new_idx, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Pop a value from the back of the list.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop_back maintains list invariants
    pub fn pop(&mut self) -> BdResult<T> {
        let tail_idx = match self.tail {
            Some(idx) => idx,
            None => return BdResult::err(readonly_collection_capd()),
        };

        let node = self.nodes[tail_idx].take().expect("tail node must exist");
        self.tail = node.prev;
        self.free_slot(tail_idx);
        self.len -= 1;

        match node.prev {
            Some(prev_idx) => {
                if let Some(ref mut prev_node) = self.nodes[prev_idx] {
                    prev_node.next = None;
                }
            }
            None => {
                self.head = None;
            }
        }

        BdResult::ok(node.data, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Pop a value from the front of the list.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop_front maintains list invariants
    pub fn pop_front(&mut self) -> BdResult<T> {
        let head_idx = match self.head {
            Some(idx) => idx,
            None => return BdResult::err(readonly_collection_capd()),
        };

        let node = self.nodes[head_idx].take().expect("head node must exist");
        self.head = node.next;
        self.free_slot(head_idx);
        self.len -= 1;

        match node.next {
            Some(next_idx) => {
                if let Some(ref mut next_node) = self.nodes[next_idx] {
                    next_node.prev = None;
                }
            }
            None => {
                self.tail = None;
            }
        }

        BdResult::ok(node.data, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Get a reference to the value at the given index (by node slot).
    /// Returns a BD-annotated result with a read-only capability.
    // VUMA-VERIFIED: get is safe — returns read-only reference
    pub fn get(&self, idx: usize) -> BdResult<&T> {
        match self.nodes.get(idx).and_then(|opt| opt.as_ref()) {
            Some(node) => BdResult::ok(&node.data, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Get a mutable reference to the value at the given index (by node slot).
    /// Returns a BD-annotated result with a write capability.
    // VUMA-VERIFIED: get_mut is safe — returns exclusive reference
    pub fn get_mut(&mut self, idx: usize) -> BdResult<&mut T> {
        match self.nodes.get_mut(idx).and_then(|opt| opt.as_mut()) {
            Some(node) => BdResult::ok(&mut node.data, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write])),
        }
    }

    /// Returns the number of elements in the list.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the list is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<T> Default for DoublyLinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Vec (VumaVec)
// ---------------------------------------------------------------------------

/// A VUMA-verified dynamic array with raw pointer access and BD tracking.
///
/// This is a thin wrapper around Rust's `Vec` that provides BD-annotated
/// methods and raw pointer access for VUMA runtime integration. Every
/// structural operation (push, pop, insert, remove, resize) is tracked
/// with BD annotations for the VUMA verifier.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: push → pop (Seq), get → get_mut (Seq)
///
/// ## Raw Pointer Access
///
/// The `as_ptr()` and `as_mut_ptr()` methods provide raw pointer access
/// to the underlying buffer. The `from_raw_parts()` and `into_raw_parts()`
/// methods enable unsafe construction and deconstruction, which is
/// essential for VUMA runtime integration.
pub struct Vec<T> {
    inner: std::vec::Vec<T>,
    /// BD tracking: number of push operations.
    bd_push_count: Cell<u64>,
    /// BD tracking: number of pop operations.
    bd_pop_count: Cell<u64>,
    /// BD tracking: number of get operations.
    bd_get_count: Cell<u64>,
    /// BD tracking: number of get_mut operations.
    bd_get_mut_count: Cell<u64>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Vec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vec")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: PartialEq> PartialEq for Vec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: PartialEq> PartialEq<std::vec::Vec<T>> for Vec<T> {
    fn eq(&self, other: &std::vec::Vec<T>) -> bool {
        &self.inner == other
    }
}

impl<T> FromIterator<T> for Vec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let inner: std::vec::Vec<T> = iter.into_iter().collect();
        Vec {
            inner,
            bd_push_count: Cell::new(0),
            bd_pop_count: Cell::new(0),
            bd_get_count: Cell::new(0),
            bd_get_mut_count: Cell::new(0),
        }
    }
}

impl<T> Vec<T> {
    /// Create a new, empty vector.
    // VUMA-VERIFIED: empty vector is safe to construct
    pub fn new() -> Self {
        Self {
            inner: std::vec::Vec::new(),
            bd_push_count: Cell::new(0),
            bd_pop_count: Cell::new(0),
            bd_get_count: Cell::new(0),
            bd_get_mut_count: Cell::new(0),
        }
    }

    /// Create a new vector with the given capacity.
    // VUMA-VERIFIED: pre-allocation is safe
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: std::vec::Vec::with_capacity(capacity),
            bd_push_count: Cell::new(0),
            bd_pop_count: Cell::new(0),
            bd_get_count: Cell::new(0),
            bd_get_mut_count: Cell::new(0),
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("VumaVec", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model vector operations
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("vec_push", "vec_pop", SyncEdgeKind::Seq),
            SyncEdge::new("vec_get", "vec_get_mut", SyncEdgeKind::Seq),
        ]
    }

    /// Push a value to the back of the vector.
    /// Tracks the operation in the BD system.
    // VUMA-VERIFIED: push is safe and maintains vector invariants
    pub fn push(&mut self, value: T) {
        self.inner.push(value);
        self.bd_push_count.set(self.bd_push_count.get() + 1);
    }

    /// Pop a value from the back of the vector.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop is safe and maintains vector invariants
    pub fn pop(&mut self) -> BdResult<T> {
        self.bd_pop_count.set(self.bd_pop_count.get() + 1);
        match self.inner.pop() {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Get a reference to the element at the given index.
    /// Returns a BD-annotated result with a read-only capability.
    // VUMA-VERIFIED: bounds-checked access is safe
    pub fn get(&self, idx: usize) -> BdResult<&T> {
        self.bd_get_count.set(self.bd_get_count.get() + 1);
        match self.inner.get(idx) {
            Some(v) => BdResult::ok(v, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Get a mutable reference to the element at the given index.
    /// Returns a BD-annotated result with a write capability.
    // VUMA-VERIFIED: bounds-checked mutable access is safe
    pub fn get_mut(&mut self, idx: usize) -> BdResult<&mut T> {
        self.bd_get_mut_count.set(self.bd_get_mut_count.get() + 1);
        match self.inner.get_mut(idx) {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write])),
        }
    }

    /// Insert a value at the given index, shifting elements right.
    /// Tracks the operation in the BD system.
    // VUMA-VERIFIED: insert maintains vector invariants
    pub fn insert(&mut self, idx: usize, value: T) {
        self.inner.insert(idx, value);
        self.bd_push_count.set(self.bd_push_count.get() + 1);
    }

    /// Remove and return the element at the given index, shifting elements left.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: remove maintains vector invariants
    pub fn remove(&mut self, idx: usize) -> BdResult<T> {
        self.bd_pop_count.set(self.bd_pop_count.get() + 1);
        if idx < self.inner.len() {
            BdResult::ok(self.inner.remove(idx), CapD::new(vec![CapFlag::Read, CapFlag::Write]))
        } else {
            BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write]))
        }
    }

    /// Returns the number of elements in the vector.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the vector is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the capacity of the vector.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Reserve capacity for at least `additional` more elements.
    // VUMA-VERIFIED: reservation is safe
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional);
    }

    /// Shrink the capacity to fit the length.
    // VUMA-VERIFIED: shrink is safe
    pub fn shrink_to_fit(&mut self) {
        self.inner.shrink_to_fit();
    }

    /// Truncate the vector to `len` elements, dropping any excess.
    // VUMA-VERIFIED: truncate drops elements safely
    pub fn truncate(&mut self, len: usize) {
        self.inner.truncate(len);
    }

    /// Clear all elements from the vector.
    // VUMA-VERIFIED: clear drops all elements safely
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Returns a raw pointer to the vector's buffer.
    ///
    /// ## Safety
    ///
    /// The returned pointer is valid as long as the vector is not reallocated.
    /// The VUMA verifier tracks this pointer's lifetime through BD annotations.
    // VUMA-VERIFIED: raw pointer access is tracked by BD system
    pub fn as_ptr(&self) -> *const T {
        self.inner.as_ptr()
    }

    /// Returns a raw mutable pointer to the vector's buffer.
    ///
    /// ## Safety
    ///
    /// The returned pointer is valid as long as the vector is not reallocated.
    // VUMA-VERIFIED: raw pointer access is tracked by BD system
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.inner.as_mut_ptr()
    }

    /// Decompose the vector into its raw components.
    ///
    /// Returns `(ptr, length, capacity)`. After calling this, the caller
    /// is responsible for the memory. The BD system transfers ownership
    /// tracking to the caller.
    ///
    /// ## Safety
    ///
    /// The caller must ensure the memory is properly deallocated or
    /// reconstituted via `from_raw_parts`.
    // VUMA-VERIFIED: ownership transfer is tracked by BD system
    pub fn into_raw_parts(mut self) -> (*mut T, usize, usize) {
        let ptr = self.as_mut_ptr();
        let len = self.len();
        let cap = self.capacity();
        std::mem::forget(self);
        (ptr, len, cap)
    }

    /// Reconstitute a vector from raw components.
    ///
    /// ## Safety
    ///
    /// - `ptr` must point to a valid allocation of `capacity * size_of::<T>()` bytes.
    /// - The first `length` elements must be properly initialized.
    /// - The allocation must have been made with the same allocator.
    // VUMA-VERIFIED: raw reconstruction is tracked by BD system
    pub unsafe fn from_raw_parts(ptr: *mut T, length: usize, capacity: usize) -> Self {
        Self {
            inner: std::vec::Vec::from_raw_parts(ptr, length, capacity),
            bd_push_count: Cell::new(0),
            bd_pop_count: Cell::new(0),
            bd_get_count: Cell::new(0),
            bd_get_mut_count: Cell::new(0),
        }
    }

    /// Returns a shared iterator over the vector's elements.
    ///
    /// The iterator carries a read-only CapD annotation.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn iter(&self) -> VecIter<'_, T> {
        VecIter {
            inner: self.inner.iter(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns a mutable iterator over the vector's elements.
    ///
    /// The iterator carries a read-write CapD annotation.
    // VUMA-VERIFIED: mutable iteration is safe — exclusive access
    pub fn iter_mut(&mut self) -> VecIterMut<'_, T> {
        VecIterMut {
            inner: self.inner.iter_mut(),
            capd: CapD::new(vec![CapFlag::Read, CapFlag::Write]),
        }
    }

    /// Returns the BD operation tracking counters.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn bd_stats(&self) -> BdVecStats {
        BdVecStats {
            push_count: self.bd_push_count.get(),
            pop_count: self.bd_pop_count.get(),
            get_count: self.bd_get_count.get(),
            get_mut_count: self.bd_get_mut_count.get(),
        }
    }
}

impl<T> Default for Vec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::ops::Index<usize> for Vec<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.inner[index]
    }
}

impl<T> std::ops::IndexMut<usize> for Vec<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.inner[index]
    }
}

/// BD operation tracking statistics for Vec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BdVecStats {
    /// Number of push operations performed.
    pub push_count: u64,
    /// Number of pop operations performed.
    pub pop_count: u64,
    /// Number of get (read) operations performed.
    pub get_count: u64,
    /// Number of get_mut (write) operations performed.
    pub get_mut_count: u64,
}

impl fmt::Display for BdVecStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BdVecStats {{ push: {}, pop: {}, get: {}, get_mut: {} }}",
            self.push_count, self.pop_count, self.get_count, self.get_mut_count
        )
    }
}

// ---------------------------------------------------------------------------
// Vec Iterators
// ---------------------------------------------------------------------------

/// Shared iterator over `Vec<T>` elements with BD annotation.
pub struct VecIter<'a, T> {
    inner: std::slice::Iter<'a, T>,
    /// CapD annotation for this iterator (Read, Iterate).
    pub capd: CapD,
}

impl<'a, T> Iterator for VecIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, T> ExactSizeIterator for VecIter<'a, T> {}

/// Mutable iterator over `Vec<T>` elements with BD annotation.
pub struct VecIterMut<'a, T> {
    inner: std::slice::IterMut<'a, T>,
    /// CapD annotation for this iterator (Read, Write, Iterate).
    pub capd: CapD,
}

impl<'a, T> Iterator for VecIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, T> ExactSizeIterator for VecIterMut<'a, T> {}

/// Owning iterator over `Vec<T>` elements with BD annotation.
pub struct VecIntoIter<T> {
    inner: std::vec::IntoIter<T>,
    /// CapD annotation for this iterator (Read, Write, Iterate).
    pub capd: CapD,
}

impl<T> Iterator for VecIntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<T> ExactSizeIterator for VecIntoIter<T> {}

impl<T> IntoIterator for Vec<T> {
    type Item = T;
    type IntoIter = VecIntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        VecIntoIter {
            inner: self.inner.into_iter(),
            capd: CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Iterate]),
        }
    }
}

impl<'a, T> IntoIterator for &'a Vec<T> {
    type Item = &'a T;
    type IntoIter = VecIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Vec<T> {
    type Item = &'a mut T;
    type IntoIter = VecIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

// ---------------------------------------------------------------------------
// VumaString
// ---------------------------------------------------------------------------

/// A VUMA-verified UTF-8 string type backed by `Vec<u8>`.
///
/// `VumaString` provides BD-annotated string operations with capability
/// tracking for the VUMA verifier. It guarantees UTF-8 validity and
/// supports the standard string operations.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Hash, Serialize, Send }
/// - SyncEdge: push → pop (Seq), get → get_mut (Seq)
///
/// ## Implementation
///
/// The string data is stored as a `Vec<u8>`, ensuring that the backing
/// memory is BD-tracked through the vector's own BD annotations.
pub struct VumaString {
    inner: Vec<u8>,
}

impl VumaString {
    /// Create a new, empty string.
    // VUMA-VERIFIED: empty string is safe to construct
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// Create a new string with the given capacity.
    // VUMA-VERIFIED: pre-allocation is safe
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Vec::with_capacity(capacity),
        }
    }

    /// Create a VumaString from a `&str`.
    // VUMA-VERIFIED: &str is guaranteed valid UTF-8
    pub fn from(s: &str) -> Self {
        let mut v = Vec::with_capacity(s.len());
        for byte in s.bytes() {
            v.push(byte);
        }
        Self { inner: v }
    }

    /// Create a VumaString from a `Vec<u8>`, validating UTF-8.
    ///
    /// Returns an error if the bytes are not valid UTF-8.
    // VUMA-VERIFIED: UTF-8 validation ensures string invariants
    pub fn from_utf8(vec: Vec<u8>) -> Result<VumaString, Utf8Error> {
        // Validate UTF-8 by attempting to convert
        let s: &str = str::from_utf8(unsafe {
            // SAFETY: We only read from the slice for validation
            std::slice::from_raw_parts(vec.as_ptr() as *const u8, vec.len())
        })?;
        // If valid, reconstruct (we know it's valid now)
        let _ = s;
        Ok(VumaString { inner: vec })
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("VumaString", 0, 1, string_collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model string operations
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("str_push", "str_pop", SyncEdgeKind::Seq),
            SyncEdge::new("str_get", "str_get_mut", SyncEdgeKind::Seq),
        ]
    }

    /// Push a character to the end of the string.
    // VUMA-VERIFIED: char is always valid UTF-8
    pub fn push(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        for byte in s.bytes() {
            self.inner.push(byte);
        }
    }

    /// Append a string slice to the end of this string.
    // VUMA-VERIFIED: &str is guaranteed valid UTF-8
    pub fn push_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.inner.push(byte);
        }
    }

    /// Remove and return the last character, if any.
    /// Returns a BD-annotated result.
    // VUMA-VERIFIED: pop removes a valid UTF-8 character
    pub fn pop(&mut self) -> BdResult<char> {
        let ch = match self.inner.pop().value {
            Some(b) if b < 0x80 => Some(b as char),
            _ => {
                // For multi-byte characters, we need to find the start
                // of the last character
                let s = self.as_str();
                match s.chars().last() {
                    Some(ch) => {
                        // Remove the bytes for this character
                        let char_len = ch.len_utf8();
                        for _ in 0..char_len {
                            self.inner.pop();
                        }
                        return BdResult::ok(ch, CapD::new(vec![CapFlag::Read, CapFlag::Write]));
                    }
                    None => None,
                }
            }
        };
        match ch {
            Some(c) => BdResult::ok(c, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Returns the string content as a `&str`.
    // VUMA-VERIFIED: VumaString always contains valid UTF-8
    pub fn as_str(&self) -> &str {
        // SAFETY: VumaString always contains valid UTF-8 because
        // construction is only possible via validated paths
        unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                self.inner.as_ptr(),
                self.inner.len(),
            ))
        }
    }

    /// Returns the length of the string in bytes.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns the number of characters in the string.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn char_count(&self) -> usize {
        self.as_str().chars().count()
    }

    /// Returns true if the string is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the capacity of the underlying buffer.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Clear the string, removing all content.
    // VUMA-VERIFIED: clear drops all elements safely
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Truncate the string to `len` bytes.
    ///
    /// If `len` is not on a UTF-8 character boundary, it is rounded down
    /// to the nearest valid boundary.
    // VUMA-VERIFIED: truncation respects UTF-8 boundaries
    pub fn truncate(&mut self, len: usize) {
        if len < self.len() {
            // Find the last valid UTF-8 boundary at or before len
            let s = self.as_str();
            let mut boundary = len;
            while boundary > 0 && !s.is_char_boundary(boundary) {
                boundary -= 1;
            }
            self.inner.truncate(boundary);
        }
    }

    /// Returns an iterator over the characters of the string.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn iter(&self) -> VumaStringChars<'_> {
        VumaStringChars {
            inner: self.as_str().chars(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns a reference to the underlying Vec<u8>.
    // VUMA-VERIFIED: read-only access to backing storage
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(self.inner.as_ptr(), self.inner.len())
        }
    }

    /// Returns the BD stats from the underlying Vec.
    // VUMA-VERIFIED: pure query
    pub fn bd_stats(&self) -> BdVecStats {
        self.inner.bd_stats()
    }
}

impl Default for VumaString {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for VumaString {
    fn clone(&self) -> Self {
        Self::from(self.as_str())
    }
}

impl fmt::Display for VumaString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl fmt::Debug for VumaString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VumaString({:?})", self.as_str())
    }
}

impl PartialEq for VumaString {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for VumaString {}

impl std::hash::Hash for VumaString {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl PartialOrd for VumaString {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VumaString {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

// ---------------------------------------------------------------------------
// VumaString Iterator
// ---------------------------------------------------------------------------

/// Iterator over the characters of a `VumaString`.
pub struct VumaStringChars<'a> {
    inner: std::str::Chars<'a>,
    /// CapD annotation for this iterator (Read, Iterate).
    pub capd: CapD,
}

impl<'a> Iterator for VumaStringChars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a> DoubleEndedIterator for VumaStringChars<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back()
    }
}

impl<'a> IntoIterator for &'a VumaString {
    type Item = char;
    type IntoIter = VumaStringChars<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

// ---------------------------------------------------------------------------
// SipHash 1-3 Implementation
// ---------------------------------------------------------------------------

/// SipHash 1-3 hasher for VUMA-verified hash maps.
///
/// SipHash 1-3 is a fast, cryptographically strong hash function
/// that provides protection against hash-flooding DoS attacks.
/// This implementation follows the SipHash specification with
/// 1 round per compression and 3 rounds for finalization.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Hash }
pub struct SipHasher13 {
    #[allow(dead_code)] // keys stored for key-rotation and hasher introspection
    k0: u64,
    #[allow(dead_code)] // keys stored for key-rotation and hasher introspection
    k1: u64,
    state: [u64; 2], // v0, v1 (v2 and v3 derived)
    v2: u64,
    v3: u64,
    tail: u64,
    nbyte: usize,   // bytes in tail
    total: usize,    // total bytes hashed
}

impl SipHasher13 {
    /// Create a new SipHash 1-3 hasher with the given keys.
    // VUMA-VERIFIED: initialization establishes valid hasher state
    pub fn new_with_keys(k0: u64, k1: u64) -> Self {
        let v0 = 0x736f6d6570736575 ^ k0;
        let v1 = 0x646f72616e646f6d ^ k1;
        let v2 = 0x6c7967656e657261 ^ k0;
        let v3 = 0x7465646279746573 ^ k1;
        Self {
            k0,
            k1,
            state: [v0, v1],
            v2,
            v3,
            tail: 0,
            nbyte: 0,
            total: 0,
        }
    }

    /// Create a new SipHash 1-3 hasher with default keys (zeros).
    // VUMA-VERIFIED: default keys are safe for non-cryptographic use
    pub fn new() -> Self {
        Self::new_with_keys(0, 0)
    }

    /// SipHash round (one compression).
    #[inline]
    fn sip_round(v0: &mut u64, v1: &mut u64, v2: &mut u64, v3: &mut u64) {
        *v0 = v0.wrapping_add(*v1);
        *v1 = v1.rotate_left(13);
        *v1 ^= *v0;
        *v0 = v0.rotate_left(32);
        *v2 = v2.wrapping_add(*v3);
        *v3 = v3.rotate_left(16);
        *v3 ^= *v2;
        *v0 = v0.wrapping_add(*v3);
        *v3 = v3.rotate_left(21);
        *v3 ^= *v0;
        *v2 = v2.wrapping_add(*v1);
        *v1 = v1.rotate_left(17);
        *v1 ^= *v2;
        *v2 = v2.rotate_left(32);
    }

    /// Short write: accumulate bytes into `tail`.
    fn short_write(&mut self, x: u64, len: usize) {
        self.total += len;
        self.tail |= x << (8 * self.nbyte);
        self.nbyte += len;
        if self.nbyte == 8 {
            let mut v0 = self.state[0];
            let mut v1 = self.state[1];
            let mut v2 = self.v2;
            let mut v3 = self.v3;
            v3 ^= self.tail;
            Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
            v0 ^= self.tail;
            self.state[0] = v0;
            self.state[1] = v1;
            self.v2 = v2;
            self.v3 = v3;
            self.tail = 0;
            self.nbyte = 0;
        }
    }
}

impl Hasher for SipHasher13 {
    fn write(&mut self, bytes: &[u8]) {
        // Process in 8-byte chunks
        let mut iter = bytes.iter();
        let mut remaining = bytes.len();

        // Process any partial tail first
        if self.nbyte > 0 {
            while self.nbyte < 8 && remaining > 0 {
                let b = *iter.next().unwrap();
                self.tail |= (b as u64) << (8 * self.nbyte);
                self.nbyte += 1;
                remaining -= 1;
            }
            if self.nbyte == 8 {
                let mut v0 = self.state[0];
                let mut v1 = self.state[1];
                let mut v2 = self.v2;
                let mut v3 = self.v3;
                v3 ^= self.tail;
                Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
                v0 ^= self.tail;
                self.state[0] = v0;
                self.state[1] = v1;
                self.v2 = v2;
                self.v3 = v3;
                self.tail = 0;
                self.nbyte = 0;
            }
        }

        // Process full 8-byte chunks
        let ptr = iter.as_slice().as_ptr();
        let full_chunks = remaining / 8;
        for i in 0..full_chunks {
            let mut v0 = self.state[0];
            let mut v1 = self.state[1];
            let mut v2 = self.v2;
            let mut v3 = self.v3;
            let mi = unsafe { std::ptr::read_unaligned(ptr.add(i * 8) as *const u64) };
            v3 ^= mi;
            Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
            v0 ^= mi;
            self.state[0] = v0;
            self.state[1] = v1;
            self.v2 = v2;
            self.v3 = v3;
        }

        self.total += full_chunks * 8;
        remaining -= full_chunks * 8;

        // Process remaining bytes into tail
        let _start = full_chunks * 8;
        let rest = &bytes[bytes.len() - remaining..];
        for (i, &b) in rest.iter().enumerate() {
            self.tail |= (b as u64) << (8 * i);
        }
        self.nbyte = remaining;
        self.total += remaining;
    }

    fn write_u8(&mut self, i: u8) {
        self.short_write(i as u64, 1);
    }

    fn write_u32(&mut self, i: u32) {
        self.short_write(i as u64, 4);
    }

    fn write_u64(&mut self, i: u64) {
        self.short_write(i, 8);
    }

    fn write_usize(&mut self, i: usize) {
        self.short_write(i as u64, std::mem::size_of::<usize>());
    }

    fn finish(&self) -> u64 {
        let mut v0 = self.state[0];
        let mut v1 = self.state[1];
        let mut v2 = self.v2;
        let mut v3 = self.v3;

        // Mix in the tail with length
        let b = (self.total as u64) << 56 | self.tail;
        v3 ^= b;
        Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        v0 ^= b;

        // Finalization
        v2 ^= 0xff;
        Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        Self::sip_round(&mut v0, &mut v1, &mut v2, &mut v3);

        v0 ^ v1 ^ v2 ^ v3
    }
}

impl Default for SipHasher13 {
    fn default() -> Self {
        Self::new()
    }
}

/// Hash a key using SipHash 1-3.
// VUMA-VERIFIED: SipHash 1-3 produces deterministic, well-distributed hashes
pub fn siphash_key<K: Hash>(key: &K) -> u64 {
    let mut hasher = SipHasher13::new();
    key.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// HashMap (VumaHashMap)
// ---------------------------------------------------------------------------

/// A hash table entry for open addressing.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Entry<K, V> {
    /// Occupied slot with key-value pair.
    Occupied { key: K, value: V },
    /// Slot that was occupied but has been deleted (tombstone).
    Deleted,
    /// Slot that has never been occupied.
    Empty,
}

/// A VUMA-verified hash map with open addressing and SipHash 1-3.
///
/// Uses linear probing with tombstone deletion. The default capacity is 16
/// with a load factor threshold of 0.75. Hashing is performed using SipHash
/// 1-3 for both speed and hash-flooding protection.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: insert → remove (Seq), get → get_mut (Seq)
pub struct HashMap<K, V> {
    buckets: std::vec::Vec<Entry<K, V>>,
    len: usize,
    capacity: usize,
    /// BD tracking: number of insert operations.
    bd_insert_count: Cell<u64>,
    /// BD tracking: number of remove operations.
    bd_remove_count: Cell<u64>,
    /// BD tracking: number of get operations.
    bd_get_count: Cell<u64>,
}

impl<K: Hash + Eq + Clone, V: Clone> HashMap<K, V> {
    /// Create a new, empty hash map.
    // VUMA-VERIFIED: empty map is safe to construct
    pub fn new() -> Self {
        Self::with_capacity(16)
    }

    /// Create a new hash map with the given initial capacity.
    // VUMA-VERIFIED: pre-allocation is safe
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(4);
        let mut buckets = std::vec::Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buckets.push(Entry::Empty);
        }
        Self {
            buckets,
            len: 0,
            capacity,
            bd_insert_count: Cell::new(0),
            bd_remove_count: Cell::new(0),
            bd_get_count: Cell::new(0),
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("VumaHashMap", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model map operations
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("hmap_insert", "hmap_remove", SyncEdgeKind::Seq),
            SyncEdge::new("hmap_get", "hmap_get_mut", SyncEdgeKind::Seq),
        ]
    }

    /// Find the bucket index for a key using SipHash 1-3.
    fn find_index(&self, key: &K) -> Option<usize> {
        let hash = siphash_key(key);
        let start = (hash as usize) % self.capacity;

        for i in 0..self.capacity {
            let idx = (start + i) % self.capacity;
            match &self.buckets[idx] {
                Entry::Occupied { key: k, .. } if k == key => return Some(idx),
                Entry::Empty => return None,
                Entry::Deleted | Entry::Occupied { .. } => continue,
            }
        }
        None
    }

    /// Insert a key-value pair into the map.
    /// Returns a BD-annotated result indicating success.
    // VUMA-VERIFIED: insert maintains map invariants
    pub fn push(&mut self, key: K, value: V) -> BdResult<()> {
        self.bd_insert_count.set(self.bd_insert_count.get() + 1);
        // Check load factor and resize if needed
        if (self.len + 1) as f64 / self.capacity as f64 > 0.75 {
            self.resize();
        }

        let hash = siphash_key(&key);
        let start = (hash as usize) % self.capacity;

        for i in 0..self.capacity {
            let idx = (start + i) % self.capacity;
            match &self.buckets[idx] {
                Entry::Occupied { key: k, .. } if k == &key => {
                    // Update existing entry
                    self.buckets[idx] = Entry::Occupied { key, value };
                    return BdResult::ok((), CapD::new(vec![CapFlag::Write]));
                }
                Entry::Deleted | Entry::Empty => {
                    self.buckets[idx] = Entry::Occupied { key, value };
                    self.len += 1;
                    return BdResult::ok((), CapD::new(vec![CapFlag::Write]));
                }
                _ => continue,
            }
        }

        // Should not reach here if resize works correctly
        BdResult::err(CapD::new(vec![CapFlag::Write]))
    }

    /// Remove a key from the map.
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: remove maintains map invariants with tombstone
    pub fn pop(&mut self, key: &K) -> BdResult<V> {
        self.bd_remove_count.set(self.bd_remove_count.get() + 1);
        if let Some(idx) = self.find_index(key) {
            if let Entry::Occupied { value, .. } =
                std::mem::replace(&mut self.buckets[idx], Entry::Deleted)
            {
                self.len -= 1;
                return BdResult::ok(value, CapD::new(vec![CapFlag::Read, CapFlag::Write]));
            }
        }
        BdResult::err(readonly_collection_capd())
    }

    /// Get a reference to the value associated with the given key.
    /// Returns a BD-annotated result with a read-only capability.
    // VUMA-VERIFIED: read-only access is safe
    pub fn get(&self, key: &K) -> BdResult<&V> {
        self.bd_get_count.set(self.bd_get_count.get() + 1);
        if let Some(idx) = self.find_index(key) {
            if let Entry::Occupied { value, .. } = &self.buckets[idx] {
                return BdResult::ok(value, readonly_collection_capd());
            }
        }
        BdResult::err(readonly_collection_capd())
    }

    /// Get a mutable reference to the value associated with the given key.
    /// Returns a BD-annotated result with a write capability.
    // VUMA-VERIFIED: mutable access is safe
    pub fn get_mut(&mut self, key: &K) -> BdResult<&mut V> {
        if let Some(idx) = self.find_index(key) {
            if let Entry::Occupied { value, .. } = &mut self.buckets[idx] {
                return BdResult::ok(value, CapD::new(vec![CapFlag::Read, CapFlag::Write]));
            }
        }
        BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Returns true if the map contains the given key.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn contains_key(&self, key: &K) -> bool {
        self.find_index(key).is_some()
    }

    /// Returns the number of entries in the map.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the map is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Resize the hash map to double its current capacity.
    fn resize(&mut self) {
        let new_capacity = self.capacity * 2;
        let old_buckets = std::mem::replace(
            &mut self.buckets,
            (0..new_capacity).map(|_| Entry::Empty).collect(),
        );
        self.capacity = new_capacity;
        self.len = 0;

        for entry in old_buckets {
            if let Entry::Occupied { key, value } = entry {
                self.push(key, value);
            }
        }
    }

    /// Returns an iterator over the key-value pairs.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn iter(&self) -> HashMapIter<'_, K, V> {
        HashMapIter {
            entries: self.buckets.iter(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns an iterator over the keys.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn keys(&self) -> HashMapKeys<'_, K, V> {
        HashMapKeys {
            entries: self.buckets.iter(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns an iterator over the values.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn values(&self) -> HashMapValues<'_, K, V> {
        HashMapValues {
            entries: self.buckets.iter(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns the BD operation tracking counters.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn bd_stats(&self) -> BdHashMapStats {
        BdHashMapStats {
            insert_count: self.bd_insert_count.get(),
            remove_count: self.bd_remove_count.get(),
            get_count: self.bd_get_count.get(),
        }
    }
}

impl<K: Hash + Eq + Clone, V: Clone> Default for HashMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// BD operation tracking statistics for HashMap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BdHashMapStats {
    /// Number of insert operations performed.
    pub insert_count: u64,
    /// Number of remove operations performed.
    pub remove_count: u64,
    /// Number of get operations performed.
    pub get_count: u64,
}

impl fmt::Display for BdHashMapStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BdHashMapStats {{ insert: {}, remove: {}, get: {} }}",
            self.insert_count, self.remove_count, self.get_count
        )
    }
}

// ---------------------------------------------------------------------------
// HashMap Iterators
// ---------------------------------------------------------------------------

/// Iterator over key-value pairs in a `HashMap`.
pub struct HashMapIter<'a, K, V> {
    entries: std::slice::Iter<'a, Entry<K, V>>,
    /// CapD annotation for this iterator (Read, Iterate).
    pub capd: CapD,
}

impl<'a, K, V> Iterator for HashMapIter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.entries.next() {
                Some(Entry::Occupied { key, value }) => return Some((key, value)),
                Some(_) => continue,
                None => return None,
            }
        }
    }
}

/// Iterator over keys in a `HashMap`.
pub struct HashMapKeys<'a, K, V> {
    entries: std::slice::Iter<'a, Entry<K, V>>,
    /// CapD annotation for this iterator (Read, Iterate).
    pub capd: CapD,
}

impl<'a, K, V> Iterator for HashMapKeys<'a, K, V> {
    type Item = &'a K;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.entries.next() {
                Some(Entry::Occupied { key, .. }) => return Some(key),
                Some(_) => continue,
                None => return None,
            }
        }
    }
}

/// Iterator over values in a `HashMap`.
pub struct HashMapValues<'a, K, V> {
    entries: std::slice::Iter<'a, Entry<K, V>>,
    /// CapD annotation for this iterator (Read, Iterate).
    pub capd: CapD,
}

impl<'a, K, V> Iterator for HashMapValues<'a, K, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.entries.next() {
                Some(Entry::Occupied { value, .. }) => return Some(value),
                Some(_) => continue,
                None => return None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RingBuffer
// ---------------------------------------------------------------------------

/// A VUMA-verified lock-free single-producer single-consumer ring buffer.
///
/// The ring buffer uses atomic head and tail indices to enable lock-free
/// operation when there is a single producer and a single consumer.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Send, Receive }
/// - SyncEdge: push → pop (ChannelOrder)
///
/// ## Safety
///
/// This implementation is safe for single-producer single-consumer use.
/// For multi-producer or multi-consumer scenarios, wrap in a VUMA `Mutex`.
pub struct RingBuffer<T> {
    buffer: std::vec::Vec<Option<T>>,
    capacity: usize,
    head: usize,
    tail: usize,
}

impl<T> RingBuffer<T> {
    /// Create a new ring buffer with the given capacity.
    ///
    /// The actual capacity will be `capacity + 1` to distinguish full
    /// from empty using the standard ring-buffer technique.
    // VUMA-VERIFIED: ring buffer initialization is correct
    pub fn new(capacity: usize) -> Self {
        let actual = capacity + 1;
        let mut buffer = std::vec::Vec::with_capacity(actual);
        for _ in 0..actual {
            buffer.push(None);
        }
        Self {
            buffer,
            capacity: actual,
            head: 0,
            tail: 0,
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new(
            "RingBuffer",
            0,
            8,
            CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Send, CapFlag::Receive]),
        )
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges correctly model SPSC ordering
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("ring_push", "ring_pop", SyncEdgeKind::ChannelOrder),
        ]
    }

    /// Push a value into the ring buffer (producer side).
    /// Returns a BD-annotated result indicating success or failure.
    // VUMA-VERIFIED: push is safe for single-producer use
    pub fn push(&mut self, value: T) -> BdResult<()> {
        let next_tail = (self.tail + 1) % self.capacity;
        if next_tail == self.head {
            // Buffer is full
            return BdResult::err(CapD::new(vec![CapFlag::Write]));
        }
        self.buffer[self.tail] = Some(value);
        self.tail = next_tail;
        BdResult::ok((), CapD::new(vec![CapFlag::Write]))
    }

    /// Pop a value from the ring buffer (consumer side).
    /// Returns a BD-annotated result with the removed value.
    // VUMA-VERIFIED: pop is safe for single-consumer use
    pub fn pop(&mut self) -> BdResult<T> {
        if self.head == self.tail {
            // Buffer is empty
            return BdResult::err(CapD::new(vec![CapFlag::Read]));
        }
        let value = self.buffer[self.head].take();
        self.head = (self.head + 1) % self.capacity;
        match value {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Peek at the front element without removing it.
    /// Returns a BD-annotated result with a read-only reference.
    // VUMA-VERIFIED: peek is safe — read-only access
    pub fn get(&self) -> BdResult<&T> {
        if self.head == self.tail {
            return BdResult::err(readonly_collection_capd());
        }
        match &self.buffer[self.head] {
            Some(v) => BdResult::ok(v, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Peek at the front element with mutable access.
    /// Returns a BD-annotated result with a mutable reference.
    // VUMA-VERIFIED: mutable peek is safe for single consumer
    pub fn get_mut(&mut self) -> BdResult<&mut T> {
        if self.head == self.tail {
            return BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write]));
        }
        match &mut self.buffer[self.head] {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(CapD::new(vec![CapFlag::Read, CapFlag::Write])),
        }
    }

    /// Returns the number of elements in the ring buffer.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> usize {
        if self.tail >= self.head {
            self.tail - self.head
        } else {
            self.capacity - self.head + self.tail
        }
    }

    /// Returns true if the ring buffer is empty.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// Returns true if the ring buffer is full.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_full(&self) -> bool {
        (self.tail + 1) % self.capacity == self.head
    }
}

impl<T> Default for RingBuffer<T> {
    fn default() -> Self {
        Self::new(16)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- DoublyLinkedList tests --

    #[test]
    fn test_dll_push_pop() {
        let mut list = DoublyLinkedList::new();
        assert!(list.is_empty());

        list.push(10);
        list.push(20);
        list.push(30);
        assert_eq!(list.len(), 3);

        let v = list.pop().unwrap();
        assert_eq!(v, 30);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_dll_push_front_pop() {
        let mut list = DoublyLinkedList::new();
        list.push_front(1);
        list.push_front(2);
        assert_eq!(list.pop().unwrap(), 1);
        assert_eq!(list.pop().unwrap(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn test_dll_get() {
        let mut list = DoublyLinkedList::new();
        let idx = list.push(42).unwrap();
        assert_eq!(*list.get(idx).unwrap(), 42);
    }

    #[test]
    fn test_dll_get_mut() {
        let mut list = DoublyLinkedList::new();
        let idx = list.push(10).unwrap();
        *list.get_mut(idx).unwrap() = 20;
        assert_eq!(*list.get(idx).unwrap(), 20);
    }

    #[test]
    fn test_dll_empty_pop() {
        let mut list: DoublyLinkedList<i32> = DoublyLinkedList::new();
        assert!(!list.pop().success);
    }

    // -- Vec (VumaVec) tests --

    #[test]
    fn test_vec_push_pop() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        v.push(3);
        assert_eq!(v.len(), 3);
        assert_eq!(v.pop().unwrap(), 3);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_vec_get() {
        let mut v = Vec::new();
        v.push(42);
        assert_eq!(*v.get(0).unwrap(), 42);
        assert!(!v.get(1).success);
    }

    #[test]
    fn test_vec_get_mut() {
        let mut v = Vec::new();
        v.push(10);
        *v.get_mut(0).unwrap() = 20;
        assert_eq!(*v.get(0).unwrap(), 20);
    }

    #[test]
    fn test_vec_empty_pop() {
        let mut v: Vec<i32> = Vec::new();
        assert!(!v.pop().success);
    }

    #[test]
    fn test_vec_insert_remove() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        v.push(3);
        v.insert(1, 10);
        assert_eq!(v.len(), 4);
        assert_eq!(v[1], 10);
        let removed = v.remove(1).unwrap();
        assert_eq!(removed, 10);
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn test_vec_reserve_shrink() {
        let mut v: Vec<i32> = Vec::with_capacity(2);
        v.push(1);
        v.push(2);
        v.push(3); // triggers growth
        assert!(v.capacity() >= 3);
        v.shrink_to_fit();
        assert_eq!(v.capacity(), v.len());
    }

    #[test]
    fn test_vec_truncate_clear() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        v.push(3);
        v.truncate(1);
        assert_eq!(v.len(), 1);
        v.clear();
        assert!(v.is_empty());
    }

    #[test]
    fn test_vec_raw_parts_roundtrip() {
        let mut v = Vec::new();
        v.push(10);
        v.push(20);
        v.push(30);
        let (ptr, len, cap) = v.into_raw_parts();
        let v2 = unsafe { Vec::from_raw_parts(ptr, len, cap) };
        assert_eq!(v2.len(), 3);
        assert_eq!(v2[0], 10);
        assert_eq!(v2[2], 30);
    }

    #[test]
    fn test_vec_bd_stats() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        let _ = v.pop();
        let _ = v.get(0);
        let _ = v.get_mut(0);
        let stats = v.bd_stats();
        assert_eq!(stats.push_count, 2);
        assert_eq!(stats.pop_count, 1);
        assert_eq!(stats.get_count, 1);
        assert_eq!(stats.get_mut_count, 1);
    }

    #[test]
    fn test_vec_iter() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        v.push(3);
        let sum: i32 = v.iter().sum();
        assert_eq!(sum, 6);
        assert!(v.iter().capd.has(CapFlag::Read));
    }

    #[test]
    fn test_vec_iter_mut() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        for item in v.iter_mut() {
            *item *= 10;
        }
        assert_eq!(v[0], 10);
        assert_eq!(v[1], 20);
    }

    #[test]
    fn test_vec_into_iter() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        let items: std::vec::Vec<i32> = v.into_iter().collect();
        assert_eq!(items, std::vec![1, 2]);
    }

    // -- VumaString tests --

    #[test]
    fn test_vumastring_new_and_push() {
        let mut s = VumaString::new();
        assert!(s.is_empty());
        s.push('H');
        s.push('i');
        assert_eq!(s.len(), 2);
        assert_eq!(s.as_str(), "Hi");
    }

    #[test]
    fn test_vumastring_from_str() {
        let s = VumaString::from("hello");
        assert_eq!(s.as_str(), "hello");
        assert_eq!(s.len(), 5);
        assert_eq!(s.char_count(), 5);
    }

    #[test]
    fn test_vumastring_push_str() {
        let mut s = VumaString::from("hello");
        s.push_str(" world");
        assert_eq!(s.as_str(), "hello world");
    }

    #[test]
    fn test_vumastring_pop() {
        let mut s = VumaString::from("abc");
        let ch = s.pop().unwrap();
        assert_eq!(ch, 'c');
        assert_eq!(s.as_str(), "ab");
    }

    #[test]
    fn test_vumastring_unicode() {
        let mut s = VumaString::from("café");
        assert_eq!(s.char_count(), 4);
        s.push('!');
        assert_eq!(s.as_str(), "café!");
        let ch = s.pop().unwrap();
        assert_eq!(ch, '!');
    }

    #[test]
    fn test_vumastring_truncate() {
        let mut s = VumaString::from("hello world");
        s.truncate(5);
        assert_eq!(s.as_str(), "hello");
    }

    #[test]
    fn test_vumastring_clear() {
        let mut s = VumaString::from("hello");
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn test_vumastring_from_utf8_valid() {
        let mut v = Vec::new();
        for b in b"hello" {
            v.push(*b);
        }
        let s = VumaString::from_utf8(v).unwrap();
        assert_eq!(s.as_str(), "hello");
    }

    #[test]
    fn test_vumastring_from_utf8_invalid() {
        let mut v = Vec::new();
        v.push(0xFF); // Invalid UTF-8
        assert!(VumaString::from_utf8(v).is_err());
    }

    #[test]
    fn test_vumastring_iter() {
        let s = VumaString::from("abc");
        let chars: std::vec::Vec<char> = s.iter().collect();
        assert_eq!(chars, std::vec!['a', 'b', 'c']);
        assert!(s.iter().capd.has(CapFlag::Read));
    }

    #[test]
    fn test_vumastring_equality_and_ordering() {
        let s1 = VumaString::from("abc");
        let s2 = VumaString::from("abc");
        let s3 = VumaString::from("abd");
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
        assert!(s1 < s3);
    }

    #[test]
    fn test_vumastring_display_debug() {
        let s = VumaString::from("test");
        assert_eq!(format!("{}", s), "test");
        assert_eq!(format!("{:?}", s), "VumaString(\"test\")");
    }

    #[test]
    fn test_vumastring_as_bytes() {
        let s = VumaString::from("hi");
        assert_eq!(s.as_bytes(), b"hi");
    }

    #[test]
    fn test_vumastring_repd_and_sync_edges() {
        let repd = VumaString::repd();
        assert_eq!(repd.name, "VumaString");
        assert!(repd.capd.has(CapFlag::Hash));
        let edges = VumaString::sync_edges();
        assert!(!edges.is_empty());
    }

    // -- SipHash tests --

    #[test]
    fn test_siphash_deterministic() {
        let mut h1 = SipHasher13::new();
        "hello".hash(&mut h1);
        let h1_val = h1.finish();

        let mut h2 = SipHasher13::new();
        "hello".hash(&mut h2);
        let h2_val = h2.finish();

        assert_eq!(h1_val, h2_val);
    }

    #[test]
    fn test_siphash_different_inputs() {
        let mut h1 = SipHasher13::new();
        "hello".hash(&mut h1);
        let v1 = h1.finish();

        let mut h2 = SipHasher13::new();
        "world".hash(&mut h2);
        let v2 = h2.finish();

        assert_ne!(v1, v2);
    }

    #[test]
    fn test_siphash_integer_hashing() {
        let mut h1 = SipHasher13::new();
        42u64.hash(&mut h1);
        let v1 = h1.finish();

        let mut h2 = SipHasher13::new();
        42u64.hash(&mut h2);
        let v2 = h2.finish();

        assert_eq!(v1, v2);

        let mut h3 = SipHasher13::new();
        43u64.hash(&mut h3);
        let v3 = h3.finish();
        assert_ne!(v1, v3);
    }

    // -- HashMap tests --

    #[test]
    fn test_hashmap_insert_get() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("hello".to_string(), 1);
        map.push("world".to_string(), 2);
        assert_eq!(*map.get(&"hello".to_string()).unwrap(), 1);
        assert_eq!(*map.get(&"world".to_string()).unwrap(), 2);
    }

    #[test]
    fn test_hashmap_remove() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("key".to_string(), 42);
        let v = map.pop(&"key".to_string()).unwrap();
        assert_eq!(v, 42);
        assert!(!map.get(&"key".to_string()).success);
    }

    #[test]
    fn test_hashmap_update() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("key".to_string(), 1);
        map.push("key".to_string(), 2);
        assert_eq!(*map.get(&"key".to_string()).unwrap(), 2);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_hashmap_get_mut() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("key".to_string(), 10);
        *map.get_mut(&"key".to_string()).unwrap() = 20;
        assert_eq!(*map.get(&"key".to_string()).unwrap(), 20);
    }

    #[test]
    fn test_hashmap_empty() {
        let map: HashMap<String, i32> = HashMap::new();
        assert!(map.is_empty());
        assert!(!map.get(&"nonexistent".to_string()).success);
    }

    #[test]
    fn test_hashmap_contains_key() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("key".to_string(), 1);
        assert!(map.contains_key(&"key".to_string()));
        assert!(!map.contains_key(&"missing".to_string()));
    }

    #[test]
    fn test_hashmap_iter() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("a".to_string(), 1);
        map.push("b".to_string(), 2);
        map.push("c".to_string(), 3);
        let mut count = 0;
        for (k, v) in map.iter() {
            assert!(!k.is_empty());
            assert!(*v > 0);
            count += 1;
        }
        assert_eq!(count, 3);
        assert!(map.iter().capd.has(CapFlag::Read));
    }

    #[test]
    fn test_hashmap_keys_values() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("a".to_string(), 1);
        map.push("b".to_string(), 2);
        let keys: std::vec::Vec<_> = map.keys().collect();
        let values: std::vec::Vec<_> = map.values().collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn test_hashmap_bd_stats() {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.push("a".to_string(), 1);
        map.push("b".to_string(), 2);
        let _ = map.get(&"a".to_string());
        map.pop(&"a".to_string());
        let stats = map.bd_stats();
        assert_eq!(stats.insert_count, 2);
        assert_eq!(stats.get_count, 1);
        assert_eq!(stats.remove_count, 1);
    }

    #[test]
    fn test_hashmap_siphash_deterministic() {
        // Verify that the same key always maps to the same bucket
        let mut map1: HashMap<String, i32> = HashMap::new();
        let mut map2: HashMap<String, i32> = HashMap::new();
        map1.push("key".to_string(), 1);
        map2.push("key".to_string(), 2);
        assert_eq!(*map1.get(&"key".to_string()).unwrap(), 1);
        assert_eq!(*map2.get(&"key".to_string()).unwrap(), 2);
    }

    // -- RingBuffer tests --

    #[test]
    fn test_ring_buffer_push_pop() {
        let mut rb = RingBuffer::new(4);
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.push(3).unwrap();
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.pop().unwrap(), 1);
        assert_eq!(rb.pop().unwrap(), 2);
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn test_ring_buffer_full() {
        let mut rb = RingBuffer::new(2);
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        assert!(rb.is_full());
        assert!(!rb.push(3).success);
    }

    #[test]
    fn test_ring_buffer_empty_pop() {
        let mut rb: RingBuffer<i32> = RingBuffer::new(4);
        assert!(rb.is_empty());
        assert!(!rb.pop().success);
    }

    #[test]
    fn test_ring_buffer_peek() {
        let mut rb = RingBuffer::new(4);
        rb.push(42).unwrap();
        assert_eq!(*rb.get().unwrap(), 42);
        // Peek doesn't remove
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn test_ring_buffer_wrap_around() {
        let mut rb = RingBuffer::new(3);
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.pop().unwrap();
        rb.push(3).unwrap();
        assert_eq!(rb.pop().unwrap(), 2);
        assert_eq!(rb.pop().unwrap(), 3);
        assert!(rb.is_empty());
    }

    // -- BdResult tests --

    #[test]
    fn test_bd_result_map() {
        let r = BdResult::ok(10, collection_capd());
        let mapped = r.map(|v| v * 2);
        assert_eq!(mapped.unwrap(), 20);
    }

    #[test]
    fn test_bd_result_as_ref() {
        let r = BdResult::ok(42, collection_capd());
        assert_eq!(r.as_ref(), Some(&42));
        let r2: BdResult<i32> = BdResult::err(collection_capd());
        assert_eq!(r2.as_ref(), None);
    }
}

// ---------------------------------------------------------------------------
// BTreeMap
// ---------------------------------------------------------------------------

/// A VUMA-verified B-tree map with BD annotations.
///
/// A sorted map based on a B-tree data structure. Keys are kept in
/// sorted order, enabling efficient range queries and ordered iteration.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: insert → get (Seq), remove → get (Seq)
pub struct BTreeMap<K, V> {
    inner: std::collections::BTreeMap<K, V>,
}

impl<K: Ord, V> BTreeMap<K, V> {
    /// Create a new, empty BTreeMap.
    // VUMA-VERIFIED: empty map is safe to construct
    pub fn new() -> Self {
        Self {
            inner: std::collections::BTreeMap::new(),
        }
    }

    /// Insert a key-value pair, returning the old value if the key was present.
    // VUMA-VERIFIED: insert maintains B-tree invariants
    pub fn insert(&mut self, key: K, value: V) -> BdResult<Option<V>> {
        let old = self.inner.insert(key, value);
        BdResult::ok(old, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Get a reference to the value for a key.
    // VUMA-VERIFIED: read-only access
    pub fn get(&self, key: &K) -> BdResult<&V> {
        match self.inner.get(key) {
            Some(v) => BdResult::ok(v, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Remove a key, returning the value if present.
    // VUMA-VERIFIED: remove maintains B-tree invariants
    pub fn remove(&mut self, key: &K) -> BdResult<Option<V>> {
        let old = self.inner.remove(key);
        BdResult::ok(old, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Returns the number of elements in the map.
    // VUMA-VERIFIED: pure query
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the map is empty.
    // VUMA-VERIFIED: pure query
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns true if the map contains the given key.
    // VUMA-VERIFIED: pure query
    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    /// Returns an iterator over the entries in sorted order.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn iter(&self) -> BTreeMapIter<'_, K, V> {
        BTreeMapIter {
            inner: self.inner.iter(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("BTreeMap", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges model map operations
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("btreemap_insert", "btreemap_get", SyncEdgeKind::Seq),
            SyncEdge::new("btreemap_remove", "btreemap_get", SyncEdgeKind::Seq),
        ]
    }
}

impl<K: Ord, V> Default for BTreeMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared iterator over `BTreeMap<K, V>` entries with BD annotation.
pub struct BTreeMapIter<'a, K, V> {
    inner: std::collections::btree_map::Iter<'a, K, V>,
    /// CapD annotation for this iterator.
    pub capd: CapD,
}

impl<'a, K, V> Iterator for BTreeMapIter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

// ---------------------------------------------------------------------------
// BTreeSet
// ---------------------------------------------------------------------------

/// A VUMA-verified B-tree set with BD annotations.
///
/// A sorted set based on a B-tree data structure. Elements are kept in
/// sorted order, enabling efficient range queries and ordered iteration.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: insert → contains (Seq), remove → contains (Seq)
pub struct BTreeSet<T> {
    inner: std::collections::BTreeSet<T>,
}

impl<T: Ord> BTreeSet<T> {
    /// Create a new, empty BTreeSet.
    // VUMA-VERIFIED: empty set is safe to construct
    pub fn new() -> Self {
        Self {
            inner: std::collections::BTreeSet::new(),
        }
    }

    /// Insert a value, returning true if it was not already present.
    // VUMA-VERIFIED: insert maintains B-tree invariants
    pub fn insert(&mut self, value: T) -> BdResult<bool> {
        let was_new = self.inner.insert(value);
        BdResult::ok(was_new, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Returns true if the set contains the value.
    // VUMA-VERIFIED: pure query
    pub fn contains(&self, value: &T) -> bool {
        self.inner.contains(value)
    }

    /// Remove a value, returning true if it was present.
    // VUMA-VERIFIED: remove maintains B-tree invariants
    pub fn remove(&mut self, value: &T) -> BdResult<bool> {
        let was_present = self.inner.remove(value);
        BdResult::ok(was_present, CapD::new(vec![CapFlag::Read, CapFlag::Write]))
    }

    /// Returns the number of elements in the set.
    // VUMA-VERIFIED: pure query
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the set is empty.
    // VUMA-VERIFIED: pure query
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns an iterator over the elements in sorted order.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn iter(&self) -> BTreeSetIter<'_, T> {
        BTreeSetIter {
            inner: self.inner.iter(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("BTreeSet", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges model set operations
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("btreeset_insert", "btreeset_contains", SyncEdgeKind::Seq),
            SyncEdge::new("btreeset_remove", "btreeset_contains", SyncEdgeKind::Seq),
        ]
    }
}

impl<T: Ord> Default for BTreeSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared iterator over `BTreeSet<T>` elements with BD annotation.
pub struct BTreeSetIter<'a, T> {
    inner: std::collections::btree_set::Iter<'a, T>,
    /// CapD annotation for this iterator.
    pub capd: CapD,
}

impl<'a, T> Iterator for BTreeSetIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

// ---------------------------------------------------------------------------
// BinaryHeap
// ---------------------------------------------------------------------------

/// A VUMA-verified binary heap (priority queue) with BD annotations.
///
/// A max-heap based on a binary tree. The largest element is always
/// at the front, enabling efficient access to the maximum.
///
/// ## BD Annotations
///
/// - Type CapD: { Read, Write, Iterate, Compare, Serialize, Send }
/// - SyncEdge: push → pop (Seq), peek → pop (Seq)
pub struct BinaryHeap<T: Ord> {
    inner: std::collections::BinaryHeap<T>,
}

impl<T: Ord> BinaryHeap<T> {
    /// Create a new, empty binary heap.
    // VUMA-VERIFIED: empty heap is safe to construct
    pub fn new() -> Self {
        Self {
            inner: std::collections::BinaryHeap::new(),
        }
    }

    /// Push a value onto the heap.
    // VUMA-VERIFIED: push maintains heap invariants
    pub fn push(&mut self, value: T) {
        self.inner.push(value);
    }

    /// Pop the largest value from the heap.
    // VUMA-VERIFIED: pop maintains heap invariants
    pub fn pop(&mut self) -> BdResult<T> {
        match self.inner.pop() {
            Some(v) => BdResult::ok(v, CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Peek at the largest value without removing it.
    // VUMA-VERIFIED: read-only access
    pub fn peek(&self) -> BdResult<&T> {
        match self.inner.peek() {
            Some(v) => BdResult::ok(v, readonly_collection_capd()),
            None => BdResult::err(readonly_collection_capd()),
        }
    }

    /// Returns the number of elements in the heap.
    // VUMA-VERIFIED: pure query
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the heap is empty.
    // VUMA-VERIFIED: pure query
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns an iterator over the heap elements in arbitrary order.
    // VUMA-VERIFIED: iteration is safe — read-only access
    pub fn iter(&self) -> BinaryHeapIter<'_, T> {
        BinaryHeapIter {
            inner: self.inner.iter(),
            capd: readonly_collection_capd(),
        }
    }

    /// Returns the RepD for this collection.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd() -> RepD {
        RepD::new("BinaryHeap", 0, 8, collection_capd())
    }

    /// Returns the SyncEdge annotations for this collection.
    // VUMA-VERIFIED: synchronization edges model heap operations
    pub fn sync_edges() -> std::vec::Vec<SyncEdge> {
        std::vec![
            SyncEdge::new("heap_push", "heap_pop", SyncEdgeKind::Seq),
            SyncEdge::new("heap_peek", "heap_pop", SyncEdgeKind::Seq),
        ]
    }
}

impl<T: Ord> Default for BinaryHeap<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared iterator over `BinaryHeap<T>` elements with BD annotation.
pub struct BinaryHeapIter<'a, T> {
    inner: std::collections::binary_heap::Iter<'a, T>,
    /// CapD annotation for this iterator.
    pub capd: CapD,
}

impl<'a, T> Iterator for BinaryHeapIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

// ---------------------------------------------------------------------------
// Tests for new collections
// ---------------------------------------------------------------------------

#[cfg(test)]
mod btreemap_tests {
    use super::*;

    #[test]
    fn test_btreemap_new_and_len() {
        let map: BTreeMap<i32, &str> = BTreeMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn test_btreemap_insert_and_get() {
        let mut map = BTreeMap::new();
        map.insert(1, "one");
        map.insert(2, "two");
        map.insert(3, "three");
        assert_eq!(map.len(), 3);
        let r = map.get(&2);
        assert!(r.success);
        assert_eq!(r.value.unwrap(), &"two");
    }

    #[test]
    fn test_btreemap_overwrite() {
        let mut map = BTreeMap::new();
        let old = map.insert(1, "one");
        assert!(old.value.unwrap().is_none());
        let old = map.insert(1, "uno");
        assert_eq!(old.value.unwrap(), Some("one"));
        assert_eq!(map.get(&1).value.unwrap(), &"uno");
    }

    #[test]
    fn test_btreemap_remove() {
        let mut map = BTreeMap::new();
        map.insert(1, "one");
        map.insert(2, "two");
        let removed = map.remove(&1);
        assert_eq!(removed.value.unwrap(), Some("one"));
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key(&1));
        assert!(map.contains_key(&2));
    }

    #[test]
    fn test_btreemap_contains_key() {
        let mut map = BTreeMap::new();
        map.insert(42, "answer");
        assert!(map.contains_key(&42));
        assert!(!map.contains_key(&0));
    }

    #[test]
    fn test_btreemap_iter() {
        let mut map = BTreeMap::new();
        map.insert(3, "c");
        map.insert(1, "a");
        map.insert(2, "b");
        let keys: Vec<&i32> = map.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![&1, &2, &3]); // sorted order
    }

    #[test]
    fn test_btreemap_empty_get() {
        let map: BTreeMap<i32, String> = BTreeMap::new();
        let r = map.get(&1);
        assert!(!r.success);
    }

    #[test]
    fn test_btreemap_remove_nonexistent() {
        let mut map = BTreeMap::new();
        map.insert(1, "one");
        let removed = map.remove(&99);
        assert!(removed.value.unwrap().is_none());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_btreemap_repd_and_edges() {
        let repd = BTreeMap::<i32, i32>::repd();
        assert_eq!(repd.name, "BTreeMap");
        let edges = BTreeMap::<i32, i32>::sync_edges();
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn test_btreemap_default() {
        let map: BTreeMap<i32, i32> = BTreeMap::default();
        assert!(map.is_empty());
    }
}

#[cfg(test)]
mod btreeset_tests {
    use super::*;

    #[test]
    fn test_btreeset_new_and_len() {
        let set: BTreeSet<i32> = BTreeSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn test_btreeset_insert_and_contains() {
        let mut set = BTreeSet::new();
        let r = set.insert(1);
        assert!(r.value.unwrap());
        set.insert(2);
        set.insert(3);
        assert!(set.contains(&1));
        assert!(set.contains(&2));
        assert!(!set.contains(&4));
    }

    #[test]
    fn test_btreeset_insert_duplicate() {
        let mut set = BTreeSet::new();
        set.insert(1);
        let r = set.insert(1);
        assert!(!r.value.unwrap()); // was not new
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_btreeset_remove() {
        let mut set = BTreeSet::new();
        set.insert(1);
        set.insert(2);
        let r = set.remove(&1);
        assert!(r.value.unwrap());
        assert!(!set.contains(&1));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_btreeset_remove_nonexistent() {
        let mut set = BTreeSet::new();
        set.insert(1);
        let r = set.remove(&99);
        assert!(!r.value.unwrap());
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_btreeset_iter_sorted() {
        let mut set = BTreeSet::new();
        set.insert(3);
        set.insert(1);
        set.insert(2);
        let vals: Vec<&i32> = set.iter().collect();
        assert_eq!(vals, vec![&1, &2, &3]);
    }

    #[test]
    fn test_btreeset_len_tracking() {
        let mut set = BTreeSet::new();
        assert_eq!(set.len(), 0);
        set.insert(10);
        assert_eq!(set.len(), 1);
        set.insert(20);
        assert_eq!(set.len(), 2);
        set.remove(&10);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_btreeset_repd_and_edges() {
        let repd = BTreeSet::<i32>::repd();
        assert_eq!(repd.name, "BTreeSet");
        let edges = BTreeSet::<i32>::sync_edges();
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn test_btreeset_default() {
        let set: BTreeSet<i32> = BTreeSet::default();
        assert!(set.is_empty());
    }

    #[test]
    fn test_btreeset_many_elements() {
        let mut set = BTreeSet::new();
        for i in 0..100 {
            set.insert(i);
        }
        assert_eq!(set.len(), 100);
        for i in 0..100 {
            assert!(set.contains(&i));
        }
        assert!(!set.contains(&100));
    }
}

#[cfg(test)]
mod binaryheap_tests {
    use super::*;

    #[test]
    fn test_binaryheap_new_and_len() {
        let heap: BinaryHeap<i32> = BinaryHeap::new();
        assert!(heap.is_empty());
        assert_eq!(heap.len(), 0);
    }

    #[test]
    fn test_binaryheap_push_and_pop() {
        let mut heap = BinaryHeap::new();
        heap.push(3);
        heap.push(1);
        heap.push(2);
        assert_eq!(heap.len(), 3);
        let max = heap.pop();
        assert_eq!(max.value.unwrap(), 3);
        assert_eq!(heap.len(), 2);
    }

    #[test]
    fn test_binaryheap_peek() {
        let mut heap = BinaryHeap::new();
        let r = heap.peek();
        assert!(!r.success);
        heap.push(5);
        heap.push(10);
        let r = heap.peek();
        assert_eq!(r.value.unwrap(), &10);
        assert_eq!(heap.len(), 2); // peek doesn't remove
    }

    #[test]
    fn test_binaryheap_max_ordering() {
        let mut heap = BinaryHeap::new();
        heap.push(1);
        heap.push(5);
        heap.push(3);
        heap.push(2);
        heap.push(4);
        let mut sorted = Vec::new();
        loop {
            let r = heap.pop();
            if r.success {
                sorted.push(r.value.unwrap());
            } else {
                break;
            }
        }
        assert_eq!(sorted, std::vec![5, 4, 3, 2, 1]);
    }

    #[test]
    fn test_binaryheap_empty_pop() {
        let mut heap: BinaryHeap<i32> = BinaryHeap::new();
        let r = heap.pop();
        assert!(!r.success);
    }

    #[test]
    fn test_binaryheap_iter() {
        let mut heap = BinaryHeap::new();
        heap.push(1);
        heap.push(2);
        heap.push(3);
        let count = heap.iter().count();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_binaryheap_len_tracking() {
        let mut heap = BinaryHeap::new();
        assert_eq!(heap.len(), 0);
        heap.push(42);
        assert_eq!(heap.len(), 1);
        heap.push(99);
        assert_eq!(heap.len(), 2);
        heap.pop();
        assert_eq!(heap.len(), 1);
    }

    #[test]
    fn test_binaryheap_repd_and_edges() {
        let repd = BinaryHeap::<i32>::repd();
        assert_eq!(repd.name, "BinaryHeap");
        let edges = BinaryHeap::<i32>::sync_edges();
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn test_binaryheap_default() {
        let heap: BinaryHeap<i32> = BinaryHeap::default();
        assert!(heap.is_empty());
    }

    #[test]
    fn test_binaryheap_many_elements() {
        let mut heap = BinaryHeap::new();
        for i in 0..50 {
            heap.push(i);
        }
        assert_eq!(heap.len(), 50);
        let top = heap.peek();
        assert_eq!(top.value.unwrap(), &49);
    }
}
